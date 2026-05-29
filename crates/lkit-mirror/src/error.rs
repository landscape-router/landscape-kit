//! Mirror-level error types.

/// Errors from mirror operations (sync, verify, etc.).
#[derive(Debug, thiserror::Error)]
pub enum MirrorError {
    /// Upload to target storage failed.
    #[error("上传失败: {0}")]
    UploadFailed(String),
    /// I/O error.
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    /// GitHub API error.
    #[error("GitHub API 错误: {0}")]
    GitHubApi(String),
    /// Target storage error.
    #[error("目标存储错误: {0}")]
    TargetError(String),
    /// Source error (from lkit-core).
    #[error("源错误: {0}")]
    Source(#[from] lkit_core::SourceError),
    /// JSON serialization error.
    #[error("序列化错误: {0}")]
    Serialization(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirror_error_display_upload() -> Result<(), Box<dyn std::error::Error>> {
        let err = MirrorError::UploadFailed("connection refused".into());
        assert!(err.to_string().contains("connection refused"));
        Ok(())
    }

    #[test]
    fn mirror_error_from_io() -> Result<(), Box<dyn std::error::Error>> {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
        let err: MirrorError = io_err.into();
        assert!(err.to_string().contains("file missing"));
        Ok(())
    }
}
