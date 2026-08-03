use semver::Version;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const PROTOCOL_VERSION: u64 = 1;
pub const WEBSERVER_X86_64: &str = "landscape-webserver-x86_64.zst";
pub const WEBSERVER_AARCH64: &str = "landscape-webserver-aarch64.zst";
pub const STATIC_ARCHIVE: &str = "static.zip";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RepositoryDescriptor {
    pub protocol_version: u64,
}

impl RepositoryDescriptor {
    pub fn v1() -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
        }
    }

    pub fn validate(&self) -> Result<(), ProtocolError> {
        validate_protocol(self.protocol_version)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct StableChannel {
    pub protocol_version: u64,
    pub version: String,
}

impl StableChannel {
    pub fn new(version: &Version) -> Result<Self, ProtocolError> {
        validate_stable_version(version)?;
        Ok(Self {
            protocol_version: PROTOCOL_VERSION,
            version: version.to_string(),
        })
    }

    pub fn parsed_version(&self) -> Result<Version, ProtocolError> {
        validate_protocol(self.protocol_version)?;
        parse_stable_version(&self.version)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ReleaseManifest {
    pub protocol_version: u64,
    pub version: String,
    pub assets: RepositoryAssets,
}

impl ReleaseManifest {
    pub fn new(version: &Version, assets: RepositoryAssets) -> Result<Self, ProtocolError> {
        validate_stable_version(version)?;
        Ok(Self {
            protocol_version: PROTOCOL_VERSION,
            version: version.to_string(),
            assets,
        })
    }

    pub fn parsed_version(&self) -> Result<Version, ProtocolError> {
        validate_protocol(self.protocol_version)?;
        parse_stable_version(&self.version)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RepositoryAssets {
    pub webserver: WebserverAssets,
    #[serde(rename = "static")]
    pub static_archive: RepositoryAsset,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WebserverAssets {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x86_64: Option<RepositoryAsset>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aarch64: Option<RepositoryAsset>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RepositoryAsset {
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

impl RepositoryAsset {
    pub fn new(url: String, sha256: String, size: u64) -> Result<Self, ProtocolError> {
        if size == 0 {
            return Err(ProtocolError::InvalidAssetSize);
        }
        if sha256.len() != 64
            || !sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(ProtocolError::InvalidSha256);
        }
        Ok(Self { url, sha256, size })
    }
}

pub fn parse_stable_version(value: &str) -> Result<Version, ProtocolError> {
    let version = Version::parse(value).map_err(ProtocolError::InvalidVersion)?;
    validate_stable_version(&version)?;
    if version.to_string() != value {
        return Err(ProtocolError::NonCanonicalVersion(value.into()));
    }
    Ok(version)
}

pub fn validate_stable_version(version: &Version) -> Result<(), ProtocolError> {
    if !version.pre.is_empty() || !version.build.is_empty() {
        return Err(ProtocolError::UnstableVersion(version.clone()));
    }
    Ok(())
}

pub fn validate_protocol(protocol_version: u64) -> Result<(), ProtocolError> {
    if protocol_version == PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(ProtocolError::UnsupportedProtocol(protocol_version))
    }
}

pub fn zip_path_parts(relative: &str) -> Result<Vec<&str>, ZipPathError> {
    if relative.contains('\\') || relative.contains('\0') {
        return Err(ZipPathError::InvalidCharacters);
    }
    let mut parts = Vec::new();
    for part in relative.split('/') {
        if part.is_empty() {
            continue;
        }
        if matches!(part, "." | "..") {
            return Err(ZipPathError::DotComponent);
        }
        if part.contains(':') {
            return Err(ZipPathError::DrivePrefix);
        }
        parts.push(part);
    }
    Ok(parts)
}

#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ZipPathError {
    #[error("path contains invalid characters")]
    InvalidCharacters,
    #[error("path contains . or .. components")]
    DotComponent,
    #[error("path contains a drive prefix")]
    DrivePrefix,
}

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("unsupported repository protocol version {0}")]
    UnsupportedProtocol(u64),
    #[error("invalid semantic version: {0}")]
    InvalidVersion(semver::Error),
    #[error("version must use canonical SemVer: {0}")]
    NonCanonicalVersion(String),
    #[error("prerelease and build metadata are not supported: {0}")]
    UnstableVersion(Version),
    #[error("asset size must be greater than zero")]
    InvalidAssetSize,
    #[error("asset sha256 must contain 64 lowercase hexadecimal characters")]
    InvalidSha256,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_canonical_stable_version() {
        assert_eq!(
            parse_stable_version("1.2.3").unwrap(),
            Version::new(1, 2, 3)
        );
    }

    #[test]
    fn rejects_prerelease_and_build_versions() {
        assert!(parse_stable_version("1.2.3-beta.1").is_err());
        assert!(parse_stable_version("1.2.3+build.1").is_err());
    }

    #[test]
    fn creates_valid_asset() {
        let asset = RepositoryAsset::new("static.zip".into(), "a".repeat(64), 42).unwrap();
        assert_eq!(asset.size, 42);
    }

    #[test]
    fn validates_zip_relative_paths() {
        assert_eq!(
            zip_path_parts("assets/app.js").unwrap(),
            ["assets", "app.js"]
        );
        assert_eq!(
            zip_path_parts("assets//app.js").unwrap(),
            ["assets", "app.js"]
        );
        assert_eq!(zip_path_parts("../app.js"), Err(ZipPathError::DotComponent));
        assert_eq!(zip_path_parts("C:/app.js"), Err(ZipPathError::DrivePrefix));
        assert_eq!(
            zip_path_parts("assets\\app.js"),
            Err(ZipPathError::InvalidCharacters)
        );
    }
}
