//! Mirror listing — show synced versions.

use crate::error::MirrorError;
use crate::target::MirrorTarget;

/// Information about a synced version.
#[derive(Debug)]
pub struct VersionInfo {
    /// Version tag.
    pub tag: String,
    /// Number of artifacts (excluding manifest).
    pub artifact_count: usize,
    /// Whether this version has a release manifest.
    pub has_manifest: bool,
}

/// List all synced versions in a mirror target under the given prefix.
pub async fn list_versions(
    target: &dyn MirrorTarget,
    prefix: &str,
) -> Result<Vec<VersionInfo>, MirrorError> {
    let all_keys = target.list(prefix).await?;

    let mut versions: Vec<VersionInfo> = Vec::new();

    let manifest_keys: Vec<&String> = all_keys
        .iter()
        .filter(|k| k.ends_with("release-manifest.json"))
        .collect();

    for manifest_key in &manifest_keys {
        let tag = manifest_key
            .strip_prefix(&format!("{prefix}/"))
            .unwrap_or(manifest_key)
            .strip_suffix("/release-manifest.json")
            .unwrap_or(manifest_key)
            .to_string();

        let version_prefix = format!("{prefix}/{tag}");
        let artifact_count = all_keys
            .iter()
            .filter(|k| k.starts_with(&version_prefix) && !k.ends_with("release-manifest.json"))
            .count();

        versions.push(VersionInfo {
            tag,
            artifact_count,
            has_manifest: true,
        });
    }

    versions.sort_by(|a, b| b.tag.cmp(&a.tag));
    Ok(versions)
}

/// Read the latest pointer from a mirror target.
pub async fn read_latest(
    target: &dyn MirrorTarget,
    prefix: &str,
) -> Result<Option<String>, MirrorError> {
    let latest_key = format!("{prefix}/latest");
    if !target.exists(&latest_key).await.unwrap_or(false) {
        return Ok(None);
    }
    let data = target.read(&latest_key).await?;
    let tag = String::from_utf8(data)
        .map_err(|e| MirrorError::TargetError(format!("invalid latest pointer: {e}")))?;
    Ok(Some(tag.trim().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::target::local::LocalTarget;

    #[tokio::test]
    async fn list_empty_mirror() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let target = LocalTarget::new(dir.path());
        let versions = list_versions(&target, "landscape").await?;
        assert!(versions.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn list_returns_versions_sorted() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let target = LocalTarget::new(dir.path());

        let manifest = lkit_core::ReleaseManifest {
            format_version: 1,
            tag: "v1.0".into(),
            generated_at: "2026-01-01T00:00:00Z".into(),
            generated_by: None,
            artifacts: vec![lkit_core::Artifact {
                name: "file.bin".into(),
                sha256: "abc".into(),
                size: 100,
                arch: None,
            }],
        };
        let json = serde_json::to_string(&manifest)?;

        target
            .upload("landscape/v1.0/release-manifest.json", json.as_bytes())
            .await?;
        target
            .upload("landscape/v1.0/file.bin", b"data")
            .await?;

        let mut manifest2 = manifest.clone();
        manifest2.tag = "v2.0".into();
        let json2 = serde_json::to_string(&manifest2)?;

        target
            .upload("landscape/v2.0/release-manifest.json", json2.as_bytes())
            .await?;
        target
            .upload("landscape/v2.0/file.bin", b"data")
            .await?;

        let versions = list_versions(&target, "landscape").await?;
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0].tag, "v2.0");
        assert_eq!(versions[1].tag, "v1.0");
        Ok(())
    }

    #[tokio::test]
    async fn read_latest_returns_tag() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let target = LocalTarget::new(dir.path());
        target.upload("landscape/latest", b"v2.0\n").await?;

        let latest = read_latest(&target, "landscape").await?;
        assert_eq!(latest.as_deref(), Some("v2.0"));
        Ok(())
    }

    #[tokio::test]
    async fn read_latest_missing() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let target = LocalTarget::new(dir.path());
        let latest = read_latest(&target, "landscape").await?;
        assert!(latest.is_none());
        Ok(())
    }
}
