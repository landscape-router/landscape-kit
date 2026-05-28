//! Landscape API client.

use async_trait::async_trait;

use lkit_core::{CoreError, LkitClient, ServiceStatus};

/// HTTP-based client for the Landscape API.
#[derive(Debug)]
pub struct LandscapeClient {
    base_url: String,
}

impl LandscapeClient {
    /// Create a new client pointing at the given Landscape API base URL.
    pub fn new(base_url: String) -> Self {
        Self { base_url }
    }
}

#[async_trait]
impl LkitClient for LandscapeClient {
    async fn get_status(&self) -> Result<ServiceStatus, CoreError> {
        Err(CoreError::Internal("not implemented".into()))
    }

    async fn health_check(&self) -> Result<bool, CoreError> {
        Err(CoreError::Internal("not implemented".into()))
    }
}
