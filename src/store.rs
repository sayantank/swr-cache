use async_trait::async_trait;
use std::fmt::Display;
use std::hash::Hash;

use crate::entry::Entry;
use crate::error::CacheError;

/// A store is a common interface for storing, reading and deleting key-value pairs.
///
/// The store implementation is responsible for cleaning up expired data on its own.
#[async_trait]
pub trait Store<N, V>: Send + Sync
where
    N: Clone + Eq + Hash + Display + Send + Sync,
    V: Clone + Send + Sync,
{
    /// A name for metrics/tracing.
    ///
    /// # Example
    /// - "memory"
    /// - "redis"
    /// - "tiered"
    fn name(&self) -> &'static str;

    /// Return the cached value.
    ///
    /// The response must be `None` for cache misses.
    async fn get(&self, namespace: N, key: &str) -> Result<Option<Entry<V>>, CacheError>;

    /// Sets the value for the given key.
    ///
    /// You are responsible for evicting expired values in your store implementation.
    /// Use the `entry.stale_until` (unix milli timestamp) field to configure expiration.
    async fn set(&self, namespace: N, key: &str, entry: Entry<V>) -> Result<(), CacheError>;

    /// Removes the key(s) from the store.
    async fn remove(&self, namespace: N, keys: &[&str]) -> Result<(), CacheError>;
}
