//! Backup use case — backup lifecycle operations.

pub mod packer;
pub mod restore;
pub mod scanner;

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::Arc;

use chrono::Utc;
use lkit_core::{
    AUTO_BACKUP_LIMIT, BackupEntry, BackupMetadata, BackupScope, LKIT_VERSION, LandscapePaths,
    LkitClient, META_REGION_SIZE, ManagerPaths, ServiceManager,
};
use rand::Rng;

use crate::error::AppError;

use self::packer::{build_archive_to, extract_verified, read_metadata, write_meta_region};
use self::scanner::discover_binary;

/// Input parameters for backup creation (private helper to reduce arg count).
struct BackupPayload<'a> {
    binary_path: &'a Path,
    init_content: &'a str,
    remark: Option<String>,
    auto: bool,
    all: bool,
}

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
        Self {
            client,
            service_manager,
            landscape_paths,
            manager_paths,
        }
    }

    /// Create a BackupUseCase from shared application state.
    pub fn from_state(state: &crate::AppState) -> Self {
        Self::new(
            state.client.clone(),
            state.service_manager.clone(),
            state.landscape_paths.clone(),
            state.manager_paths.clone(),
        )
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
            binary_path.metadata().map_err(|e| AppError::Backup(format!("stat binary: {e}")))?.len()
                + dir_size(&self.landscape_paths.static_dir).unwrap_or(0)
                + init_content.len() as u64
        } + META_REGION_SIZE;

        fs::create_dir_all(&self.manager_paths.backup_dir)
            .map_err(|e| AppError::Backup(format!("mkdir backup_dir: {e}")))?;

        check_space(estimated_need, &self.manager_paths.backup_dir)?;

        // 5. Execute core logic with staging cleanup guard
        self.do_create(&binary_path, &init_content, remark, auto, all).await
    }

    async fn do_create(
        &self,
        binary_path: &Path,
        init_content: &str,
        remark: Option<String>,
        auto: bool,
        all: bool,
    ) -> Result<BackupEntry, AppError> {
        let ts = Utc::now().format("%Y%m%d-%H%M%S").to_string();
        let rand_suffix: u32 = rand::rng().random();
        let tag = format!("{ts}-{rand_suffix:08x}");
        let staging_dir = self.manager_paths.tmp_dir.join(format!("staging-{tag}"));
        fs::create_dir_all(&staging_dir)
            .map_err(|e| AppError::Backup(format!("mkdir staging: {e}")))?;

        let payload = BackupPayload { binary_path, init_content, remark, auto, all };
        let result = self.build_backup(&staging_dir, payload, &tag).await;
        if result.is_err() {
            let _ = fs::remove_dir_all(&staging_dir);
        }
        result
    }

    async fn build_backup(
        &self,
        staging_dir: &Path,
        payload: BackupPayload<'_>,
        tag: &str,
    ) -> Result<BackupEntry, AppError> {
        clean_orphan_tmps(&self.manager_paths.backup_dir);

        let scope = if payload.all { BackupScope::Full } else { BackupScope::Minimal };

        if payload.all {
            copy_dir_all(&self.landscape_paths.home, staging_dir)?;
        } else {
            fs::copy(payload.binary_path, staging_dir.join("landscape-webserver"))
                .map_err(|e| AppError::Backup(format!("copy binary: {e}")))?;
            copy_dir_all(&self.landscape_paths.static_dir, &staging_dir.join("static"))?;
            fs::write(staging_dir.join("landscape_init.toml"), payload.init_content)
                .map_err(|e| AppError::Backup(format!("write init: {e}")))?;
        }

        // 6. Build .lkb
        let tmp_file = self.manager_paths.backup_dir.join(format!(".tmp-{tag}"));

        let filename = {
            let mut file = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(&tmp_file)
                .map_err(|e| AppError::Backup(format!("create .tmp: {e}")))?;

            let checksum = build_archive_to(staging_dir, &mut file, META_REGION_SIZE)?;

            let hostname = nix::unistd::gethostname()
                .ok()
                .and_then(|h| h.into_string().ok())
                .unwrap_or_else(|| "unknown".into());

            let short_hash = checksum
                .strip_prefix("sha256:")
                .and_then(|h| h.get(..8))
                .unwrap_or(&checksum[..8.min(checksum.len())]);

            let ts = tag.rsplit_once('-').map_or(tag, |(l, _)| l);
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
                remark: payload.remark,
                auto: payload.auto,
                scope,
                checksum,
            };

            write_meta_region(&mut file, &metadata)?;

            file.sync_all().map_err(|e| AppError::Backup(format!("fsync: {e}")))?;

            filename
        };

        // 7. Rename to final name
        let final_path = self.manager_paths.backup_dir.join(&filename);
        fs::rename(&tmp_file, &final_path).map_err(|e| AppError::Backup(format!("rename: {e}")))?;

        // 8. Cleanup staging
        let _ = fs::remove_dir_all(staging_dir); // best-effort: clean staging after success

        // 9. Trim auto backups if this is an auto backup
        if payload.auto
            && let Err(e) = self.trim_auto_backups()
        {
            tracing::warn!("trim auto backups failed: {e}");
        }

        // 10. Return entry
        let file_size = fs::metadata(&final_path).map(|m| m.len()).unwrap_or(0);

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
            let filename =
                path.file_name().and_then(|n| n.to_str()).unwrap_or("unknown").to_string();

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
            let filename =
                direct.file_name().and_then(|n| n.to_str()).unwrap_or("unknown").to_string();

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
            let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

            let expected_prefix = format!("lkit-backup-{id_or_path}");
            if filename.starts_with(&expected_prefix) {
                let file_size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                let filename = filename.to_string();
                let metadata = read_metadata_from_path(&path)?;
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
        if target.exists() && target.read_dir().is_ok_and(|mut d| d.next().is_some()) && !force {
            return Err(AppError::Backup(format!(
                "target directory not empty: {}",
                target.display()
            )));
        }

        fs::create_dir_all(target).map_err(|e| AppError::Backup(format!("mkdir target: {e}")))?;

        let mut file =
            fs::File::open(&entry.path).map_err(|e| AppError::Backup(format!("open: {e}")))?;

        let result = extract_verified(&mut file, &entry.checksum, target);
        if result.is_err() {
            let _ = fs::remove_dir_all(target); // best-effort: clean dirty extraction
            result?;
        }

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
        fs::remove_file(&entry.path).map_err(|e| AppError::Backup(format!("delete file: {e}")))?;
        Ok(())
    }

    // ── trim ──

    fn trim_auto_backups(&self) -> Result<(), AppError> {
        let entries = self.list()?;
        let auto_entries: Vec<&BackupEntry> = entries.iter().filter(|e| e.auto).collect();

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

/// Remove stale `.tmp-*` files from `dir` that are older than 1 hour.
/// These are left behind when a previous `create()` process was interrupted.
fn clean_orphan_tmps(dir: &Path) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if !name_str.starts_with(".tmp-") {
            continue;
        }
        if let Ok(meta) = entry.metadata()
            && let Ok(modified) = meta.modified()
            && modified.elapsed().unwrap_or_default().as_secs() > 3600
        {
            let _ = fs::remove_file(entry.path()); // best-effort: clean orphan tmp file
        }
    }
}

fn read_metadata_from_path(path: &Path) -> Result<BackupMetadata, AppError> {
    let mut file = fs::File::open(path).map_err(|e| AppError::Backup(format!("open: {e}")))?;
    read_metadata(&mut file)
}

/// Recursively copy a directory tree.
pub(super) fn copy_dir_all(src: &Path, dst: &Path) -> Result<(), AppError> {
    fs::create_dir_all(dst)
        .map_err(|e| AppError::Backup(format!("mkdir {dst}: {e}", dst = dst.display())))?;

    for entry in fs::read_dir(src)
        .map_err(|e| AppError::Backup(format!("read_dir {src}: {e}", src = src.display())))?
        .flatten()
    {
        let src_path = entry.path();
        let file_name =
            src_path.file_name().ok_or_else(|| AppError::Backup("invalid filename".into()))?;
        let dst_path = dst.join(file_name);

        if src_path.is_dir() {
            copy_dir_all(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path).map_err(|e| {
                AppError::Backup(format!("copy {src_path}: {e}", src_path = src_path.display()))
            })?;
        }
    }

    Ok(())
}

/// Estimate the total byte size of all files under a directory.
fn dir_size(dir: &Path) -> Result<u64, AppError> {
    let mut total = 0u64;
    for entry in
        fs::read_dir(dir).map_err(|e| AppError::Backup(format!("read_dir: {e}")))?.flatten()
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
pub(super) fn check_space(need_bytes: u64, check_path: &Path) -> Result<(), AppError> {
    let stat = nix::sys::statvfs::statvfs(check_path)
        .map_err(|e| AppError::Backup(format!("statvfs: {e}")))?;

    let available = stat.blocks_available() as u64 * stat.fragment_size() as u64;
    let twenty_mb = 20u64 * 1024 * 1024;
    let twenty_percent = need_bytes / 5;
    let safety_margin = twenty_mb.max(twenty_percent);
    let required = need_bytes + safety_margin;

    if required > available {
        return Err(AppError::SpaceInsufficient { need: required, available });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use lkit_core::CoreError;
    use tempfile::TempDir;

    struct MockClient {
        healthy: bool,
        version: String,
        config_content: String,
    }

    #[async_trait]
    impl LkitClient for MockClient {
        async fn get_version(&self) -> Result<String, CoreError> {
            Ok(self.version.clone())
        }

        async fn health_check(&self) -> Result<bool, CoreError> {
            if self.healthy { Ok(true) } else { Err(CoreError::Internal("unreachable".into())) }
        }

        async fn export_config(&self) -> Result<String, CoreError> {
            Ok(self.config_content.clone())
        }
    }

    struct MockServiceManager;

    #[async_trait]
    impl ServiceManager for MockServiceManager {
        async fn status(&self) -> Result<lkit_core::ServiceState, CoreError> {
            Ok(lkit_core::ServiceState { active: true, enabled: true, pid: Some(1) })
        }
        async fn start(&self) -> Result<(), CoreError> {
            Ok(())
        }
        async fn stop(&self) -> Result<(), CoreError> {
            Ok(())
        }
        async fn restart(&self) -> Result<(), CoreError> {
            Ok(())
        }
    }

    fn setup() -> Result<(TempDir, BackupUseCase), Box<dyn std::error::Error>> {
        let tmp = TempDir::new()?;
        let landscape_home = tmp.path().join("landscape");
        let manager_home = tmp.path().join("manager");

        std::fs::create_dir_all(&landscape_home)?;
        std::fs::create_dir_all(landscape_home.join("static"))?;
        std::fs::write(landscape_home.join("landscape-webserver"), b"fake binary")?;
        std::fs::write(landscape_home.join("landscape.toml"), b"[web]\nport = 6443\n")?;

        std::fs::create_dir_all(manager_home.join("backup"))?;
        std::fs::create_dir_all(manager_home.join("tmp"))?;

        let landscape_paths = LandscapePaths::new(landscape_home);
        let manager_paths = ManagerPaths::new(manager_home);

        let client = Arc::new(MockClient {
            healthy: true,
            version: "0.19.2".into(),
            config_content: "version = \"0.19.2\"\n".into(),
        });

        let use_case = BackupUseCase::new(
            client,
            Arc::new(MockServiceManager),
            landscape_paths,
            manager_paths,
        );

        Ok((tmp, use_case))
    }

    #[tokio::test]
    async fn test_create_minimal_backup() -> Result<(), Box<dyn std::error::Error>> {
        let (_tmp, use_case) = setup()?;

        let entry = use_case.create(None, false, false).await?;
        assert_eq!(entry.scope, BackupScope::Minimal);
        assert!(entry.path.exists());
        assert!(entry.filename.ends_with(".lkb"));
        assert!(entry.file_size > 0);
        assert!(!entry.backup_id.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_list_empty() -> Result<(), Box<dyn std::error::Error>> {
        let (_tmp, use_case) = setup()?;

        let entries = use_case.list()?;
        assert!(entries.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_resolve_not_found() -> Result<(), Box<dyn std::error::Error>> {
        let (_tmp, use_case) = setup()?;

        let result = use_case.resolve("nonexistent-id");
        match result {
            Err(AppError::BackupNotFound(_)) => Ok(()),
            other => Err(format!("expected BackupNotFound, got {other:?}").into()),
        }
    }
}
