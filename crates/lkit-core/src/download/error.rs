//! Download-level error types.

/// Errors from artifact download operations.
#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    /// Network request failed.
    #[error("网络请求失败: {0}")]
    Network(String),
    /// I/O error.
    #[error("IO 错误: {0}")]
    Io(String),
    /// SHA-256 checksum mismatch.
    #[error("校验不匹配: 期望 {expected}, 实际 {actual}")]
    ChecksumMismatch { expected: String, actual: String },
    /// Download incomplete — partial transfer.
    #[error("下载不完整: {downloaded}/{total} 字节")]
    Incomplete { downloaded: u64, total: u64 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_error_display_checksum_mismatch() -> Result<(), Box<dyn std::error::Error>> {
        let err = DownloadError::ChecksumMismatch {
            expected: "aaa".into(),
            actual: "bbb".into(),
        };
        assert!(err.to_string().contains("aaa"));
        assert!(err.to_string().contains("bbb"));
        Ok(())
    }

    #[test]
    fn download_error_display_incomplete() -> Result<(), Box<dyn std::error::Error>> {
        let err = DownloadError::Incomplete {
            downloaded: 50,
            total: 100,
        };
        assert!(err.to_string().contains("50/100"));
        Ok(())
    }
}
