//! Store implementations for the cache library.

pub mod memory;
pub mod moka;
pub mod redis;

pub use memory::{EvictOnSetConfig, HashMapStore, HashMapStoreConfig};
pub use moka::{MokaStore, MokaStoreConfig};
pub use redis::{RedisStore, RedisStoreConfig};
