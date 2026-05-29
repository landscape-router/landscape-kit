//! HTTP mirror source — fetches release info from an HTTP(S) mirror.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;

use lkit_core::{Artifact, ReleaseManifest, ReleaseSource, SourceError};

/// A release source backed by an HTTP(S) mirror.
///
/// Expects mirror directory layout: `<base_url>/latest` (text file with tag)
/// and `<base_url>/<tag>/release-manifest.json`.
pub struct HttpMirrorSource {
    name: String,
    /// Base URL pointing to the product directory (e.g. "https://mirror.example.com/landscape").
    base_url: String,
    client: Client,
}

impl HttpMirrorSource {
    /// Create a new HTTP mirror source.
    ///
    /// `base_url` should point to the product directory (no trailing slash).
    pub fn new(name: impl Into<String>, base_url: &str, client: Client) -> Self {
        Self {
            name: name.into(),
            base_url: base_url.trim_end_matches('/').to_string(),
            client,
        }
    }
}

#[async_trait]
impl ReleaseSource for HttpMirrorSource {
    fn name(&self) -> &str {
        &self.name
    }

    async fn latest_tag(&self) -> Result<String, SourceError> {
        let url = format!("{}/latest", self.base_url);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| SourceError::Network(e.to_string()))?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(SourceError::Network("latest pointer not found".into()));
        }

        let resp = resp
            .error_for_status()
            .map_err(|e| SourceError::Network(e.to_string()))?;

        let text = resp
            .text()
            .await
            .map_err(|e| SourceError::Network(e.to_string()))?;

        Ok(text.trim().to_string())
    }

    async fn list_versions(&self) -> Result<Vec<String>, SourceError> {
        Err(SourceError::Config(
            "HTTP mirror does not support listing all versions; \
             use --tag or --latest 1 instead of --all/--latest N/--since"
                .into(),
        ))
    }

    async fn get_artifacts(&self, tag: &str) -> Result<ReleaseManifest, SourceError> {
        let manifest_url = format!("{}/{}/release-manifest.json", self.base_url, tag);
        let resp = self
            .client
            .get(&manifest_url)
            .send()
            .await
            .map_err(|e| SourceError::Network(e.to_string()))?;

        if resp.status().is_success() {
            let text = resp
                .text()
                .await
                .map_err(|e| SourceError::Network(e.to_string()))?;
            return serde_json::from_str(&text)
                .map_err(|e| SourceError::InvalidManifest(e.to_string()));
        }

        // Fallback: try SHASUM256sum.txt
        let shasum_url = format!("{}/{}/SHASUM256sum.txt", self.base_url, tag);
        let resp = self
            .client
            .get(&shasum_url)
            .send()
            .await
            .map_err(|e| SourceError::Network(e.to_string()))?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(SourceError::VersionNotFound { tag: tag.into() });
        }

        let resp = resp
            .error_for_status()
            .map_err(|e| SourceError::Network(e.to_string()))?;

        let text = resp
            .text()
            .await
            .map_err(|e| SourceError::Network(e.to_string()))?;

        parse_shasum_file(&text, tag)
    }

    fn artifact_url(&self, tag: &str, name: &str) -> String {
        format!("{}/{}/{}", self.base_url, tag, name)
    }

    async fn probe(&self, tag: &str) -> Result<Duration, SourceError> {
        let url = format!("{}/{}/release-manifest.json", self.base_url, tag);
        let start = std::time::Instant::now();

        let resp = self
            .client
            .head(&url)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| SourceError::Network(e.to_string()))?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            let shasum_url = format!("{}/{}/SHASUM256sum.txt", self.base_url, tag);
            let resp = self
                .client
                .head(&shasum_url)
                .timeout(Duration::from_secs(10))
                .send()
                .await
                .map_err(|e| SourceError::Network(e.to_string()))?;

            if resp.status() == reqwest::StatusCode::NOT_FOUND {
                return Err(SourceError::VersionNotFound { tag: tag.into() });
            }
            resp.error_for_status()
                .map_err(|e| SourceError::Network(e.to_string()))?;
            return Ok(start.elapsed());
        }

        resp.error_for_status()
            .map_err(|e| SourceError::Network(e.to_string()))?;

        Ok(start.elapsed())
    }
}

/// Parse SHASUM256sum.txt into a ReleaseManifest.
fn parse_shasum_file(content: &str, tag: &str) -> Result<ReleaseManifest, SourceError> {
    let artifacts: Vec<Artifact> = content
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let (hash, name) = line.split_once(char::is_whitespace)?;
            let name = name.trim();
            // Skip SHASUM files (they reference themselves)
            if name.contains("SHASUM") {
                return None;
            }
            let arch = lkit_core::parse_arch(name).map(|info| {
                if info.musl {
                    format!("{}-musl", info.arch)
                } else {
                    info.arch
                }
            });
            Some(Artifact {
                name: name.to_string(),
                sha256: hash.to_string(),
                size: 0,
                arch,
            })
        })
        .collect();

    if artifacts.is_empty() {
        return Err(SourceError::InvalidManifest(
            "SHASUM256sum.txt is empty or unparseable".into(),
        ));
    }

    Ok(ReleaseManifest {
        format_version: 1,
        tag: tag.to_string(),
        generated_at: String::new(),
        generated_by: None,
        artifacts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_mirror_artifact_url() -> Result<(), Box<dyn std::error::Error>> {
        let client = Client::new();
        let src = HttpMirrorSource::new("test", "https://mirror.example.com/landscape", client);
        assert_eq!(
            src.artifact_url("v0.19.2", "static.zip"),
            "https://mirror.example.com/landscape/v0.19.2/static.zip"
        );
        Ok(())
    }

    #[test]
    fn http_mirror_strips_trailing_slash() -> Result<(), Box<dyn std::error::Error>> {
        let client = Client::new();
        let src = HttpMirrorSource::new("test", "https://mirror.example.com/landscape/", client);
        assert_eq!(
            src.artifact_url("v1.0", "file.bin"),
            "https://mirror.example.com/landscape/v1.0/file.bin"
        );
        Ok(())
    }

    #[test]
    fn parse_shasum_valid() -> Result<(), Box<dyn std::error::Error>> {
        let content = "abc123  file1.bin\ndef456  file2.zip\n";
        let manifest = parse_shasum_file(content, "v1.0")?;
        assert_eq!(manifest.artifacts.len(), 2);
        assert_eq!(manifest.artifacts[0].name, "file1.bin");
        assert_eq!(manifest.artifacts[0].sha256, "abc123");
        assert_eq!(manifest.artifacts[1].name, "file2.zip");
        Ok(())
    }

    #[test]
    fn parse_shasum_empty_fails() -> Result<(), Box<dyn std::error::Error>> {
        let result = parse_shasum_file("", "v1.0");
        assert!(result.is_err());
        Ok(())
    }

    #[test]
    fn parse_shasum_skips_blank_lines() -> Result<(), Box<dyn std::error::Error>> {
        let content = "abc123  file.bin\n\n\n";
        let manifest = parse_shasum_file(content, "v1.0")?;
        assert_eq!(manifest.artifacts.len(), 1);
        Ok(())
    }
}
