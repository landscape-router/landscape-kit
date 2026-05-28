//! Cross-layer trait definitions for dependency injection.

use async_trait::async_trait;

use crate::error::CoreError;
use crate::models::ServiceStatus;

/// Abstraction over Landscape API and local system queries.
///
/// Defined in lkit-core, implemented in lkit-client, injected into lkit-app.
/// Enables test mocking without depending on a concrete client.
#[async_trait]
pub trait LkitClient: Send + Sync {
    /// Retrieve current Landscape service status.
    async fn get_status(&self) -> Result<ServiceStatus, CoreError>;

    /// Check if the Landscape API is healthy.
    async fn health_check(&self) -> Result<bool, CoreError>;
}
