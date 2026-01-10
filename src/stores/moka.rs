use async_trait::async_trait;
use moka::future::Cache;
use std::fmt::Display;
use std::hash::Hash;
use std::marker::PhantomData;
use std::time::Duration;

use crate::entry::Entry;
use crate::error::CacheError;
use crate::store::Store;
use crate::utils::{build_cache_key, now_ms};

/// Configuration for MokaStore.
#[derive(Debug, Clone)]
pub struct MokaStoreConfig {
    /// Maximum number of entries the cache can hold.
    pub max_capacity: u64,

    /// Time to live: entries are expired after this duration from insertion.
    /// `None` means entries never expire based on time (only by size limit).
    pub time_to_live: Option<Duration>,

    /// Time to idle: entries are expired if not accessed within this duration.
    /// `None` means entries don't expire based on idle time.
    pub time_to_idle: Option<Duration>,
}

impl Default for MokaStoreConfig {
    fn default() -> Self {
        MokaStoreConfig {
            max_capacity: 10_000,
            time_to_live: None,
            time_to_idle: None,
        }
    }
}

/// High-performance concurrent cache store using Moka.
///
/// MokaStore provides:
/// - Lock-free concurrent access for reads and writes
/// - Automatic background eviction with configurable policies
/// - Excellent performance under high concurrency (>8 threads)
/// - Suitable for large cache sizes (>10,000 items)
///
/// Use this store for production workloads requiring:
/// - High throughput
/// - Low P99 latency
/// - Predictable performance under load
pub struct MokaStore<N, V>
where
    N: Clone + Eq + Hash + Display + Send + Sync,
    V: Clone + Send + Sync,
{
    cache: Cache<String, Entry<V>>,
    _marker: PhantomData<N>,
}

impl<N, V> MokaStore<N, V>
where
    N: Clone + Eq + Hash + Display + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    /// Create a new MokaStore with the given configuration.
    ///
    /// # Example
    /// ```ignore
    /// let config = MokaStoreConfig {
    ///     max_capacity: 10_000,
    ///     time_to_live: Some(Duration::from_secs(300)),
    ///     time_to_idle: Some(Duration::from_secs(60)),
    /// };
    /// let store = MokaStore::new(config);
    /// ```
    pub fn new(config: MokaStoreConfig) -> Self {
        let mut builder = Cache::builder().max_capacity(config.max_capacity);

        if let Some(ttl) = config.time_to_live {
            builder = builder.time_to_live(ttl);
        }

        if let Some(tti) = config.time_to_idle {
            builder = builder.time_to_idle(tti);
        }

        MokaStore {
            cache: builder.build(),
            _marker: PhantomData,
        }
    }

    /// Get cache statistics (for monitoring/debugging).
    pub fn stats(&self) -> (u64, u64) {
        let entry_count = self.cache.entry_count();
        let weighted_size = self.cache.weighted_size();
        (entry_count, weighted_size)
    }
}

#[async_trait]
impl<N, V> Store<N, V> for MokaStore<N, V>
where
    N: Clone + Eq + Hash + Display + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    fn name(&self) -> &'static str {
        "moka"
    }

    async fn get(&self, namespace: N, key: &str) -> Result<Option<Entry<V>>, CacheError> {
        let cache_key = build_cache_key(&namespace, key);

        match self.cache.get(&cache_key).await {
            Some(entry) => {
                let now = now_ms();

                // Check if expired based on our Entry timestamps
                if now >= entry.stale_until {
                    // Entry is expired, remove it
                    self.cache.invalidate(&cache_key).await;
                    return Ok(None);
                }

                Ok(Some(entry))
            }
            None => Ok(None),
        }
    }

    async fn set(&self, namespace: N, key: &str, entry: Entry<V>) -> Result<(), CacheError> {
        let cache_key = build_cache_key(&namespace, key);

        // Insert into Moka cache
        // Moka handles eviction automatically based on capacity
        self.cache.insert(cache_key, entry).await;

        Ok(())
    }

    async fn remove(&self, namespace: N, keys: &[&str]) -> Result<(), CacheError> {
        for key in keys {
            let cache_key = build_cache_key(&namespace, key);
            self.cache.invalidate(&cache_key).await;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    async fn test_get_set_remove() {
        let store: MokaStore<TestNamespace, String> = MokaStore::new(MokaStoreConfig::default());

        // Initially empty
        let result = store.get(TestNamespace::Users, "key1").await.unwrap();
        assert!(result.is_none());

        // Set a value
        let now = now_ms();
        let entry = Entry::new("value1".to_string(), now + 60_000, now + 300_000);
        store
            .set(TestNamespace::Users, "key1", entry)
            .await
            .unwrap();

        // Get the value
        let result = store.get(TestNamespace::Users, "key1").await.unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().value, "value1");

        // Remove the value
        store.remove(TestNamespace::Users, &["key1"]).await.unwrap();

        // Should be gone
        let result = store.get(TestNamespace::Users, "key1").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_expired_entry_removed() {
        let store: MokaStore<TestNamespace, String> = MokaStore::new(MokaStoreConfig::default());

        // Set a value that's already expired
        let now = now_ms();
        let entry = Entry::new("value1".to_string(), now - 1000, now - 500);
        store
            .set(TestNamespace::Users, "expired_key", entry)
            .await
            .unwrap();

        // Should return None and remove the entry
        let result = store
            .get(TestNamespace::Users, "expired_key")
            .await
            .unwrap();
        assert!(result.is_none());

        // Verify it was removed
        let result = store
            .get(TestNamespace::Users, "expired_key")
            .await
            .unwrap();
        assert!(result.is_none());
    }
}
