//! Release source abstractions — traits, models, errors.

pub mod config;
mod error;
mod manifest;
mod trait_def;

pub use config::{default_source, SourceConfig, SourceType};
pub use error::SourceError;
pub use manifest::{Artifact, ReleaseManifest};
pub use trait_def::ReleaseSource;
