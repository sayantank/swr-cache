use std::fmt::Display;
use std::future::Future;
use std::hash::Hash;
use std::sync::Arc;

use crate::error::CacheError;
use crate::store::Store;
use crate::swr::{SetOptions, SwrCache};
use crate::tiered::TieredStore;

/// High-level cache API that combines TieredStore with SwrCache.
///
/// This provides a simple interface for caching with stale-while-revalidate semantics
/// and multi-tier storage.
#[derive(Clone)]
pub struct Namespace<N, V>
where
    N: Clone + Eq + Hash + Display + Send + Sync,
    V: Clone + Send + Sync,
{
    cache: SwrCache<N, V>,
}

impl<N, V> Namespace<N, V>
where
    N: Clone + Eq + Hash + Display + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    /// Create a new Namespace with the given stores and timing configuration.
    ///
    /// # Arguments
    /// * `stores` - List of stores to use (checked in order, first hit wins)
    /// * `fresh_ms` - Time in milliseconds before an entry becomes stale
    /// * `stale_ms` - Time in milliseconds before an entry expires completely
    ///
    /// # Example
    /// ```ignore
    /// let memory = Arc::new(HashMapStore::new(HashMapStoreConfig::default()));
    /// let cache = Namespace::new(vec![memory], 60_000, 300_000);
    /// ```
    pub fn new(stores: Vec<Arc<dyn Store<N, V>>>, fresh_ms: i64, stale_ms: i64) -> Self {
        let tiered_store = Arc::new(TieredStore::from_stores(stores));
        let cache = SwrCache::new(tiered_store, fresh_ms, stale_ms);
        Namespace { cache }
    }

    /// Create a new Namespace with optional stores (for dynamic configuration).
    ///
    /// `None` values are filtered out, allowing conditional store inclusion.
    ///
    /// # Example
    /// ```ignore
    /// let cache = Namespace::with_optional_stores(
    ///     vec![
    ///         Some(memory_store),
    ///         if enable_redis { Some(redis_store) } else { None },
    ///     ],
    ///     60_000,
    ///     300_000,
    /// );
    /// ```
    pub fn with_optional_stores(
        stores: Vec<Option<Arc<dyn Store<N, V>>>>,
        fresh_ms: i64,
        stale_ms: i64,
    ) -> Self {
        let tiered_store = Arc::new(TieredStore::new(stores));
        let cache = SwrCache::new(tiered_store, fresh_ms, stale_ms);
        Namespace { cache }
    }

    /// Return the cached value.
    ///
    /// Returns `None` for cache misses.
    pub async fn get(&self, namespace: N, key: &str) -> Result<Option<V>, CacheError> {
        self.cache.get(namespace, key).await
    }

    /// Set the value in the cache.
    pub async fn set(&self, namespace: N, key: &str, value: V) -> Result<(), CacheError> {
        self.cache.set(namespace, key, value, None).await
    }

    /// Set the value in the cache with custom timing options.
    pub async fn set_with_options(
        &self,
        namespace: N,
        key: &str,
        value: V,
        fresh_ms: i64,
        stale_ms: i64,
    ) -> Result<(), CacheError> {
        self.cache
            .set(
                namespace,
                key,
                value,
                Some(SetOptions { fresh_ms, stale_ms }),
            )
            .await
    }

    /// Remove the key from the cache.
    pub async fn remove(&self, namespace: N, key: &str) -> Result<(), CacheError> {
        self.cache.remove(namespace, key).await
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
    /// * `load_from_origin` - Function to load the value if not cached or stale
    ///
    /// # Example
    /// ```ignore
    /// let user = cache.swr(Namespace::Users, "user:123", |key| async {
    ///     db.get_user(&key).await
    /// }).await?;
    /// ```
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
        self.cache.swr(namespace, key, load_from_origin).await
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
        Sessions,
    }

    impl Display for TestNamespace {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                TestNamespace::Users => write!(f, "users"),
                TestNamespace::Sessions => write!(f, "sessions"),
            }
        }
    }

    #[tokio::test]
    async fn test_namespace_basic_operations() {
        let store: Arc<dyn Store<TestNamespace, String>> =
            Arc::new(HashMapStore::new(HashMapStoreConfig::default()));
        let cache = Namespace::new(vec![store], 60_000, 300_000);

        // Set and get
        cache
            .set(TestNamespace::Users, "user:1", "Alice".to_string())
            .await
            .unwrap();

        let result = cache.get(TestNamespace::Users, "user:1").await.unwrap();
        assert_eq!(result, Some("Alice".to_string()));

        // Different namespace should not find it
        let result = cache.get(TestNamespace::Sessions, "user:1").await.unwrap();
        assert!(result.is_none());

        // Remove
        cache.remove(TestNamespace::Users, "user:1").await.unwrap();
        let result = cache.get(TestNamespace::Users, "user:1").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_namespace_swr() {
        let store: Arc<dyn Store<TestNamespace, String>> =
            Arc::new(HashMapStore::new(HashMapStoreConfig::default()));
        let cache = Namespace::new(vec![store], 60_000, 300_000);

        let call_count = Arc::new(AtomicUsize::new(0));
        let call_count_clone = call_count.clone();

        // First call - cache miss
        let result = cache
            .swr(TestNamespace::Users, "user:1", move |_key| {
                let count = call_count_clone.clone();
                async move {
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
            .swr(TestNamespace::Users, "user:1", move |_key| {
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
