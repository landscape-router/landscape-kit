//! Landscape API HTTP client.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;

use lkit_core::{CoreError, LkitClient, ServiceStatus};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// HTTP-based client for the Landscape API.
pub struct LandscapeClient {
    base_url: String,
    http: Client,
}

impl LandscapeClient {
    /// Create a new client pointing at the given Landscape API base URL.
    ///
    /// `base_url` should include scheme, e.g. "http://127.0.0.1:8080".
    pub fn new(base_url: String) -> Result<Self, CoreError> {
        let http = Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| CoreError::Internal(format!("failed to create HTTP client: {e}")))?;
        Ok(Self { base_url, http })
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

        let resp = resp
            .error_for_status()
            .map_err(|e| CoreError::Internal(format!("API returned error: {e}")))?;

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
    fn url_construction_basic() -> Result<(), Box<dyn std::error::Error>> {
        let client = LandscapeClient::new("http://127.0.0.1:8080".into())?;
        assert_eq!(
            client.url("/api/v1/status"),
            "http://127.0.0.1:8080/api/v1/status"
        );
        Ok(())
    }

    #[test]
    fn url_construction_trailing_slash() -> Result<(), Box<dyn std::error::Error>> {
        let client = LandscapeClient::new("http://127.0.0.1:8080/".into())?;
        assert_eq!(
            client.url("/api/v1/health"),
            "http://127.0.0.1:8080/api/v1/health"
        );
        Ok(())
    }
}
