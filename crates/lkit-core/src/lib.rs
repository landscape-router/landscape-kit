//! lkit-core: shared models, configuration types, error types, and cross-layer traits.

mod error;
mod models;
mod paths;
mod traits;

pub use error::CoreError;
pub use models::ServiceStatus;
pub use paths::{LandscapePaths, ManagerPaths};
pub use traits::LkitClient;
