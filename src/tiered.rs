use async_trait::async_trait;
use futures::future::join_all;
use std::sync::Arc;

use crate::entry::{StorageMode, StoredEntry};
use crate::error::CacheError;
use crate::store::Store;

/// TieredStore is a cache that checks multiple stores in order.
///
/// Stores are checked in the order they are provided.
/// The first store to return a value will be used to populate all previous stores.
///
/// TieredStore is type-agnostic and works with any combination of store types.
pub struct TieredStore {
    tiers: Vec<Arc<dyn Store>>,
}

impl TieredStore {
    /// Create a new tiered store.
    ///
    /// Stores are checked in the order they are provided.
    /// The first store to return a value will be used to populate all previous stores.
    ///
    /// `stores` can accept `None` as members to allow you to construct the tiers dynamically.
    ///
    /// # Example
    /// ```ignore
    /// TieredStore::new(vec![
    ///     Some(Arc::new(memory_store)),
    ///     if enable_redis { Some(Arc::new(redis_store)) } else { None },
    /// ])
    /// ```
    pub fn new(stores: Vec<Option<Arc<dyn Store>>>) -> Self {
        let tiers = stores.into_iter().flatten().collect();
        TieredStore { tiers }
    }

    /// Create a tiered store from a vec of stores (no optional filtering).
    pub fn from_stores(stores: Vec<Arc<dyn Store>>) -> Self {
        TieredStore { tiers: stores }
    }
}

#[async_trait]
impl Store for TieredStore {
    fn name(&self) -> &'static str {
        "tiered"
    }

    fn storage_mode(&self) -> StorageMode {
        // TieredStore adapts to its stores, but prefers Typed for efficiency
        self.tiers
            .first()
            .map(|tier| tier.storage_mode())
            .unwrap_or(StorageMode::Serialized)
    }

    async fn get(&self, namespace: &str, key: &str) -> Result<Option<StoredEntry>, CacheError> {
        if self.tiers.is_empty() {
            return Ok(None);
        }

        for (i, tier) in self.tiers.iter().enumerate() {
            let res = tier.get(namespace, key).await?;

            if let Some(entry) = res {
                // Fill all lower (earlier) tiers with this value in the background
                if i > 0 {
                    let lower_tiers: Vec<_> = self.tiers[..i].to_vec();
                    let entry_clone = entry.clone();
                    let namespace_clone = namespace.to_string();
                    let key_clone = key.to_string();

                    tokio::spawn(async move {
                        for tier in lower_tiers {
                            // Convert entry to match tier's storage mode
                            let target_mode = tier.storage_mode();
                            if let Ok(converted_entry) =
                                entry_clone.clone().convert_for_mode(target_mode)
                            {
                                match tier
                                    .set(&namespace_clone, &key_clone, converted_entry)
                                    .await
                                {
                                    Ok(_) => {
                                        tracing::debug!(
                                            "Successfully populated lower tier: tier={}, namespace={}, key={}",
                                            tier.name(),
                                            namespace_clone,
                                            key_clone
                                        );
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            "Failed to populate lower tier: tier={}, namespace={}, key={}, error={}",
                                            tier.name(),
                                            namespace_clone,
                                            key_clone,
                                            e
                                        );
                                    }
                                }
                            }
                        }
                    });
                }

                return Ok(Some(entry));
            }
        }

        Ok(None)
    }

    async fn set(&self, namespace: &str, key: &str, entry: StoredEntry) -> Result<(), CacheError> {
        // Set on all tiers in parallel, converting entry format to match each tier's storage mode
        let futures: Vec<_> = self
            .tiers
            .iter()
            .map(|tier| {
                let tier = tier.clone();
                let namespace = namespace.to_string();
                let key = key.to_string();
                let entry = entry.clone();
                let target_mode = tier.storage_mode();

                async move {
                    // Convert entry to match tier's storage mode
                    let converted_entry = entry.convert_for_mode(target_mode)?;
                    tier.set(&namespace, &key, converted_entry).await
                }
            })
            .collect();

        let results = join_all(futures).await;

        // Return first error if any
        for result in results {
            result?;
        }

        Ok(())
    }

    async fn remove(&self, namespace: &str, keys: &[&str]) -> Result<(), CacheError> {
        // Remove from all tiers in parallel
        let keys_owned: Vec<String> = keys.iter().map(|s| s.to_string()).collect();

        let futures: Vec<_> = self
            .tiers
            .iter()
            .map(|tier| {
                let tier = tier.clone();
                let namespace = namespace.to_string();
                let keys = keys_owned.clone();
                async move {
                    let key_refs: Vec<&str> = keys.iter().map(|s| s.as_str()).collect();
                    tier.remove(&namespace, &key_refs).await
                }
            })
            .collect();

        let results = join_all(futures).await;

        // Return first error if any
        for result in results {
            result?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stores::memory::{HashMapStore, HashMapStoreConfig};

    fn now_ms() -> i64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
    }

    #[tokio::test]
    async fn test_tiered_get_populates_lower_tiers() {
        let l1: Arc<dyn Store> = Arc::new(HashMapStore::new(HashMapStoreConfig::default()));
        let l2: Arc<dyn Store> = Arc::new(HashMapStore::new(HashMapStoreConfig::default()));

        // Set value only in L2
        let now = now_ms();
        let entry = StoredEntry::from_typed("value1".to_string(), now + 60_000, now + 300_000);
        l2.set("users", "key1", entry).await.unwrap();

        // L1 should be empty
        let result = l1.get("users", "key1").await.unwrap();
        assert!(result.is_none());

        // Create tiered store
        let tiered = TieredStore::from_stores(vec![l1.clone(), l2.clone()]);

        // Get from tiered - should find in L2
        let result = tiered.get("users", "key1").await.unwrap();
        assert!(result.is_some());
        let stored = result.unwrap();
        let typed_entry: crate::entry::Entry<String> = stored.into_typed().unwrap();
        assert_eq!(typed_entry.value, "value1");

        // Give background task time to populate L1
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // L1 should now have the value
        let result = l1.get("users", "key1").await.unwrap();
        assert!(result.is_some());
        let stored = result.unwrap();
        let typed_entry: crate::entry::Entry<String> = stored.into_typed().unwrap();
        assert_eq!(typed_entry.value, "value1");
    }
}
