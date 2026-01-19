use async_trait::async_trait;
use std::collections::HashMap;
use tokio::sync::RwLock;

use crate::entry::{StorageMode, StoredEntry};
use crate::error::CacheError;
use crate::store::Store;

use crate::utils::{build_cache_key, now_ms, rand_simple};

/// Configuration for eviction on set operations.
#[derive(Debug, Clone)]
pub struct EvictOnSetConfig {
    /// Provide a number between 0 and 1 to calculate whether eviction should run on each set.
    ///
    /// - `1.0` -> run eviction on every `set`
    /// - `0.5` -> run eviction on every 2nd `set` (on average)
    /// - `0.0` -> disable eviction
    pub frequency: f64,

    /// Remove items until the number of items in the map is lower than `max_items`.
    pub max_items: usize,
}

/// Configuration for HashMapStore.
#[derive(Debug, Clone, Default)]
pub struct HashMapStoreConfig {
    /// Remove expired entries on every `set` operation.
    pub evict_on_set: Option<EvictOnSetConfig>,
}

/// Thread-safe in-memory cache store using HashMap with RwLock.
///
/// This is a simple, zero-dependency store suitable for:
/// - Low to moderate concurrency (<8 threads)
/// - Small to medium cache sizes (<1000 items)
/// - Applications prioritizing simplicity over performance
///
/// HashMapStore is type-agnostic and can store values of any type.
/// It prefers `StoredEntry::Typed` for zero-copy storage but can handle
/// both typed and serialized entries.
///
/// For high-concurrency scenarios, consider using `MokaStore` instead.
pub struct HashMapStore {
    state: RwLock<HashMap<String, StoredEntry>>,
    evict_on_set: Option<EvictOnSetConfig>,
}

impl HashMapStore {
    /// Create a new HashMapStore with the given configuration.
    pub fn new(config: HashMapStoreConfig) -> Self {
        HashMapStore {
            state: RwLock::new(HashMap::new()),
            evict_on_set: config.evict_on_set,
        }
    }

    /// Run eviction if configured and random check passes.
    async fn maybe_evict(&self) {
        let Some(ref config) = self.evict_on_set else {
            return;
        };

        // Check frequency
        if config.frequency <= 0.0 {
            return;
        }

        let should_evict = if config.frequency >= 1.0 {
            true
        } else {
            rand_simple() < config.frequency
        };

        if !should_evict {
            return;
        }

        let mut state = self.state.write().await;
        let now = now_ms();

        // First delete all expired entries
        state.retain(|_, v| v.stale_until() > now);

        // If still over max_items, remove oldest entries
        if state.len() > config.max_items {
            // Collect keys to remove (oldest first based on expiry)
            let mut entries: Vec<_> = state
                .iter()
                .map(|(k, v)| (k.clone(), v.stale_until()))
                .collect();
            entries.sort_by_key(|(_, expires)| *expires);

            let to_remove = state.len() - config.max_items;
            for (key, _) in entries.into_iter().take(to_remove) {
                state.remove(&key);
            }
        }
    }
}

#[async_trait]
impl Store for HashMapStore {
    fn name(&self) -> &'static str {
        "hashmap"
    }

    fn storage_mode(&self) -> StorageMode {
        StorageMode::Typed
    }

    async fn get(&self, namespace: &str, key: &str) -> Result<Option<StoredEntry>, CacheError> {
        let cache_key = build_cache_key(namespace, key);
        let state = self.state.read().await;

        let Some(stored) = state.get(&cache_key) else {
            return Ok(None);
        };

        let now = now_ms();
        if stored.is_expired(now) {
            // Entry is expired, remove it
            drop(state);
            let mut state = self.state.write().await;
            state.remove(&cache_key);
            return Ok(None);
        }

        // Clone the StoredEntry - this is cheap for Typed (Arc clone)
        // and necessary for Serialized (String clone)
        Ok(Some(stored.clone()))
    }

    async fn set(&self, namespace: &str, key: &str, entry: StoredEntry) -> Result<(), CacheError> {
        let cache_key = build_cache_key(namespace, key);

        {
            let mut state = self.state.write().await;
            state.insert(cache_key, entry);
        }

        self.maybe_evict().await;
        Ok(())
    }

    async fn remove(&self, namespace: &str, keys: &[&str]) -> Result<(), CacheError> {
        let mut state = self.state.write().await;

        for key in keys {
            let cache_key = build_cache_key(namespace, key);
            state.remove(&cache_key);
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_get_set_remove() {
        let store = HashMapStore::new(HashMapStoreConfig::default());

        // Initially empty
        let result = store.get("users", "key1").await.unwrap();
        assert!(result.is_none());

        // Set a value using Typed variant
        let now = now_ms();
        let entry = StoredEntry::from_typed("value1".to_string(), now + 60_000, now + 300_000);
        store.set("users", "key1", entry).await.unwrap();

        // Get the value
        let result = store.get("users", "key1").await.unwrap();
        assert!(result.is_some());
        let stored = result.unwrap();

        // Convert back to typed entry
        let typed_entry: crate::entry::Entry<String> = stored.into_typed().unwrap();
        assert_eq!(typed_entry.value, "value1");

        // Set another value to test
        let entry2 = StoredEntry::from_typed("value2".to_string(), now + 60_000, now + 300_000);
        store.set("users", "key2", entry2).await.unwrap();

        // Remove the value
        store.remove("users", &["key1"]).await.unwrap();

        // Should be gone
        let result = store.get("users", "key1").await.unwrap();
        assert!(result.is_none());

        // key2 should still exist
        let result = store.get("users", "key2").await.unwrap();
        assert!(result.is_some());
    }
}
