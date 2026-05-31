//! lkit-core: shared models, configuration types, error types, and cross-layer traits.

pub mod download;
mod error;
pub mod host;
pub mod install;
mod models;
mod paths;
pub mod source;
pub mod system_detect;
mod traits;

pub use download::{
    ArtifactDownloader, DownloadConfig, DownloadError, DownloadProgress, NoopProgress,
};
pub use error::CoreError;
pub use host::HostInstaller;
pub use install::{
    InstallConfig, LanSetup, LandscapeServiceConfig, NetworkSetup, SourceSelection, WanMode,
    WanSetup,
};
pub use models::{
    ApiResponse, DiagnosticCheck, DiagnosticResult, ExportInitConfigResponse, ServiceState,
    SystemInfoResponse,
};
pub use paths::{LandscapePaths, ManagerPaths};
pub use source::name_parser::{ArchInfo, parse_arch};
pub use source::{
    Artifact, ReleaseManifest, ReleaseSource, SourceConfig, SourceError, SourceType,
    compare_semver, default_sources,
};
pub use system_detect::{LibcType, SystemTarget, detect};
pub use traits::{LkitClient, LogReader, ServiceManager};
