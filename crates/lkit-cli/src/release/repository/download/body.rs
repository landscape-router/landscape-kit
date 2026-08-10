use std::path::Path;

use futures_util::StreamExt;
use reqwest::Response;
use semver::Version;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use super::super::{Asset, RepositoryError};
use super::{IDLE_TIMEOUT, hex};
use crate::interaction::presentation::DownloadProgress;

pub(super) async fn write_asset_response(
    version: &Version,
    asset: &Asset,
    temp_path: &Path,
    response: Response,
    progress: &mut DownloadProgress,
) -> Result<(), RepositoryError> {
    let mut file = tokio::fs::File::create(temp_path)
        .await
        .map_err(RepositoryError::Io)?;
    let mut hasher = Sha256::new();
    let mut written: u64 = 0;
    let mut stream = response.bytes_stream();
    loop {
        let chunk = tokio::time::timeout(IDLE_TIMEOUT, stream.next())
            .await
            .map_err(|_| {
                RepositoryError::Timeout(
                    "no data received for 30 seconds while downloading asset".into(),
                )
            })?
            .transpose()
            .map_err(RepositoryError::Request)?;
        let Some(chunk) = chunk else { break };
        written = written.saturating_add(chunk.len() as u64);
        if written > asset.size {
            return Err(RepositoryError::AssetSizeMismatch {
                version: version.clone(),
                expected: asset.size,
                actual: written,
            });
        }
        hasher.update(&chunk);
        file.write_all(&chunk).await.map_err(RepositoryError::Io)?;
        progress.set_position(written);
    }
    if written != asset.size {
        return Err(RepositoryError::AssetSizeMismatch {
            version: version.clone(),
            expected: asset.size,
            actual: written,
        });
    }
    let actual = hex(&hasher.finalize());
    if actual != asset.sha256 {
        return Err(RepositoryError::AssetSha256Mismatch {
            version: version.clone(),
            expected: asset.sha256.clone(),
            actual,
        });
    }
    file.sync_all().await.map_err(RepositoryError::Io)?;
    Ok(())
}

pub(super) async fn read_body_limited(
    response: Response,
    limit: u64,
) -> Result<Vec<u8>, RepositoryError> {
    if response
        .content_length()
        .is_some_and(|length| length > limit)
    {
        return Err(RepositoryError::MetadataTooLarge);
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(RepositoryError::Request)?;
        let new_length = body.len().saturating_add(chunk.len());
        if new_length as u64 > limit {
            return Err(RepositoryError::MetadataTooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}
