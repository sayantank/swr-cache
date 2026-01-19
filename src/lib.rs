//! swr-cache - A stale-while-revalidate (SWR) cache library for Rust
//!
//! This library provides a flexible caching solution with:
//! - Stale-while-revalidate (SWR) semantics
//! - Multi-tier storage support
//! - Background revalidation
//! - Deduplication of concurrent origin loads
//!
//! # Example
//!
//! ```ignore
//! use swr_cache::{CacheBuilder, Namespace, HashMapStore, HashMapStoreConfig};
//! use std::sync::Arc;
//!
//! #[tokio::main]
//! async fn main() {
//!     let memory = Arc::new(HashMapStore::new(HashMapStoreConfig::default()));
//!
//!     // Create namespaces
//!     let users = Namespace::new("users", vec![memory.clone()], 60_000, 300_000);
//!     let sessions = Namespace::new("sessions", vec![memory], 60_000, 300_000);
//!
//!     // Build cache with multiple namespaces
//!     let cache = CacheBuilder::new()
//!         .add("users", users)
//!         .add("sessions", sessions)
//!         .build();
//!
//!     // Access specific namespace
//!     let users_cache = cache.namespace("users");
//!
//!     // SWR pattern - callback receives the actual key
//!     let user = users_cache.swr("user:123", |id| async move {
//!         // Load from database - 'id' is "user:123"
//!         Some(format!("User data for {}", id))
//!     }).await.unwrap();
//! }
//! ```

mod builder;
mod entry;
mod error;
mod namespace;
mod store;
pub mod stores;
mod swr;
mod tiered;
mod utils;

// Re-export public API
pub use builder::{Cache, CacheBuilder};
pub use entry::Entry;
pub use error::CacheError;
pub use namespace::Namespace;
pub use store::Store;
pub use stores::memory::{EvictOnSetConfig, HashMapStore, HashMapStoreConfig};
pub use stores::moka::{MokaStore, MokaStoreConfig};
pub use stores::redis::{RedisStore, RedisStoreConfig};
pub use swr::{SetOptions, SwrCache};
pub use tiered::TieredStore;
