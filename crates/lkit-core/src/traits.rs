//! Cross-layer trait definitions for dependency injection.

use async_trait::async_trait;

use crate::error::CoreError;
use crate::models::ServiceState;

/// Abstraction over Landscape API and local system queries.
///
/// Defined in lkit-core, implemented in lkit-client, injected into lkit-app.
/// Enables test mocking without depending on a concrete client.
#[async_trait]
pub trait LkitClient: Send + Sync {
    /// Retrieve the currently installed Landscape version via API.
    async fn get_version(&self) -> Result<String, CoreError>;

    /// Check if the Landscape API is healthy.
    async fn health_check(&self) -> Result<bool, CoreError>;

    /// Export the running configuration as landscape_init.toml content.
    async fn export_config(&self) -> Result<String, CoreError>;
}

/// Abstraction over systemd service management.
///
/// Defined in lkit-core, implemented in lkit-client as shell calls.
/// Enables mocking in tests and future D-Bus backend swap.
#[async_trait]
pub trait ServiceManager: Send + Sync {
    /// Query systemd for the service's current state.
    async fn status(&self) -> Result<ServiceState, CoreError>;
    /// Start the service via systemctl.
    async fn start(&self) -> Result<(), CoreError>;
    /// Stop the service via systemctl.
    async fn stop(&self) -> Result<(), CoreError>;
    /// Restart the service via systemctl.
    async fn restart(&self) -> Result<(), CoreError>;
}

/// Abstraction over log file reading.
///
/// Defined in lkit-core, implemented in lkit-client as file I/O.
#[async_trait]
pub trait LogReader: Send + Sync {
    /// Read the most recent `lines` lines from the log directory.
    async fn recent_lines(&self, lines: usize) -> Result<Vec<String>, CoreError>;
}
