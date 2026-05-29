//! S3-compatible release source (AWS S3, Cloudflare R2, MinIO).
//!
//! Reads release artifacts from an S3 bucket, enabling mirror-to-mirror
//! workflows from private buckets.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use rusty_s3::{Bucket, Credentials, S3Action, UrlStyle};

use lkit_core::{ReleaseManifest, ReleaseSource, SourceError};

/// A release source backed by S3-compatible object storage.
///
/// Uses the same bucket layout as [`S3Target`](lkit_mirror::target::s3::S3Target):
/// `<prefix>/latest`, `<prefix>/<tag>/release-manifest.json`, `<prefix>/<tag>/<artifact>`.
pub struct S3Source {
    name: String,
    bucket: Bucket,
    credentials: Credentials,
    client: Client,
    /// Key prefix within the bucket (e.g. "landscape"). No trailing slash.
    prefix: String,
}

impl S3Source {
    /// Create a new S3 source.
    ///
    /// - `endpoint`: S3 endpoint URL (e.g. "https://account.r2.cloudflarestorage.com")
    /// - `bucket_name`: Bucket name
    /// - `access_key` / `secret_key`: Credentials
    /// - `prefix`: Key prefix within the bucket (e.g. "landscape")
    pub fn new(
        name: impl Into<String>,
        endpoint: &str,
        bucket_name: &str,
        access_key: &str,
        secret_key: &str,
        prefix: &str,
    ) -> Result<Self, SourceError> {
        let url = endpoint
            .parse()
            .map_err(|e| SourceError::Config(format!("invalid S3 endpoint: {e}")))?;

        let bucket = Bucket::new(url, UrlStyle::Path, bucket_name.to_string(), "auto")
            .map_err(|e| SourceError::Config(format!("invalid S3 bucket config: {e}")))?;

        let credentials = Credentials::new(access_key, secret_key);
        let client = Client::builder()
            .timeout(Duration::from_secs(300))
            .build()
            .map_err(|e| SourceError::Config(format!("failed to create HTTP client: {e}")))?;

        Ok(Self {
            name: name.into(),
            bucket,
            credentials,
            client,
            prefix: prefix.trim_matches('/').to_string(),
        })
    }

    fn full_key(&self, key: &str) -> String {
        if self.prefix.is_empty() {
            key.to_string()
        } else {
            format!("{}/{}", self.prefix, key.trim_start_matches('/'))
        }
    }

    /// Sign a GET URL for the given key.
    fn sign_get(&self, key: &str) -> String {
        let full = self.full_key(key);
        let action = self.bucket.get_object(Some(&self.credentials), &full);
        action.sign(Duration::from_secs(3600)).to_string()
    }
}

#[async_trait]
impl ReleaseSource for S3Source {
    fn name(&self) -> &str {
        &self.name
    }

    async fn latest_tag(&self) -> Result<String, SourceError> {
        let url = self.sign_get("latest");
        let resp = self
            .client
            .get(url)
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
        use std::collections::BTreeSet;

        let mut versions = BTreeSet::new();
        let full_prefix = if self.prefix.is_empty() {
            String::new()
        } else {
            format!("{}/", self.prefix)
        };

        let mut continuation_token: Option<String> = None;

        loop {
            let mut action = self.bucket.list_objects_v2(Some(&self.credentials));
            if !full_prefix.is_empty() {
                action.with_prefix(&full_prefix);
            }
            action.with_max_keys(1000);

            if let Some(ref token) = continuation_token {
                action.with_continuation_token(token);
            }

            let url = action.sign(Duration::from_secs(3600)).to_string();

            let resp = self
                .client
                .get(url)
                .send()
                .await
                .map_err(|e| SourceError::Network(e.to_string()))?;

            if !resp.status().is_success() {
                return Err(SourceError::Network(format!(
                    "S3 LIST returned {}",
                    resp.status()
                )));
            }

            let body = resp
                .bytes()
                .await
                .map_err(|e| SourceError::Network(e.to_string()))?;

            let body_str = std::str::from_utf8(&body)
                .map_err(|e| SourceError::Network(format!("invalid UTF-8 in S3 response: {e}")))?;

            let list_resp = rusty_s3::actions::ListObjectsV2::parse_response(body_str)
                .map_err(|e| SourceError::Network(format!("failed to parse S3 response: {e}")))?;

            // Extract version directories from object keys.
            // Keys look like: "landscape/v0.19.2/manifest.json"
            for obj in &list_resp.contents {
                let key = if obj.key.starts_with(&full_prefix) {
                    &obj.key[full_prefix.len()..]
                } else {
                    &obj.key
                };
                if let Some(slash_pos) = key.find('/') {
                    let dir = &key[..slash_pos];
                    if dir.starts_with('v') && !dir.is_empty() {
                        versions.insert(dir.to_string());
                    }
                }
            }

            if list_resp.next_continuation_token.is_none() {
                break;
            }
            continuation_token = list_resp.next_continuation_token;
        }

        let mut sorted: Vec<String> = versions.into_iter().collect();
        // Sort newest-first to match GitHub API convention.
        sorted.sort_by(|a, b| compare_semver(b, a));
        Ok(sorted)
    }

    async fn get_artifacts(&self, tag: &str) -> Result<ReleaseManifest, SourceError> {
        let manifest_key = format!("{}/release-manifest.json", tag);
        let url = self.sign_get(&manifest_key);

        let resp = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| SourceError::Network(e.to_string()))?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(SourceError::VersionNotFound {
                tag: tag.to_string(),
            });
        }

        let resp = resp
            .error_for_status()
            .map_err(|e| SourceError::Network(e.to_string()))?;

        let text = resp
            .text()
            .await
            .map_err(|e| SourceError::Network(e.to_string()))?;

        serde_json::from_str(&text).map_err(|e| SourceError::InvalidManifest(e.to_string()))
    }

    fn artifact_url(&self, tag: &str, name: &str) -> String {
        let key = format!("{}/{}", tag, name);
        self.sign_get(&key)
    }

    async fn probe(&self, tag: &str) -> Result<Duration, SourceError> {
        let manifest_key = format!("{}/release-manifest.json", tag);
        let url = self.sign_get(&manifest_key);
        let start = std::time::Instant::now();

        let resp = self
            .client
            .head(url)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| SourceError::Network(e.to_string()))?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(SourceError::VersionNotFound {
                tag: tag.to_string(),
            });
        }

        resp.error_for_status()
            .map_err(|e| SourceError::Network(e.to_string()))?;

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
    fn s3_source_full_key_with_prefix() -> Result<(), Box<dyn std::error::Error>> {
        let src = S3Source::new(
            "test",
            "https://test.r2.cloudflarestorage.com",
            "bucket",
            "ak",
            "sk",
            "landscape",
        )?;
        assert_eq!(
            src.full_key("v1.0/manifest.json"),
            "landscape/v1.0/manifest.json"
        );
        Ok(())
    }

    #[test]
    fn s3_source_full_key_without_prefix() -> Result<(), Box<dyn std::error::Error>> {
        let src = S3Source::new(
            "test",
            "https://test.r2.cloudflarestorage.com",
            "bucket",
            "ak",
            "sk",
            "",
        )?;
        assert_eq!(src.full_key("v1.0/manifest.json"), "v1.0/manifest.json");
        Ok(())
    }

    #[test]
    fn s3_source_name() -> Result<(), Box<dyn std::error::Error>> {
        let src = S3Source::new("my-s3", "https://s3.example.com", "bucket", "ak", "sk", "")?;
        assert_eq!(src.name(), "my-s3");
        Ok(())
    }
}
