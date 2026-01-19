//! Builder API for creating multi-namespace cache instances.
//!
//! This module provides a convenient way to create cache instances with multiple
//! namespaces, each with its own configuration.

use crate::namespace::Namespace;

/// Builder for creating cache instances with multiple namespaces.
///
/// This builder pattern allows you to define multiple namespaces, each with their own
/// stores and timing configuration, and then build a complete cache instance.
///
/// # Example
///
/// ```ignore
/// use swr_cache::{CacheBuilder, Namespace, HashMapStore, HashMapStoreConfig};
/// use std::sync::Arc;
///
/// let memory = Arc::new(HashMapStore::new(HashMapStoreConfig::default()));
///
/// let account_ns = Namespace::new("account", vec![memory.clone()], 60_000, 300_000);
/// let user_ns = Namespace::new("user", vec![memory], 60_000, 300_000);
///
/// let cache = CacheBuilder::new()
///     .add("account", account_ns)
///     .add("user", user_ns)
///     .build();
///
/// // Access namespaces by key
/// let account = cache.get("account").unwrap();
/// let user_data = account.get("chronark").await?;
/// ```
pub struct CacheBuilder<V>
where
    V: Clone + Send + Sync + 'static,
{
    namespaces: Vec<(String, Namespace<V>)>,
}

impl<V> CacheBuilder<V>
where
    V: Clone + Send + Sync + 'static,
{
    /// Create a new CacheBuilder.
    pub fn new() -> Self {
        CacheBuilder {
            namespaces: Vec::new(),
        }
    }

    /// Add a namespace to the cache.
    ///
    /// # Arguments
    /// * `name` - The name/key for this namespace
    /// * `namespace` - The configured Namespace instance
    pub fn add(mut self, name: &str, namespace: Namespace<V>) -> Self {
        self.namespaces.push((name.to_string(), namespace));
        self
    }

    /// Build the cache, returning a Cache instance.
    pub fn build(self) -> Cache<V> {
        Cache {
            namespaces: self.namespaces,
        }
    }
}

impl<V> Default for CacheBuilder<V>
where
    V: Clone + Send + Sync + 'static,
{
    fn default() -> Self {
        Self::new()
    }
}

/// A cache instance containing multiple named namespaces.
///
/// This provides access to the configured namespaces by name.
pub struct Cache<V>
where
    V: Clone + Send + Sync + 'static,
{
    namespaces: Vec<(String, Namespace<V>)>,
}

impl<V> Cache<V>
where
    V: Clone + Send + Sync + 'static,
{
    /// Get a namespace by name.
    ///
    /// Returns `None` if the namespace doesn't exist.
    pub fn get(&self, name: &str) -> Option<&Namespace<V>> {
        self.namespaces
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, ns)| ns)
    }

    /// Get a namespace by name, panicking if it doesn't exist.
    ///
    /// This is useful when you know the namespace exists and want cleaner code.
    pub fn namespace(&self, name: &str) -> &Namespace<V> {
        self.get(name)
            .unwrap_or_else(|| panic!("Namespace '{}' not found", name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stores::memory::{HashMapStore, HashMapStoreConfig};
    use std::sync::Arc;

    #[tokio::test]
    async fn test_cache_builder() {
        let store = Arc::new(HashMapStore::new(HashMapStoreConfig::default()));

        let account_ns = Namespace::new("account", vec![store.clone()], 60_000, 300_000);
        let user_ns = Namespace::new("user", vec![store], 60_000, 300_000);

        let cache = CacheBuilder::new()
            .add("account", account_ns)
            .add("user", user_ns)
            .build();

        // Test get
        let user_namespace = cache.get("user").unwrap();
        user_namespace
            .set("chronark", "test_value".to_string())
            .await
            .unwrap();

        let result = user_namespace.get("chronark").await.unwrap();
        assert_eq!(result, Some("test_value".to_string()));

        // Test namespace (panic version)
        let account_namespace = cache.namespace("account");
        account_namespace
            .set("acc1", "account_data".to_string())
            .await
            .unwrap();

        let result = account_namespace.get("acc1").await.unwrap();
        assert_eq!(result, Some("account_data".to_string()));

        // Verify isolation
        let user_namespace = cache.get("user").unwrap();
        let result = user_namespace.get("acc1").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_cache_builder_swr() {
        let store = Arc::new(HashMapStore::new(HashMapStoreConfig::default()));
        let user_ns = Namespace::new("user", vec![store], 60_000, 300_000);

        let cache = CacheBuilder::new().add("user", user_ns).build();

        let user_namespace = cache.namespace("user");
        let result = user_namespace
            .swr("chronark", |key| async move {
                assert_eq!(key, "chronark");
                Some("loaded_data".to_string())
            })
            .await
            .unwrap();

        assert_eq!(result, Some("loaded_data".to_string()));
    }
}
