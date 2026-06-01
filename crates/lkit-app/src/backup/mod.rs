//! Backup use case — backup lifecycle operations.

pub mod packer;
pub mod scanner;

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::Arc;

use chrono::Utc;
use lkit_core::{
    BackupEntry, BackupMetadata, BackupScope, LandscapePaths, LkitClient, ManagerPaths,
    ServiceManager, AUTO_BACKUP_LIMIT, LKIT_VERSION, META_REGION_SIZE,
};

use crate::error::AppError;

use self::packer::{build_archive_to, extract_verified, read_metadata, write_meta_region};
use self::scanner::discover_binary;

/// Backup use case — all backup lifecycle operations.
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
        Self { client, service_manager, landscape_paths, manager_paths }
    }

    // ── create ──

    /// Create a new backup.
    pub async fn create(
        &self,
        remark: Option<String>,
        auto: bool,
        all: bool,
    ) -> Result<BackupEntry, AppError> {
        // 1. Check API reachable
        self.client
            .health_check()
            .await
            .map_err(|e| AppError::Backup(format!("API unreachable: {e}")))?;

        // 2. Discover binary
        let binary_path = discover_binary(&self.landscape_paths.home)?;

        // 3. Export config
        let init_content = self
            .client
            .export_config()
            .await
            .map_err(|e| AppError::Backup(format!("export_config failed: {e}")))?;

        // 4. Space precheck
        let estimated_need = if all {
            dir_size(&self.landscape_paths.home)?
        } else {
            binary_path.metadata().map(|m| m.len()).unwrap_or(0)
                + dir_size(&self.landscape_paths.static_dir).unwrap_or(0)
                + init_content.len() as u64
        } + META_REGION_SIZE;
        check_space(estimated_need, &self.manager_paths.backup_dir)?;

        // 5. Build staging
        let ts = Utc::now().format("%Y%m%d-%H%M%S").to_string();
        let rand_suffix: u32 = rand::random();
        let staging_dir = self.manager_paths.tmp_dir.join(format!("staging-{ts}-{rand_suffix:08x}"));
        fs::create_dir_all(&staging_dir)
            .map_err(|e| AppError::Backup(format!("mkdir staging: {e}")))?;

        fs::create_dir_all(&self.manager_paths.backup_dir)
            .map_err(|e| AppError::Backup(format!("mkdir backup_dir: {e}")))?;

        let scope = if all { BackupScope::Full } else { BackupScope::Minimal };

        if all {
            copy_dir_all(&self.landscape_paths.home, &staging_dir)?;
        } else {
            fs::copy(&binary_path, staging_dir.join("landscape-webserver"))
                .map_err(|e| AppError::Backup(format!("copy binary: {e}")))?;
            copy_dir_all(
                &self.landscape_paths.static_dir,
                &staging_dir.join("static"),
            )?;
            fs::write(staging_dir.join("landscape_init.toml"), &init_content)
                .map_err(|e| AppError::Backup(format!("write init: {e}")))?;
        }

        // 6. Build .lkb
        let tmp_file = self
            .manager_paths
            .backup_dir
            .join(format!(".tmp-{ts}-{rand_suffix:08x}"));

        let filename = {
            let mut file = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(&tmp_file)
                .map_err(|e| AppError::Backup(format!("create .tmp: {e}")))?;

            let checksum = build_archive_to(&staging_dir, &mut file, META_REGION_SIZE)?;

            let hostname = nix::unistd::gethostname()
                .ok()
                .and_then(|h| h.into_string().ok())
                .unwrap_or_else(|| "unknown".into());

            let short_hash = &checksum[7..15];

            let backup_id = format!("{ts}-{short_hash}");
            let filename = format!("lkit-backup-{backup_id}.lkb");

            let metadata = BackupMetadata {
                backup_id: backup_id.clone(),
                created_at: Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
                landscape_version: self
                    .client
                    .get_version()
                    .await
                    .unwrap_or_else(|_| "unknown".into()),
                lkit_version: LKIT_VERSION.to_owned(),
                hostname,
                remark,
                auto,
                scope,
                checksum,
            };

            write_meta_region(&mut file, &metadata)?;

            file.sync_all()
                .map_err(|e| AppError::Backup(format!("fsync: {e}")))?;

            filename
        };

        // 7. Rename to final name
        let final_path = self.manager_paths.backup_dir.join(&filename);
        fs::rename(&tmp_file, &final_path)
            .map_err(|e| AppError::Backup(format!("rename: {e}")))?;

        // 8. Cleanup staging
        let _ = fs::remove_dir_all(&staging_dir);

        // 9. Trim auto backups if this is an auto backup
        if auto {
            if let Err(e) = self.trim_auto_backups() {
                tracing::warn!("trim auto backups failed: {e}");
            }
        }

        // 10. Return entry
        let file_size = fs::metadata(&final_path)
            .map(|m| m.len())
            .unwrap_or(0);

        let metadata = read_metadata_from_path(&final_path)?;

        Ok(BackupEntry {
            backup_id: metadata.backup_id,
            created_at: metadata.created_at,
            landscape_version: metadata.landscape_version,
            lkit_version: metadata.lkit_version,
            hostname: metadata.hostname,
            remark: metadata.remark,
            auto: metadata.auto,
            scope: metadata.scope,
            checksum: metadata.checksum,
            filename,
            file_size,
            path: final_path,
        })
    }

    // ── list ──

    /// List all backups in `backup_dir`.
    pub fn list(&self) -> Result<Vec<BackupEntry>, AppError> {
        fs::create_dir_all(&self.manager_paths.backup_dir)
            .map_err(|e| AppError::Backup(format!("mkdir backup_dir: {e}")))?;

        let mut entries = Vec::new();
        let dir = fs::read_dir(&self.manager_paths.backup_dir)
            .map_err(|e| AppError::Backup(format!("read backup_dir: {e}")))?;

        for entry in dir.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("lkb") {
                continue;
            }

            let file_size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            let filename = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();

            match read_metadata_from_path(&path) {
                Ok(metadata) => {
                    entries.push(BackupEntry {
                        backup_id: metadata.backup_id,
                        created_at: metadata.created_at,
                        landscape_version: metadata.landscape_version,
                        lkit_version: metadata.lkit_version,
                        hostname: metadata.hostname,
                        remark: metadata.remark,
                        auto: metadata.auto,
                        scope: metadata.scope,
                        checksum: metadata.checksum,
                        filename,
                        file_size,
                        path: path.clone(),
                    });
                }
                Err(_) => {
                    entries.push(BackupEntry {
                        backup_id: "corrupted".into(),
                        created_at: String::new(),
                        landscape_version: String::new(),
                        lkit_version: String::new(),
                        hostname: String::new(),
                        remark: Some("(corrupted)".into()),
                        auto: false,
                        scope: BackupScope::Minimal,
                        checksum: String::new(),
                        filename,
                        file_size,
                        path,
                    });
                }
            }
        }

        entries.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        Ok(entries)
    }

    // ── resolve ──

    /// Resolve a backup by ID or direct file path.
    pub fn resolve(&self, id_or_path: &str) -> Result<BackupEntry, AppError> {
        let direct = Path::new(id_or_path);
        if direct.is_file() {
            let file_size = fs::metadata(direct).map(|m| m.len()).unwrap_or(0);
            let filename = direct
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
                .to_string();

            let metadata = read_metadata_from_path(direct)?;
            return Ok(BackupEntry {
                backup_id: metadata.backup_id,
                created_at: metadata.created_at,
                landscape_version: metadata.landscape_version,
                lkit_version: metadata.lkit_version,
                hostname: metadata.hostname,
                remark: metadata.remark,
                auto: metadata.auto,
                scope: metadata.scope,
                checksum: metadata.checksum,
                filename,
                file_size,
                path: direct.to_path_buf(),
            });
        }

        fs::create_dir_all(&self.manager_paths.backup_dir)
            .map_err(|e| AppError::Backup(format!("mkdir backup_dir: {e}")))?;

        let dir = fs::read_dir(&self.manager_paths.backup_dir)
            .map_err(|e| AppError::Backup(format!("read backup_dir: {e}")))?;

        for entry in dir.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("lkb") {
                continue;
            }
            let filename = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");

            let expected_prefix = format!("lkit-backup-{id_or_path}");
            if filename.starts_with(&expected_prefix) {
                let file_size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                let filename = filename.to_string();
                let metadata = match read_metadata_from_path(&path) {
                    Ok(m) => m,
                    Err(e) => return Err(e),
                };
                return Ok(BackupEntry {
                    backup_id: metadata.backup_id,
                    created_at: metadata.created_at,
                    landscape_version: metadata.landscape_version,
                    lkit_version: metadata.lkit_version,
                    hostname: metadata.hostname,
                    remark: metadata.remark,
                    auto: metadata.auto,
                    scope: metadata.scope,
                    checksum: metadata.checksum,
                    filename,
                    file_size,
                    path: path.clone(),
                });
            }
        }

        Err(AppError::BackupNotFound(id_or_path.to_owned()))
    }

    // ── extract ──

    /// Extract a backup to `target` directory.
    pub fn extract(&self, entry: &BackupEntry, target: &Path, force: bool) -> Result<(), AppError> {
        if target.exists() && target.read_dir().map_or(false, |mut d| d.next().is_some()) {
            if !force {
                return Err(AppError::Backup(format!(
                    "target directory not empty: {}",
                    target.display()
                )));
            }
        }

        fs::create_dir_all(target)
            .map_err(|e| AppError::Backup(format!("mkdir target: {e}")))?;

        let mut file = fs::File::open(&entry.path)
            .map_err(|e| AppError::Backup(format!("open: {e}")))?;

        extract_verified(&mut file, &entry.checksum, target)?;

        let binary = target.join("landscape-webserver");
        if binary.exists() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = fs::metadata(&binary)
                    .map_err(|e| AppError::Backup(format!("stat binary: {e}")))?
                    .permissions();
                perms.set_mode(0o755);
                fs::set_permissions(&binary, perms)
                    .map_err(|e| AppError::Backup(format!("chmod binary: {e}")))?;
            }
        }

        Ok(())
    }

    // ── delete ──

    /// Delete a backup file.
    pub fn delete(&self, entry: &BackupEntry) -> Result<(), AppError> {
        fs::remove_file(&entry.path)
            .map_err(|e| AppError::Backup(format!("delete file: {e}")))?;
        Ok(())
    }

    // ── trim ──

    fn trim_auto_backups(&self) -> Result<(), AppError> {
        let entries = self.list()?;
        let mut auto_entries: Vec<&BackupEntry> = entries.iter().filter(|e| e.auto).collect();
        auto_entries.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        if auto_entries.len() <= AUTO_BACKUP_LIMIT {
            return Ok(());
        }

        for old in &auto_entries[AUTO_BACKUP_LIMIT..] {
            fs::remove_file(&old.path)
                .map_err(|e| AppError::Backup(format!("trim {old}: {e}", old = old.filename)))?;
        }

        Ok(())
    }
}

// ── helpers ──

fn read_metadata_from_path(path: &Path) -> Result<BackupMetadata, AppError> {
    let mut file = fs::File::open(path)
        .map_err(|e| AppError::Backup(format!("open: {e}")))?;
    read_metadata(&mut file)
}

/// Recursively copy a directory tree.
pub(crate) fn copy_dir_all(src: &Path, dst: &Path) -> Result<(), AppError> {
    fs::create_dir_all(dst)
        .map_err(|e| AppError::Backup(format!("mkdir {dst}: {e}", dst = dst.display())))?;

    for entry in fs::read_dir(src)
        .map_err(|e| AppError::Backup(format!("read_dir {src}: {e}", src = src.display())))?
        .flatten()
    {
        let src_path = entry.path();
        let file_name = src_path
            .file_name()
            .ok_or_else(|| AppError::Backup("invalid filename".into()))?;
        let dst_path = dst.join(file_name);

        if src_path.is_dir() {
            copy_dir_all(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)
                .map_err(|e| AppError::Backup(format!("copy {src_path}: {e}", src_path = src_path.display())))?;
        }
    }

    Ok(())
}

/// Estimate the total byte size of all files under a directory.
fn dir_size(dir: &Path) -> Result<u64, AppError> {
    let mut total = 0u64;
    for entry in fs::read_dir(dir)
        .map_err(|e| AppError::Backup(format!("read_dir: {e}")))?
        .flatten()
    {
        let path = entry.path();
        if path.is_dir() {
            total += dir_size(&path)?;
        } else {
            total += entry.metadata().map(|m| m.len()).unwrap_or(0);
        }
    }
    Ok(total)
}

/// Check that `need_bytes + 20MB` is available on the partition containing `check_path`.
pub(crate) fn check_space(need_bytes: u64, check_path: &Path) -> Result<(), AppError> {
    let stat = nix::sys::statvfs::statvfs(check_path)
        .map_err(|e| AppError::Backup(format!("statvfs: {e}")))?;

    let available = stat.blocks_available() as u64 * stat.fragment_size() as u64;
    let safety_margin = 20 * 1024 * 1024;
    let required = need_bytes + safety_margin;

    if required > available {
        return Err(AppError::SpaceInsufficient {
            need: required,
            available,
        });
    }

    Ok(())
}
