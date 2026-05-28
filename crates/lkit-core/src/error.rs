//! Core error types shared across all layers.

/// Errors originating from the core layer.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// Invalid or missing configuration value.
    #[error("configuration error: {0}")]
    Config(String),

    /// Path validation or resolution failure.
    #[error("invalid path: {0}")]
    InvalidPath(String),

    /// Business rule or input validation failure.
    #[error("validation failed: {0}")]
    Validation(String),

    /// JSON serialization or deserialization failure.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// Filesystem or network I/O failure.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// Catch-all for unexpected internal errors.
    #[error("internal error: {0}")]
    Internal(String),
}
