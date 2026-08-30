pub(crate) mod archive;
pub(crate) mod download;
pub(crate) mod github;
pub(crate) mod http;
#[cfg(test)]
pub(crate) mod test_server;

use semver::Version;
use thiserror::Error;
use url::Url;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Architecture {
    X86_64,
    Aarch64,
}

impl Architecture {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "aarch64",
        }
    }

    pub(crate) fn from_key(key: &str) -> Option<Self> {
        match key {
            "x86_64" => Some(Self::X86_64),
            "aarch64" => Some(Self::Aarch64),
            _ => None,
        }
    }

    pub(crate) fn host() -> Option<Self> {
        Self::from_key(std::env::consts::ARCH)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AssetEncoding {
    Identity,
    Zstd,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Asset {
    pub(crate) url: Url,
    pub(crate) sha256: String,
    pub(crate) size: u64,
    pub(crate) encoding: AssetEncoding,
}

impl Asset {
    pub(crate) fn checked(
        url: Url,
        sha256: String,
        size: u64,
        encoding: AssetEncoding,
    ) -> Result<Self, RepositoryError> {
        if size == 0 {
            return Err(RepositoryError::InvalidRelease(
                "asset size must be greater than 0".into(),
            ));
        }
        if sha256.len() != 64
            || !sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(RepositoryError::InvalidRelease(
                "asset sha256 must be 64 lowercase hex characters".into(),
            ));
        }
        Ok(Self {
            url,
            sha256,
            size,
            encoding,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReleaseAssets {
    pub(crate) webserver: Asset,
    pub(crate) static_archive: Asset,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Release {
    pub(crate) version: Version,
    pub(crate) assets: ReleaseAssets,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProviderKind {
    Github,
    Http,
}

impl ProviderKind {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Github => "github",
            Self::Http => "http",
        }
    }

    pub(crate) fn from_key(key: &str) -> Option<Self> {
        match key {
            "github" => Some(Self::Github),
            "http" => Some(Self::Http),
            _ => None,
        }
    }
}

pub(crate) enum ReleaseProvider {
    Github(Box<github::GithubRepository>),
    Http(Box<http::HttpRepository>),
}

impl ReleaseProvider {
    pub(crate) async fn latest(
        &self,
        architecture: Architecture,
    ) -> Result<Option<Release>, RepositoryError> {
        match self {
            Self::Github(provider) => provider.latest(architecture).await,
            Self::Http(provider) => provider.latest(architecture).await,
        }
    }

    pub(crate) async fn release(
        &self,
        version: &Version,
        architecture: Architecture,
    ) -> Result<Release, RepositoryError> {
        match self {
            Self::Github(provider) => provider.release(version, architecture).await,
            Self::Http(provider) => provider.release(version, architecture).await,
        }
    }

    /// 解析前端源 latest/stable 的 `static` 资产（静态-only，不要求 webserver）。
    pub(crate) async fn latest_static_archive(&self) -> Result<Asset, RepositoryError> {
        match self {
            Self::Github(provider) => provider.latest_static_archive().await,
            Self::Http(provider) => provider.latest_static_archive().await,
        }
    }

    pub(crate) fn kind(&self) -> ProviderKind {
        match self {
            Self::Github(_) => ProviderKind::Github,
            Self::Http(_) => ProviderKind::Http,
        }
    }

    pub(crate) fn location(&self) -> &str {
        match self {
            Self::Github(provider) => provider.location(),
            Self::Http(provider) => provider.location(),
        }
    }
}

pub(crate) fn provider_for(
    kind: ProviderKind,
    location: &str,
) -> Result<ReleaseProvider, RepositoryError> {
    Ok(match kind {
        ProviderKind::Github => {
            ReleaseProvider::Github(Box::new(github::GithubRepository::new(location)?))
        }
        ProviderKind::Http => ReleaseProvider::Http(Box::new(http::HttpRepository::new(location)?)),
    })
}

#[derive(Debug, Error)]
pub(crate) enum RepositoryError {
    #[error("invalid repository URL: {0}")]
    InvalidUrl(url::ParseError),
    #[error("unsafe repository URL: {0}")]
    UnsafeUrl(String),
    #[error("failed to create HTTP client: {0}")]
    BuildClient(reqwest::Error),
    #[error("repository request failed: {0}")]
    Request(reqwest::Error),
    #[error("network transfer timed out: {0}")]
    Timeout(String),
    #[error("repository returned an unexpected status code: {0}")]
    UnexpectedStatus(reqwest::StatusCode),
    #[error("repository rate limited, retry after {0} seconds")]
    RateLimited(u64),
    #[error("repository metadata exceeds the 10 MiB limit")]
    MetadataTooLarge,
    #[error("repository metadata is not valid JSON: {0}")]
    InvalidJson(serde_json::Error),
    #[error("unsupported repository protocol version {0}")]
    UnsupportedProtocol(u64),
    #[error("invalid repository release entry: {0}")]
    InvalidRelease(String),
    #[error("version {version} does not exist in this repository")]
    VersionUnavailable { version: Version },
    #[error("version {version} is missing the {architecture:?} webserver asset")]
    MissingArchitecture {
        version: Version,
        architecture: Architecture,
    },
    #[error(
        "size of downloaded asset for version {version} does not match the declaration: expected {expected} bytes, got {actual} bytes"
    )]
    AssetSizeMismatch {
        version: Version,
        expected: u64,
        actual: u64,
    },
    #[error(
        "SHA-256 of asset for version {version} does not match the declaration: expected {expected}, got {actual}"
    )]
    AssetSha256Mismatch {
        version: Version,
        expected: String,
        actual: String,
    },
    #[error("file operation failed: {0}")]
    Io(std::io::Error),
    #[error("failed to decompress the webserver asset for version {version}: {reason}")]
    Decompress { version: Version, reason: String },
    #[error("failed to extract the static package for version {version}: {reason}")]
    Extract { version: Version, reason: String },
    #[error("invalid GitHub token")]
    InvalidToken,
    #[error("GitHub tag conflict: {0}")]
    TagConflict(String),
    #[error("failed to parse checksum manifest: {0}")]
    ChecksumParse(String),
}

impl RepositoryError {
    pub(crate) fn is_retryable(&self) -> bool {
        match self {
            Self::Request(error) => download::is_retryable_request_error(error),
            Self::Timeout(_) => true,
            _ => false,
        }
    }
}
