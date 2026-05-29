//! Source configuration — serde models for lkit.toml [[sources]] section.

use serde::{Deserialize, Serialize};

/// A configured release source from lkit.toml.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SourceConfig {
    /// Unique source identifier.
    pub name: String,
    /// Source type: "github", "http", or "local".
    #[serde(rename = "type")]
    pub source_type: SourceType,
    /// Priority — lower values are preferred. Equal priorities are probed concurrently.
    pub priority: u32,
    /// HTTP(S) base URL for http type. Points to product directory.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// GitHub repo in "owner/repo" format for github type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// Local filesystem path for local type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// S3 endpoint URL for s3 type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// S3 bucket name for s3 type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bucket: Option<String>,
    /// S3 region for s3 type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
}

/// Source type discriminator.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SourceType {
    /// GitHub Releases API.
    Github,
    /// HTTP(S) mirror (including R2 public buckets).
    Http,
    /// Local filesystem directory.
    Local,
    /// S3-compatible object storage.
    S3,
}

/// Built-in default sources.
///
/// Returns R2 (priority 10) and GitHub (priority 100) as fallback.
/// User-configured sources in `lkit.toml` take precedence.
pub fn default_sources() -> Vec<SourceConfig> {
    vec![
        SourceConfig {
            name: "r2-official".into(),
            source_type: SourceType::Http,
            priority: 10,
            base_url: Some("https://pub-1e112154ee8a4b909c204b5325aba1f3.r2.dev/landscape".into()),
            repo: None,
            path: None,
            endpoint: None,
            bucket: None,
            region: None,
        },
        SourceConfig {
            name: "github-default".into(),
            source_type: SourceType::Github,
            priority: 100,
            base_url: None,
            repo: Some("ThisSeanZhang/landscape".into()),
            path: None,
            endpoint: None,
            bucket: None,
            region: None,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_sources_includes_r2_and_github() -> Result<(), Box<dyn std::error::Error>> {
        let sources = default_sources();
        assert_eq!(sources.len(), 2);
        // R2 is first (higher priority)
        assert_eq!(sources[0].name, "r2-official");
        assert_eq!(sources[0].source_type, SourceType::Http);
        assert_eq!(sources[0].priority, 10);
        assert!(
            sources[0]
                .base_url
                .as_deref()
                .ok_or("r2-official missing base_url")?
                .contains("r2.dev")
        );
        // GitHub is fallback
        assert_eq!(sources[1].name, "github-default");
        assert_eq!(sources[1].source_type, SourceType::Github);
        assert_eq!(sources[1].priority, 100);
        Ok(())
    }

    #[test]
    fn source_config_serde_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
        let config = SourceConfig {
            name: "local".into(),
            source_type: SourceType::Local,
            priority: 30,
            base_url: None,
            repo: None,
            path: Some("/srv/mirror".into()),
            endpoint: None,
            bucket: None,
            region: None,
        };
        let json = serde_json::to_string(&config)?;
        let decoded: SourceConfig = serde_json::from_str(&json)?;
        assert_eq!(config, decoded);
        Ok(())
    }

    #[test]
    fn s3_config_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
        let config = SourceConfig {
            name: "s3-test".into(),
            source_type: SourceType::S3,
            priority: 5,
            base_url: None,
            repo: None,
            path: None,
            endpoint: Some("https://xxx.r2.cloudflarestorage.com".into()),
            bucket: Some("releases".into()),
            region: Some("auto".into()),
        };
        let json = serde_json::to_string(&config)?;
        let decoded: SourceConfig = serde_json::from_str(&json)?;
        assert_eq!(config, decoded);
        Ok(())
    }
}
