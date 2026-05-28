//! Core error types shared across all layers.

/// Errors originating from the core layer.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("configuration error: {0}")]
    Config(String),

    #[error("invalid path: {0}")]
    InvalidPath(String),

    #[error("validation failed: {0}")]
    Validation(String),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("internal error: {0}")]
    Internal(String),
}
