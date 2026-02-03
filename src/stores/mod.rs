//! Store implementations for the cache library.

pub mod memory;
pub mod metrics;
pub mod moka;
pub mod redis;

pub use memory::{EvictOnSetConfig, HashMapStore, HashMapStoreConfig};
pub use metrics::{CacheEntryStatus, CacheMetric, MetricsSink, MetricsStore};
pub use moka::{MokaStore, MokaStoreConfig};
pub use redis::{RedisStore, RedisStoreConfig};
