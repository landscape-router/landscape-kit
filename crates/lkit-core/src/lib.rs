//! lkit-core: shared models, configuration types, error types, and cross-layer traits.

pub mod download;
mod error;
mod models;
mod paths;
pub mod source;
mod traits;

pub use download::{
    ArtifactDownloader, DownloadConfig, DownloadError, DownloadProgress, NoopProgress,
};
pub use error::CoreError;
pub use models::{DiagnosticCheck, DiagnosticResult, ServiceState, ServiceStatus};
pub use paths::{LandscapePaths, ManagerPaths};
pub use source::{
    Artifact, ReleaseManifest, ReleaseSource, SourceConfig, SourceError, SourceType, default_source,
};
pub use traits::{LkitClient, LogReader, ServiceManager};
