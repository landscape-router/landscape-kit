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
}

/// Built-in default source (GitHub, lowest priority).
pub fn default_source() -> SourceConfig {
    SourceConfig {
        name: "github-default".into(),
        source_type: SourceType::Github,
        priority: 100,
        base_url: None,
        repo: Some("ThisSeanZhang/landscape".into()),
        path: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_source_is_github_with_lowest_priority() -> Result<(), Box<dyn std::error::Error>> {
        let src = default_source();
        assert_eq!(src.source_type, SourceType::Github);
        assert_eq!(src.priority, 100);
        assert_eq!(src.repo.as_deref(), Some("ThisSeanZhang/landscape"));
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
        };
        let json = serde_json::to_string(&config)?;
        let decoded: SourceConfig = serde_json::from_str(&json)?;
        assert_eq!(config, decoded);
        Ok(())
    }
}
