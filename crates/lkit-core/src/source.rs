//! Release source abstractions — traits, models, errors.

pub mod config;
mod error;
mod manifest;
pub mod name_parser;
mod trait_def;
pub mod version;

pub use config::{SourceConfig, SourceType, default_sources};
pub use error::SourceError;
pub use manifest::{Artifact, ReleaseManifest};
pub use trait_def::ReleaseSource;
pub use version::compare_semver;
