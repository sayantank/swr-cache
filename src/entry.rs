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
    ///
    /// The serializer function allows conversion to Serialized format when needed
    /// (e.g., when writing to persistent stores in a tiered cache).
    Typed {
        value: Arc<dyn Any + Send + Sync>,
        fresh_until: i64,
        stale_until: i64,
        /// Optional serializer that can convert this value to JSON.
        /// If None, the entry cannot be converted to Serialized format.
        serializer: Option<Arc<dyn Fn() -> Result<String, CacheError> + Send + Sync>>,
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
    ///
    /// For values that implement Serialize, use `from_typed_with_serializer` instead
    /// to enable conversion to Serialized format when needed by tiered caches.
    pub fn from_typed<V>(value: V, fresh_until: i64, stale_until: i64) -> Self
    where
        V: Send + Sync + 'static,
    {
        StoredEntry::Typed {
            value: Arc::new(value),
            fresh_until,
            stale_until,
            serializer: None,
        }
    }

    /// Create a StoredEntry from a typed value with serialization support.
    ///
    /// This version includes a serializer callback that enables conversion
    /// to Serialized format, which is needed when using tiered caches with
    /// mixed storage modes (e.g., MokaStore + RedisStore).
    pub fn from_typed_with_serializer<V>(value: V, fresh_until: i64, stale_until: i64) -> Self
    where
        V: Clone + Serialize + Send + Sync + 'static,
    {
        let value_arc = Arc::new(value);
        let value_for_serializer = value_arc.clone();

        let serializer = Arc::new(move || {
            let val = value_for_serializer.as_ref().clone();
            let entry = Entry::new(val, fresh_until, stale_until);
            serde_json::to_string(&entry)
                .map_err(|e| CacheError::Serialization(format!("Serialization failed: {}", e)))
        });

        StoredEntry::Typed {
            value: value_arc,
            fresh_until,
            stale_until,
            serializer: Some(serializer),
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
                serializer: _,
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

    /// Convert Typed entry to Serialized format using the stored serializer.
    ///
    /// Returns the same entry if already Serialized, or an error if Typed but
    /// no serializer is available.
    pub fn to_serialized(self) -> Result<Self, CacheError> {
        match self {
            StoredEntry::Serialized { .. } => Ok(self),
            StoredEntry::Typed {
                serializer: Some(ser),
                fresh_until,
                stale_until,
                ..
            } => {
                let data = ser()?;
                Ok(StoredEntry::Serialized {
                    data,
                    fresh_until,
                    stale_until,
                })
            }
            StoredEntry::Typed {
                serializer: None, ..
            } => Err(CacheError::Serialization(
                "Cannot serialize Typed entry: no serializer available".to_string(),
            )),
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

    /// Convert StoredEntry to match the target storage mode.
    ///
    /// This is used by TieredStore to ensure entries are in the correct format
    /// for each tier's storage preference.
    ///
    /// # Arguments
    /// * `target_mode` - The desired storage mode
    ///
    /// # Returns
    /// * `Ok(StoredEntry)` - Entry converted to target mode (or unchanged if already correct)
    /// * `Err(CacheError)` - Conversion failed (no serializer available or serialization error)
    ///
    /// # Examples
    /// ```ignore
    /// // Convert Typed to Serialized for Redis storage
    /// let serialized = typed_entry.convert_for_mode(StorageMode::Serialized)?;
    /// ```
    pub fn convert_for_mode(self, target_mode: StorageMode) -> Result<Self, CacheError> {
        match (&self, target_mode) {
            // Already in target mode - no conversion needed
            (StoredEntry::Typed { .. }, StorageMode::Typed) => Ok(self),
            (StoredEntry::Serialized { .. }, StorageMode::Serialized) => Ok(self),

            // Convert Typed to Serialized
            (StoredEntry::Typed { .. }, StorageMode::Serialized) => self.to_serialized(),

            // Convert Serialized to Typed
            // Keep as Serialized - the receiving store will handle conversion if needed.
            // This avoids unnecessary deserialization.
            (StoredEntry::Serialized { .. }, StorageMode::Typed) => Ok(self),
        }
    }
}
