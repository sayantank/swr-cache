use serde::{Serialize, de::DeserializeOwned};
use std::future::Future;
use std::sync::Arc;

use crate::error::CacheError;
use crate::store::Store;
use crate::swr::{SetOptions, SwrCache};
use crate::tiered::TieredStore;

/// High-level cache API that combines TieredStore with SwrCache.
///
/// This provides a simple interface for caching with stale-while-revalidate semantics
/// and multi-tier storage. Each cache is isolated by a namespace string.
///
/// `Cache` is type-agnostic at the store level, meaning the same store instances
/// can be reused across multiple `Cache<V>` instances with different types.
#[derive(Clone)]
pub struct Cache<V>
where
    V: Clone + Send + Sync,
{
    namespace: String,
    swr_cache: SwrCache<V>,
}

impl<V> Cache<V>
where
    V: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
{
    /// Create a new Cache with the given stores and timing configuration.
    ///
    /// # Arguments
    /// * `namespace` - The namespace string to isolate this cache
    /// * `stores` - List of type-agnostic stores to use (checked in order, first hit wins)
    /// * `fresh_ms` - Time in milliseconds before an entry becomes stale
    /// * `stale_ms` - Time in milliseconds before an entry expires completely
    ///
    /// # Example
    /// ```ignore
    /// let l1_store = Arc::new(MokaStore::new(MokaStoreConfig::default()));
    /// let l2_store = Arc::new(RedisStore::new(config).await?);
    ///
    /// let apikey_cache: Cache<ApiKey> = Cache::new(
    ///     "apikeys",
    ///     vec![l1_store.clone(), l2_store.clone()],
    ///     60_000,
    ///     300_000,
    /// );
    /// ```
    pub fn new(namespace: &str, stores: Vec<Arc<dyn Store>>, fresh_ms: i64, stale_ms: i64) -> Self {
        let tiered_store = Arc::new(TieredStore::from_stores(stores));
        let swr_cache = SwrCache::new(tiered_store, fresh_ms, stale_ms);
        Cache {
            namespace: namespace.to_string(),
            swr_cache,
        }
    }

    /// Create a new Cache with optional stores (for dynamic configuration).
    ///
    /// `None` values are filtered out, allowing conditional store inclusion.
    ///
    /// # Example
    /// ```ignore
    /// let cache: Cache<User> = Cache::with_optional_stores(
    ///     "users",
    ///     vec![
    ///         Some(memory_store),
    ///         if enable_redis { Some(redis_store) } else { None },
    ///     ],
    ///     60_000,
    ///     300_000,
    /// );
    /// ```
    pub fn with_optional_stores(
        namespace: &str,
        stores: Vec<Option<Arc<dyn Store>>>,
        fresh_ms: i64,
        stale_ms: i64,
    ) -> Self {
        let tiered_store = Arc::new(TieredStore::new(stores));
        let swr_cache = SwrCache::new(tiered_store, fresh_ms, stale_ms);
        Cache {
            namespace: namespace.to_string(),
            swr_cache,
        }
    }

    /// Return the cached value.
    ///
    /// Returns `None` for cache misses.
    pub async fn get(&self, key: &str) -> Result<Option<V>, CacheError> {
        self.swr_cache.get(&self.namespace, key).await
    }

    /// Set the value in the cache.
    pub async fn set(&self, key: &str, value: V) -> Result<(), CacheError> {
        self.swr_cache.set(&self.namespace, key, value, None).await
    }

    /// Set the value in the cache with custom timing options.
    pub async fn set_with_options(
        &self,
        key: &str,
        value: V,
        fresh_ms: i64,
        stale_ms: i64,
    ) -> Result<(), CacheError> {
        self.swr_cache
            .set(
                &self.namespace,
                key,
                value,
                Some(SetOptions { fresh_ms, stale_ms }),
            )
            .await
    }

    /// Remove the key from the cache.
    pub async fn remove(&self, key: &str) -> Result<(), CacheError> {
        self.swr_cache.remove(&self.namespace, key).await
    }

    /// Stale-while-revalidate: Get the cached value or load from origin.
    ///
    /// This method implements the SWR pattern:
    /// - If the value is fresh, return it immediately
    /// - If the value is stale, return it and revalidate in the background
    /// - If the value is missing or expired, load from origin
    ///
    /// # Arguments
    /// * `key` - The cache key
    /// * `load_from_origin` - Function to load the value if not cached or stale
    ///
    /// # Example
    /// ```ignore
    /// let user = cache.swr("user:123", |key| async move {
    ///     db.get_user(&key).await
    /// }).await?;
    /// ```
    pub async fn swr<F, Fut>(&self, key: &str, load_from_origin: F) -> Result<Option<V>, CacheError>
    where
        F: FnOnce(String) -> Fut + Send + 'static,
        Fut: Future<Output = Option<V>> + Send,
    {
        self.swr_cache
            .swr(&self.namespace, key, load_from_origin)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stores::memory::{HashMapStore, HashMapStoreConfig};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn test_cache_basic_operations() {
        let store: Arc<dyn Store> = Arc::new(HashMapStore::new(HashMapStoreConfig::default()));
        let users_cache: Cache<String> = Cache::new("users", vec![store.clone()], 60_000, 300_000);
        let sessions_cache: Cache<String> = Cache::new("sessions", vec![store], 60_000, 300_000);

        // Set and get
        users_cache
            .set("user:1", "Alice".to_string())
            .await
            .unwrap();

        let result = users_cache.get("user:1").await.unwrap();
        assert_eq!(result, Some("Alice".to_string()));

        // Different namespace should not find it
        let result = sessions_cache.get("user:1").await.unwrap();
        assert!(result.is_none());

        // Remove
        users_cache.remove("user:1").await.unwrap();
        let result = users_cache.get("user:1").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_cache_swr() {
        let store: Arc<dyn Store> = Arc::new(HashMapStore::new(HashMapStoreConfig::default()));
        let cache: Cache<String> = Cache::new("users", vec![store], 60_000, 300_000);

        let call_count = Arc::new(AtomicUsize::new(0));
        let call_count_clone = call_count.clone();

        // First call - cache miss
        let result = cache
            .swr("user:1", move |key| {
                let count = call_count_clone.clone();
                async move {
                    // Verify we receive the actual key
                    assert_eq!(key, "user:1");
                    count.fetch_add(1, Ordering::SeqCst);
                    Some("Bob".to_string())
                }
            })
            .await
            .unwrap();

        assert_eq!(result, Some("Bob".to_string()));
        assert_eq!(call_count.load(Ordering::SeqCst), 1);

        // Wait for background cache
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Second call - cache hit
        let call_count_clone = call_count.clone();
        let result = cache
            .swr("user:1", move |_key| {
                let count = call_count_clone.clone();
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    Some("Should not be called".to_string())
                }
            })
            .await
            .unwrap();

        assert_eq!(result, Some("Bob".to_string()));
        // Origin should not have been called again
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }
}
