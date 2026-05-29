//! Mirror sync — download releases from a source and push to mirror target.

use std::collections::HashMap;
use std::path::Path;

use lkit_core::{ReleaseManifest, ReleaseSource};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
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

/// Return the tag with the highest version number.
fn max_tag(tags: &[String]) -> Option<&String> {
    tags.iter().max_by(|a, b| lkit_core::compare_semver(a, b))
}

/// Run sync from a release source to a mirror target.
pub async fn run_sync(
    config: &SyncConfig,
    source: &dyn ReleaseSource,
    target: &dyn MirrorTarget,
) -> Result<SyncResult, MirrorError> {
    let tags = resolve_tags(source, &config.scope).await?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| MirrorError::GitHubApi(e.to_string()))?;

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

        // Check if the release has any artifacts before syncing.
        match source.get_artifacts(tag).await {
            Ok(manifest) if manifest.artifacts.is_empty() => {
                tracing::warn!("skipping {tag} — no downloadable artifacts");
                skipped.push(tag.clone());
                continue;
            }
            Err(e) => {
                let msg = e.to_string();
                tracing::warn!("failed to sync {tag}: {msg}");
                failed.push((tag.clone(), msg));
                continue;
            }
            _ => {}
        }

        info!("syncing {tag}...");

        match sync_version(&client, source, target, &config.prefix, tag).await {
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

    // Update latest pointer: follow upstream's notion of "latest".
    // Only write the pointer if the upstream latest actually exists on target
    // (either just synced or was already there).
    update_latest_pointer(source, target, &config.prefix, &synced, &skipped).await?;

    Ok(SyncResult {
        synced,
        skipped,
        failed,
    })
}

/// Update the latest pointer on target, following upstream's notion of "latest".
///
/// Queries `source.latest_tag()`. If that tag exists on target (either just synced
/// or already present), writes it as the latest pointer. Falls back to semver-max
/// of synced tags if the source does not support `latest_tag()`.
async fn update_latest_pointer(
    source: &dyn ReleaseSource,
    target: &dyn MirrorTarget,
    prefix: &str,
    synced: &[String],
    skipped: &[String],
) -> Result<(), MirrorError> {
    if synced.is_empty() && skipped.is_empty() {
        return Ok(());
    }

    let latest_key = format!("{prefix}/latest");

    // Try upstream first.
    if let Ok(upstream_latest) = source.latest_tag().await {
        let on_target = synced.contains(&upstream_latest) || skipped.contains(&upstream_latest);
        if on_target {
            target
                .upload(&latest_key, upstream_latest.as_bytes())
                .await
                .map_err(|e| MirrorError::UploadFailed(format!("failed to update latest: {e}")))?;
            return Ok(());
        }
        // Upstream latest not on target — don't touch the pointer.
        return Ok(());
    }

    // Fallback: semver max of newly synced tags.
    if let Some(new_max) = max_tag(synced) {
        let should_update = match target.read(&latest_key).await {
            Ok(data) => {
                let existing = String::from_utf8_lossy(&data).trim().to_string();
                lkit_core::compare_semver(new_max, &existing) == std::cmp::Ordering::Greater
            }
            Err(_) => true,
        };
        if should_update {
            target
                .upload(&latest_key, new_max.as_bytes())
                .await
                .map_err(|e| MirrorError::UploadFailed(format!("failed to update latest: {e}")))?;
        }
    }

    Ok(())
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
                // list_versions() returns newest-first (GitHub API order).
                // "since X" means versions NEWER than X, which are before X in the list.
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
    client: &reqwest::Client,
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

    // Collect sha256 values from all sources (SHASUM256sum.txt + computed).
    let mut sha256_map: HashMap<String, String> = HashMap::new();

    // Check if SHASUM256sum.txt is in the artifact list.
    let has_shasum = manifest
        .artifacts
        .iter()
        .any(|a| a.name == "SHASUM256sum.txt");

    // Download SHASUM256sum.txt first (if present) to get reference hashes.
    if has_shasum {
        let shasum_name = "SHASUM256sum.txt";
        let url = source.artifact_url(tag, shasum_name);
        info!("downloading SHASUM256sum.txt from {}", url);

        let tmp = tempfile::NamedTempFile::new()?;
        download_to_file(client, &url, tmp.path()).await?;
        let data = tokio::fs::read(tmp.path()).await?;

        // Parse: each line is "<hash>  <filename>"
        if let Ok(text) = std::str::from_utf8(&data) {
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if let Some((hash, name)) = line.split_once(char::is_whitespace) {
                    sha256_map.insert(name.trim().to_string(), hash.trim().to_string());
                }
            }
        }

        let key = format!("{version_prefix}/SHASUM256sum.txt");
        target.upload(&key, &data).await?;
        debug!("uploaded SHASUM256sum.txt");
    }

    // Upload artifacts first, manifest last.
    // This ensures a partial sync never leaves a manifest in target
    // (which would cause the version to be skipped on next run).
    for artifact in &manifest.artifacts {
        if artifact.name == "SHASUM256sum.txt" && has_shasum {
            continue; // already handled above
        }

        let url = source.artifact_url(tag, &artifact.name);
        info!("downloading {} from {}", artifact.name, url);

        let tmp = tempfile::NamedTempFile::new()?;
        let computed_hash = download_to_file(client, &url, tmp.path()).await?;

        // Verify against SHASUM256sum.txt hash if available.
        if let Some(expected) = sha256_map.get(&artifact.name) {
            if computed_hash != *expected {
                return Err(MirrorError::TargetError(format!(
                    "checksum mismatch for {}: expected {}, got {}",
                    artifact.name, expected, computed_hash
                )));
            }
        } else if !artifact.sha256.is_empty() && computed_hash != artifact.sha256 {
            // Verify against source-provided hash if present.
            return Err(MirrorError::TargetError(format!(
                "checksum mismatch for {}: expected {}, got {}",
                artifact.name, artifact.sha256, computed_hash
            )));
        }

        // Store hash: prefer SHASUM value, fall back to computed.
        sha256_map
            .entry(artifact.name.clone())
            .or_insert(computed_hash);

        let data = tokio::fs::read(tmp.path()).await?;
        let key = format!("{version_prefix}/{}", artifact.name);
        target.upload(&key, &data).await?;
        debug!("uploaded {}", artifact.name);
    }

    // Build manifest with populated sha256 values.
    let stored_artifacts: Vec<lkit_core::Artifact> = manifest
        .artifacts
        .into_iter()
        .map(|a| lkit_core::Artifact {
            sha256: sha256_map.get(&a.name).cloned().unwrap_or(a.sha256),
            ..a
        })
        .collect();

    let manifest_for_storage = ReleaseManifest {
        format_version: 1,
        tag: tag.to_string(),
        generated_at: now_iso8601(),
        generated_by: Some(format!("lkit {}", env!("CARGO_PKG_VERSION"))),
        artifacts: stored_artifacts,
    };

    let manifest_json = serde_json::to_string_pretty(&manifest_for_storage)?;
    let manifest_key = format!("{version_prefix}/release-manifest.json");
    target
        .upload(&manifest_key, manifest_json.as_bytes())
        .await?;

    Ok(())
}

/// Stream-download a URL to a temp file, computing SHA-256 on the fly.
///
/// Returns the SHA-256 hex digest of the downloaded content.
/// Supports `http(s)://` (via reqwest) and `file://` (local copy).
async fn download_to_file(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
) -> Result<String, MirrorError> {
    if url.starts_with("file://") {
        return download_file_url(url, dest).await;
    }
    download_http(client, url, dest).await
}

/// Download via HTTP(S) with streaming SHA-256.
async fn download_http(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
) -> Result<String, MirrorError> {
    use futures::StreamExt;

    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| MirrorError::GitHubApi(e.to_string()))?;

    let resp = resp
        .error_for_status()
        .map_err(|e| MirrorError::GitHubApi(e.to_string()))?;

    let mut file = tokio::fs::File::create(dest).await?;
    let mut hasher = Sha256::new();
    let mut stream = resp.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| MirrorError::GitHubApi(e.to_string()))?;
        hasher.update(&chunk);
        file.write_all(&chunk).await.map_err(MirrorError::Io)?;
    }

    file.flush().await.map_err(MirrorError::Io)?;
    Ok(format!("{:x}", hasher.finalize()))
}

/// Copy a local file (file:// URL) to dest, computing SHA-256 on the fly.
async fn download_file_url(url: &str, dest: &Path) -> Result<String, MirrorError> {
    let path_str = url.strip_prefix("file://").unwrap_or(url);
    let src_path = Path::new(path_str);

    let mut src = tokio::fs::File::open(src_path).await.map_err(|e| {
        MirrorError::Io(std::io::Error::new(
            e.kind(),
            format!("failed to open {path_str}: {e}"),
        ))
    })?;
    let mut dst = tokio::fs::File::create(dest).await?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 8192];

    loop {
        let n = src.read(&mut buf).await.map_err(|e| {
            MirrorError::Io(std::io::Error::new(
                e.kind(),
                format!("failed to read {path_str}: {e}"),
            ))
        })?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        dst.write_all(&buf[..n]).await.map_err(MirrorError::Io)?;
    }

    dst.flush().await.map_err(MirrorError::Io)?;
    Ok(format!("{:x}", hasher.finalize()))
}

/// Compute SHA-256 hex digest.
pub fn compute_sha256(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

/// ISO 8601 UTC timestamp for "now".
///
/// Hand-rolled to avoid pulling in `chrono` or `jiff` for a single timestamp.
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

    use crate::target::local::LocalTarget;
    use lkit_core::{Artifact, ReleaseManifest, ReleaseSource, SourceError};
    use std::time::Duration;

    struct MockSource {
        artifacts: Vec<Artifact>,
    }

    #[async_trait::async_trait]
    impl ReleaseSource for MockSource {
        fn name(&self) -> &str {
            "mock"
        }
        async fn latest_tag(&self) -> Result<String, SourceError> {
            Ok("v1.0".into())
        }
        async fn list_versions(&self) -> Result<Vec<String>, SourceError> {
            Ok(vec!["v1.0".into()])
        }
        async fn get_artifacts(&self, tag: &str) -> Result<ReleaseManifest, SourceError> {
            Ok(ReleaseManifest {
                format_version: 1,
                tag: tag.into(),
                generated_at: String::new(),
                generated_by: None,
                artifacts: self.artifacts.clone(),
            })
        }
        fn artifact_url(&self, _tag: &str, name: &str) -> String {
            format!("http://mock/{name}")
        }
        async fn probe(&self, _tag: &str) -> Result<Duration, SourceError> {
            Ok(Duration::from_millis(1))
        }
    }

    #[tokio::test]
    async fn sync_version_does_not_leave_manifest_on_failure()
    -> Result<(), Box<dyn std::error::Error>> {
        // When artifact download fails, manifest must NOT be uploaded.
        let dir = tempfile::tempdir()?;
        let target = LocalTarget::new(dir.path());

        let source = MockSource {
            artifacts: vec![Artifact {
                name: "test.bin".into(),
                sha256: "abc".into(),
                size: 100,
                arch: None,
            }],
        };

        let config = SyncConfig {
            prefix: "landscape".into(),
            scope: SyncScope::Tag("v1.0".into()),
            force: false,
        };

        let result = run_sync(&config, &source, &target).await;
        // sync will fail because download_bytes hits http://mock/test.bin
        match result {
            Err(_) => {}
            Ok(r) => assert!(!r.failed.is_empty(), "expected sync failure"),
        }

        // Critical: manifest must NOT be present in target
        let manifest_exists = target
            .exists("landscape/v1.0/release-manifest.json")
            .await?;
        assert!(
            !manifest_exists,
            "manifest should not exist when sync fails"
        );
        Ok(())
    }

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

    #[tokio::test]
    async fn since_scope_returns_newer_versions() -> Result<(), Box<dyn std::error::Error>> {
        // list_versions returns newest-first (GitHub API order)
        struct MockListSource;

        #[async_trait::async_trait]
        impl ReleaseSource for MockListSource {
            fn name(&self) -> &str {
                "mock"
            }
            async fn latest_tag(&self) -> Result<String, SourceError> {
                Ok("v3.0".into())
            }
            async fn list_versions(&self) -> Result<Vec<String>, SourceError> {
                Ok(vec!["v3.0".into(), "v2.0".into(), "v1.0".into()])
            }
            async fn get_artifacts(&self, _tag: &str) -> Result<ReleaseManifest, SourceError> {
                Ok(ReleaseManifest {
                    format_version: 1,
                    tag: "v1.0".into(),
                    generated_at: String::new(),
                    generated_by: None,
                    artifacts: vec![],
                })
            }
            fn artifact_url(&self, _tag: &str, _name: &str) -> String {
                String::new()
            }
            async fn probe(&self, _tag: &str) -> Result<Duration, SourceError> {
                Ok(Duration::from_millis(1))
            }
        }

        let tags = resolve_tags(&MockListSource, &SyncScope::Since("v1.0".into())).await?;
        // --since v1.0 should return versions NEWER than v1.0
        assert_eq!(tags, vec!["v3.0", "v2.0"]);
        Ok(())
    }

    #[test]
    fn days_to_ymd_leap_year() -> Result<(), Box<dyn std::error::Error>> {
        // 2000-01-01 = day 10957 since epoch
        // 2000-02-29 = day 10957 + 31 + 28 = day 11016
        let (y, m, d) = days_to_ymd(11016);
        assert_eq!((y, m, d), (2000, 2, 29));
        Ok(())
    }

    #[test]
    fn days_to_ymd_end_of_leap_year() -> Result<(), Box<dyn std::error::Error>> {
        // 2000-12-31 = day 10957 + 365 = 11322 (year 2000 has 366 days, but
        // 2000-01-01 is day 10957 and 2000-12-31 is the 366th day = 10957+365)
        let (y, m, d) = days_to_ymd(11322);
        assert_eq!((y, m, d), (2000, 12, 31));
        Ok(())
    }

    #[test]
    fn max_tag_selects_highest() {
        let tags: Vec<String> = vec!["v0.9.0".into(), "v0.19.2".into(), "v0.18.3".into()];
        assert_eq!(max_tag(&tags).map(|s| s.as_str()), Some("v0.19.2"));
    }

    #[test]
    fn max_tag_empty_list() {
        let tags: Vec<String> = vec![];
        assert!(max_tag(&tags).is_none());
    }

    #[test]
    fn max_tag_single() {
        let tags: Vec<String> = vec!["v1.0".into()];
        assert_eq!(max_tag(&tags).map(|s| s.as_str()), Some("v1.0"));
    }
}
