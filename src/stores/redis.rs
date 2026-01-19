use async_trait::async_trait;
use redis::AsyncCommands;
use redis::aio::MultiplexedConnection;

use crate::entry::{StorageMode, StoredEntry};
use crate::error::CacheError;
use crate::store::Store;

use crate::utils::{build_cache_key, now_ms};

/// Configuration for RedisStore.
#[derive(Debug, Clone)]
pub struct RedisStoreConfig {
    /// Redis connection URL.
    ///
    /// Format: `redis://[username:password@]host[:port][/database]`
    ///
    /// # Examples
    /// - `redis://localhost:6379`
    /// - `redis://user:password@localhost:6379/0`
    /// - `rediss://user:password@host:6379` (TLS)
    pub url: String,

    /// If true, data never expires in Redis (no TTL set).
    ///
    /// This is useful for keeping stale data available even when the origin fails.
    /// When disabled, stale data persists in Redis and can be served during revalidation,
    /// providing resilience during origin outages.
    ///
    /// Note: The application still tracks `stale_until` timestamps and considers
    /// entries stale/expired based on these values, but Redis won't auto-evict them.
    ///
    /// Default: `false` (expiration enabled, Redis auto-evicts after TTL)
    pub disable_expiration: bool,
}

/// Redis-backed cache store.
///
/// RedisStore is type-agnostic and stores all values as JSON strings in Redis.
/// It always uses `StoredEntry::Serialized` format.
///
/// By default, TTL is set based on `stale_until`.
/// When `disable_expiration` is enabled, data persists indefinitely in Redis, which is
/// useful for keeping stale data available during origin outages.
pub struct RedisStore {
    connection: MultiplexedConnection,
    disable_expiration: bool,
}

impl RedisStore {
    /// Create a new RedisStore with the given configuration.
    ///
    /// # Arguments
    /// * `config` - Redis configuration including connection URL
    ///
    /// # Returns
    /// * `Ok(RedisStore)` - Successfully connected store
    /// * `Err(CacheError)` - Connection failed
    ///
    /// # Example
    /// ```ignore
    /// let config = RedisStoreConfig {
    ///     url: "redis://localhost:6379".to_string(),
    ///     disable_expiration: false,
    /// };
    /// let store = RedisStore::new(config).await?;
    /// ```
    pub async fn new(config: RedisStoreConfig) -> Result<Self, CacheError> {
        let client = redis::Client::open(config.url.as_str()).map_err(|e| {
            CacheError::operation("redis", "", format!("Failed to create Redis client: {}", e))
        })?;

        let connection = client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| {
                CacheError::operation("redis", "", format!("Failed to connect to Redis: {}", e))
            })?;

        Ok(RedisStore {
            connection,
            disable_expiration: config.disable_expiration,
        })
    }

    /// Calculate TTL in seconds from stale_until timestamp.
    fn calculate_ttl_seconds(stale_until: i64) -> u64 {
        let now = now_ms();
        if stale_until <= now {
            return 1; // Minimum TTL of 1 second
        }
        ((stale_until - now) / 1000).max(1) as u64
    }
}

#[async_trait]
impl Store for RedisStore {
    fn name(&self) -> &'static str {
        "redis"
    }

    fn storage_mode(&self) -> StorageMode {
        StorageMode::Serialized
    }

    async fn get(&self, namespace: &str, key: &str) -> Result<Option<StoredEntry>, CacheError> {
        let cache_key = build_cache_key(namespace, key);
        let mut conn = self.connection.clone();

        let result: Option<String> = conn
            .get(&cache_key)
            .await
            .map_err(|e| CacheError::operation("redis", key, format!("GET failed: {}", e)))?;

        match result {
            Some(json_str) => {
                // Parse the JSON to extract metadata
                #[derive(serde::Deserialize)]
                struct EntryMetadata {
                    fresh_until: i64,
                    stale_until: i64,
                }

                let metadata: EntryMetadata = serde_json::from_str(&json_str).map_err(|e| {
                    CacheError::operation("redis", key, format!("Metadata parse failed: {}", e))
                })?;

                // Check if expired
                let now = now_ms();
                if now >= metadata.stale_until {
                    // Entry is expired
                    if !self.disable_expiration {
                        // Delete in background when expiration is enabled
                        let mut del_conn = self.connection.clone();
                        let del_key = cache_key.clone();
                        tokio::spawn(async move {
                            let _: Result<(), _> = del_conn.del(del_key).await;
                        });
                        return Ok(None);
                    }
                    // With disable_expiration, return expired entry for potential fallback
                }

                Ok(Some(StoredEntry::Serialized {
                    data: json_str,
                    fresh_until: metadata.fresh_until,
                    stale_until: metadata.stale_until,
                }))
            }
            None => Ok(None),
        }
    }

    async fn set(&self, namespace: &str, key: &str, entry: StoredEntry) -> Result<(), CacheError> {
        let cache_key = build_cache_key(namespace, key);
        let mut conn = self.connection.clone();

        // RedisStore only accepts Serialized entries
        // TieredStore handles conversion based on storage_mode()
        let (json_str, stale_until) = match entry {
            StoredEntry::Serialized {
                data, stale_until, ..
            } => (data, stale_until),
            StoredEntry::Typed { .. } => {
                return Err(CacheError::Serialization(
                    "RedisStore received Typed entry but requires Serialized. \
                     This indicates a bug in TieredStore conversion logic."
                        .to_string(),
                ));
            }
        };

        if self.disable_expiration {
            // Store without TTL - data persists indefinitely
            let _: () = conn
                .set(&cache_key, json_str)
                .await
                .map_err(|e| CacheError::operation("redis", key, format!("SET failed: {}", e)))?;
        } else {
            // Store with TTL - Redis auto-evicts after expiration
            let ttl_seconds = Self::calculate_ttl_seconds(stale_until);
            let _: () = conn
                .set_ex(&cache_key, json_str, ttl_seconds)
                .await
                .map_err(|e| CacheError::operation("redis", key, format!("SETEX failed: {}", e)))?;
        }

        Ok(())
    }

    async fn remove(&self, namespace: &str, keys: &[&str]) -> Result<(), CacheError> {
        if keys.is_empty() {
            return Ok(());
        }

        let mut conn = self.connection.clone();
        let cache_keys: Vec<String> = keys.iter().map(|k| build_cache_key(namespace, k)).collect();

        let _: () = conn.del(&cache_keys).await.map_err(|e| {
            CacheError::operation("redis", cache_keys.join(","), format!("DEL failed: {}", e))
        })?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: These tests require a running Redis instance.
    // Run with: cargo test --features redis-tests -- --ignored

    #[tokio::test]
    #[ignore = "requires running Redis instance"]
    async fn test_redis_get_set_remove() {
        let config = RedisStoreConfig {
            url: "redis://localhost:6379".to_string(),
            disable_expiration: false,
        };

        let store = RedisStore::new(config).await.unwrap();

        // Initially empty
        let result = store.get("users", "test_key").await.unwrap();
        assert!(result.is_none());

        // Set a value using Serialized variant
        let now = now_ms();
        let json_data = serde_json::json!({
            "value": "test_value",
            "fresh_until": now + 60_000,
            "stale_until": now + 300_000,
        })
        .to_string();

        let entry = StoredEntry::from_serialized(json_data, now + 60_000, now + 300_000);
        store.set("users", "test_key", entry).await.unwrap();

        // Get the value
        let result = store.get("users", "test_key").await.unwrap();
        assert!(result.is_some());

        // Remove the value
        store.remove("users", &["test_key"]).await.unwrap();

        // Should be gone
        let result = store.get("users", "test_key").await.unwrap();
        assert!(result.is_none());
    }
}
