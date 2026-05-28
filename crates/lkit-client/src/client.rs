//! Landscape API HTTP client.

use async_trait::async_trait;
use reqwest::Client;

use lkit_core::{CoreError, LkitClient, ServiceStatus};

/// HTTP-based client for the Landscape API.
pub struct LandscapeClient {
    base_url: String,
    http: Client,
}

impl LandscapeClient {
    /// Create a new client pointing at the given Landscape API base URL.
    ///
    /// `base_url` should include scheme, e.g. "http://127.0.0.1:8080".
    pub fn new(base_url: String) -> Self {
        Self {
            base_url,
            http: Client::new(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), path)
    }
}

#[async_trait]
impl LkitClient for LandscapeClient {
    async fn get_status(&self) -> Result<ServiceStatus, CoreError> {
        let url = self.url("/api/v1/status");
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| CoreError::Internal(format!("API request failed: {e}")))?;

        let status = resp
            .json::<ServiceStatus>()
            .await
            .map_err(|e| CoreError::Internal(format!("failed to parse API response: {e}")))?;

        Ok(status)
    }

    async fn health_check(&self) -> Result<bool, CoreError> {
        let url = self.url("/api/v1/health");
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| CoreError::Internal(format!("API request failed: {e}")))?;

        Ok(resp.status().is_success())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_construction_basic() {
        let client = LandscapeClient::new("http://127.0.0.1:8080".into());
        assert_eq!(
            client.url("/api/v1/status"),
            "http://127.0.0.1:8080/api/v1/status"
        );
    }

    #[test]
    fn url_construction_trailing_slash() {
        let client = LandscapeClient::new("http://127.0.0.1:8080/".into());
        assert_eq!(
            client.url("/api/v1/health"),
            "http://127.0.0.1:8080/api/v1/health"
        );
    }
}
