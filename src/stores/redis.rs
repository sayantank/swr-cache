use async_trait::async_trait;
use redis::AsyncCommands;
use redis::aio::MultiplexedConnection;
use serde::{Serialize, de::DeserializeOwned};
use std::fmt::Display;
use std::hash::Hash;
use std::marker::PhantomData;

use crate::entry::Entry;
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
}

/// Redis-backed cache store.
///
/// Values are stored as JSON strings with TTL based on `stale_until`.
/// Requires `V` to implement `Serialize` and `DeserializeOwned`.
pub struct RedisStore<N, V>
where
    N: Clone + Eq + Hash + Display + Send + Sync,
    V: Clone + Serialize + DeserializeOwned + Send + Sync,
{
    connection: MultiplexedConnection,
    _marker: PhantomData<(N, V)>,
}

impl<N, V> RedisStore<N, V>
where
    N: Clone + Eq + Hash + Display + Send + Sync,
    V: Clone + Serialize + DeserializeOwned + Send + Sync,
{
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
            _marker: PhantomData,
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
impl<N, V> Store<N, V> for RedisStore<N, V>
where
    N: Clone + Eq + Hash + Display + Send + Sync,
    V: Clone + Serialize + DeserializeOwned + Send + Sync,
{
    fn name(&self) -> &'static str {
        "redis"
    }

    async fn get(&self, namespace: N, key: &str) -> Result<Option<Entry<V>>, CacheError> {
        let cache_key = build_cache_key(&namespace, key);
        let mut conn = self.connection.clone();

        let result: Option<String> = conn
            .get(&cache_key)
            .await
            .map_err(|e| CacheError::operation("redis", key, format!("GET failed: {}", e)))?;

        match result {
            Some(json_str) => {
                let entry: Entry<V> = serde_json::from_str(&json_str).map_err(|e| {
                    CacheError::operation("redis", key, format!("Deserialization failed: {}", e))
                })?;

                // Check if expired
                let now = now_ms();
                if now >= entry.stale_until {
                    // Entry is expired, delete it in background
                    let mut del_conn = self.connection.clone();
                    let del_key = cache_key.clone();
                    tokio::spawn(async move {
                        let _: Result<(), _> = del_conn.del(del_key).await;
                    });
                    return Ok(None);
                }

                Ok(Some(entry))
            }
            None => Ok(None),
        }
    }

    async fn set(&self, namespace: N, key: &str, entry: Entry<V>) -> Result<(), CacheError> {
        let cache_key = build_cache_key(&namespace, key);
        let mut conn = self.connection.clone();

        let json_str = serde_json::to_string(&entry).map_err(|e| {
            CacheError::operation("redis", key, format!("Serialization failed: {}", e))
        })?;

        let ttl_seconds = Self::calculate_ttl_seconds(entry.stale_until);

        let _: () = conn
            .set_ex(&cache_key, json_str, ttl_seconds)
            .await
            .map_err(|e| CacheError::operation("redis", key, format!("SETEX failed: {}", e)))?;

        Ok(())
    }

    async fn remove(&self, namespace: N, keys: &[&str]) -> Result<(), CacheError> {
        if keys.is_empty() {
            return Ok(());
        }

        let mut conn = self.connection.clone();
        let cache_keys: Vec<String> = keys
            .iter()
            .map(|k| build_cache_key(&namespace, k))
            .collect();

        let _: () = conn.del(&cache_keys).await.map_err(|e| {
            CacheError::operation("redis", &cache_keys.join(","), format!("DEL failed: {}", e))
        })?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: These tests require a running Redis instance.
    // Run with: cargo test --features redis-tests -- --ignored

    #[derive(Clone, Eq, PartialEq, Hash)]
    enum TestNamespace {
        Users,
    }

    impl Display for TestNamespace {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                TestNamespace::Users => write!(f, "users"),
            }
        }
    }

    #[tokio::test]
    #[ignore = "requires running Redis instance"]
    async fn test_redis_get_set_remove() {
        let config = RedisStoreConfig {
            url: "redis://localhost:6379".to_string(),
        };

        let store: RedisStore<TestNamespace, String> = RedisStore::new(config).await.unwrap();

        // Initially empty
        let result = store.get(TestNamespace::Users, "test_key").await.unwrap();
        assert!(result.is_none());

        // Set a value
        let now = now_ms();
        let entry = Entry::new("test_value".to_string(), now + 60_000, now + 300_000);
        store
            .set(TestNamespace::Users, "test_key", entry)
            .await
            .unwrap();

        // Get the value
        let result = store.get(TestNamespace::Users, "test_key").await.unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().value, "test_value");

        // Remove the value
        store
            .remove(TestNamespace::Users, &["test_key"])
            .await
            .unwrap();

        // Should be gone
        let result = store.get(TestNamespace::Users, "test_key").await.unwrap();
        assert!(result.is_none());
    }
}
