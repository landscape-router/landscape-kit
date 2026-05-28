//! Source-level error types.

/// Errors from release source operations.
#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    /// Network request failed.
    #[error("网络请求失败: {0}")]
    Network(String),
    /// I/O error.
    #[error("IO 错误: {0}")]
    Io(String),
    /// Invalid source configuration.
    #[error("配置错误: {0}")]
    Config(String),
    /// Requested version tag does not exist.
    #[error("版本 {tag} 不存在")]
    VersionNotFound { tag: String },
    /// Requested artifact does not exist.
    #[error("制品 {name} 不存在")]
    ArtifactNotFound { name: String },
    /// Manifest parsing failed.
    #[error("manifest 解析失败: {0}")]
    InvalidManifest(String),
    /// Probe timed out.
    #[error("源探测超时")]
    ProbeTimeout,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_error_display_network() -> Result<(), Box<dyn std::error::Error>> {
        let err = SourceError::Network("timeout".into());
        assert_eq!(err.to_string(), "网络请求失败: timeout");
        Ok(())
    }

    #[test]
    fn source_error_display_config() -> Result<(), Box<dyn std::error::Error>> {
        let err = SourceError::Config("invalid repo format".into());
        assert!(err.to_string().contains("invalid repo format"));
        Ok(())
    }

    #[test]
    fn source_error_display_version_not_found() -> Result<(), Box<dyn std::error::Error>> {
        let err = SourceError::VersionNotFound {
            tag: "v1.0".into(),
        };
        assert!(err.to_string().contains("v1.0"));
        Ok(())
    }

    #[test]
    fn source_error_display_probe_timeout() -> Result<(), Box<dyn std::error::Error>> {
        let err = SourceError::ProbeTimeout;
        assert_eq!(err.to_string(), "源探测超时");
        Ok(())
    }
}
