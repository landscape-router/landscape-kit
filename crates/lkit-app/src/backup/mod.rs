//! Backup use case: create, list, restore, rebuild, and delete backup packages.

pub mod builder;
pub(crate) mod restorer;
pub(crate) mod scanner;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use lkit_core::{LandscapePaths, LkitClient, ManagerPaths, ServiceManager};
use serde::{Deserialize, Serialize};

use crate::AppError;

/// Current backup format version. Increment on breaking changes to the archive layout
/// or metadata schema. Reader must check exact equality.
pub const BACKUP_FORMAT_VERSION: u32 = 1;

/// Metadata stored inside each backup archive as `metadata.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupMetadata {
    pub format_version: u32,
    pub backup_id: String,
    pub created_at: String,
    pub landscape_version: String,
    pub hostname: String,
    pub checksums: HashMap<String, String>,
    pub remark: Option<String>,
    pub auto: bool,
}

/// A discovered backup entry with resolved path and metadata.
#[derive(Debug, Clone, Serialize)]
pub struct BackupEntry {
    /// Backup identifier, e.g. "20260531-143022-a1b2c3d4".
    pub id: String,
    /// Absolute path to the archive file.
    pub path: PathBuf,
    /// Parsed metadata from within the archive.
    pub metadata: BackupMetadata,
}

/// Backup use case providing all backup lifecycle operations.
pub struct BackupUseCase {
    client: Arc<dyn LkitClient>,
    service_manager: Arc<dyn ServiceManager>,
    landscape_paths: LandscapePaths,
    manager_paths: ManagerPaths,
}

impl BackupUseCase {
    /// Create a new BackupUseCase with injected dependencies.
    pub fn new(
        client: Arc<dyn LkitClient>,
        service_manager: Arc<dyn ServiceManager>,
        landscape_paths: LandscapePaths,
        manager_paths: ManagerPaths,
    ) -> Self {
        Self {
            client,
            service_manager,
            landscape_paths,
            manager_paths,
        }
    }

    /// Create a new backup of the running Landscape installation.
    ///
    /// ⚠️ Requires the Landscape API to be reachable (step 1 calls export_config).
    /// Does NOT need the service to be stopped — binary and static assets
    /// are read-only during runtime.
    pub async fn create(&self, remark: Option<String>) -> Result<BackupEntry, AppError> {
        let ts = builder::timestamp();
        let staging = self.manager_paths.tmp_dir.join(format!("staging-{ts}"));
        std::fs::create_dir_all(&staging)?;

        // 1. Export current config.
        let config_content = self.client.export_config().await?;

        // 2. Discover binary path.
        let binary_path = scanner::discover_binary(&self.landscape_paths)?;

        // 3. Copy binary and static assets to staging.
        let binary_staging = staging.join("landscape-webserver");
        std::fs::copy(&binary_path, &binary_staging)?;
        copy_dir_all(&self.landscape_paths.static_dir, &staging.join("static"))?;

        // 4. Write landscape_init.toml to staging.
        std::fs::write(staging.join("landscape_init.toml"), &config_content)?;

        // Compute checksums.
        let mut checksums = HashMap::new();
        checksums.insert("landscape-webserver".into(), builder::sha256_file(&binary_staging)?);
        checksums
            .insert("landscape_init.toml".into(), builder::sha256_data(config_content.as_bytes()));

        // 5. Get version and hostname.
        let landscape_version = self.client.get_version().await?;
        let hostname = hostname();

        // 6. Build backup ID and metadata.
        let backup_id = format!("{ts}-pending");
        let created_at = builder::rfc3339();
        let metadata = BackupMetadata {
            format_version: BACKUP_FORMAT_VERSION,
            backup_id: backup_id.clone(),
            created_at,
            landscape_version,
            hostname,
            checksums,
            remark,
            auto: false,
        };

        // Write metadata.json.
        let meta_json = serde_json::to_string_pretty(&metadata)
            .map_err(|e| AppError::Backup(format!("metadata serialization failed: {e}")))?;
        std::fs::write(staging.join("metadata.json"), &meta_json)?;

        // 7. Build package to temp file with pending ID, then compute file hash.
        let tmp_output = self.manager_paths.tmp_dir.join(format!("lkit-backup-{ts}.tar.gz.tmp"));
        builder::build_package(&staging, &tmp_output)?;
        let file_hash = builder::sha256_file(&tmp_output)?;
        let short_hash = &file_hash[..8];

        // 8. Repack with the final backup ID baked into metadata.json.
        let final_id = format!("{ts}-{short_hash}");
        let final_metadata = BackupMetadata { backup_id: final_id.clone(), ..metadata };
        let final_meta_json = serde_json::to_string_pretty(&final_metadata)
            .map_err(|e| AppError::Backup(format!("metadata serialization failed: {e}")))?;
        std::fs::write(staging.join("metadata.json"), &final_meta_json)?;
        builder::build_package(&staging, &tmp_output)?;

        // 9. Atomic rename: fsync + rename.
        let final_name = format!("lkit-backup-{ts}-{short_hash}.tar.gz");
        let backup_dir = &self.manager_paths.backup_dir;
        std::fs::create_dir_all(backup_dir)?;
        let final_path = backup_dir.join(&final_name);
        let parent = final_path.parent().unwrap_or(Path::new("."));
        let tmp_file = std::fs::File::open(&tmp_output)?;
        tmp_file.sync_all()?;
        std::fs::rename(&tmp_output, &final_path)?;
        let final_file = std::fs::File::open(&final_path)?;
        final_file.sync_all()?;
        if let Ok(dir) = std::fs::File::open(parent) {
            dir.sync_all()?;
        }

        // 10. Cleanup staging.
        let _ = std::fs::remove_dir_all(&staging);

        Ok(BackupEntry {
            id: final_id,
            path: final_path,
            metadata: final_metadata,
        })
    }

    /// List all backups in the backup directory, sorted by creation time descending.
    pub async fn list(&self) -> Result<Vec<BackupEntry>, AppError> {
        let backup_dir = &self.manager_paths.backup_dir;
        if !backup_dir.exists() {
            return Ok(Vec::new());
        }

        let mut entries = Vec::new();
        let dir = std::fs::read_dir(backup_dir)?;
        for entry in dir.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("gz") {
                continue;
            }
            if !path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("lkit-backup-"))
            {
                continue;
            }
            match restorer::read_metadata(&path) {
                Ok(meta) => {
                    entries.push(BackupEntry { id: meta.backup_id.clone(), path, metadata: meta });
                }
                Err(e) => {
                    tracing::warn!("Skipping unreadable backup {:?}: {e}", path);
                }
            }
        }

        // Sort by created_at descending.
        entries.sort_by(|a, b| b.metadata.created_at.cmp(&a.metadata.created_at));
        Ok(entries)
    }

    /// Resolve a backup identifier or file path to a BackupEntry.
    ///
    /// If `id_or_path` matches the `{YYYYMMDD-HHMMSS}-{sha256[:8]}` pattern,
    /// looks up the file in the configured backup directory. Otherwise treats
    /// it as a direct filesystem path.
    pub async fn resolve(&self, id_or_path: &str) -> Result<BackupEntry, AppError> {
        let path = if is_backup_id(id_or_path) {
            let backup_dir = &self.manager_paths.backup_dir;
            let filename = format!("lkit-backup-{}.tar.gz", id_or_path);
            backup_dir.join(filename)
        } else if !id_or_path.contains('/') && !id_or_path.contains('\\') {
            // Fallback: scan backup dir for a filename containing the given ID.
            // Handles legacy archives whose metadata still carries a "-pending" suffix.
            scan_backup_dir(&self.manager_paths.backup_dir, id_or_path).ok_or_else(|| {
                AppError::BackupNotFound(format!("backup file not found: {id_or_path}"))
            })?
        } else {
            PathBuf::from(id_or_path)
        };

        if !path.exists() {
            return Err(AppError::BackupNotFound(format!(
                "backup file not found: {}",
                path.display()
            )));
        }

        let meta = restorer::read_metadata(&path)?;
        if meta.format_version != BACKUP_FORMAT_VERSION {
            return Err(AppError::Backup(format!(
                "incompatible format version: got {}, expected {}",
                meta.format_version, BACKUP_FORMAT_VERSION
            )));
        }

        Ok(BackupEntry { id: meta.backup_id.clone(), path, metadata: meta })
    }

    /// Restore a backup: extract, replace binary/static/config, health check.
    ///
    /// Called from the hidden `_do_restore` subcommand running under systemd-run.
    /// On health check failure the previous state in `recovery_dir` is restored.
    pub async fn restore(&self, entry: &BackupEntry, recovery_dir: &Path) -> Result<(), AppError> {
        let ts = builder::timestamp();
        let staging = self.manager_paths.tmp_dir.join(format!("restore-{ts}"));

        // Extract to staging.
        let extracted = restorer::extract_package(&entry.path, &staging)?;

        // Verify required files exist.
        if !extracted.contains_key("landscape-webserver") {
            return Err(AppError::Backup("backup is missing landscape-webserver".into()));
        }

        // Stop service.
        self.service_manager.stop().await?;

        // Perform replacement with recovery safety.
        let replace_result = self.do_replace(&extracted).await;

        if let Err(e) = replace_result {
            // Restart service regardless.
            let _ = self.service_manager.start().await;
            return Err(e);
        }

        // Start service.
        self.service_manager.start().await?;

        // Health check.
        match restorer::health_check(&self.landscape_paths) {
            Ok(()) => {
                // Success: clean up recovery directory.
                let _ = std::fs::remove_dir_all(recovery_dir);
                let _ = std::fs::remove_dir_all(&staging);
                Ok(())
            }
            Err(e) => {
                // Health check failed — rollback from recovery_dir.
                tracing::warn!("Health check failed, rolling back from {:?}", recovery_dir);
                let _ = self.rollback_from(recovery_dir).await;
                let _ = self.service_manager.start().await;
                let _ = std::fs::remove_dir_all(&staging);
                Err(AppError::HealthCheckFailed(format!("restore failed, rolled back: {e}")))
            }
        }
    }

    /// Extract a backup archive to the given target directory without modifying services.
    pub async fn rebuild(&self, entry: &BackupEntry, target: &Path) -> Result<(), AppError> {
        let extracted = restorer::extract_package(&entry.path, target)?;

        // Ensure binary has execute permission.
        if let Some(bin_path) = extracted.get("landscape-webserver") {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(bin_path, std::fs::Permissions::from_mode(0o755))?;
        }

        Ok(())
    }

    /// Delete a backup archive from the filesystem.
    pub async fn delete(&self, entry: &BackupEntry) -> Result<(), AppError> {
        if entry.path.exists() {
            std::fs::remove_file(&entry.path)?;
        }
        Ok(())
    }

    /// Replace landscape files with extracted backup contents.
    async fn do_replace(&self, extracted: &HashMap<String, PathBuf>) -> Result<(), AppError> {
        // Ensure the destination directory exists (first-time restore from scratch).
        std::fs::create_dir_all(&self.landscape_paths.home)?;

        // Delete the init lock so Landscape re-initializes from landscape_init.toml
        // on next start. Without this, the old database persists and the restored
        // config is silently ignored.
        let _ = std::fs::remove_file(&self.landscape_paths.init_lock);

        // Replace binary.
        if let Some(src) = extracted.get("landscape-webserver") {
            let dst = self.landscape_paths.home.join("landscape-webserver");
            std::fs::copy(src, &dst)?;
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dst, std::fs::Permissions::from_mode(0o755))?;
        }

        // Replace static directory.
        if let Some(src) = extracted.get("static") {
            let dst = self.landscape_paths.home.join("static");
            if dst.exists() {
                let _ = std::fs::remove_dir_all(&dst);
            }
            copy_dir_all(src, &dst)?;
        }

        // Write landscape_init.toml.
        if let Some(src) = extracted.get("landscape_init.toml") {
            let dst = self.landscape_paths.home.join("landscape_init.toml");
            std::fs::copy(src, &dst)?;
        }

        Ok(())
    }

    /// Roll back to the files saved in recovery_dir before restore.
    async fn rollback_from(&self, recovery_dir: &Path) -> Result<(), AppError> {
        let home = &self.landscape_paths.home;

        // Restore binary.
        let recovery_bin = recovery_dir.join("landscape-webserver");
        if recovery_bin.exists() {
            std::fs::copy(&recovery_bin, home.join("landscape-webserver"))?;
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                home.join("landscape-webserver"),
                std::fs::Permissions::from_mode(0o755),
            )?;
        }

        // Restore static.
        let recovery_static = recovery_dir.join("static");
        if recovery_static.exists() {
            let dst = home.join("static");
            if dst.exists() {
                let _ = std::fs::remove_dir_all(&dst);
            }
            copy_dir_all(&recovery_static, &dst)?;
        }

        // Restore config.
        let recovery_config = recovery_dir.join("landscape_init.toml");
        if recovery_config.exists() {
            std::fs::copy(&recovery_config, home.join("landscape_init.toml"))?;
        }

        Ok(())
    }
}

/// Recursively copy a directory.
fn copy_dir_all(src: &Path, dst: &Path) -> Result<(), AppError> {
    if !src.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let file_name = entry.file_name();
        let dst_path = dst.join(&file_name);
        if src_path.is_dir() {
            copy_dir_all(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

/// Get the system hostname, falling back to "unknown".
fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}

/// Scan the backup directory for an archive whose filename contains the given
/// ID (or its timestamp prefix) as a substring.
///
/// Handles legacy IDs like `20260531-182107-pending` where the hash suffix
/// differs from the actual filename (`20260531-182107-5cf7b362`).
fn scan_backup_dir(backup_dir: &Path, id: &str) -> Option<PathBuf> {
    // Use the timestamp prefix for matching (everything before the last dash segment).
    let match_key = id.rsplit_once('-').map_or(id, |(prefix, _)| prefix);
    let dir = std::fs::read_dir(backup_dir).ok()?;
    for entry in dir.filter_map(|e| e.ok()) {
        let path = entry.path();
        let name = path.file_name()?.to_string_lossy();
        if name.starts_with("lkit-backup-") && name.ends_with(".tar.gz") && name.contains(match_key)
        {
            return Some(path);
        }
    }
    None
}

/// Check if a string matches the backup ID format `{YYYYMMDD-HHMMSS}-{sha256[:8]}`.
fn is_backup_id(s: &str) -> bool {
    let parts: Vec<&str> = s.splitn(2, '-').collect();
    if parts.len() != 2 {
        return false;
    }
    let datetime = parts[0];
    let hash = parts[1];
    if datetime.len() != 15 || hash.len() != 8 {
        return false;
    }
    datetime.bytes().all(|b| b.is_ascii_digit() || b == b'-')
        && hash.bytes().all(|b| b.is_ascii_hexdigit())
}
