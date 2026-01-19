use async_trait::async_trait;

use crate::entry::{StorageMode, StoredEntry};
use crate::error::CacheError;

/// A store is a common interface for storing, reading and deleting key-value pairs.
///
/// Stores are type-agnostic and work with `StoredEntry` which can hold either
/// typed values (for in-memory stores) or serialized JSON (for persistent stores).
///
/// The store implementation is responsible for cleaning up expired data on its own.
#[async_trait]
pub trait Store: Send + Sync {
    /// A name for metrics/tracing.
    ///
    /// # Example
    /// - "memory"
    /// - "redis"
    /// - "tiered"
    fn name(&self) -> &'static str;

    /// Returns the storage mode preference for this store.
    ///
    /// - `StorageMode::Typed`: In-memory stores prefer typed values (zero-copy)
    /// - `StorageMode::Serialized`: Persistent stores prefer serialized JSON
    ///
    /// Default implementation returns `Serialized` for safety.
    fn storage_mode(&self) -> StorageMode {
        StorageMode::Serialized
    }

    /// Return the cached value.
    ///
    /// The response must be `None` for cache misses.
    async fn get(&self, namespace: &str, key: &str) -> Result<Option<StoredEntry>, CacheError>;

    /// Sets the value for the given key.
    ///
    /// You are responsible for evicting expired values in your store implementation.
    /// Use the `entry.stale_until()` (unix milli timestamp) to configure expiration.
    async fn set(&self, namespace: &str, key: &str, entry: StoredEntry) -> Result<(), CacheError>;

    /// Removes the key(s) from the store.
    async fn remove(&self, namespace: &str, keys: &[&str]) -> Result<(), CacheError>;
}
