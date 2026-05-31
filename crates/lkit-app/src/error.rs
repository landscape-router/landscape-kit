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

    /// Backup operation error.
    #[error("backup error: {0}")]
    Backup(String),

    /// Requested backup not found.
    #[error("backup not found: {0}")]
    BackupNotFound(String),

    /// Insufficient disk space for backup.
    #[error("space insufficient: need {need} bytes, have {available} bytes")]
    SpaceInsufficient { need: u64, available: u64 },

    /// Health check failed after restore.
    #[error("health check failed after restore: {0}")]
    HealthCheckFailed(String),

    /// I/O error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
