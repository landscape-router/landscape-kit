//! Application-layer error types.

use lkit_core::CoreError;

/// Errors from the use case layer.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// Error from the core layer.
    #[error("{0}")]
    Core(#[from] CoreError),

    /// Error from the API client layer.
    #[error("client error: {0}")]
    Client(String),

    /// Requested resource not found.
    #[error("not found: {0}")]
    NotFound(String),

    /// Insufficient permissions for the operation.
    #[error("permission denied: {0}")]
    PermissionDenied(String),

    /// Configuration generation failure.
    #[error("configuration generation failed: {0}")]
    ConfigGeneration(String),
}
