//! Release manifest schema — describes artifacts in a release.

use serde::{Deserialize, Serialize};

/// A release manifest listing all artifacts for a given version.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReleaseManifest {
    /// Manifest format version (currently 1).
    pub format_version: u32,
    /// Release tag, e.g. "v0.19.2".
    pub tag: String,
    /// ISO 8601 generation timestamp.
    pub generated_at: String,
    /// Tool that generated this manifest.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub generated_by: Option<String>,
    /// List of artifacts in this release.
    pub artifacts: Vec<Artifact>,
}

impl ReleaseManifest {
    /// Find an artifact by name.
    pub fn find_artifact(&self, name: &str) -> Option<&Artifact> {
        self.artifacts.iter().find(|a| a.name == name)
    }

    /// Filter artifacts by architecture. Returns arch-independent + matching arch artifacts.
    pub fn artifacts_for_arch(&self, arch: &str) -> Vec<&Artifact> {
        self.artifacts.iter().filter(|a| a.arch.as_deref().is_none_or(|a| a == arch)).collect()
    }
}

/// A single artifact within a release.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Artifact {
    /// File name of the artifact.
    pub name: String,
    /// SHA-256 hex digest.
    pub sha256: String,
    /// File size in bytes.
    pub size: u64,
    /// Architecture tag (e.g. "x86_64"), or null for arch-independent files.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_manifest() -> ReleaseManifest {
        ReleaseManifest {
            format_version: 1,
            tag: "v0.19.2".into(),
            generated_at: "2026-05-29T12:00:00Z".into(),
            generated_by: Some("lkit 0.1.0".into()),
            artifacts: vec![
                Artifact {
                    name: "landscape-webserver-x86_64".into(),
                    sha256: "abc123".into(),
                    size: 128669136,
                    arch: Some("x86_64".into()),
                },
                Artifact {
                    name: "landscape-webserver-aarch64".into(),
                    sha256: "def456".into(),
                    size: 118326864,
                    arch: Some("aarch64".into()),
                },
                Artifact {
                    name: "static.zip".into(),
                    sha256: "ghi789".into(),
                    size: 2094841,
                    arch: None,
                },
            ],
        }
    }

    #[test]
    fn manifest_serde_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
        let manifest = sample_manifest();
        let json = serde_json::to_string_pretty(&manifest)?;
        let decoded: ReleaseManifest = serde_json::from_str(&json)?;
        assert_eq!(manifest, decoded);
        Ok(())
    }

    #[test]
    fn find_artifact_returns_match() -> Result<(), Box<dyn std::error::Error>> {
        let manifest = sample_manifest();
        let artifact = manifest.find_artifact("static.zip");
        assert!(artifact.is_some());
        assert_eq!(artifact.map(|a| &a.name).ok_or("not found")?, "static.zip");
        Ok(())
    }

    #[test]
    fn find_artifact_returns_none_for_missing() -> Result<(), Box<dyn std::error::Error>> {
        let manifest = sample_manifest();
        assert!(manifest.find_artifact("nonexistent.tar.gz").is_none());
        Ok(())
    }

    #[test]
    fn artifacts_for_arch_includes_matching_and_none() -> Result<(), Box<dyn std::error::Error>> {
        let manifest = sample_manifest();
        let x86 = manifest.artifacts_for_arch("x86_64");
        assert_eq!(x86.len(), 2);
        assert!(x86.iter().any(|a| a.name == "landscape-webserver-x86_64"));
        assert!(x86.iter().any(|a| a.name == "static.zip"));
        Ok(())
    }

    #[test]
    fn artifacts_for_arch_excludes_other_arch() -> Result<(), Box<dyn std::error::Error>> {
        let manifest = sample_manifest();
        let x86 = manifest.artifacts_for_arch("x86_64");
        assert!(!x86.iter().any(|a| a.name == "landscape-webserver-aarch64"));
        Ok(())
    }

    #[test]
    fn manifest_json_omits_none_fields() -> Result<(), Box<dyn std::error::Error>> {
        let manifest = ReleaseManifest {
            format_version: 1,
            tag: "v1.0".into(),
            generated_at: "2026-01-01T00:00:00Z".into(),
            generated_by: None,
            artifacts: vec![],
        };
        let json = serde_json::to_string(&manifest)?;
        assert!(!json.contains("generated_by"));
        Ok(())
    }
}
