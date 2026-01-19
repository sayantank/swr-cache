/// Error type for cache operations.
#[derive(Debug, Clone, thiserror::Error)]
pub enum CacheError {
    /// A cache operation failed.
    #[error("[{tier}] cache error for key '{key}': {message}")]
    Operation {
        tier: String,
        key: String,
        message: String,
    },
    /// Serialization or deserialization failed.
    #[error("Serialization error: {0}")]
    Serialization(String),
}

impl CacheError {
    /// Create a new operation error.
    pub fn operation(
        tier: impl Into<String>,
        key: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        CacheError::Operation {
            tier: tier.into(),
            key: key.into(),
            message: message.into(),
        }
    }
}
