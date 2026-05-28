//! Artifact download abstractions — traits, config, errors.

mod error;
mod trait_def;

pub use error::DownloadError;
pub use trait_def::{ArtifactDownloader, DownloadConfig, DownloadProgress, NoopProgress};
