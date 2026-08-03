use lkit_repository::{
    ProtocolError, ReleaseManifest, RepositoryAsset, RepositoryDescriptor, StableChannel,
};
use reqwest::header::HeaderMap;
use semver::Version;
use serde::de::DeserializeOwned;
use url::Url;

use super::download::{DownloadClient, validate_network_url};
use super::{
    Architecture, Asset, AssetEncoding, ProviderKind, Release, ReleaseAssets, RepositoryError,
};

#[derive(Debug)]
pub(crate) struct HttpRepository {
    base_url: Url,
    repository_url: Url,
    stable_url: Url,
    client: DownloadClient,
}

impl HttpRepository {
    pub(crate) fn new(base_url: &str) -> Result<Self, RepositoryError> {
        let base_url = normalize_base_url(base_url)?;
        let repository_url = base_url
            .join("repository.json")
            .map_err(RepositoryError::InvalidUrl)?;
        let stable_url = base_url
            .join("channels/stable.json")
            .map_err(RepositoryError::InvalidUrl)?;
        Ok(Self {
            base_url,
            repository_url,
            stable_url,
            client: DownloadClient::new()?,
        })
    }

    pub(crate) async fn latest(
        &self,
        architecture: Architecture,
    ) -> Result<Option<Release>, RepositoryError> {
        self.validate_repository().await?;
        let Some(channel) = self
            .get_json::<StableChannel>(self.stable_url.clone(), true)
            .await?
        else {
            return Ok(None);
        };
        let version = channel
            .parsed_version()
            .map_err(|error| protocol_error(error, "stable pointer"))?;
        self.release_manifest(&version, architecture)
            .await
            .map(Some)
    }

    pub(crate) async fn release(
        &self,
        version: &Version,
        architecture: Architecture,
    ) -> Result<Release, RepositoryError> {
        self.validate_repository().await?;
        if !version.pre.is_empty() {
            return Err(RepositoryError::InvalidRelease(format!(
                "v1 does not allow installing prerelease version {version}"
            )));
        }
        self.release_manifest(version, architecture).await
    }

    pub(crate) fn kind(&self) -> ProviderKind {
        ProviderKind::Http
    }

    pub(crate) fn location(&self) -> &str {
        self.base_url.as_str()
    }

    async fn validate_repository(&self) -> Result<(), RepositoryError> {
        let descriptor = self
            .get_json::<RepositoryDescriptor>(self.repository_url.clone(), false)
            .await?
            .expect("必填元数据不会返回 None");
        descriptor
            .validate()
            .map_err(|error| protocol_error(error, "repository.json"))
    }

    async fn release_manifest(
        &self,
        version: &Version,
        architecture: Architecture,
    ) -> Result<Release, RepositoryError> {
        let manifest_url = self
            .base_url
            .join(&format!("releases/{version}/manifest.json"))
            .map_err(RepositoryError::InvalidUrl)?;
        let manifest = self
            .get_json::<ReleaseManifest>(manifest_url.clone(), true)
            .await?
            .ok_or_else(|| RepositoryError::VersionUnavailable {
                version: version.clone(),
            })?;
        let manifest_version = manifest
            .parsed_version()
            .map_err(|error| protocol_error(error, "version manifest"))?;
        if &manifest_version != version {
            return Err(RepositoryError::InvalidRelease(format!(
                "manifest version {} does not match requested version {version}",
                manifest.version
            )));
        }

        let webserver = match architecture {
            Architecture::X86_64 => manifest.assets.webserver.x86_64.as_ref(),
            Architecture::Aarch64 => manifest.assets.webserver.aarch64.as_ref(),
        }
        .ok_or_else(|| RepositoryError::MissingArchitecture {
            version: version.clone(),
            architecture,
        })?;
        let manifest_base_url = manifest_url
            .join("./")
            .map_err(RepositoryError::InvalidUrl)?;

        Ok(Release {
            version: version.clone(),
            assets: ReleaseAssets {
                webserver: resolve_asset(&manifest_base_url, webserver, AssetEncoding::Zstd)?,
                static_archive: resolve_asset(
                    &manifest_base_url,
                    &manifest.assets.static_archive,
                    AssetEncoding::Identity,
                )?,
            },
        })
    }

    async fn get_json<T: DeserializeOwned>(
        &self,
        url: Url,
        allow_missing: bool,
    ) -> Result<Option<T>, RepositoryError> {
        let Some((_, body)) = self
            .client
            .get_metadata(url, HeaderMap::new(), allow_missing)
            .await?
        else {
            return Ok(None);
        };
        serde_json::from_slice(&body)
            .map(Some)
            .map_err(RepositoryError::InvalidJson)
    }
}

fn normalize_base_url(value: &str) -> Result<Url, RepositoryError> {
    let mut url = Url::parse(value).map_err(RepositoryError::InvalidUrl)?;
    validate_network_url(&url)?;

    if url.query().is_some() || url.fragment().is_some() {
        return Err(RepositoryError::UnsafeUrl(
            "base URL must not contain a query or fragment".into(),
        ));
    }

    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }

    Ok(url)
}

fn protocol_error(error: ProtocolError, source: &str) -> RepositoryError {
    match error {
        ProtocolError::UnsupportedProtocol(version) => {
            RepositoryError::UnsupportedProtocol(version)
        }
        error => RepositoryError::InvalidRelease(format!("{source} is invalid: {error}")),
    }
}

fn resolve_asset(
    base_url: &Url,
    asset: &RepositoryAsset,
    encoding: AssetEncoding,
) -> Result<Asset, RepositoryError> {
    let url = match Url::parse(&asset.url) {
        Ok(url) => url,
        Err(url::ParseError::RelativeUrlWithoutBase) => {
            let url = base_url
                .join(&asset.url)
                .map_err(RepositoryError::InvalidUrl)?;
            if url.scheme() != base_url.scheme()
                || url.host_str() != base_url.host_str()
                || url.port_or_known_default() != base_url.port_or_known_default()
                || !url.path().starts_with(base_url.path())
            {
                return Err(RepositoryError::UnsafeUrl(
                    "relative asset URL escapes the release directory".into(),
                ));
            }
            url
        }
        Err(error) => return Err(RepositoryError::InvalidUrl(error)),
    };
    validate_network_url(&url)?;
    Asset::checked(url, asset.sha256.clone(), asset.size, encoding)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::super::download::DownloadClient;
    use super::super::test_server::{TestResponse, TestServer};
    use super::*;

    fn start_repository_server(files: &HashMap<String, Vec<u8>>) -> TestServer {
        let files = files.clone();
        TestServer::start(move |path| match files.get(path) {
            Some(body) => TestResponse::ok(body.clone()),
            None => TestResponse::status(404, "Not Found", Vec::new()),
        })
    }

    fn fast_repository(server: &TestServer) -> HttpRepository {
        let base = normalize_base_url(&server.base).unwrap();
        let repository_url = base.join("repository.json").unwrap();
        let stable_url = base.join("channels/stable.json").unwrap();
        let client = DownloadClient::new().unwrap().with_retry_timing(
            [
                std::time::Duration::from_millis(10),
                std::time::Duration::from_millis(10),
            ],
            std::time::Duration::from_millis(1),
        );
        HttpRepository {
            base_url: base,
            repository_url,
            stable_url,
            client,
        }
    }

    fn repository_files() -> HashMap<String, Vec<u8>> {
        let mut files = HashMap::new();
        files.insert(
            "/repository.json".into(),
            br#"{"protocol_version":1}"#.to_vec(),
        );
        files.insert(
            "/channels/stable.json".into(),
            br#"{"protocol_version":1,"version":"1.2.3"}"#.to_vec(),
        );
        files.insert(
            "/releases/1.2.3/manifest.json".into(),
            manifest("landscape-webserver-x86_64.zst", "1.2.3", true),
        );
        files
    }

    #[tokio::test]
    async fn resolves_latest_stable_from_repository() {
        let server = start_repository_server(&repository_files());
        let repository = HttpRepository::new(&server.base).unwrap();
        let release = repository
            .latest(Architecture::X86_64)
            .await
            .unwrap()
            .expect("应解析出 stable 版本");
        assert_eq!(release.version, Version::parse("1.2.3").unwrap());
        assert_eq!(
            release.assets.webserver.url.as_str(),
            format!(
                "{}/releases/1.2.3/landscape-webserver-x86_64.zst",
                server.base
            )
        );
        assert_eq!(
            server.request_paths(),
            vec![
                "/repository.json",
                "/channels/stable.json",
                "/releases/1.2.3/manifest.json"
            ]
        );
    }

    #[tokio::test]
    async fn latest_returns_none_without_stable_channel() {
        let mut files = repository_files();
        files.remove("/channels/stable.json");
        let server = start_repository_server(&files);
        let repository = HttpRepository::new(&server.base).unwrap();
        assert!(
            repository
                .latest(Architecture::X86_64)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn explicit_release_missing_manifest_is_unavailable() {
        let server = start_repository_server(&repository_files());
        let repository = HttpRepository::new(&server.base).unwrap();
        let error = repository
            .release(&Version::parse("9.9.9").unwrap(), Architecture::X86_64)
            .await
            .unwrap_err();
        assert!(matches!(error, RepositoryError::VersionUnavailable { .. }));
    }

    #[tokio::test]
    async fn rejects_unsupported_repository_protocol() {
        let mut files = repository_files();
        files.insert(
            "/repository.json".into(),
            br#"{"protocol_version":2}"#.to_vec(),
        );
        let server = start_repository_server(&files);
        let repository = HttpRepository::new(&server.base).unwrap();
        let error = repository.latest(Architecture::X86_64).await.unwrap_err();
        assert!(matches!(error, RepositoryError::UnsupportedProtocol(2)));
    }

    #[tokio::test]
    async fn rejects_missing_repository_descriptor() {
        let server = start_repository_server(&HashMap::new());
        let repository = HttpRepository::new(&server.base).unwrap();
        let error = repository.latest(Architecture::X86_64).await.unwrap_err();
        assert!(matches!(error, RepositoryError::UnexpectedStatus(_)));
    }

    #[tokio::test]
    async fn rejects_invalid_json_manifest() {
        let mut files = repository_files();
        files.insert(
            "/releases/1.2.3/manifest.json".into(),
            b"not valid json".to_vec(),
        );
        let server = start_repository_server(&files);
        let repository = HttpRepository::new(&server.base).unwrap();
        let error = repository.latest(Architecture::X86_64).await.unwrap_err();
        assert!(matches!(error, RepositoryError::InvalidJson(_)));
    }

    #[tokio::test]
    async fn fails_after_three_attempts_on_server_error() {
        let server = TestServer::start(|_| {
            TestResponse::status(500, "Internal Server Error", b"boom".to_vec())
        });
        let repository = fast_repository(&server);
        let error = repository.latest(Architecture::X86_64).await.unwrap_err();
        assert!(matches!(error, RepositoryError::UnexpectedStatus(_)));
        assert_eq!(server.request_count(), 3);
    }

    #[tokio::test]
    async fn rejects_declared_body_over_limit() {
        let server = TestServer::start(|_| {
            let size = 11 * 1024 * 1024;
            TestResponse::raw(
                200,
                "OK",
                vec![("Content-Length".into(), size.to_string())],
                Vec::new(),
            )
        });
        let repository = fast_repository(&server);
        let error = repository.latest(Architecture::X86_64).await.unwrap_err();
        assert!(matches!(error, RepositoryError::MetadataTooLarge));
    }

    #[tokio::test]
    async fn rejects_streamed_body_over_limit() {
        let server = TestServer::start(|_| {
            let body = vec![0u8; 11 * 1024 * 1024];
            TestResponse::raw(200, "OK", Vec::new(), body)
        });
        let repository = fast_repository(&server);
        let error = repository.latest(Architecture::X86_64).await.unwrap_err();
        assert!(matches!(error, RepositoryError::MetadataTooLarge));
    }

    #[tokio::test]
    async fn follows_loopback_redirect() {
        let files = repository_files();
        let server = TestServer::start(move |path| {
            if path == "/repository.json" {
                TestResponse::redirect(302, "/repository-redirected.json")
            } else if path == "/repository-redirected.json" {
                TestResponse::ok(files["/repository.json"].clone())
            } else {
                match files.get(path) {
                    Some(body) => TestResponse::ok(body.clone()),
                    None => TestResponse::status(404, "Not Found", Vec::new()),
                }
            }
        });
        let repository = HttpRepository::new(&server.base).unwrap();
        let release = repository
            .latest(Architecture::X86_64)
            .await
            .unwrap()
            .expect("重定向后应能解析");
        assert_eq!(release.version, Version::parse("1.2.3").unwrap());
        assert!(server.request_paths().contains(&"/repository.json".into()));
        assert!(
            server
                .request_paths()
                .contains(&"/repository-redirected.json".into())
        );
    }

    #[tokio::test]
    async fn rejects_unsafe_redirect() {
        let files = repository_files();
        let server = TestServer::start(move |path| {
            if path == "/repository.json" {
                TestResponse::redirect(302, "http://example.com/repository.json")
            } else {
                match files.get(path) {
                    Some(body) => TestResponse::ok(body.clone()),
                    None => TestResponse::status(404, "Not Found", Vec::new()),
                }
            }
        });
        let repository = HttpRepository::new(&server.base).unwrap();
        let error = repository.latest(Architecture::X86_64).await.unwrap_err();
        assert!(matches!(error, RepositoryError::UnexpectedStatus(_)));
        assert_eq!(server.request_count(), 1);
    }

    #[tokio::test]
    async fn rejects_truncated_response() {
        let server = TestServer::start(|_| {
            TestResponse::raw(
                200,
                "OK",
                vec![("Content-Length".into(), "1000".into())],
                b"short".to_vec(),
            )
        });
        let repository = fast_repository(&server);
        let error = repository.latest(Architecture::X86_64).await.unwrap_err();
        assert!(matches!(error, RepositoryError::Request(_)));
        assert_eq!(server.request_count(), 3);
    }

    #[tokio::test]
    async fn fails_on_connection_refused() {
        let repository = HttpRepository::new("http://127.0.0.1:1/").unwrap();
        let error = repository.latest(Architecture::X86_64).await.unwrap_err();
        assert!(matches!(error, RepositoryError::Request(_)));
    }

    const SHA256: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn manifest(webserver_url: &str, version: &str, include_aarch64: bool) -> Vec<u8> {
        let aarch64 = if include_aarch64 {
            format!(
                r#", "aarch64": {{"url": "landscape-webserver-aarch64.zst", "sha256": "{SHA256}", "size": 11}}"#
            )
        } else {
            String::new()
        };
        format!(
            r#"{{
                "protocol_version": 1,
                "version": "{version}",
                "assets": {{
                    "webserver": {{
                        "x86_64": {{"url": "{webserver_url}", "sha256": "{SHA256}", "size": 10}}{aarch64}
                    }},
                    "static": {{"url": "static.zip", "sha256": "{SHA256}", "size": 20}}
                }}
            }}"#
        )
        .into_bytes()
    }

    fn parse_manifest(
        body: &[u8],
        requested_version: &str,
        architecture: Architecture,
    ) -> Result<Release, RepositoryError> {
        let manifest: ReleaseManifest =
            serde_json::from_slice(body).map_err(RepositoryError::InvalidJson)?;
        let requested_version = Version::parse(requested_version).unwrap();
        let manifest_version = manifest
            .parsed_version()
            .map_err(|error| protocol_error(error, "版本 manifest"))?;
        if manifest_version != requested_version {
            return Err(RepositoryError::InvalidRelease(format!(
                "manifest 版本 {} 与请求版本 {requested_version} 不一致",
                manifest.version
            )));
        }
        let webserver = match architecture {
            Architecture::X86_64 => manifest.assets.webserver.x86_64.as_ref(),
            Architecture::Aarch64 => manifest.assets.webserver.aarch64.as_ref(),
        }
        .ok_or_else(|| RepositoryError::MissingArchitecture {
            version: requested_version.clone(),
            architecture,
        })?;
        let base_url = Url::parse(&format!(
            "https://example.com/mirror/releases/{requested_version}/"
        ))
        .unwrap();
        Ok(Release {
            version: requested_version,
            assets: ReleaseAssets {
                webserver: resolve_asset(&base_url, webserver, AssetEncoding::Zstd)?,
                static_archive: resolve_asset(
                    &base_url,
                    &manifest.assets.static_archive,
                    AssetEncoding::Identity,
                )?,
            },
        })
    }

    #[test]
    fn normalizes_repository_base_url() {
        let repository = HttpRepository::new("https://example.com/mirror").unwrap();
        assert_eq!(repository.base_url.as_str(), "https://example.com/mirror/");
        assert_eq!(
            repository.repository_url.as_str(),
            "https://example.com/mirror/repository.json"
        );
        assert_eq!(
            repository.stable_url.as_str(),
            "https://example.com/mirror/channels/stable.json"
        );
    }

    #[test]
    fn rejects_insecure_remote_http() {
        let error = HttpRepository::new("http://example.com/repository").unwrap_err();
        assert!(matches!(error, RepositoryError::UnsafeUrl(_)));
    }

    #[test]
    fn accepts_loopback_http() {
        assert!(HttpRepository::new("http://127.0.0.1:9000/repository").is_ok());
    }

    #[test]
    fn rejects_query_and_fragment_in_base_url() {
        assert!(matches!(
            HttpRepository::new("https://example.com/repo?x=1"),
            Err(RepositoryError::UnsafeUrl(_))
        ));
        assert!(matches!(
            HttpRepository::new("https://example.com/repo#frag"),
            Err(RepositoryError::UnsafeUrl(_))
        ));
    }

    #[test]
    fn parses_version_manifest() {
        let release = parse_manifest(
            &manifest("landscape-webserver-x86_64.zst", "1.2.3", true),
            "1.2.3",
            Architecture::X86_64,
        )
        .unwrap();

        assert_eq!(release.version, Version::parse("1.2.3").unwrap());
        assert_eq!(
            release.assets.webserver.url.as_str(),
            "https://example.com/mirror/releases/1.2.3/landscape-webserver-x86_64.zst"
        );
        assert_eq!(release.assets.webserver.encoding, AssetEncoding::Zstd);
        assert_eq!(
            release.assets.static_archive.encoding,
            AssetEncoding::Identity
        );
    }

    #[test]
    fn rejects_manifest_version_mismatch() {
        let error = parse_manifest(
            &manifest("landscape-webserver-x86_64.zst", "1.2.4", true),
            "1.2.3",
            Architecture::X86_64,
        )
        .unwrap_err();
        assert!(matches!(error, RepositoryError::InvalidRelease(_)));
    }

    #[test]
    fn rejects_relative_asset_escape() {
        let error = parse_manifest(
            &manifest("../private/backend.zst", "1.2.3", true),
            "1.2.3",
            Architecture::X86_64,
        )
        .unwrap_err();
        assert!(matches!(error, RepositoryError::UnsafeUrl(_)));
    }

    #[test]
    fn rejects_prerelease_manifest() {
        let error = parse_manifest(
            &manifest("landscape-webserver-x86_64.zst", "1.2.3-rc.1", true),
            "1.2.3-rc.1",
            Architecture::X86_64,
        )
        .unwrap_err();
        assert!(matches!(error, RepositoryError::InvalidRelease(_)));
    }

    #[test]
    fn rejects_missing_architecture() {
        let error = parse_manifest(
            &manifest("landscape-webserver-x86_64.zst", "1.2.3", false),
            "1.2.3",
            Architecture::Aarch64,
        )
        .unwrap_err();
        assert!(matches!(error, RepositoryError::MissingArchitecture { .. }));
    }

    #[test]
    fn rejects_invalid_sha256() {
        let body = String::from_utf8(manifest("landscape-webserver-x86_64.zst", "1.2.3", true))
            .unwrap()
            .replace(SHA256, "zzzz")
            .into_bytes();
        let error = parse_manifest(&body, "1.2.3", Architecture::X86_64).unwrap_err();
        assert!(matches!(error, RepositoryError::InvalidRelease(_)));
    }

    #[test]
    fn validates_repository_protocol() {
        assert!(
            RepositoryDescriptor {
                protocol_version: 1
            }
            .validate()
            .is_ok()
        );
        assert!(matches!(
            protocol_error(
                RepositoryDescriptor {
                    protocol_version: 2
                }
                .validate()
                .unwrap_err(),
                "repository.json"
            ),
            RepositoryError::UnsupportedProtocol(2)
        ));
    }

    #[test]
    fn parses_stable_channel_version() {
        let channel: StableChannel =
            serde_json::from_slice(br#"{"protocol_version":1,"version":"1.2.3"}"#).unwrap();
        assert_eq!(
            channel.parsed_version().unwrap(),
            Version::parse("1.2.3").unwrap()
        );
    }
}
