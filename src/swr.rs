use std::collections::HashMap;
use std::fmt::Display;
use std::future::Future;
use std::hash::Hash;
use std::sync::Arc;
use tokio::sync::{RwLock, oneshot};

use crate::entry::Entry;
use crate::error::CacheError;
use crate::store::Store;
use crate::utils::{build_cache_key, now_ms};

/// Result of an internal get operation.
struct GetResult<V> {
    value: Option<V>,
    revalidate: bool,
}

/// Options for setting cache entries.
pub struct SetOptions {
    /// Time in milliseconds until the entry becomes stale.
    pub fresh_ms: i64,
    /// Time in milliseconds until the entry expires completely.
    pub stale_ms: i64,
}

/// Internal cache implementation with stale-while-revalidate support.
pub struct SwrCache<N, V>
where
    N: Clone + Eq + Hash + Display + Send + Sync,
    V: Clone + Send + Sync,
{
    store: Arc<dyn Store<N, V>>,
    fresh_ms: i64,
    stale_ms: i64,
    /// To prevent concurrent revalidation of the same data, all revalidations are deduplicated.
    revalidating:
        Arc<RwLock<HashMap<String, Arc<tokio::sync::Mutex<Option<oneshot::Receiver<Option<V>>>>>>>>,
}

impl<N, V> Clone for SwrCache<N, V>
where
    N: Clone + Eq + Hash + Display + Send + Sync,
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

impl<N, V> SwrCache<N, V>
where
    N: Clone + Eq + Hash + Display + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    /// Create a new SWR cache.
    ///
    /// # Arguments
    /// * `store` - The underlying store implementation
    /// * `fresh_ms` - Default time in milliseconds before an entry becomes stale
    /// * `stale_ms` - Default time in milliseconds before an entry expires completely
    pub fn new(store: Arc<dyn Store<N, V>>, fresh_ms: i64, stale_ms: i64) -> Self {
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
    pub async fn get(&self, namespace: N, key: &str) -> Result<Option<V>, CacheError> {
        let result = self.get_internal(namespace, key).await?;
        Ok(result.value)
    }

    /// Internal get that also indicates if revalidation is needed.
    async fn get_internal(&self, namespace: N, key: &str) -> Result<GetResult<V>, CacheError> {
        let res = self.store.get(namespace.clone(), key).await?;

        let Some(entry) = res else {
            return Ok(GetResult {
                value: None,
                revalidate: false,
            });
        };

        let now = now_ms();

        // Entry has completely expired
        if now >= entry.stale_until {
            // Remove in background
            let store = self.store.clone();
            let namespace = namespace.clone();
            let key = key.to_string();
            tokio::spawn(async move {
                let _ = store.remove(namespace, &[&key]).await;
            });

            return Ok(GetResult {
                value: None,
                revalidate: false,
            });
        }

        // Entry is stale but still usable
        if now >= entry.fresh_until {
            return Ok(GetResult {
                value: Some(entry.value),
                revalidate: true,
            });
        }

        // Entry is fresh
        Ok(GetResult {
            value: Some(entry.value),
            revalidate: false,
        })
    }

    /// Set the value in the cache.
    pub async fn set(
        &self,
        namespace: N,
        key: &str,
        value: V,
        opts: Option<SetOptions>,
    ) -> Result<(), CacheError> {
        let now = now_ms();
        let fresh_ms = opts.as_ref().map(|o| o.fresh_ms).unwrap_or(self.fresh_ms);
        let stale_ms = opts.as_ref().map(|o| o.stale_ms).unwrap_or(self.stale_ms);

        let entry = Entry::new(value, now + fresh_ms, now + stale_ms);
        self.store.set(namespace, key, entry).await
    }

    /// Removes the key from the cache.
    pub async fn remove(&self, namespace: N, key: &str) -> Result<(), CacheError> {
        self.store.remove(namespace, &[key]).await
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
    /// * `load_from_origin` - Function to load the value if not cached
    pub async fn swr<F, Fut>(
        &self,
        namespace: N,
        key: &str,
        load_from_origin: F,
    ) -> Result<Option<V>, CacheError>
    where
        F: FnOnce(String) -> Fut + Send + 'static,
        Fut: Future<Output = Option<V>> + Send,
    {
        let res = self.get_internal(namespace.clone(), key).await;

        match res {
            Ok(GetResult {
                value: Some(value),
                revalidate: true,
            }) => {
                // Return stale value, but revalidate in background
                self.spawn_revalidation(namespace.clone(), key, load_from_origin);
                Ok(Some(value))
            }
            Ok(GetResult {
                value: Some(value),
                revalidate: false,
            }) => {
                // Return fresh value
                Ok(Some(value))
            }
            Ok(GetResult { value: None, .. }) | Err(_) => {
                // Cache miss or error - load from origin
                match self
                    .deduplicated_load_from_origin(namespace.clone(), key, load_from_origin)
                    .await
                {
                    Ok(value) => {
                        // Cache in background
                        if let Some(ref v) = value {
                            let store = self.store.clone();
                            let namespace = namespace.clone();
                            let key = key.to_string();
                            let now = now_ms();
                            let entry =
                                Entry::new(v.clone(), now + self.fresh_ms, now + self.stale_ms);

                            tokio::spawn(async move {
                                let _ = store.set(namespace, &key, entry).await;
                            });
                        }
                        Ok(value)
                    }
                    Err(e) => Err(CacheError::operation(self.store.name(), key, e.to_string())),
                }
            }
        }
    }

    /// Spawn a background revalidation task.
    fn spawn_revalidation<F, Fut>(&self, namespace: N, key: &str, load_from_origin: F)
    where
        F: FnOnce(String) -> Fut + Send + 'static,
        Fut: Future<Output = Option<V>> + Send,
    {
        let store = self.store.clone();
        let revalidating = self.revalidating.clone();
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

            // Load from origin
            let value = load_from_origin(key.clone()).await;

            // Update cache
            if let Some(ref v) = value {
                let now = crate::utils::now_ms();
                let entry = Entry::new(v.clone(), now + fresh_ms, now + stale_ms);
                let _ = store.set(namespace, &key, entry).await;
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
        namespace: N,
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
                if let Ok(Some(entry)) = self.store.get(namespace, key).await {
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

        // Load from origin
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
    async fn test_swr_cache_miss_loads_from_origin() {
        let store: Arc<dyn Store<TestNamespace, String>> =
            Arc::new(HashMapStore::new(HashMapStoreConfig::default()));
        let cache = SwrCache::new(store, 60_000, 300_000);

        let call_count = Arc::new(AtomicUsize::new(0));
        let call_count_clone = call_count.clone();

        let result = cache
            .swr(TestNamespace::Users, "key1", move |_key| {
                let count = call_count_clone.clone();
                async move {
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
            .swr(TestNamespace::Users, "key1", move |_key| {
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
        let store: Arc<dyn Store<TestNamespace, String>> =
            Arc::new(HashMapStore::new(HashMapStoreConfig::default()));
        let cache = SwrCache::new(store, 60_000, 300_000);

        // Initially empty
        let result = cache.get(TestNamespace::Users, "key1").await.unwrap();
        assert!(result.is_none());

        // Set a value
        cache
            .set(TestNamespace::Users, "key1", "value1".to_string(), None)
            .await
            .unwrap();

        // Get the value
        let result = cache.get(TestNamespace::Users, "key1").await.unwrap();
        assert_eq!(result, Some("value1".to_string()));

        // Remove the value
        cache.remove(TestNamespace::Users, "key1").await.unwrap();

        // Should be gone
        let result = cache.get(TestNamespace::Users, "key1").await.unwrap();
        assert!(result.is_none());
    }
}
