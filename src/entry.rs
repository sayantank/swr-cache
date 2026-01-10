use serde::{Deserialize, Serialize};

/// A cache entry containing a value and its expiration times.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry<V> {
    /// The cached value.
    pub value: V,

    /// Unix timestamp in milliseconds.
    /// Before this time the entry is considered fresh and valid.
    pub fresh_until: i64,

    /// Unix timestamp in milliseconds.
    /// Do not use data after this point as it is considered no longer valid.
    /// You can use this field to configure automatic eviction in your store implementation.
    pub stale_until: i64,
}

impl<V> Entry<V> {
    /// Create a new cache entry.
    pub fn new(value: V, fresh_until: i64, stale_until: i64) -> Self {
        Entry {
            value,
            fresh_until,
            stale_until,
        }
    }

    /// Check if the entry is still fresh (not yet stale).
    pub fn is_fresh(&self, now_ms: i64) -> bool {
        now_ms < self.fresh_until
    }

    /// Check if the entry is stale but still usable.
    pub fn is_stale(&self, now_ms: i64) -> bool {
        now_ms >= self.fresh_until && now_ms < self.stale_until
    }

    /// Check if the entry has expired and should not be used.
    pub fn is_expired(&self, now_ms: i64) -> bool {
        now_ms >= self.stale_until
    }
}
