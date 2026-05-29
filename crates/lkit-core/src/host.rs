//! Host installer trait for file system and systemd operations.
//!
//! Defined in lkit-core (consumer), implemented in lkit-client (producer).

use std::path::Path;

use async_trait::async_trait;

use crate::error::CoreError;

/// Abstraction over host-level installation operations.
///
/// Handles file writing, directory creation, permission management,
/// and systemd service lifecycle. Defined here (consumer side),
/// implemented in lkit-client as [`SystemInstaller`](../lkit_client/struct.SystemInstaller.html).
#[async_trait]
pub trait HostInstaller: Send + Sync {
    /// Create a directory and all necessary parent directories.
    async fn create_dir_all(&self, path: &Path) -> Result<(), CoreError>;

    /// Write binary content to a file, creating it if it doesn't exist.
    async fn write_file(&self, path: &Path, contents: &[u8]) -> Result<(), CoreError>;

    /// Set Unix file permissions (e.g., `0o600`).
    async fn set_permissions(&self, path: &Path, mode: u32) -> Result<(), CoreError>;

    /// Execute `systemctl daemon-reload`.
    async fn daemon_reload(&self) -> Result<(), CoreError>;

    /// Execute `systemctl enable <unit>`.
    async fn enable_service(&self, unit: &str) -> Result<(), CoreError>;

    /// Execute `systemctl start <unit>`.
    async fn start_service(&self, unit: &str) -> Result<(), CoreError>;
}
