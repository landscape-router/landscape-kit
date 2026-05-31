//! Landscape API HTTP client.

use std::time::Duration;

use async_trait::async_trait;
use reqwest::Client;

use lkit_core::{ApiResponse, CoreError, ExportInitConfigResponse, LkitClient, SystemInfoResponse};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// HTTP-based client for the Landscape API.
pub struct LandscapeClient {
    base_url: String,
    http: Client,
    /// Path to the JWT token file, if available.
    api_token_path: Option<std::path::PathBuf>,
}

impl LandscapeClient {
    /// Create a new client pointing at the given Landscape API base URL.
    ///
    /// `base_url` should include scheme, e.g. "http://127.0.0.1:8080".
    pub fn new(
        base_url: String,
        api_token_path: Option<std::path::PathBuf>,
    ) -> Result<Self, CoreError> {
        let http = Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .danger_accept_invalid_certs(true)
            .build()
            .map_err(|e| CoreError::Internal(format!("failed to create HTTP client: {e}")))?;
        Ok(Self { base_url, http, api_token_path })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), path)
    }

    /// Read JWT token from file and return as `Authorization: Bearer` value.
    fn bearer_token(&self) -> Option<String> {
        let path = self.api_token_path.as_ref()?;
        let token = std::fs::read_to_string(path).ok()?;
        let token = token.trim().to_string();
        if token.is_empty() { None } else { Some(format!("Bearer {token}")) }
    }

    /// Build an authenticated GET request builder.
    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        let req = self.http.get(self.url(path));
        match self.bearer_token() {
            Some(token) => req.header("Authorization", token),
            None => req,
        }
    }

    /// Deserialize the standard `ApiResponse<T>` and return `data` on success.
    async fn unwrap_response<T: serde::de::DeserializeOwned>(
        &self,
        resp: reqwest::Response,
    ) -> Result<T, CoreError> {
        let status = resp.status();
        let wrapped: ApiResponse<T> = resp
            .json()
            .await
            .map_err(|e| CoreError::Internal(format!("failed to parse API response: {e}")))?;

        match wrapped.data {
            Some(data) => Ok(data),
            None => {
                let msg = wrapped.message.unwrap_or_else(|| "no error message".into());
                Err(CoreError::Internal(format!("API error ({}): {msg}", status.as_u16())))
            }
        }
    }
}

#[async_trait]
impl LkitClient for LandscapeClient {
    async fn get_version(&self) -> Result<String, CoreError> {
        let resp = self
            .get("/api/v1/system/info")
            .send()
            .await
            .map_err(|e| CoreError::Internal(format!("API request failed: {e}")))?;

        let resp = resp
            .error_for_status()
            .map_err(|e| CoreError::Internal(format!("API returned error: {e}")))?;

        let info: SystemInfoResponse = self.unwrap_response(resp).await?;
        Ok(info.landscape_version)
    }

    async fn health_check(&self) -> Result<bool, CoreError> {
        let resp = self
            .get("/api/v1/system/info")
            .send()
            .await
            .map_err(|e| CoreError::Internal(format!("API request failed: {e}")))?;

        Ok(resp.status().is_success())
    }

    async fn export_config(&self) -> Result<String, CoreError> {
        let resp = self
            .get("/api/v1/system/config/export")
            .send()
            .await
            .map_err(|e| CoreError::Internal(format!("API request failed: {e}")))?;

        let resp = resp
            .error_for_status()
            .map_err(|e| CoreError::Internal(format!("API returned error: {e}")))?;

        let export: ExportInitConfigResponse = self.unwrap_response(resp).await?;
        Ok(export.content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_construction_basic() -> Result<(), Box<dyn std::error::Error>> {
        let client = LandscapeClient::new("http://127.0.0.1:8080".into(), None)?;
        assert_eq!(client.url("/api/v1/system/info"), "http://127.0.0.1:8080/api/v1/system/info");
        Ok(())
    }

    #[test]
    fn url_construction_trailing_slash() -> Result<(), Box<dyn std::error::Error>> {
        let client = LandscapeClient::new("http://127.0.0.1:8080/".into(), None)?;
        assert_eq!(client.url("/api/v1/system/info"), "http://127.0.0.1:8080/api/v1/system/info");
        Ok(())
    }
}
