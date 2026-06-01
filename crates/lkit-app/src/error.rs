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

    /// Generic backup operation error.
    #[error("backup error: {0}")]
    Backup(String),

    /// Requested backup snapshot not found.
    #[error("backup not found: {0}")]
    BackupNotFound(String),

    /// Insufficient disk space for backup.
    #[error("space insufficient: need {need} bytes, have {available} bytes")]
    SpaceInsufficient { need: u64, available: u64 },

    /// Backup checksum verification failed.
    #[error("checksum mismatch")]
    ChecksumMismatch,

    /// Backup archive is corrupted or unreadable.
    #[error("backup corrupted: {0}")]
    BackupCorrupted(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backup_error_display() {
        let err = AppError::Backup("binary not found".into());
        assert_eq!(err.to_string(), "backup error: binary not found");

        let err = AppError::BackupNotFound("20260601-xxxx-nope".into());
        assert_eq!(err.to_string(), "backup not found: 20260601-xxxx-nope");

        let err = AppError::SpaceInsufficient { need: 100, available: 50 };
        assert!(err.to_string().contains("space insufficient"));
        assert!(err.to_string().contains("100"));
        assert!(err.to_string().contains("50"));

        let err = AppError::ChecksumMismatch;
        assert_eq!(err.to_string(), "checksum mismatch");

        let err = AppError::BackupCorrupted("magic mismatch".into());
        assert_eq!(err.to_string(), "backup corrupted: magic mismatch");
    }
}
