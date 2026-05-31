//! Mirror verification — check integrity of synced releases.

use lkit_core::ReleaseManifest;

use crate::error::MirrorError;
use crate::sync::compute_sha256;
use crate::target::MirrorTarget;

/// Verification result for a single version.
#[derive(Debug)]
pub struct VersionVerifyResult {
    /// Version tag that was verified.
    pub tag: String,
    /// Whether verification passed.
    pub passed: bool,
    /// List of errors found (empty if passed).
    pub errors: Vec<String>,
}

/// Verify all versions in a mirror target under the given prefix.
pub async fn verify(
    target: &dyn MirrorTarget,
    prefix: &str,
) -> Result<Vec<VersionVerifyResult>, MirrorError> {
    let mut results = Vec::new();

    let all_keys = target.list(prefix).await?;
    let manifest_keys: Vec<String> =
        all_keys.into_iter().filter(|k| k.ends_with("release-manifest.json")).collect();

    for manifest_key in &manifest_keys {
        let tag = manifest_key
            .strip_prefix(&format!("{prefix}/"))
            .unwrap_or(manifest_key)
            .strip_suffix("/release-manifest.json")
            .unwrap_or(manifest_key)
            .to_string();

        let result = verify_version(target, prefix, &tag).await;
        results.push(result);
    }

    // Check latest pointer
    let latest_key = format!("{prefix}/latest");
    if target.exists(&latest_key).await.unwrap_or(false)
        && let Ok(data) = target.read(&latest_key).await
        && let Ok(latest_tag) = String::from_utf8(data)
    {
        let latest_tag = latest_tag.trim();
        let latest_manifest = format!("{prefix}/{latest_tag}/release-manifest.json");
        if !target.exists(&latest_manifest).await.unwrap_or(false) {
            results.push(VersionVerifyResult {
                tag: format!("latest -> {latest_tag}"),
                passed: false,
                errors: vec![format!(
                    "latest points to {latest_tag} but that version does not exist"
                )],
            });
        }
    }

    Ok(results)
}

/// Verify a single version's integrity.
async fn verify_version(target: &dyn MirrorTarget, prefix: &str, tag: &str) -> VersionVerifyResult {
    let mut errors = Vec::new();
    let manifest_key = format!("{prefix}/{tag}/release-manifest.json");

    let manifest_data = match target.read(&manifest_key).await {
        Ok(data) => data,
        Err(e) => {
            return VersionVerifyResult {
                tag: tag.to_string(),
                passed: false,
                errors: vec![format!("cannot read manifest: {e}")],
            };
        }
    };

    let manifest: ReleaseManifest = match serde_json::from_slice(&manifest_data) {
        Ok(m) => m,
        Err(e) => {
            return VersionVerifyResult {
                tag: tag.to_string(),
                passed: false,
                errors: vec![format!("invalid manifest JSON: {e}")],
            };
        }
    };

    for artifact in &manifest.artifacts {
        let key = format!("{prefix}/{tag}/{}", artifact.name);

        if !target.exists(&key).await.unwrap_or(false) {
            errors.push(format!("missing artifact: {}", artifact.name));
            continue;
        }

        if !artifact.sha256.is_empty() {
            if let Ok(data) = target.read(&key).await {
                let actual = compute_sha256(&data);
                if actual != artifact.sha256 {
                    errors.push(format!(
                        "checksum mismatch for {}: expected {}, got {}",
                        artifact.name, artifact.sha256, actual
                    ));
                }
            } else {
                errors.push(format!("cannot read artifact: {}", artifact.name));
            }
        }
    }

    VersionVerifyResult {
        tag: tag.to_string(),
        passed: errors.is_empty(),
        errors,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::target::local::LocalTarget;

    #[tokio::test]
    async fn verify_empty_mirror() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let target = LocalTarget::new(dir.path());
        let results = verify(&target, "landscape").await?;
        assert!(results.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn verify_valid_version() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let target = LocalTarget::new(dir.path());

        let manifest = lkit_core::ReleaseManifest {
            format_version: 1,
            tag: "v1.0".into(),
            generated_at: "2026-01-01T00:00:00Z".into(),
            generated_by: None,
            artifacts: vec![lkit_core::Artifact {
                name: "test.txt".into(),
                sha256: compute_sha256(b"hello"),
                size: 5,
                arch: None,
            }],
        };

        target
            .upload(
                "landscape/v1.0/release-manifest.json",
                serde_json::to_string_pretty(&manifest)?.as_bytes(),
            )
            .await?;
        target.upload("landscape/v1.0/test.txt", b"hello").await?;

        let results = verify(&target, "landscape").await?;
        assert_eq!(results.len(), 1);
        assert!(results[0].passed);
        Ok(())
    }

    #[tokio::test]
    async fn verify_detects_checksum_mismatch() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let target = LocalTarget::new(dir.path());

        let manifest = lkit_core::ReleaseManifest {
            format_version: 1,
            tag: "v1.0".into(),
            generated_at: "2026-01-01T00:00:00Z".into(),
            generated_by: None,
            artifacts: vec![lkit_core::Artifact {
                name: "test.txt".into(),
                sha256: "wrong_hash".into(),
                size: 5,
                arch: None,
            }],
        };

        target
            .upload(
                "landscape/v1.0/release-manifest.json",
                serde_json::to_string_pretty(&manifest)?.as_bytes(),
            )
            .await?;
        target.upload("landscape/v1.0/test.txt", b"hello").await?;

        let results = verify(&target, "landscape").await?;
        assert_eq!(results.len(), 1);
        assert!(!results[0].passed);
        assert!(results[0].errors[0].contains("checksum mismatch"));
        Ok(())
    }
}
