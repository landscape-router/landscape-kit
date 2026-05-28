//! lkit-app: use case layer — business logic for install, backup, upgrade, status, diagnose, config.

mod error;

use std::sync::Arc;

use lkit_core::{LandscapePaths, LkitClient, ManagerPaths};

pub use error::AppError;

/// Central application state, assembled in lkit-cli main() and passed to use cases.
pub struct AppState {
    /// Landscape API client (trait object for testability).
    pub client: Arc<dyn LkitClient>,
    /// Discovered Landscape installation paths.
    pub landscape_paths: LandscapePaths,
    /// Manager working directory paths.
    pub manager_paths: ManagerPaths,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("landscape_paths", &self.landscape_paths)
            .field("manager_paths", &self.manager_paths)
            .field("client", &"<dyn LkitClient>")
            .finish()
    }
}
