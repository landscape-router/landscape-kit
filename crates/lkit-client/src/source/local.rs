//! Local filesystem source — reads release artifacts from a local directory.

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;

use lkit_core::{ReleaseManifest, ReleaseSource, SourceError};

/// A release source backed by a local directory.
///
/// Expects directory layout: `<path>/latest` (text file), `<path>/<tag>/release-manifest.json`.
pub struct LocalSource {
    name: String,
    path: PathBuf,
}

impl LocalSource {
    /// Create a new local source.
    pub fn new(name: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self {
            name: name.into(),
            path: path.into(),
        }
    }
}

#[async_trait]
impl ReleaseSource for LocalSource {
    fn name(&self) -> &str {
        &self.name
    }

    async fn latest_tag(&self) -> Result<String, SourceError> {
        let latest_path = self.path.join("latest");
        let content = tokio::fs::read_to_string(&latest_path).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                SourceError::Network("latest pointer not found".into())
            } else {
                SourceError::Io(e.to_string())
            }
        })?;
        Ok(content.trim().to_string())
    }

    async fn list_versions(&self) -> Result<Vec<String>, SourceError> {
        let mut versions = Vec::new();
        let mut entries = tokio::fs::read_dir(&self.path)
            .await
            .map_err(|e| SourceError::Io(e.to_string()))?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| SourceError::Io(e.to_string()))?
        {
            if entry
                .file_type()
                .await
                .map_err(|e| SourceError::Io(e.to_string()))?
                .is_dir()
            {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('v') {
                    versions.push(name);
                }
            }
        }

        // Sort newest-first to match GitHub API convention (required by --since).
        versions.sort_by(|a, b| compare_semver(b, a));
        Ok(versions)
    }

    async fn get_artifacts(&self, tag: &str) -> Result<ReleaseManifest, SourceError> {
        let manifest_path = self.path.join(tag).join("release-manifest.json");
        let content = tokio::fs::read_to_string(&manifest_path)
            .await
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    SourceError::VersionNotFound { tag: tag.into() }
                } else {
                    SourceError::Io(e.to_string())
                }
            })?;

        serde_json::from_str(&content).map_err(|e| SourceError::InvalidManifest(e.to_string()))
    }

    fn artifact_url(&self, tag: &str, name: &str) -> String {
        format!("file://{}/{}/{}", self.path.display(), tag, name)
    }

    async fn probe(&self, tag: &str) -> Result<Duration, SourceError> {
        let manifest_path = self.path.join(tag).join("release-manifest.json");
        let start = std::time::Instant::now();

        if tokio::fs::metadata(&manifest_path).await.is_err() {
            return Err(SourceError::VersionNotFound { tag: tag.into() });
        }

        Ok(start.elapsed())
    }
}

/// Compare two semver-style tags (e.g. "v1.2.3") component by component.
fn compare_semver(a: &str, b: &str) -> std::cmp::Ordering {
    let parse = |s: &str| -> Vec<u64> {
        s.trim_start_matches('v')
            .split('.')
            .filter_map(|c| c.parse::<u64>().ok())
            .collect()
    };
    let va = parse(a);
    let vb = parse(b);
    for (ca, cb) in va.iter().zip(vb.iter()) {
        match ca.cmp(cb) {
            std::cmp::Ordering::Equal => continue,
            other => return other,
        }
    }
    va.len().cmp(&vb.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_source_name() -> Result<(), Box<dyn std::error::Error>> {
        let src = LocalSource::new("my-local", "/tmp/mirror");
        assert_eq!(src.name(), "my-local");
        Ok(())
    }

    #[test]
    fn local_source_artifact_url() -> Result<(), Box<dyn std::error::Error>> {
        let src = LocalSource::new("test", "/srv/mirror/landscape");
        assert_eq!(
            src.artifact_url("v0.19.2", "static.zip"),
            "file:///srv/mirror/landscape/v0.19.2/static.zip"
        );
        Ok(())
    }

    #[tokio::test]
    async fn latest_tag_reads_pointer() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        tokio::fs::write(dir.path().join("latest"), "v2.0.0\n").await?;
        let src = LocalSource::new("test", dir.path());
        assert_eq!(src.latest_tag().await?, "v2.0.0");
        Ok(())
    }

    #[tokio::test]
    async fn latest_tag_missing_file() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let src = LocalSource::new("test", dir.path());
        assert!(src.latest_tag().await.is_err());
        Ok(())
    }

    #[tokio::test]
    async fn list_versions_returns_v_dirs() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        tokio::fs::create_dir(dir.path().join("v1.0.0")).await?;
        tokio::fs::create_dir(dir.path().join("v2.0.0")).await?;
        tokio::fs::write(dir.path().join("latest"), "v2.0.0").await?;

        let src = LocalSource::new("test", dir.path());
        let mut versions = src.list_versions().await?;
        versions.sort();
        assert_eq!(versions, vec!["v1.0.0", "v2.0.0"]);
        Ok(())
    }

    #[tokio::test]
    async fn list_versions_empty_dir() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let src = LocalSource::new("test", dir.path());
        let versions = src.list_versions().await?;
        assert!(versions.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn get_artifacts_reads_manifest() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let tag_dir = dir.path().join("v1.0");
        tokio::fs::create_dir(&tag_dir).await?;

        let manifest = lkit_core::ReleaseManifest {
            format_version: 1,
            tag: "v1.0".into(),
            generated_at: "2026-01-01T00:00:00Z".into(),
            generated_by: None,
            artifacts: vec![lkit_core::Artifact {
                name: "test.bin".into(),
                sha256: "abc".into(),
                size: 100,
                arch: None,
            }],
        };
        let json = serde_json::to_string_pretty(&manifest)?;
        tokio::fs::write(tag_dir.join("release-manifest.json"), json).await?;

        let src = LocalSource::new("test", dir.path());
        let result = src.get_artifacts("v1.0").await?;
        assert_eq!(result.tag, "v1.0");
        assert_eq!(result.artifacts.len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn get_artifacts_missing_version() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let src = LocalSource::new("test", dir.path());
        let result = src.get_artifacts("v999").await;
        assert!(matches!(result, Err(SourceError::VersionNotFound { .. })));
        Ok(())
    }

    #[tokio::test]
    async fn probe_existing_version() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let tag_dir = dir.path().join("v1.0");
        tokio::fs::create_dir(&tag_dir).await?;
        tokio::fs::write(tag_dir.join("release-manifest.json"), "{}").await?;

        let src = LocalSource::new("test", dir.path());
        let latency = src.probe("v1.0").await?;
        assert!(latency < Duration::from_secs(1));
        Ok(())
    }

    #[tokio::test]
    async fn probe_missing_version() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let src = LocalSource::new("test", dir.path());
        let result = src.probe("v999").await;
        assert!(matches!(result, Err(SourceError::VersionNotFound { .. })));
        Ok(())
    }

    #[test]
    fn compare_semver_major() {
        assert_eq!(compare_semver("v2.0", "v1.0"), std::cmp::Ordering::Greater);
    }

    #[test]
    fn compare_semver_minor() {
        assert_eq!(
            compare_semver("v0.19.2", "v0.9.0"),
            std::cmp::Ordering::Greater
        );
    }

    #[test]
    fn compare_semver_equal() {
        assert_eq!(compare_semver("v1.0", "v1.0"), std::cmp::Ordering::Equal);
    }

    #[tokio::test]
    async fn list_versions_newest_first() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        tokio::fs::create_dir(dir.path().join("v0.17.5")).await?;
        tokio::fs::create_dir(dir.path().join("v0.19.2")).await?;
        tokio::fs::create_dir(dir.path().join("v0.9.0")).await?;
        tokio::fs::write(dir.path().join("latest"), "v0.19.2").await?;

        let src = LocalSource::new("test", dir.path());
        let versions = src.list_versions().await?;
        // Must be newest-first regardless of filesystem order.
        assert_eq!(versions, vec!["v0.19.2", "v0.17.5", "v0.9.0"]);
        Ok(())
    }
}
