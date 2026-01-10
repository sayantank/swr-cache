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
//! use swr_cache::{Namespace, HashMapStore, HashMapStoreConfig};
//! use std::sync::Arc;
//!
//! #[derive(Clone, Hash, Eq, PartialEq)]
//! enum CacheNamespace {
//!     Users,
//!     Sessions,
//! }
//!
//! impl std::fmt::Display for CacheNamespace {
//!     fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
//!         match self {
//!             CacheNamespace::Users => write!(f, "users"),
//!             CacheNamespace::Sessions => write!(f, "sessions"),
//!         }
//!     }
//! }
//!
//! #[tokio::main]
//! async fn main() {
//!     let memory = Arc::new(HashMapStore::new(HashMapStoreConfig::default()));
//!
//!     let cache = Namespace::new(
//!         vec![memory],
//!         60_000,   // 1 minute fresh
//!         300_000,  // 5 minutes stale
//!     );
//!
//!     // SWR pattern - returns cached or fetches from origin
//!     let user = cache.swr(CacheNamespace::Users, "user:123", |key| async {
//!         // Load from database or API
//!         Some(format!("User data for {}", key))
//!     }).await.unwrap();
//! }
//! ```

mod entry;
mod error;
mod namespace;
mod store;
pub mod stores;
mod swr;
mod tiered;
mod utils;

// Re-export public API
pub use entry::Entry;
pub use error::CacheError;
pub use namespace::Namespace;
pub use store::Store;
pub use stores::memory::{EvictOnSetConfig, HashMapStore, HashMapStoreConfig};
pub use stores::moka::{MokaStore, MokaStoreConfig};
pub use stores::redis::{RedisStore, RedisStoreConfig};
pub use swr::{SetOptions, SwrCache};
pub use tiered::TieredStore;
