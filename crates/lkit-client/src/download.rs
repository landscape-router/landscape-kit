//! HTTP-based artifact downloader using reqwest.

use std::path::Path;

use async_trait::async_trait;
use reqwest::Client;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use lkit_core::{ArtifactDownloader, DownloadConfig, DownloadError, DownloadProgress};

/// HTTP downloader for release artifacts.
pub struct HttpDownloader {
    client: Client,
}

impl HttpDownloader {
    /// Create a new downloader with the given reqwest client.
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    /// Create a downloader with default client settings.
    pub fn with_defaults() -> Result<Self, reqwest::Error> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .build()?;
        Ok(Self::new(client))
    }
}

#[async_trait]
impl ArtifactDownloader for HttpDownloader {
    async fn download(
        &self,
        url: &str,
        dest: &Path,
        _config: &DownloadConfig,
        progress: Option<&dyn DownloadProgress>,
    ) -> Result<(), DownloadError> {
        let resp = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| DownloadError::Network(e.to_string()))?;

        let resp = resp
            .error_for_status()
            .map_err(|e| DownloadError::Network(e.to_string()))?;

        let total = resp.content_length().unwrap_or(0);
        let file_name = dest
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();

        if let Some(p) = progress {
            p.on_file_start(&file_name, total);
        }

        let mut file = tokio::fs::File::create(dest)
            .await
            .map_err(|e| DownloadError::Io(e.to_string()))?;

        let mut downloaded: u64 = 0;
        let mut stream = resp.bytes_stream();

        use futures::StreamExt;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| DownloadError::Network(e.to_string()))?;
            file.write_all(&chunk)
                .await
                .map_err(|e| DownloadError::Io(e.to_string()))?;
            downloaded += chunk.len() as u64;
            if let Some(p) = progress {
                p.on_file_progress(&file_name, downloaded);
            }
        }

        file.flush()
            .await
            .map_err(|e| DownloadError::Io(e.to_string()))?;

        if total > 0 && downloaded < total {
            return Err(DownloadError::Incomplete { downloaded, total });
        }

        if let Some(p) = progress {
            p.on_file_complete(&file_name);
        }

        Ok(())
    }
}

/// Compute SHA-256 hex digest of a file using streaming reads.
pub async fn sha256_file(path: &Path) -> Result<String, std::io::Error> {
    use tokio::io::AsyncReadExt;

    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];

    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }

    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_downloader_creation() -> Result<(), Box<dyn std::error::Error>> {
        let _downloader = HttpDownloader::with_defaults()?;
        Ok(())
    }

    #[tokio::test]
    async fn sha256_file_computes_correct_hash() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let file_path = dir.path().join("test.bin");
        tokio::fs::write(&file_path, b"hello world").await?;

        let hash = sha256_file(&file_path).await?;
        assert_eq!(
            hash,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
        Ok(())
    }
}
