use serde::{Deserialize, Serialize};
use std::any::Any;
use std::sync::Arc;

use crate::error::CacheError;

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

/// Type-erased storage entry that supports both typed and serialized storage.
///
/// This enum allows stores to be type-agnostic while still supporting zero-copy
/// storage for in-memory stores (via `Typed`) and efficient serialization for
/// persistent stores (via `Serialized`).
#[derive(Clone)]
pub enum StoredEntry {
    /// For in-memory stores: zero-copy storage of typed values.
    ///
    /// The value is stored as a type-erased `Arc<dyn Any>` which can be
    /// cloned cheaply (just increments reference count) and downcast back
    /// to the original type without serialization overhead.
    Typed {
        value: Arc<dyn Any + Send + Sync>,
        fresh_until: i64,
        stale_until: i64,
    },
    /// For persistent stores: serialized JSON storage.
    ///
    /// The value is stored as a JSON string, suitable for storage in
    /// Redis, files, or other persistent backends.
    Serialized {
        data: String,
        fresh_until: i64,
        stale_until: i64,
    },
}

/// Storage mode preference for stores.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageMode {
    /// Store prefers typed values (in-memory stores).
    Typed,
    /// Store prefers serialized values (persistent stores).
    Serialized,
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

impl StoredEntry {
    /// Create a StoredEntry from a typed value (for in-memory stores).
    pub fn from_typed<V>(value: V, fresh_until: i64, stale_until: i64) -> Self
    where
        V: Send + Sync + 'static,
    {
        StoredEntry::Typed {
            value: Arc::new(value),
            fresh_until,
            stale_until,
        }
    }

    /// Create a StoredEntry from serialized data (for persistent stores).
    pub fn from_serialized(data: String, fresh_until: i64, stale_until: i64) -> Self {
        StoredEntry::Serialized {
            data,
            fresh_until,
            stale_until,
        }
    }

    /// Convert StoredEntry to a typed Entry<V>.
    ///
    /// This handles both variants:
    /// - `Typed`: Attempts to downcast the Arc value to V and clones it
    /// - `Serialized`: Deserializes the JSON string to V
    pub fn into_typed<V>(self) -> Result<Entry<V>, CacheError>
    where
        V: Clone + Serialize + for<'de> Deserialize<'de> + Send + Sync + 'static,
    {
        match self {
            StoredEntry::Typed {
                value,
                fresh_until,
                stale_until,
            } => {
                // Try to downcast Arc<dyn Any> to Arc<V>
                let typed_arc = value
                    .downcast::<V>()
                    .map_err(|_| CacheError::Serialization("Type downcast failed".to_string()))?;

                // Clone the value out of the Arc
                // This is necessary because we need an owned V, not Arc<V>
                let typed_value = (*typed_arc).clone();

                Ok(Entry {
                    value: typed_value,
                    fresh_until,
                    stale_until,
                })
            }
            StoredEntry::Serialized {
                data,
                fresh_until: _,
                stale_until: _,
            } => {
                // The data is serialized Entry<V>, not just V
                let entry: Entry<V> = serde_json::from_str(&data).map_err(|e| {
                    CacheError::Serialization(format!("Deserialization failed: {}", e))
                })?;
                // Note: We use the timestamps from the serialized Entry, not the StoredEntry fields
                // This ensures consistency with what was originally stored
                Ok(entry)
            }
        }
    }

    /// Get the fresh_until timestamp.
    pub fn fresh_until(&self) -> i64 {
        match self {
            StoredEntry::Typed { fresh_until, .. } => *fresh_until,
            StoredEntry::Serialized { fresh_until, .. } => *fresh_until,
        }
    }

    /// Get the stale_until timestamp.
    pub fn stale_until(&self) -> i64 {
        match self {
            StoredEntry::Typed { stale_until, .. } => *stale_until,
            StoredEntry::Serialized { stale_until, .. } => *stale_until,
        }
    }

    /// Check if the entry is still fresh (not yet stale).
    pub fn is_fresh(&self, now_ms: i64) -> bool {
        now_ms < self.fresh_until()
    }

    /// Check if the entry is stale but still usable.
    pub fn is_stale(&self, now_ms: i64) -> bool {
        now_ms >= self.fresh_until() && now_ms < self.stale_until()
    }

    /// Check if the entry has expired and should not be used.
    pub fn is_expired(&self, now_ms: i64) -> bool {
        now_ms >= self.stale_until()
    }
}
