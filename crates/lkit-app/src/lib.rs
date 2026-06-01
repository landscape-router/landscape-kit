//! lkit-app: use case layer — business logic for install, backup, upgrade, status, diagnose, config.

pub mod backup;
pub mod diagnose;
pub mod install;
pub mod logs;
pub mod service;
pub mod source;
pub mod status;

mod error;

use std::sync::Arc;

use lkit_core::{LandscapePaths, LkitClient, LogReader, ManagerPaths, ServiceManager};

pub use error::AppError;

/// Central application state, assembled in lkit-cli main() and passed to use cases.
pub struct AppState {
    /// Landscape API client (trait object for testability).
    pub client: Arc<dyn LkitClient>,
    /// Systemd service manager (trait object).
    pub service_manager: Arc<dyn ServiceManager>,
    /// Log file reader (trait object).
    pub log_reader: Arc<dyn LogReader>,
    /// Discovered Landscape installation paths.
    pub landscape_paths: LandscapePaths,
    /// Manager working directory paths.
    pub manager_paths: ManagerPaths,
}

impl AppState {
    /// Create a new AppState with all dependencies injected.
    pub fn new(
        client: Arc<dyn LkitClient>,
        service_manager: Arc<dyn ServiceManager>,
        log_reader: Arc<dyn LogReader>,
        landscape_paths: LandscapePaths,
        manager_paths: ManagerPaths,
    ) -> Self {
        Self {
            client,
            service_manager,
            log_reader,
            landscape_paths,
            manager_paths,
        }
    }
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("landscape_paths", &self.landscape_paths)
            .field("manager_paths", &self.manager_paths)
            .field("client", &"<dyn LkitClient>")
            .field("service_manager", &"<dyn ServiceManager>")
            .field("log_reader", &"<dyn LogReader>")
            .finish()
    }
}
