//! Mirror sync — download releases from a source and push to mirror target.

use lkit_core::{ReleaseManifest, ReleaseSource};
use sha2::{Digest, Sha256};
use tracing::{debug, info};

use crate::error::MirrorError;
use crate::target::MirrorTarget;

/// Sync scope — which releases to sync.
#[derive(Debug, Clone)]
pub enum SyncScope {
    /// Only sync the latest release (default).
    Latest,
    /// Sync a specific tag.
    Tag(String),
    /// Sync the N most recent releases.
    LatestN(u32),
    /// Sync all releases after this tag (exclusive).
    Since(String),
    /// Sync all historical releases.
    All,
}

/// Sync configuration.
#[derive(Debug, Clone)]
pub struct SyncConfig {
    /// Product prefix in the mirror (e.g. "landscape").
    pub prefix: String,
    /// Sync scope.
    pub scope: SyncScope,
    /// Force re-sync even if version already exists.
    pub force: bool,
}

/// Sync result summary.
#[derive(Debug)]
pub struct SyncResult {
    /// Versions that were synced.
    pub synced: Vec<String>,
    /// Versions that were skipped (already exist).
    pub skipped: Vec<String>,
    /// Versions that failed with error messages.
    pub failed: Vec<(String, String)>,
}

/// Run sync from a release source to a mirror target.
pub async fn run_sync(
    config: &SyncConfig,
    source: &dyn ReleaseSource,
    target: &dyn MirrorTarget,
) -> Result<SyncResult, MirrorError> {
    let tags = resolve_tags(source, &config.scope).await?;

    let mut synced = Vec::new();
    let mut skipped = Vec::new();
    let mut failed = Vec::new();

    for tag in &tags {
        let version_prefix = format!("{}/{}", config.prefix, tag);

        if !config.force {
            let manifest_key = format!("{}/release-manifest.json", version_prefix);
            if target.exists(&manifest_key).await.unwrap_or(false) {
                debug!("skipping {tag} — already exists");
                skipped.push(tag.clone());
                continue;
            }
        }

        info!("syncing {tag}...");

        match sync_version(source, target, &config.prefix, tag).await {
            Ok(()) => {
                info!("synced {tag}");
                synced.push(tag.clone());
            }
            Err(e) => {
                let msg = e.to_string();
                tracing::warn!("failed to sync {tag}: {msg}");
                failed.push((tag.clone(), msg));
            }
        }
    }

    // Update latest pointer if we synced anything
    if let Some(latest) = synced.last() {
        let latest_key = format!("{}/latest", config.prefix);
        target
            .upload(&latest_key, latest.as_bytes())
            .await
            .map_err(|e| MirrorError::UploadFailed(format!("failed to update latest: {e}")))?;
    }

    Ok(SyncResult {
        synced,
        skipped,
        failed,
    })
}

/// Resolve tags based on sync scope.
async fn resolve_tags(
    source: &dyn ReleaseSource,
    scope: &SyncScope,
) -> Result<Vec<String>, MirrorError> {
    match scope {
        SyncScope::Tag(tag) => Ok(vec![tag.clone()]),
        SyncScope::Latest => {
            let tag = source.latest_tag().await.map_err(MirrorError::Source)?;
            Ok(vec![tag])
        }
        SyncScope::LatestN(n) => {
            let versions = source.list_versions().await.map_err(MirrorError::Source)?;
            Ok(versions.into_iter().take(*n as usize).collect())
        }
        SyncScope::Since(since_tag) => {
            let versions = source.list_versions().await.map_err(MirrorError::Source)?;
            let pos = versions.iter().position(|v| v == since_tag);
            match pos {
                Some(i) => Ok(versions.into_iter().take(i).collect()),
                None => Err(MirrorError::GitHubApi(format!(
                    "tag {since_tag} not found in release history"
                ))),
            }
        }
        SyncScope::All => {
            let versions = source.list_versions().await.map_err(MirrorError::Source)?;
            Ok(versions)
        }
    }
}

/// Sync a single version from source to target.
async fn sync_version(
    source: &dyn ReleaseSource,
    target: &dyn MirrorTarget,
    prefix: &str,
    tag: &str,
) -> Result<(), MirrorError> {
    let manifest = source
        .get_artifacts(tag)
        .await
        .map_err(|e| MirrorError::GitHubApi(format!("failed to get artifacts for {tag}: {e}")))?;

    let version_prefix = format!("{prefix}/{tag}");

    let manifest_for_storage = ReleaseManifest {
        format_version: 1,
        tag: tag.to_string(),
        generated_at: now_iso8601(),
        generated_by: Some(format!("lkit {}", env!("CARGO_PKG_VERSION"))),
        artifacts: manifest.artifacts.clone(),
    };

    let manifest_json = serde_json::to_string_pretty(&manifest_for_storage)?;

    let manifest_key = format!("{version_prefix}/release-manifest.json");
    target
        .upload(&manifest_key, manifest_json.as_bytes())
        .await?;

    for artifact in &manifest.artifacts {
        let url = source.artifact_url(tag, &artifact.name);
        info!("downloading {} from {}", artifact.name, url);

        let data = download_bytes(&url).await?;

        if !artifact.sha256.is_empty() {
            let actual = compute_sha256(&data);
            if actual != artifact.sha256 {
                return Err(MirrorError::TargetError(format!(
                    "checksum mismatch for {}: expected {}, got {}",
                    artifact.name, artifact.sha256, actual
                )));
            }
        }

        let key = format!("{version_prefix}/{}", artifact.name);
        target.upload(&key, &data).await?;
        debug!("uploaded {}", artifact.name);
    }

    Ok(())
}

/// Download bytes from a URL.
async fn download_bytes(url: &str) -> Result<Vec<u8>, MirrorError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| MirrorError::GitHubApi(e.to_string()))?;

    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| MirrorError::GitHubApi(e.to_string()))?;

    let resp = resp
        .error_for_status()
        .map_err(|e| MirrorError::GitHubApi(e.to_string()))?;

    resp.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| MirrorError::GitHubApi(e.to_string()))
}

/// Compute SHA-256 hex digest.
pub fn compute_sha256(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

/// ISO 8601 UTC timestamp for "now".
fn now_iso8601() -> String {
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();
    let days = secs / 86400;
    let remaining = secs % 86400;
    let hours = remaining / 3600;
    let minutes = (remaining % 3600) / 60;
    let seconds = remaining % 60;
    let (y, m, d) = days_to_ymd(days);
    format!("{y:04}-{m:02}-{d:02}T{hours:02}:{minutes:02}:{seconds:02}Z")
}

/// Convert days since Unix epoch to (year, month, day).
fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    let mut y = 1970;
    let mut remaining = days;
    loop {
        let days_in_year = if is_leap(y) { 366 } else { 365 };
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        y += 1;
    }
    let leap = is_leap(y);
    let month_days: [u64; 12] = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut m = 1;
    for &days_in_month in &month_days {
        if remaining < days_in_month {
            break;
        }
        remaining -= days_in_month;
        m += 1;
    }
    (y, m, remaining + 1)
}

fn is_leap(year: u64) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_sha256_known_value() -> Result<(), Box<dyn std::error::Error>> {
        let hash = compute_sha256(b"hello world");
        assert_eq!(
            hash,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
        Ok(())
    }

    #[test]
    fn sync_scope_variants() -> Result<(), Box<dyn std::error::Error>> {
        let tag = SyncScope::Tag("v1.0".into());
        assert!(matches!(tag, SyncScope::Tag(s) if s == "v1.0"));
        let latest = SyncScope::Latest;
        assert!(matches!(latest, SyncScope::Latest));
        Ok(())
    }

    #[test]
    fn now_iso8601_format() -> Result<(), Box<dyn std::error::Error>> {
        let ts = now_iso8601();
        // Basic format check: YYYY-MM-DDTHH:MM:SSZ
        assert!(ts.ends_with('Z'));
        assert!(ts.contains('T'));
        assert_eq!(ts.len(), 20);
        Ok(())
    }
}
