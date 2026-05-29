//! S3-compatible mirror target (AWS S3, Cloudflare R2, MinIO).

use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;
use rusty_s3::{Bucket, Credentials, S3Action, UrlStyle};

use super::MirrorTarget;
use crate::error::MirrorError;

/// Mirror target backed by S3-compatible object storage.
pub struct S3Target {
    bucket: Bucket,
    credentials: Credentials,
    client: Client,
    prefix: String,
}

impl S3Target {
    /// Create a new S3 target.
    ///
    /// - `endpoint`: S3 endpoint URL (e.g. "https://account.r2.cloudflarestorage.com")
    /// - `bucket_name`: Bucket name
    /// - `access_key` / `secret_key`: Credentials
    /// - `prefix`: Optional key prefix within the bucket
    pub fn new(
        endpoint: &str,
        bucket_name: &str,
        access_key: &str,
        secret_key: &str,
        prefix: &str,
    ) -> Result<Self, MirrorError> {
        let url = endpoint
            .parse()
            .map_err(|e| MirrorError::TargetError(format!("invalid endpoint: {e}")))?;

        let bucket = Bucket::new(url, UrlStyle::Path, bucket_name.to_string(), "auto")
            .map_err(|e| MirrorError::TargetError(format!("invalid bucket config: {e}")))?;

        let credentials = Credentials::new(access_key, secret_key);
        let client = Client::builder()
            .timeout(Duration::from_secs(300))
            .build()
            .map_err(|e| MirrorError::TargetError(format!("failed to create HTTP client: {e}")))?;

        Ok(Self {
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
}

#[async_trait]
impl MirrorTarget for S3Target {
    async fn upload(&self, key: &str, data: &[u8]) -> Result<(), MirrorError> {
        let full_key = self.full_key(key);
        let action = self.bucket.put_object(Some(&self.credentials), &full_key);
        let url = action.sign(Duration::from_secs(3600));

        let resp = self
            .client
            .put(url)
            .body(data.to_vec())
            .send()
            .await
            .map_err(|e| MirrorError::UploadFailed(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(MirrorError::UploadFailed(format!(
                "S3 PUT returned {}",
                resp.status()
            )));
        }

        Ok(())
    }

    async fn exists(&self, key: &str) -> Result<bool, MirrorError> {
        let full_key = self.full_key(key);
        let action = self.bucket.head_object(Some(&self.credentials), &full_key);
        let url = action.sign(Duration::from_secs(3600));

        let resp = self
            .client
            .head(url)
            .send()
            .await
            .map_err(|e| MirrorError::TargetError(e.to_string()))?;

        Ok(resp.status().is_success())
    }

    async fn read(&self, key: &str) -> Result<Vec<u8>, MirrorError> {
        let full_key = self.full_key(key);
        let action = self.bucket.get_object(Some(&self.credentials), &full_key);
        let url = action.sign(Duration::from_secs(3600));

        let resp = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| MirrorError::TargetError(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(MirrorError::TargetError(format!(
                "S3 GET returned {}",
                resp.status()
            )));
        }

        resp.bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| MirrorError::TargetError(e.to_string()))
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>, MirrorError> {
        let full_prefix = self.full_key(prefix);
        let mut all_keys = Vec::new();
        let mut continuation_token: Option<String> = None;

        loop {
            let mut action = self.bucket.list_objects_v2(Some(&self.credentials));
            action.with_prefix(&full_prefix);
            action.with_max_keys(1000);

            if let Some(ref token) = continuation_token {
                action.with_continuation_token(token);
            }

            let url = action.sign(Duration::from_secs(3600));

            let resp = self
                .client
                .get(url)
                .send()
                .await
                .map_err(|e| MirrorError::TargetError(e.to_string()))?;

            if !resp.status().is_success() {
                return Err(MirrorError::TargetError(format!(
                    "S3 LIST returned {}",
                    resp.status()
                )));
            }

            let body = resp
                .bytes()
                .await
                .map_err(|e| MirrorError::TargetError(e.to_string()))?;

            let list_resp =
                rusty_s3::actions::ListObjectsV2::parse_response(&body).map_err(|e| {
                    MirrorError::TargetError(format!("failed to parse S3 response: {e}"))
                })?;

            for obj in &list_resp.contents {
                all_keys.push(obj.key.clone());
            }

            if list_resp.next_continuation_token.is_none() {
                break;
            }
            continuation_token = list_resp.next_continuation_token;
        }

        Ok(all_keys)
    }

    async fn delete(&self, key: &str) -> Result<(), MirrorError> {
        let full_key = self.full_key(key);
        let action = self
            .bucket
            .delete_object(Some(&self.credentials), &full_key);
        let url = action.sign(Duration::from_secs(3600));

        let resp = self
            .client
            .delete(url)
            .send()
            .await
            .map_err(|e| MirrorError::TargetError(e.to_string()))?;

        if !resp.status().is_success() && resp.status().as_u16() != 404 {
            return Err(MirrorError::TargetError(format!(
                "S3 DELETE returned {}",
                resp.status()
            )));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s3_target_new_parses_params() -> Result<(), Box<dyn std::error::Error>> {
        let target = S3Target::new(
            "https://test.r2.cloudflarestorage.com",
            "my-bucket",
            "access-key",
            "secret-key",
            "landscape",
        )?;
        assert_eq!(target.prefix, "landscape");
        Ok(())
    }

    #[test]
    fn full_key_with_prefix() -> Result<(), Box<dyn std::error::Error>> {
        let target = S3Target::new(
            "https://test.r2.cloudflarestorage.com",
            "bucket",
            "ak",
            "sk",
            "landscape",
        )?;
        assert_eq!(
            target.full_key("v1.0/manifest.json"),
            "landscape/v1.0/manifest.json"
        );
        Ok(())
    }

    #[test]
    fn full_key_without_prefix() -> Result<(), Box<dyn std::error::Error>> {
        let target = S3Target::new(
            "https://test.r2.cloudflarestorage.com",
            "bucket",
            "ak",
            "sk",
            "",
        )?;
        assert_eq!(target.full_key("v1.0/manifest.json"), "v1.0/manifest.json");
        Ok(())
    }
}
