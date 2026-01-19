use serde::{Serialize, de::DeserializeOwned};
use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use tokio::sync::{RwLock, oneshot};

use crate::entry::{Entry, StorageMode, StoredEntry};
use crate::error::CacheError;
use crate::store::Store;
use crate::utils::{build_cache_key, now_ms};

/// Type alias for the complex revalidation state type.
type RevalidationState<V> =
    Arc<RwLock<HashMap<String, Arc<tokio::sync::Mutex<Option<oneshot::Receiver<Option<V>>>>>>>>;

/// Result of an internal get operation.
struct GetResult<V> {
    value: Option<V>,
    revalidate: bool,
    is_expired: bool,
}

/// Options for setting cache entries.
pub struct SetOptions {
    /// Time in milliseconds until the entry becomes stale.
    pub fresh_ms: i64,
    /// Time in milliseconds until the entry expires completely.
    pub stale_ms: i64,
}

/// Internal cache implementation with stale-while-revalidate support.
///
/// SwrCache handles type conversion between typed values (`V`) and type-erased
/// storage (`StoredEntry`). It requires `V` to implement `Serialize + DeserializeOwned`
/// to support conversion between typed and serialized formats.
pub struct SwrCache<V>
where
    V: Clone + Send + Sync,
{
    store: Arc<dyn Store>,
    fresh_ms: i64,
    stale_ms: i64,
    /// To prevent concurrent revalidation of the same data, all revalidations are deduplicated.
    revalidating: RevalidationState<V>,
}

impl<V> Clone for SwrCache<V>
where
    V: Clone + Send + Sync,
{
    fn clone(&self) -> Self {
        SwrCache {
            store: Arc::clone(&self.store),
            fresh_ms: self.fresh_ms,
            stale_ms: self.stale_ms,
            revalidating: Arc::clone(&self.revalidating),
        }
    }
}

impl<V> SwrCache<V>
where
    V: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
{
    /// Create a new SWR cache.
    ///
    /// # Arguments
    /// * `store` - The underlying store implementation
    /// * `fresh_ms` - Default time in milliseconds before an entry becomes stale
    /// * `stale_ms` - Default time in milliseconds before an entry expires completely
    pub fn new(store: Arc<dyn Store>, fresh_ms: i64, stale_ms: i64) -> Self {
        SwrCache {
            store,
            fresh_ms,
            stale_ms,
            revalidating: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Return the cached value.
    ///
    /// The response will be `None` for cache misses.
    pub async fn get(&self, namespace: &str, key: &str) -> Result<Option<V>, CacheError> {
        let result = self.get_internal(namespace, key).await?;
        Ok(result.value)
    }

    /// Internal get that also indicates if revalidation is needed.
    async fn get_internal(&self, namespace: &str, key: &str) -> Result<GetResult<V>, CacheError> {
        let res = self.store.get(namespace, key).await?;

        let Some(stored_entry) = res else {
            return Ok(GetResult {
                value: None,
                revalidate: false,
                is_expired: false,
            });
        };

        // Convert StoredEntry to Entry<V>
        let entry: Entry<V> = stored_entry.into_typed()?;

        let now = now_ms();

        // Entry has completely expired
        if now >= entry.stale_until {
            // Don't remove - let it serve as fallback
            // Store implementations with disable_expiration will handle this
            return Ok(GetResult {
                value: Some(entry.value),
                revalidate: true,
                is_expired: true,
            });
        }

        // Entry is stale but still usable
        if now >= entry.fresh_until {
            return Ok(GetResult {
                value: Some(entry.value),
                revalidate: true,
                is_expired: false,
            });
        }

        // Entry is fresh
        Ok(GetResult {
            value: Some(entry.value),
            revalidate: false,
            is_expired: false,
        })
    }

    /// Set the value in the cache.
    pub async fn set(
        &self,
        namespace: &str,
        key: &str,
        value: V,
        opts: Option<SetOptions>,
    ) -> Result<(), CacheError> {
        let now = now_ms();
        let fresh_ms = opts.as_ref().map(|o| o.fresh_ms).unwrap_or(self.fresh_ms);
        let stale_ms = opts.as_ref().map(|o| o.stale_ms).unwrap_or(self.stale_ms);

        // Convert to StoredEntry based on store's preference
        let stored_entry = match self.store.storage_mode() {
            StorageMode::Typed => StoredEntry::from_typed(value, now + fresh_ms, now + stale_ms),
            StorageMode::Serialized => {
                let json_data =
                    serde_json::to_string(&Entry::new(value, now + fresh_ms, now + stale_ms))
                        .map_err(|e| {
                            CacheError::Serialization(format!("Serialization failed: {}", e))
                        })?;
                StoredEntry::from_serialized(json_data, now + fresh_ms, now + stale_ms)
            }
        };

        self.store.set(namespace, key, stored_entry).await
    }

    /// Removes the key from the cache.
    pub async fn remove(&self, namespace: &str, key: &str) -> Result<(), CacheError> {
        self.store.remove(namespace, &[key]).await
    }

    /// Cache a value in the background.
    fn cache_value(&self, namespace: &str, key: &str, value: V) {
        let store = self.store.clone();
        let storage_mode = store.storage_mode();
        let namespace = namespace.to_string();
        let key = key.to_string();
        let now = now_ms();
        let fresh_until = now + self.fresh_ms;
        let stale_until = now + self.stale_ms;

        tokio::spawn(async move {
            let stored_entry = match storage_mode {
                StorageMode::Typed => StoredEntry::from_typed(value, fresh_until, stale_until),
                StorageMode::Serialized => {
                    match serde_json::to_string(&Entry::new(value, fresh_until, stale_until)) {
                        Ok(json_data) => {
                            StoredEntry::from_serialized(json_data, fresh_until, stale_until)
                        }
                        Err(_) => return, // Skip caching on serialization error
                    }
                }
            };
            let _ = store.set(&namespace, &key, stored_entry).await;
        });
    }

    /// Stale-while-revalidate: Get the cached value or load from origin.
    ///
    /// This method implements the SWR pattern:
    /// - If the value is fresh, return it immediately
    /// - If the value is stale, return it and revalidate in the background
    /// - If the value is missing or expired, load from origin
    ///
    /// # Arguments
    /// * `namespace` - The cache namespace
    /// * `key` - The cache key
    /// * `load_from_origin` - Function to load the value if not cached (receives the key)
    pub async fn swr<F, Fut>(
        &self,
        namespace: &str,
        key: &str,
        load_from_origin: F,
    ) -> Result<Option<V>, CacheError>
    where
        F: FnOnce(String) -> Fut + Send + 'static,
        Fut: Future<Output = Option<V>> + Send,
    {
        let res = self.get_internal(namespace, key).await;

        match res {
            Ok(GetResult {
                value: Some(value),
                revalidate: true,
                is_expired: false,
            }) => {
                // Return stale value, revalidate in background
                self.spawn_revalidation(namespace, key, load_from_origin);
                Ok(Some(value))
            }
            Ok(GetResult {
                value: Some(value),
                revalidate: true,
                is_expired: true,
            }) => {
                // Expired data - try origin synchronously, fallback to expired
                match self
                    .deduplicated_load_from_origin(namespace, key, load_from_origin)
                    .await
                {
                    Ok(Some(fresh_value)) => {
                        // Origin succeeded - cache and return fresh
                        self.cache_value(namespace, key, fresh_value.clone());
                        Ok(Some(fresh_value))
                    }
                    Ok(None) | Err(_) => {
                        // Origin failed - return expired data as fallback
                        Ok(Some(value))
                    }
                }
            }
            Ok(GetResult {
                value: Some(value),
                revalidate: false,
                is_expired: false,
            }) => {
                // Return fresh value
                Ok(Some(value))
            }
            Ok(GetResult {
                value: Some(_),
                revalidate: false,
                is_expired: true,
            }) => {
                // This shouldn't happen (expired should always trigger revalidation)
                // but handle it defensively by treating as cache miss
                match self
                    .deduplicated_load_from_origin(namespace, key, load_from_origin)
                    .await
                {
                    Ok(value) => {
                        if let Some(ref v) = value {
                            self.cache_value(namespace, key, v.clone());
                        }
                        Ok(value)
                    }
                    Err(e) => Err(CacheError::operation(self.store.name(), key, e.to_string())),
                }
            }
            Ok(GetResult { value: None, .. }) | Err(_) => {
                // Cache miss or error - load from origin
                match self
                    .deduplicated_load_from_origin(namespace, key, load_from_origin)
                    .await
                {
                    Ok(value) => {
                        // Cache in background
                        if let Some(ref v) = value {
                            self.cache_value(namespace, key, v.clone());
                        }
                        Ok(value)
                    }
                    Err(e) => Err(CacheError::operation(self.store.name(), key, e.to_string())),
                }
            }
        }
    }

    /// Spawn a background revalidation task.
    fn spawn_revalidation<F, Fut>(&self, namespace: &str, key: &str, load_from_origin: F)
    where
        F: FnOnce(String) -> Fut + Send + 'static,
        Fut: Future<Output = Option<V>> + Send,
    {
        let store = self.store.clone();
        let storage_mode = store.storage_mode();
        let revalidating = self.revalidating.clone();
        let namespace = namespace.to_string();
        let key = key.to_string();
        let revalidate_key = build_cache_key(&namespace, &key);
        let fresh_ms = self.fresh_ms;
        let stale_ms = self.stale_ms;

        tokio::spawn(async move {
            // Check if already revalidating
            {
                let map = revalidating.read().await;
                if map.contains_key(&revalidate_key) {
                    return;
                }
            }

            // Mark as revalidating
            {
                let mut map = revalidating.write().await;
                map.insert(
                    revalidate_key.clone(),
                    Arc::new(tokio::sync::Mutex::new(None)),
                );
            }

            // Load from origin - pass the actual key, not the composite cache key
            let value = load_from_origin(key.clone()).await;

            // Update cache
            if let Some(v) = value {
                let now = crate::utils::now_ms();
                let fresh_until = now + fresh_ms;
                let stale_until = now + stale_ms;

                let stored_entry = match storage_mode {
                    StorageMode::Typed => StoredEntry::from_typed(v, fresh_until, stale_until),
                    StorageMode::Serialized => {
                        match serde_json::to_string(&Entry::new(v, fresh_until, stale_until)) {
                            Ok(json_data) => {
                                StoredEntry::from_serialized(json_data, fresh_until, stale_until)
                            }
                            Err(_) => {
                                // Remove from revalidating and return
                                let mut map = revalidating.write().await;
                                map.remove(&revalidate_key);
                                return;
                            }
                        }
                    }
                };
                let _ = store.set(&namespace, &key, stored_entry).await;
            }

            // Remove from revalidating
            {
                let mut map = revalidating.write().await;
                map.remove(&revalidate_key);
            }
        });
    }

    /// Deduplicate concurrent loads from origin.
    async fn deduplicated_load_from_origin<F, Fut>(
        &self,
        namespace: &str,
        key: &str,
        load_from_origin: F,
    ) -> Result<Option<V>, Box<dyn std::error::Error + Send + Sync>>
    where
        F: FnOnce(String) -> Fut + Send + 'static,
        Fut: Future<Output = Option<V>> + Send,
    {
        let revalidate_key = build_cache_key(&namespace, key);

        // Check if already loading
        {
            let map = self.revalidating.read().await;
            if map.contains_key(&revalidate_key) {
                // Wait for the other request to complete
                // For simplicity, we just wait a bit and retry from cache
                drop(map);
                tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

                // Try cache again
                if let Ok(Some(stored_entry)) = self.store.get(namespace, key).await
                    && let Ok(entry) = stored_entry.into_typed::<V>()
                {
                    return Ok(Some(entry.value));
                }
            }
        }

        // Mark as loading
        {
            let mut map = self.revalidating.write().await;
            map.insert(
                revalidate_key.clone(),
                Arc::new(tokio::sync::Mutex::new(None)),
            );
        }

        // Load from origin - pass the actual key, not the composite cache key
        let result = load_from_origin(key.to_string()).await;

        // Remove from revalidating
        {
            let mut map = self.revalidating.write().await;
            map.remove(&revalidate_key);
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stores::memory::{HashMapStore, HashMapStoreConfig};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn test_swr_cache_miss_loads_from_origin() {
        let store: Arc<dyn Store> = Arc::new(HashMapStore::new(HashMapStoreConfig::default()));
        let cache = SwrCache::new(store, 60_000, 300_000);

        let call_count = Arc::new(AtomicUsize::new(0));
        let call_count_clone = call_count.clone();

        let result = cache
            .swr("users", "key1", move |key| {
                let count = call_count_clone.clone();
                async move {
                    // Verify we receive the actual key, not composite
                    assert_eq!(key, "key1");
                    count.fetch_add(1, Ordering::SeqCst);
                    Some("loaded_value".to_string())
                }
            })
            .await
            .unwrap();

        assert_eq!(result, Some("loaded_value".to_string()));
        assert_eq!(call_count.load(Ordering::SeqCst), 1);

        // Wait for background cache set
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Second call should hit cache
        let call_count_clone = call_count.clone();
        let result = cache
            .swr("users", "key1", move |_key| {
                let count = call_count_clone.clone();
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    Some("should_not_be_called".to_string())
                }
            })
            .await
            .unwrap();

        assert_eq!(result, Some("loaded_value".to_string()));
        // Origin should not have been called again
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_get_and_set() {
        let store: Arc<dyn Store> = Arc::new(HashMapStore::new(HashMapStoreConfig::default()));
        let cache = SwrCache::new(store, 60_000, 300_000);

        // Initially empty
        let result = cache.get("users", "key1").await.unwrap();
        assert!(result.is_none());

        // Set a value
        cache
            .set("users", "key1", "value1".to_string(), None)
            .await
            .unwrap();

        // Get the value
        let result = cache.get("users", "key1").await.unwrap();
        assert_eq!(result, Some("value1".to_string()));

        // Remove the value
        cache.remove("users", "key1").await.unwrap();

        // Should be gone
        let result = cache.get("users", "key1").await.unwrap();
        assert!(result.is_none());
    }
}
