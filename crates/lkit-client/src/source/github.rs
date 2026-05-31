//! GitHub Releases source — fetches release info via GitHub REST API.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue, USER_AGENT};
use serde::Deserialize;

use lkit_core::{Artifact, ReleaseManifest, ReleaseSource, SourceError};

/// A release source backed by GitHub Releases API.
pub struct GithubSource {
    name: String,
    owner: String,
    repo: String,
    client: Client,
    token: Option<String>,
}

/// GitHub release JSON (subset).
#[derive(Debug, Deserialize)]
struct GhRelease {
    tag_name: String,
    assets: Vec<GhAsset>,
}

/// GitHub release asset JSON (subset).
#[derive(Debug, Deserialize)]
#[expect(dead_code)]
struct GhAsset {
    name: String,
    size: u64,
    browser_download_url: String,
}

impl GithubSource {
    /// Create a new GitHub source.
    ///
    /// `repo` is "owner/repo" format. Reads `GITHUB_TOKEN` env var if set.
    pub fn new(name: impl Into<String>, repo: &str, client: Client) -> Result<Self, SourceError> {
        let parts: Vec<&str> = repo.splitn(2, '/').collect();
        if parts.len() != 2 {
            return Err(SourceError::Config(format!(
                "invalid repo format: {repo} (expected owner/repo)"
            )));
        }
        let token = std::env::var("GITHUB_TOKEN").ok();
        Ok(Self {
            name: name.into(),
            owner: parts[0].to_string(),
            repo: parts[1].to_string(),
            client,
            token,
        })
    }

    fn api_base(&self) -> String {
        format!("https://api.github.com/repos/{}/{}", self.owner, self.repo)
    }

    fn auth_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static("lkit"));
        if let Some(ref token) = self.token
            && let Ok(val) = HeaderValue::from_str(&format!("Bearer {token}"))
        {
            headers.insert(AUTHORIZATION, val);
        }
        headers
    }
}

#[async_trait]
impl ReleaseSource for GithubSource {
    fn name(&self) -> &str {
        &self.name
    }

    async fn latest_tag(&self) -> Result<String, SourceError> {
        let url = format!("{}/releases/latest", self.api_base());
        let resp = self
            .client
            .get(&url)
            .headers(self.auth_headers())
            .send()
            .await
            .map_err(|e| SourceError::Network(e.to_string()))?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(SourceError::VersionNotFound { tag: "latest".into() });
        }

        let resp = resp.error_for_status().map_err(|e| SourceError::Network(e.to_string()))?;

        let release: GhRelease =
            resp.json().await.map_err(|e| SourceError::InvalidManifest(e.to_string()))?;

        Ok(release.tag_name)
    }

    async fn list_versions(&self) -> Result<Vec<String>, SourceError> {
        let mut all_tags = Vec::new();
        let mut url = format!("{}/releases?per_page=100", self.api_base());

        loop {
            let resp = self
                .client
                .get(&url)
                .headers(self.auth_headers())
                .send()
                .await
                .map_err(|e| SourceError::Network(e.to_string()))?;

            let next_url = extract_next_link(resp.headers());
            let resp = resp.error_for_status().map_err(|e| SourceError::Network(e.to_string()))?;

            let releases: Vec<GhRelease> =
                resp.json().await.map_err(|e| SourceError::InvalidManifest(e.to_string()))?;

            let has_releases = !releases.is_empty();
            all_tags.extend(releases.into_iter().map(|r| r.tag_name));

            match next_url {
                Some(next) if has_releases => url = next,
                _ => break,
            }
        }

        Ok(all_tags)
    }

    async fn get_artifacts(&self, tag: &str) -> Result<ReleaseManifest, SourceError> {
        let url = format!("{}/releases/tags/{tag}", self.api_base());
        let resp = self
            .client
            .get(&url)
            .headers(self.auth_headers())
            .send()
            .await
            .map_err(|e| SourceError::Network(e.to_string()))?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(SourceError::VersionNotFound { tag: tag.into() });
        }

        let resp = resp.error_for_status().map_err(|e| SourceError::Network(e.to_string()))?;

        let release: GhRelease =
            resp.json().await.map_err(|e| SourceError::InvalidManifest(e.to_string()))?;

        // GitHub Releases API does not provide per-asset checksums.
        // sha256 is left empty; install flow uses SHASUM256sum.txt
        // (included as an asset) for verification when available.
        let artifacts = release
            .assets
            .into_iter()
            .map(|a| {
                let arch = lkit_core::parse_arch(&a.name)
                    .map(|info| if info.musl { format!("{}-musl", info.arch) } else { info.arch });
                Artifact {
                    name: a.name,
                    sha256: String::new(),
                    size: a.size,
                    arch,
                }
            })
            .collect();

        Ok(ReleaseManifest {
            format_version: 1,
            tag: release.tag_name,
            generated_at: String::new(),
            generated_by: None,
            artifacts,
        })
    }

    fn artifact_url(&self, tag: &str, name: &str) -> String {
        format!("https://github.com/{}/{}/releases/download/{tag}/{name}", self.owner, self.repo,)
    }

    async fn probe(&self, tag: &str) -> Result<Duration, SourceError> {
        let url = format!("{}/releases/tags/{tag}", self.api_base());
        let start = std::time::Instant::now();

        let resp = self
            .client
            .head(&url)
            .headers(self.auth_headers())
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| SourceError::Network(e.to_string()))?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(SourceError::VersionNotFound { tag: tag.into() });
        }

        resp.error_for_status().map_err(|e| SourceError::Network(e.to_string()))?;

        Ok(start.elapsed())
    }
}

/// Extract the next page URL from the GitHub `Link` header.
fn extract_next_link(headers: &reqwest::header::HeaderMap) -> Option<String> {
    let link = headers.get("link")?.to_str().ok()?;
    for part in link.split(',') {
        let part = part.trim();
        if let (Some(url_start), Some(url_end)) = (part.find('<'), part.find('>')) {
            let url = &part[url_start + 1..url_end];
            if part.contains("rel=\"next\"") {
                return Some(url.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_source_parses_repo() -> Result<(), Box<dyn std::error::Error>> {
        let client = Client::new();
        let src = GithubSource::new("test", "owner/repo", client)?;
        assert_eq!(src.owner, "owner");
        assert_eq!(src.repo, "repo");
        Ok(())
    }

    #[test]
    fn github_source_rejects_invalid_repo() -> Result<(), Box<dyn std::error::Error>> {
        let client = Client::new();
        let result = GithubSource::new("test", "invalid-no-slash", client);
        assert!(result.is_err());
        Ok(())
    }

    #[test]
    fn artifact_url_construction() -> Result<(), Box<dyn std::error::Error>> {
        let client = Client::new();
        let src = GithubSource::new("test", "ThisSeanZhang/landscape", client)?;
        let url = src.artifact_url("v0.19.2", "landscape-webserver-x86_64");
        assert_eq!(
            url,
            "https://github.com/ThisSeanZhang/landscape/releases/download/v0.19.2/landscape-webserver-x86_64"
        );
        Ok(())
    }

    #[test]
    fn github_source_name() -> Result<(), Box<dyn std::error::Error>> {
        let client = Client::new();
        let src = GithubSource::new("my-github", "owner/repo", client)?;
        assert_eq!(src.name(), "my-github");
        Ok(())
    }
}
