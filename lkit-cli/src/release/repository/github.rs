use std::collections::HashMap;

use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderName, HeaderValue, USER_AGENT};
use semver::Version;
use serde::Deserialize;
use url::Url;

use super::download::{DownloadClient, validate_network_url};
use super::{
    Architecture, Asset, AssetEncoding, ProviderKind, Release, ReleaseAssets, RepositoryError,
};

pub(crate) const DEFAULT_REPOSITORY: &str = "ThisSeanZhang/landscape";

const API_ROOT: &str = "https://api.github.com";
const USER_AGENT_VALUE: &str = concat!("lkit/", env!("CARGO_PKG_VERSION"));

pub(crate) struct GithubRepository {
    repository: String,
    client: DownloadClient,
    token: Option<String>,
}

impl GithubRepository {
    pub(crate) fn new(repository: &str) -> Result<Self, RepositoryError> {
        validate_repository_name(repository)?;
        let client = DownloadClient::new()?;
        let token = std::env::var("GITHUB_TOKEN")
            .ok()
            .filter(|value| !value.is_empty());
        Ok(Self {
            repository: repository.to_string(),
            client,
            token,
        })
    }
}

impl GithubRepository {
    pub(crate) async fn latest(
        &self,
        architecture: Architecture,
    ) -> Result<Option<Release>, RepositoryError> {
        let headers = github_headers(self.token.as_deref())?;
        let url = Url::parse(&format!(
            "{API_ROOT}/repos/{}/releases/latest",
            self.repository
        ))
        .map_err(RepositoryError::InvalidUrl)?;
        let Some((_, body)) = self.client.get_metadata(url, headers.clone(), true).await? else {
            return Ok(None);
        };
        let release: GithubRelease =
            serde_json::from_slice(&body).map_err(RepositoryError::InvalidJson)?;
        let Some(version) = latest_release_version(&release)? else {
            return Ok(None);
        };
        self.build_release(&headers, release, &version, architecture)
            .await
            .map(Some)
    }

    pub(crate) async fn release(
        &self,
        version: &Version,
        architecture: Architecture,
    ) -> Result<Release, RepositoryError> {
        if !version.pre.is_empty() {
            return Err(RepositoryError::InvalidRelease(format!(
                "v1 does not allow installing prerelease version {version}"
            )));
        }
        let headers = github_headers(self.token.as_deref())?;
        let release = self.release_by_tag(&headers, version).await?;
        if release.draft || release.prerelease {
            return Err(RepositoryError::InvalidRelease(format!(
                "the release for version {version} is a draft or prerelease"
            )));
        }
        self.build_release(&headers, release, version, architecture)
            .await
    }

    pub(crate) fn kind(&self) -> ProviderKind {
        ProviderKind::Github
    }

    pub(crate) fn location(&self) -> &str {
        &self.repository
    }

    /// 解析该前端源 latest release 的 `static.zip` 资产（静态-only：不要求
    /// webserver 二进制）。latest 必须携带 `static.zip` 与 `SHASUM256sum.txt`，
    /// 摘要按清单严格解析。
    pub(crate) async fn latest_static_archive(&self) -> Result<Asset, RepositoryError> {
        let headers = github_headers(self.token.as_deref())?;
        let url = Url::parse(&format!(
            "{API_ROOT}/repos/{}/releases/latest",
            self.repository
        ))
        .map_err(RepositoryError::InvalidUrl)?;
        let Some((_, body)) = self.client.get_metadata(url, headers.clone(), true).await? else {
            return Err(RepositoryError::InvalidRelease(format!(
                "frontend repository {} has no latest release",
                self.repository
            )));
        };
        let release: GithubRelease =
            serde_json::from_slice(&body).map_err(RepositoryError::InvalidJson)?;
        if release.draft || release.prerelease {
            return Err(RepositoryError::InvalidRelease(format!(
                "the latest release in frontend repository {} is a draft or prerelease",
                self.repository
            )));
        }
        self.static_asset_from_release(&headers, release).await
    }

    /// 从 release 解析静态资产（静态-only）。与后端 `build_release` 不同，
    /// 只要求 `static.zip` + `SHASUM256sum.txt`。
    async fn static_asset_from_release(
        &self,
        headers: &HeaderMap,
        release: GithubRelease,
    ) -> Result<Asset, RepositoryError> {
        let find = |name: &str| -> Result<&GithubAsset, RepositoryError> {
            let mut matches = release.assets.iter().filter(|asset| asset.name == name);
            let asset = matches.next().ok_or_else(|| {
                RepositoryError::InvalidRelease(format!(
                    "the latest release in frontend repository {} is missing the {name} asset",
                    self.repository
                ))
            })?;
            if matches.next().is_some() {
                return Err(RepositoryError::InvalidRelease(format!(
                    "the latest release in frontend repository {} contains duplicate asset {name}",
                    self.repository
                )));
            }
            Ok(asset)
        };
        let static_archive = find("static.zip")?;
        let checksum_asset = find("SHASUM256sum.txt")?;

        let checksum_url = Url::parse(&checksum_asset.browser_download_url)
            .map_err(RepositoryError::InvalidUrl)?;
        validate_github_download_url(&checksum_url, &self.repository)?;
        let Some((_, body)) = self
            .client
            .get_metadata(checksum_url, headers.clone(), false)
            .await?
        else {
            unreachable!("required metadata does not return None");
        };
        let checksums = parse_checksums(&body)?;
        let static_sha = checksums.get("static.zip").ok_or_else(|| {
            RepositoryError::ChecksumParse(
                "SHASUM256sum.txt is missing a checksum for static.zip".into(),
            )
        })?;

        let static_url = Url::parse(&static_archive.browser_download_url)
            .map_err(RepositoryError::InvalidUrl)?;
        validate_github_download_url(&static_url, &self.repository)?;
        Asset::checked(
            static_url,
            static_sha.clone(),
            static_archive.size,
            AssetEncoding::Identity,
        )
    }

    async fn release_by_tag(
        &self,
        headers: &HeaderMap,
        version: &Version,
    ) -> Result<GithubRelease, RepositoryError> {
        let canonical = version.to_string();
        let with_v = format!("v{canonical}");
        let bare = self.fetch_tag(headers, &canonical).await?;
        let prefixed = self.fetch_tag(headers, &with_v).await?;
        match (bare, prefixed) {
            (Some(_), Some(_)) => Err(RepositoryError::TagConflict(format!(
                "tags {canonical} and {with_v} both exist"
            ))),
            (Some(release), None) | (None, Some(release)) => Ok(release),
            (None, None) => Err(RepositoryError::VersionUnavailable {
                version: version.clone(),
            }),
        }
    }

    async fn fetch_tag(
        &self,
        headers: &HeaderMap,
        tag: &str,
    ) -> Result<Option<GithubRelease>, RepositoryError> {
        let url = Url::parse(&format!(
            "{API_ROOT}/repos/{}/releases/tags/{tag}",
            self.repository
        ))
        .map_err(RepositoryError::InvalidUrl)?;
        match self.client.get_metadata(url, headers.clone(), true).await? {
            None => Ok(None),
            Some((_, body)) => serde_json::from_slice(&body)
                .map(Some)
                .map_err(RepositoryError::InvalidJson),
        }
    }

    async fn build_release(
        &self,
        headers: &HeaderMap,
        release: GithubRelease,
        version: &Version,
        architecture: Architecture,
    ) -> Result<Release, RepositoryError> {
        let asset_name = match architecture {
            Architecture::X86_64 => "landscape-webserver-x86_64",
            Architecture::Aarch64 => "landscape-webserver-aarch64",
        };
        let webserver = unique_asset(&release.assets, asset_name, version)?.ok_or_else(|| {
            RepositoryError::MissingArchitecture {
                version: version.clone(),
                architecture,
            }
        })?;
        let static_archive =
            unique_asset(&release.assets, "static.zip", version)?.ok_or_else(|| {
                RepositoryError::InvalidRelease(format!(
                    "release {version} is missing the static.zip asset"
                ))
            })?;
        let checksum_asset = unique_asset(&release.assets, "SHASUM256sum.txt", version)?
            .ok_or_else(|| {
                RepositoryError::InvalidRelease(format!(
                    "release {version} is missing SHASUM256sum.txt"
                ))
            })?;

        let checksum_url = Url::parse(&checksum_asset.browser_download_url)
            .map_err(RepositoryError::InvalidUrl)?;
        validate_github_download_url(&checksum_url, &self.repository)?;
        let Some((_, body)) = self
            .client
            .get_metadata(checksum_url, headers.clone(), false)
            .await?
        else {
            unreachable!("必填元数据不会返回 None")
        };
        let checksums = parse_checksums(&body)?;
        let webserver_sha = checksums.get(asset_name).ok_or_else(|| {
            RepositoryError::ChecksumParse(format!(
                "SHASUM256sum.txt is missing a checksum for {asset_name}"
            ))
        })?;
        let static_sha = checksums.get("static.zip").ok_or_else(|| {
            RepositoryError::ChecksumParse(
                "SHASUM256sum.txt is missing a checksum for static.zip".into(),
            )
        })?;

        let webserver_url =
            Url::parse(&webserver.browser_download_url).map_err(RepositoryError::InvalidUrl)?;
        validate_github_download_url(&webserver_url, &self.repository)?;
        let static_url = Url::parse(&static_archive.browser_download_url)
            .map_err(RepositoryError::InvalidUrl)?;
        validate_github_download_url(&static_url, &self.repository)?;

        Ok(Release {
            version: version.clone(),
            assets: ReleaseAssets {
                webserver: Asset::checked(
                    webserver_url,
                    webserver_sha.clone(),
                    webserver.size,
                    AssetEncoding::Identity,
                )?,
                static_archive: Asset::checked(
                    static_url,
                    static_sha.clone(),
                    static_archive.size,
                    AssetEncoding::Identity,
                )?,
            },
        })
    }
}

fn github_headers(token: Option<&str>) -> Result<HeaderMap, RepositoryError> {
    let mut headers = HeaderMap::new();
    headers.insert(
        ACCEPT,
        HeaderValue::from_static("application/vnd.github+json"),
    );
    headers.insert(USER_AGENT, HeaderValue::from_static(USER_AGENT_VALUE));
    headers.insert(
        HeaderName::from_static("x-github-api-version"),
        HeaderValue::from_static("2022-11-28"),
    );
    if let Some(token) = token {
        let value = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| {
            RepositoryError::UnsafeUrl("GITHUB_TOKEN contains invalid characters".into())
        })?;
        headers.insert(AUTHORIZATION, value);
    }
    Ok(headers)
}

fn validate_repository_name(repository: &str) -> Result<(), RepositoryError> {
    let mut parts = repository.split('/');
    let owner = parts.next().unwrap_or_default();
    let name = parts.next().unwrap_or_default();
    if owner.is_empty()
        || name.is_empty()
        || parts.next().is_some()
        || !owner
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(RepositoryError::InvalidRelease(
            "GitHub repository must use owner/repo format".into(),
        ));
    }
    Ok(())
}

fn validate_github_download_url(url: &Url, repository: &str) -> Result<(), RepositoryError> {
    validate_network_url(url)?;
    let prefix = format!("/{repository}/releases/download/");
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || url.port_or_known_default() != Some(443)
        || url.fragment().is_some()
        || !url.path().starts_with(&prefix)
    {
        return Err(RepositoryError::UnsafeUrl(
            "GitHub asset URL must point into the current repository's release download directory"
                .into(),
        ));
    }
    Ok(())
}

fn unique_asset<'a>(
    assets: &'a [GithubAsset],
    name: &str,
    version: &Version,
) -> Result<Option<&'a GithubAsset>, RepositoryError> {
    let mut matches = assets.iter().filter(|asset| asset.name == name);
    let asset = matches.next();
    if matches.next().is_some() {
        return Err(RepositoryError::InvalidRelease(format!(
            "release {version} contains duplicate asset {name}"
        )));
    }
    Ok(asset)
}

fn parse_tag(tag: &str) -> Option<Version> {
    let value = tag.strip_prefix('v').unwrap_or(tag);
    let version = Version::parse(value).ok()?;
    if version.to_string() != value || !version.pre.is_empty() {
        return None;
    }
    Some(version)
}

fn latest_release_version(release: &GithubRelease) -> Result<Option<Version>, RepositoryError> {
    if release.draft || release.prerelease {
        return Err(RepositoryError::InvalidRelease(
            "GitHub latest release must not be a draft or prerelease".into(),
        ));
    }
    Ok(parse_tag(&release.tag_name))
}

/// 严格解析 GNU `sha256sum` 文本格式：每行 64 位小写十六进制、空格或星号、
/// 文件名。返回文件名基名到摘要的映射。
fn parse_checksums(body: &[u8]) -> Result<HashMap<String, String>, RepositoryError> {
    let text = std::str::from_utf8(body).map_err(|_| {
        RepositoryError::ChecksumParse("checksum manifest is not valid UTF-8".into())
    })?;
    let mut map = HashMap::new();
    for (index, line) in text.lines().enumerate() {
        if line.is_empty() {
            continue;
        }
        let line_number = index + 1;
        let bytes = line.as_bytes();
        if bytes.len() < 67 {
            return Err(RepositoryError::ChecksumParse(format!(
                "line {line_number} has an invalid format"
            )));
        }
        let hex = &bytes[..64];
        if !hex
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
        {
            return Err(RepositoryError::ChecksumParse(format!(
                "line {line_number} checksum is not 64 lowercase hex characters"
            )));
        }
        if bytes[64] != b' ' || !matches!(bytes[65], b' ' | b'*') {
            return Err(RepositoryError::ChecksumParse(format!(
                "line {line_number} is missing the GNU sha256sum separator"
            )));
        }
        let name = &line[66..];
        if name.is_empty()
            || name.starts_with('\\')
            || name.contains('/')
            || name == "."
            || name == ".."
        {
            return Err(RepositoryError::ChecksumParse(format!(
                "line {line_number} has an invalid file name"
            )));
        }
        if map
            .insert(name.to_string(), String::from_utf8_lossy(hex).to_string())
            .is_some()
        {
            return Err(RepositoryError::ChecksumParse(format!(
                "line {line_number} contains duplicate file name {name}"
            )));
        }
    }
    Ok(map)
}

#[derive(Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Deserialize)]
struct GithubAsset {
    name: String,
    size: u64,
    browser_download_url: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tag_with_and_without_v() {
        assert_eq!(
            parse_tag("0.19.2").unwrap(),
            Version::parse("0.19.2").unwrap()
        );
        assert_eq!(
            parse_tag("v0.19.2").unwrap(),
            Version::parse("0.19.2").unwrap()
        );
        assert_eq!(parse_tag("v0.19.2-rc.1"), None);
        assert_eq!(parse_tag("0.19"), None);
        assert_eq!(parse_tag("release-0.19.2"), None);
    }

    #[test]
    fn validates_latest_release_version() {
        let release = GithubRelease {
            tag_name: "v1.2.3".into(),
            draft: false,
            prerelease: false,
            assets: Vec::new(),
        };
        assert_eq!(
            latest_release_version(&release).unwrap(),
            Some(Version::new(1, 2, 3))
        );

        let invalid_tag = GithubRelease {
            tag_name: "latest".into(),
            draft: false,
            prerelease: false,
            assets: Vec::new(),
        };
        assert!(latest_release_version(&invalid_tag).unwrap().is_none());

        let prerelease = GithubRelease {
            tag_name: "1.2.3".into(),
            draft: false,
            prerelease: true,
            assets: Vec::new(),
        };
        assert!(latest_release_version(&prerelease).is_err());
    }

    #[test]
    fn parses_checksums() {
        let body = b"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef  landscape-webserver-x86_64\n0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef *static.zip\n0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef  landscape-webserver-aarch64\n";
        let map = parse_checksums(body).unwrap();
        assert_eq!(map.len(), 3);
        assert!(map.contains_key("landscape-webserver-x86_64"));
        assert!(map.contains_key("static.zip"));
        assert!(map.contains_key("landscape-webserver-aarch64"));
    }

    #[test]
    fn rejects_malformed_checksums() {
        let upper = b"0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF  landscape-webserver-x86_64\n";
        assert!(parse_checksums(upper).is_err());

        let no_separator = b"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef/landscape-webserver-x86_64\n";
        assert!(parse_checksums(no_separator).is_err());

        let not_utf8 = [0xFFu8; 66];
        assert!(parse_checksums(&not_utf8).is_err());
    }

    #[test]
    fn rejects_checksum_paths_and_duplicates() {
        let digest = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        assert!(parse_checksums(format!("{digest}  ./asset\n").as_bytes()).is_err());
        assert!(parse_checksums(format!("{digest}  asset\n{digest} *asset\n").as_bytes()).is_err());
    }

    #[test]
    fn validates_repository_names() {
        assert!(validate_repository_name("owner/repo").is_ok());
        assert!(validate_repository_name("owner").is_err());
        assert!(validate_repository_name("owner/repo/extra").is_err());
        assert!(validate_repository_name("owner/re po").is_err());
    }

    #[test]
    fn validates_release_download_urls() {
        let valid = Url::parse("https://github.com/owner/repo/releases/download/v1.2.3/static.zip")
            .unwrap();
        assert!(validate_github_download_url(&valid, "owner/repo").is_ok());
        let wrong_repository =
            Url::parse("https://github.com/other/repo/releases/download/v1.2.3/static.zip")
                .unwrap();
        assert!(validate_github_download_url(&wrong_repository, "owner/repo").is_err());
    }
}
