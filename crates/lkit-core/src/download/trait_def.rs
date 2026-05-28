//! ArtifactDownloader trait — abstraction over file download implementations.

use std::path::Path;

use async_trait::async_trait;

use super::error::DownloadError;

/// Configuration for download behavior.
#[derive(Debug, Clone)]
pub struct DownloadConfig {
    /// Number of files to download concurrently.
    pub concurrent_files: usize,
    /// Number of chunks per file (1 = no chunking).
    pub chunks_per_file: usize,
}

impl Default for DownloadConfig {
    fn default() -> Self {
        Self {
            concurrent_files: 4,
            chunks_per_file: 1,
        }
    }
}

/// Progress callback trait for download operations.
pub trait DownloadProgress: Send + Sync {
    /// Called when a file download starts.
    fn on_file_start(&self, name: &str, total_bytes: u64);
    /// Called periodically with download progress.
    fn on_file_progress(&self, name: &str, bytes_downloaded: u64);
    /// Called when a file download completes.
    fn on_file_complete(&self, name: &str);
}

/// Abstraction over artifact downloading.
///
/// Defined in lkit-core, implemented in lkit-client as HttpDownloader.
/// URL correctness is guaranteed by the caller (SourceResolver).
#[async_trait]
pub trait ArtifactDownloader: Send + Sync {
    /// Download a single file from `url` to `dest`.
    ///
    /// `config` controls parallelism. `progress` is optional callback.
    async fn download(
        &self,
        url: &str,
        dest: &Path,
        config: &DownloadConfig,
        progress: Option<&dyn DownloadProgress>,
    ) -> Result<(), DownloadError>;
}

/// No-op progress implementation for testing and default use.
pub struct NoopProgress;

impl DownloadProgress for NoopProgress {
    fn on_file_start(&self, _name: &str, _total_bytes: u64) {}
    fn on_file_progress(&self, _name: &str, _bytes_downloaded: u64) {}
    fn on_file_complete(&self, _name: &str) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_config_default_values() -> Result<(), Box<dyn std::error::Error>> {
        let config = DownloadConfig::default();
        assert_eq!(config.concurrent_files, 4);
        assert_eq!(config.chunks_per_file, 1);
        Ok(())
    }

    #[test]
    fn noop_progress_does_not_panic() -> Result<(), Box<dyn std::error::Error>> {
        let progress = NoopProgress;
        progress.on_file_start("test.bin", 1024);
        progress.on_file_progress("test.bin", 512);
        progress.on_file_complete("test.bin");
        Ok(())
    }
}
