//! Metrics middleware for cache stores.
//!
//! This module provides a `MetricsStore` wrapper that emits metrics for all
//! cache operations (reads, writes, removes) to a user-provided sink.
//!
//! # Example
//!
//! ```ignore
//! use std::sync::Arc;
//! use swr_cache::{Cache, MokaStore, MokaStoreConfig, Store};
//! use swr_cache::{CacheMetric, MetricsSink, MetricsStore};
//!
//! // Create metrics sink
//! let sink = Arc::new(MyMetricsSink::new());
//!
//! // Wrap store with metrics
//! let moka = Arc::new(MokaStore::new(MokaStoreConfig::default()));
//! let store: Arc<dyn Store> = Arc::new(MetricsStore::new(moka, sink.clone()));
//!
//! // Use in Cache - metrics emitted automatically
//! let cache: Cache<String> = Cache::new("users", vec![store], 60_000, 300_000);
//! ```

use async_trait::async_trait;
use std::sync::Arc;
use std::time::Instant;

use crate::entry::{StorageMode, StoredEntry};
use crate::error::CacheError;
use crate::store::Store;
use crate::utils::now_ms;

/// Status of a cache entry on read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheEntryStatus {
    /// Entry is fresh (before fresh_until).
    Fresh,
    /// Entry is stale but usable (between fresh_until and stale_until).
    Stale,
}

/// Metrics emitted by the MetricsStore wrapper.
#[derive(Debug, Clone)]
pub enum CacheMetric {
    /// Emitted on every cache read (get) operation.
    Read {
        /// The cache key that was read.
        key: String,
        /// Whether the key was found in the cache.
        hit: bool,
        /// Status of the entry (only present when hit=true).
        status: Option<CacheEntryStatus>,
        /// Latency of the operation in milliseconds.
        latency_ms: f64,
        /// Name of the store tier (from Store::name()).
        tier: String,
        /// The namespace of the cache operation.
        namespace: String,
    },
    /// Emitted on every cache write (set) operation.
    Write {
        /// The cache key that was written.
        key: String,
        /// Latency of the operation in milliseconds.
        latency_ms: f64,
        /// Name of the store tier (from Store::name()).
        tier: String,
        /// The namespace of the cache operation.
        namespace: String,
    },
    /// Emitted on every cache remove operation.
    Remove {
        /// Number of keys in the remove batch.
        key_count: usize,
        /// First key in the batch (for debugging/identification).
        first_key: Option<String>,
        /// Latency of the operation in milliseconds.
        latency_ms: f64,
        /// Name of the store tier (from Store::name()).
        tier: String,
        /// The namespace of the cache operation.
        namespace: String,
    },
}

/// Trait for receiving cache metrics.
///
/// Implement this trait to collect metrics from `MetricsStore`.
///
/// # Example
///
/// ```ignore
/// use std::sync::Mutex;
/// use async_trait::async_trait;
/// use swr_cache::{CacheMetric, MetricsSink};
///
/// struct BufferedSink {
///     buffer: Mutex<Vec<CacheMetric>>,
/// }
///
/// #[async_trait]
/// impl MetricsSink for BufferedSink {
///     fn emit(&self, metric: CacheMetric) {
///         self.buffer.lock().unwrap().push(metric);
///     }
///
///     async fn flush(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
///         // Send buffered metrics to your backend
///         Ok(())
///     }
/// }
/// ```
#[async_trait]
pub trait MetricsSink: Send + Sync {
    /// Emit a single metric.
    ///
    /// This is called synchronously in the hot path of cache operations.
    /// Implementations should be fast (e.g., buffer metrics in memory).
    fn emit(&self, metric: CacheMetric);

    /// Flush any buffered metrics.
    ///
    /// Called when the caller wants to ensure all metrics are persisted.
    /// This is typically called at shutdown or at periodic intervals.
    async fn flush(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
}

/// A store wrapper that emits metrics for all operations.
///
/// `MetricsStore` wraps any `Store` implementation and emits metrics
/// for read, write, and remove operations to a user-provided sink.
///
/// # Example
///
/// ```ignore
/// let inner = Arc::new(MokaStore::new(MokaStoreConfig::default()));
/// let sink = Arc::new(MyMetricsSink::new());
/// let store: Arc<dyn Store> = Arc::new(MetricsStore::new(inner, sink));
///
/// // Use store normally - metrics are emitted automatically
/// store.get("users", "user:123").await?;
/// ```
pub struct MetricsStore {
    inner: Arc<dyn Store>,
    sink: Arc<dyn MetricsSink>,
    tier_name: String,
}

impl MetricsStore {
    /// Create a new MetricsStore wrapping the given store.
    ///
    /// # Arguments
    /// * `inner` - The store to wrap
    /// * `sink` - The metrics sink to emit metrics to
    pub fn new(inner: Arc<dyn Store>, sink: Arc<dyn MetricsSink>) -> Self {
        let tier_name = inner.name().to_string();
        MetricsStore {
            inner,
            sink,
            tier_name,
        }
    }

    /// Get a reference to the metrics sink.
    pub fn sink(&self) -> &Arc<dyn MetricsSink> {
        &self.sink
    }

    fn elapsed_ms(start: Instant) -> f64 {
        start.elapsed().as_secs_f64() * 1000.0
    }
}

#[async_trait]
impl Store for MetricsStore {
    fn name(&self) -> &'static str {
        "metrics"
    }

    fn storage_mode(&self) -> StorageMode {
        self.inner.storage_mode()
    }

    async fn get(&self, namespace: &str, key: &str) -> Result<Option<StoredEntry>, CacheError> {
        let start = Instant::now();
        let result = self.inner.get(namespace, key).await;
        let latency_ms = Self::elapsed_ms(start);

        let (hit, status) = match &result {
            Ok(Some(entry)) => {
                let now = now_ms();
                let status = if entry.is_fresh(now) {
                    Some(CacheEntryStatus::Fresh)
                } else if entry.is_stale(now) {
                    Some(CacheEntryStatus::Stale)
                } else {
                    None
                };
                (true, status)
            }
            Ok(None) => (false, None),
            Err(_) => (false, None),
        };

        self.sink.emit(CacheMetric::Read {
            key: key.to_string(),
            hit,
            status,
            latency_ms,
            tier: self.tier_name.clone(),
            namespace: namespace.to_string(),
        });

        result
    }

    async fn set(&self, namespace: &str, key: &str, entry: StoredEntry) -> Result<(), CacheError> {
        let start = Instant::now();
        let result = self.inner.set(namespace, key, entry).await;
        let latency_ms = Self::elapsed_ms(start);

        self.sink.emit(CacheMetric::Write {
            key: key.to_string(),
            latency_ms,
            tier: self.tier_name.clone(),
            namespace: namespace.to_string(),
        });

        result
    }

    async fn remove(&self, namespace: &str, keys: &[&str]) -> Result<(), CacheError> {
        let start = Instant::now();
        let result = self.inner.remove(namespace, keys).await;
        let latency_ms = Self::elapsed_ms(start);

        self.sink.emit(CacheMetric::Remove {
            key_count: keys.len(),
            first_key: keys.first().map(|k| k.to_string()),
            latency_ms,
            tier: self.tier_name.clone(),
            namespace: namespace.to_string(),
        });

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stores::memory::{HashMapStore, HashMapStoreConfig};
    use std::sync::Mutex;

    struct TestSink {
        metrics: Mutex<Vec<CacheMetric>>,
    }

    impl TestSink {
        fn new() -> Self {
            TestSink {
                metrics: Mutex::new(Vec::new()),
            }
        }

        fn take_metrics(&self) -> Vec<CacheMetric> {
            std::mem::take(&mut *self.metrics.lock().unwrap())
        }
    }

    #[async_trait]
    impl MetricsSink for TestSink {
        fn emit(&self, metric: CacheMetric) {
            self.metrics.lock().unwrap().push(metric);
        }

        async fn flush(&self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
            Ok(())
        }
    }

    fn now_ms_test() -> i64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
    }

    #[tokio::test]
    async fn test_read_miss() {
        let inner: Arc<dyn Store> = Arc::new(HashMapStore::new(HashMapStoreConfig::default()));
        let sink = Arc::new(TestSink::new());
        let store = MetricsStore::new(inner, sink.clone());

        let result = store.get("users", "key1").await.unwrap();
        assert!(result.is_none());

        let metrics = sink.take_metrics();
        assert_eq!(metrics.len(), 1);

        match &metrics[0] {
            CacheMetric::Read {
                key,
                hit,
                status,
                tier,
                namespace,
                latency_ms,
            } => {
                assert_eq!(key, "key1");
                assert!(!hit);
                assert!(status.is_none());
                assert_eq!(tier, "hashmap");
                assert_eq!(namespace, "users");
                assert!(*latency_ms >= 0.0);
            }
            _ => panic!("Expected Read metric"),
        }
    }

    #[tokio::test]
    async fn test_read_hit_fresh() {
        let inner: Arc<dyn Store> = Arc::new(HashMapStore::new(HashMapStoreConfig::default()));
        let sink = Arc::new(TestSink::new());
        let store = MetricsStore::new(inner.clone(), sink.clone());

        let now = now_ms_test();
        let entry = StoredEntry::from_typed("value".to_string(), now + 60_000, now + 300_000);
        inner.set("users", "key1", entry).await.unwrap();

        sink.take_metrics(); // Clear set metric from inner

        let result = store.get("users", "key1").await.unwrap();
        assert!(result.is_some());

        let metrics = sink.take_metrics();
        assert_eq!(metrics.len(), 1);

        match &metrics[0] {
            CacheMetric::Read {
                key,
                hit,
                status,
                tier,
                ..
            } => {
                assert_eq!(key, "key1");
                assert!(hit);
                assert_eq!(*status, Some(CacheEntryStatus::Fresh));
                assert_eq!(tier, "hashmap");
            }
            _ => panic!("Expected Read metric"),
        }
    }

    #[tokio::test]
    async fn test_read_hit_stale() {
        let inner: Arc<dyn Store> = Arc::new(HashMapStore::new(HashMapStoreConfig::default()));
        let sink = Arc::new(TestSink::new());
        let store = MetricsStore::new(inner.clone(), sink.clone());

        let now = now_ms_test();
        // Entry is stale: fresh_until is in the past, stale_until is in the future
        let entry = StoredEntry::from_typed("value".to_string(), now - 1000, now + 300_000);
        inner.set("users", "key1", entry).await.unwrap();

        sink.take_metrics();

        let result = store.get("users", "key1").await.unwrap();
        assert!(result.is_some());

        let metrics = sink.take_metrics();
        assert_eq!(metrics.len(), 1);

        match &metrics[0] {
            CacheMetric::Read { hit, status, .. } => {
                assert!(hit);
                assert_eq!(*status, Some(CacheEntryStatus::Stale));
            }
            _ => panic!("Expected Read metric"),
        }
    }

    #[tokio::test]
    async fn test_write_metric() {
        let inner: Arc<dyn Store> = Arc::new(HashMapStore::new(HashMapStoreConfig::default()));
        let sink = Arc::new(TestSink::new());
        let store = MetricsStore::new(inner, sink.clone());

        let now = now_ms_test();
        let entry = StoredEntry::from_typed("value".to_string(), now + 60_000, now + 300_000);
        store.set("users", "key1", entry).await.unwrap();

        let metrics = sink.take_metrics();
        assert_eq!(metrics.len(), 1);

        match &metrics[0] {
            CacheMetric::Write {
                key,
                tier,
                namespace,
                latency_ms,
            } => {
                assert_eq!(key, "key1");
                assert_eq!(tier, "hashmap");
                assert_eq!(namespace, "users");
                assert!(*latency_ms >= 0.0);
            }
            _ => panic!("Expected Write metric"),
        }
    }

    #[tokio::test]
    async fn test_remove_metric() {
        let inner: Arc<dyn Store> = Arc::new(HashMapStore::new(HashMapStoreConfig::default()));
        let sink = Arc::new(TestSink::new());
        let store = MetricsStore::new(inner, sink.clone());

        store
            .remove("users", &["key1", "key2", "key3"])
            .await
            .unwrap();

        let metrics = sink.take_metrics();
        assert_eq!(metrics.len(), 1);

        match &metrics[0] {
            CacheMetric::Remove {
                key_count,
                first_key,
                tier,
                namespace,
                latency_ms,
            } => {
                assert_eq!(*key_count, 3);
                assert_eq!(first_key.as_deref(), Some("key1"));
                assert_eq!(tier, "hashmap");
                assert_eq!(namespace, "users");
                assert!(*latency_ms >= 0.0);
            }
            _ => panic!("Expected Remove metric"),
        }
    }

    #[tokio::test]
    async fn test_remove_empty_keys() {
        let inner: Arc<dyn Store> = Arc::new(HashMapStore::new(HashMapStoreConfig::default()));
        let sink = Arc::new(TestSink::new());
        let store = MetricsStore::new(inner, sink.clone());

        store.remove("users", &[]).await.unwrap();

        let metrics = sink.take_metrics();
        assert_eq!(metrics.len(), 1);

        match &metrics[0] {
            CacheMetric::Remove {
                key_count,
                first_key,
                ..
            } => {
                assert_eq!(*key_count, 0);
                assert!(first_key.is_none());
            }
            _ => panic!("Expected Remove metric"),
        }
    }

    #[tokio::test]
    async fn test_storage_mode_delegation() {
        let inner: Arc<dyn Store> = Arc::new(HashMapStore::new(HashMapStoreConfig::default()));
        let sink = Arc::new(TestSink::new());
        let store = MetricsStore::new(inner.clone(), sink);

        assert_eq!(store.storage_mode(), inner.storage_mode());
    }

    #[tokio::test]
    async fn test_tier_name_captured() {
        let inner: Arc<dyn Store> = Arc::new(HashMapStore::new(HashMapStoreConfig::default()));
        let sink = Arc::new(TestSink::new());
        let store = MetricsStore::new(inner, sink.clone());

        store.get("ns", "k").await.unwrap();

        let metrics = sink.take_metrics();
        match &metrics[0] {
            CacheMetric::Read { tier, .. } => {
                assert_eq!(tier, "hashmap");
            }
            _ => panic!("Expected Read metric"),
        }
    }
}
