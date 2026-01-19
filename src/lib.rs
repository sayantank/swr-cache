//! swr-cache - A stale-while-revalidate (SWR) cache library for Rust
//!
//! This library provides a flexible caching solution with:
//! - Stale-while-revalidate (SWR) semantics
//! - Multi-tier storage support
//! - Background revalidation
//! - Deduplication of concurrent origin loads
//! - Type-agnostic stores that can be reused across different value types
//!
//! # Example
//!
//! ```ignore
//! use swr_cache::{Cache, MokaStore, MokaStoreConfig, RedisStore, RedisStoreConfig};
//! use std::sync::Arc;
//! use serde::{Serialize, Deserialize};
//!
//! #[derive(Clone, Serialize, Deserialize)]
//! struct User {
//!     id: String,
//!     name: String,
//! }
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     // Create type-agnostic stores (can be reused across different types)
//!     let l1_store = Arc::new(MokaStore::new(MokaStoreConfig::default()));
//!     let l2_store = Arc::new(RedisStore::new(RedisStoreConfig {
//!         url: "redis://localhost:6379".into(),
//!         disable_expiration: false,
//!     }).await?);
//!
//!     // Create a Cache for User type
//!     let user_cache: Cache<User> = Cache::new(
//!         "users",
//!         vec![l1_store.clone(), l2_store.clone()],
//!         60_000,  // fresh for 60 seconds
//!         300_000, // stale for 5 minutes
//!     );
//!
//!     // SWR pattern - callback receives the actual key
//!     let user = user_cache.swr("user:123", |id| async move {
//!         // Load from database - 'id' is "user:123"
//!         Some(User {
//!             id: id.clone(),
//!             name: format!("User {}", id),
//!         })
//!     }).await?;
//!
//!     Ok(())
//! }
//! ```

mod cache;
mod entry;
mod error;
mod store;
pub mod stores;
mod swr;
mod tiered;
mod utils;

// Re-export public API
pub use cache::Cache;
pub use entry::{Entry, StorageMode, StoredEntry};
pub use error::CacheError;
pub use store::Store;
pub use stores::memory::{EvictOnSetConfig, HashMapStore, HashMapStoreConfig};
pub use stores::moka::{MokaStore, MokaStoreConfig};
pub use stores::redis::{RedisStore, RedisStoreConfig};
pub use swr::{SetOptions, SwrCache};
pub use tiered::TieredStore;
