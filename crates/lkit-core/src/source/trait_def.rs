//! ReleaseSource trait — abstraction over release artifact sources.

use std::time::Duration;

use async_trait::async_trait;

use super::error::SourceError;
use super::manifest::ReleaseManifest;

/// Abstraction over a release source (GitHub, HTTP mirror, local directory).
///
/// Defined in lkit-core, implemented in lkit-client. Enables multi-source
/// concurrent probing and testability via mock implementations.
#[async_trait]
pub trait ReleaseSource: Send + Sync {
    /// Human-readable name for logs and diagnostics.
    fn name(&self) -> &str;

    /// Resolve the latest version tag from this source.
    ///
    /// For GitHub: calls /repos/{owner}/{repo}/releases/latest.
    /// For HTTP mirrors: reads `<base_url>/latest` text file.
    /// For local: reads `<path>/latest` text file.
    async fn latest_tag(&self) -> Result<String, SourceError>;

    /// List available version tags from this source.
    async fn list_versions(&self) -> Result<Vec<String>, SourceError>;

    /// Get the release manifest for a specific tag.
    async fn get_artifacts(&self, tag: &str) -> Result<ReleaseManifest, SourceError>;

    /// Build the download URL for a specific artifact.
    fn artifact_url(&self, tag: &str, name: &str) -> String;

    /// Health-check probe — HEAD request to verify source availability.
    /// Returns the round-trip latency on success.
    async fn probe(&self, tag: &str) -> Result<Duration, SourceError>;
}
