//! Cross-layer data models.

use serde::{Deserialize, Serialize};

/// Current status of the Landscape service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceStatus {
    /// Landscape version, if detected.
    pub landscape_version: Option<String>,
    /// Whether the systemd unit is active (running).
    pub systemd_active: bool,
    /// Whether the systemd unit is enabled (auto-start).
    pub systemd_enabled: bool,
    /// Whether the Landscape API is reachable.
    pub api_reachable: bool,
}
