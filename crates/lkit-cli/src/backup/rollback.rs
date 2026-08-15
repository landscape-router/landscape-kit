use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use super::artifacts::{hash_file, hash_str};
use super::backup::{self, BackupMetadata};
use super::health::{self, DocsProbe, HealthOptions, StartupOptions};
use super::plan::InstallError;
use super::root::InstallRoot;
use super::state::{self, InstallState, StateServiceManager};
use super::transaction::{Phase, TransactionFile};
use crate::service::manager::{ManagedService, ServiceManager};
use crate::service::resolv;

/// 切换前写入事务备份目录的旧状态快照文件名。
pub(crate) const STATE_SNAPSHOT_NAME: &str = "previous-state.json";

pub(crate) fn write_state_snapshot(
    root: &InstallRoot,
    transaction_id: &str,
    state: &InstallState,
) -> Result<(), InstallError> {
    let dir = root.canonical.join("backups").join(transaction_id);
    std::fs::create_dir_all(&dir).map_err(InstallError::Io)?;
    let bytes = serde_json::to_vec_pretty(state).map_err(InstallError::StateWrite)?;
    write_atomic(&dir.join(STATE_SNAPSHOT_NAME), &bytes, 0o600)
}

pub(crate) fn read_state_snapshot(
    root: &InstallRoot,
    transaction_id: &str,
) -> Result<InstallState, InstallError> {
    let path = root
        .canonical
        .join("backups")
        .join(transaction_id)
        .join(STATE_SNAPSHOT_NAME);
    let bytes = std::fs::read(&path).map_err(|_| {
        InstallError::CorruptedTransaction(format!(
            "transaction backup snapshot {} is missing",
            path.display()
        ))
    })?;
    let state: InstallState = serde_json::from_slice(&bytes).map_err(|error| {
        InstallError::CorruptedTransaction(format!(
            "{} is not a valid state snapshot: {error}",
            path.display()
        ))
    })?;
    state::validate_state(&state)?;
    Ok(state)
}

/// `.lkb` 配置级回滚(规格 `.lkb` 回滚流程):
/// 1. 标记 `rolling_back`;2. 停止失败版本;3. `data` 移入事务目录 `failed-data`;
///    4. 创建新空 `data`;5. 校验并解包 `.lkb`;6. 用备份重建 `releases/<from_version>`
///    (已存在则先移入 `replaced-release`);7. 恢复 `geo_tmp`;8. 写入导出的初始化配置;
///    9. 原子恢复 `current`;10. systemd 启动并完整健康检查;11. 重新提交旧版本状态;
///    12. 标记 `rolled_back`。任一步失败时标记 `failed` 并返回错误。
pub(crate) async fn rollback_switch<P: DocsProbe>(
    root: &InstallRoot,
    transaction: &TransactionFile,
    snapshot: &InstallState,
    systemd: &dyn ServiceManager,
    health: &HealthOptions<P>,
) -> Result<(), InstallError> {
    super::transaction::mark_phase(root, transaction, Phase::RollingBack)?;
    let is_systemd = snapshot.service.manager == StateServiceManager::Systemd;
    if is_systemd {
        systemd.stop_and_wait(
            ManagedService::LandscapeRouter,
            &(|| {
                systemd
                    .active_state(ManagedService::LandscapeRouter)
                    .map(|state| state != "active")
                    .unwrap_or(true)
            }),
        )?;
    }
    let tx_dir = root
        .canonical
        .join("transactions")
        .join(&transaction.transaction_id);
    std::fs::create_dir_all(&tx_dir).map_err(InstallError::Io)?;
    move_data_aside(&root.canonical.join("data"), &tx_dir.join("failed-data"))?;
    let data = root.canonical.join("data");

    let backup_ref = transaction.backup.as_ref().ok_or_else(|| {
        InstallError::CorruptedTransaction(
            "switch transaction is missing its .lkb backup reference".into(),
        )
    })?;
    let lkb_bytes =
        std::fs::read(root.canonical.join(&backup_ref.path)).map_err(InstallError::Io)?;
    let restore_dir = tx_dir.join("restore");
    let _ = std::fs::remove_dir_all(&restore_dir);
    let metadata = backup::extract_lkb(&lkb_bytes, &restore_dir)?;

    let from_version = transaction.from_version.as_deref().ok_or_else(|| {
        InstallError::CorruptedTransaction("switch transaction is missing from_version".into())
    })?;
    rebuild_release_from_backup(root, &tx_dir, from_version, &restore_dir)?;
    let geo_tmp_source = restore_dir.join("geo_tmp");
    if geo_tmp_source.is_dir() {
        copy_tree(&geo_tmp_source, &data.join("geo_tmp"))?;
    }
    let init_config =
        std::fs::read(restore_dir.join("landscape_init.toml")).map_err(InstallError::Io)?;
    write_file_atomic(&data.join("landscape_init.toml"), &init_config, 0o600)?;

    restore_current(root, &format!("releases/{from_version}"))?;

    let state = build_restored_state(root, transaction, snapshot, &metadata, &restore_dir)?;
    if is_systemd {
        systemd.register(
            ManagedService::LandscapeRouter,
            &root.canonical.join("service/landscape-router.service"),
        )?;
        systemd.enable(ManagedService::LandscapeRouter)?;
        systemd.start(ManagedService::LandscapeRouter)?;
        let pid = systemd.main_pid(ManagedService::LandscapeRouter)?;
        if pid == 0 {
            return Err(InstallError::Systemd(
                "restored service did not produce a main pid".into(),
            ));
        }
        let options = StartupOptions {
            ports: &health.ports,
            expected_pid: pid,
            docs: &health.docs,
            unit_state: Some(&(|| systemd.active_state(ManagedService::LandscapeRouter).ok())),
            init_required: true,
            data_dir: &data,
            startup_timeout: health.startup_timeout,
            stable_duration: health.stable_duration,
        };
        if let Err(error) = health::wait_for_startup(&options).await {
            let _ = super::transaction::mark_phase(root, transaction, Phase::Failed);
            return Err(error);
        }
        if let Err(error) = health::observe_stable(&options).await {
            let _ = super::transaction::mark_phase(root, transaction, Phase::Failed);
            return Err(error);
        }
    }
    super::state::write_state(root, &state)?;
    super::transaction::mark_phase(root, transaction, Phase::RolledBack)?;
    Ok(())
}

/// 无 `.lkb` 切换失败回滚(`--allow-no-backup`):目标版本启动失败时,
/// 停止目标进程,按 `systemd_before` 恢复 unit 注册与 enabled 状态(不启动),
/// 恢复 `/etc/resolv.conf`,原子恢复 `current` 链接,之后才在切换前服务
/// 运行时重新启动原版本并做健康检查。启动必须发生在 `current` 恢复之后,
/// 避免旧服务在目标版本上启动。原版本 release 与 data 均未被修改,
/// 因此不需要从备份重建;但无法恢复被目标版本重新初始化过的数据,
/// 失败后 `rolled_back` 仍按普通回滚提交。
pub(crate) async fn rollback_no_backup<P: DocsProbe>(
    root: &InstallRoot,
    transaction: &TransactionFile,
    previous: &InstallState,
    systemd: &dyn ServiceManager,
    health: &HealthOptions<P>,
) -> Result<(), InstallError> {
    super::transaction::mark_phase(root, transaction, Phase::RollingBack)?;
    let is_systemd = previous.service.manager == StateServiceManager::Systemd;
    let mut was_active = false;
    if is_systemd {
        systemd.stop_and_wait(
            ManagedService::LandscapeRouter,
            &(|| {
                systemd
                    .active_state(ManagedService::LandscapeRouter)
                    .map(|state| state != "active")
                    .unwrap_or(true)
            }),
        )?;
        let before = transaction.systemd_before.as_ref().ok_or_else(|| {
            InstallError::CorruptedTransaction(
                "no-backup switch transaction is missing systemd_before".into(),
            )
        })?;
        was_active = before.active;
        let unit_origin = root
            .canonical
            .join("service")
            .join(systemd.service_name(ManagedService::LandscapeRouter));
        systemd.restore_registration(ManagedService::LandscapeRouter, before, &unit_origin)?;
        if let Some(backup_path) = &transaction.resolv_conf_backup {
            let backup_dir = root.canonical.join(backup_path);
            resolv::restore(systemd.resolv_conf(), &backup_dir)?;
        }
    }
    let previous_current = transaction.previous_current.as_deref().ok_or_else(|| {
        InstallError::CorruptedTransaction(
            "no-backup switch transaction is missing previous_current".into(),
        )
    })?;
    restore_current(root, previous_current)?;

    if is_systemd && was_active {
        systemd.start(ManagedService::LandscapeRouter)?;
        let pid = systemd.main_pid(ManagedService::LandscapeRouter)?;
        if pid == 0 {
            return Err(InstallError::Systemd(
                "restored service did not produce a main pid".into(),
            ));
        }
        let data = root.canonical.join("data");
        let options = StartupOptions {
            ports: &health.ports,
            expected_pid: pid,
            docs: &health.docs,
            unit_state: Some(&(|| systemd.active_state(ManagedService::LandscapeRouter).ok())),
            init_required: true,
            data_dir: &data,
            startup_timeout: health.startup_timeout,
            stable_duration: health.stable_duration,
        };
        health::wait_for_startup(&options).await?;
        health::observe_stable(&options).await?;
    }

    let mut restored = previous.clone();
    restored.last_transaction_id = Some(transaction.transaction_id.clone());
    restored.committed_at = Some(chrono::Utc::now());
    super::state::write_state(root, &restored)?;
    super::transaction::mark_phase(root, transaction, Phase::RolledBack)?;
    Ok(())
}

/// 将当前 `data` 移入事务目录 `failed-data`。幂等:
/// 上次回滚中断时 `failed-data` 已存在,则丢弃上次尝试创建的新 `data`,
/// 重新从备份恢复;`data` 不存在时直接进入创建新目录的步骤。
pub(crate) fn move_data_aside(data: &Path, failed_data: &Path) -> Result<(), InstallError> {
    std::fs::create_dir_all(failed_data.parent().expect("failed-data has a parent"))
        .map_err(InstallError::Io)?;
    if failed_data.exists() {
        if data.exists() {
            std::fs::remove_dir_all(data).map_err(InstallError::Io)?;
        }
    } else if data.exists() {
        std::fs::rename(data, failed_data).map_err(InstallError::Io)?;
    }
    std::fs::create_dir_all(data).map_err(InstallError::Io)?;
    Ok(())
}

pub(crate) fn rebuild_release_from_backup(
    root: &InstallRoot,
    tx_dir: &Path,
    from_version: &str,
    restore_dir: &Path,
) -> Result<(), InstallError> {
    let releases = root.canonical.join("releases");
    let from_rel = releases.join(from_version);
    let replaced = tx_dir.join("replaced-release");
    if from_rel.exists() {
        let _ = std::fs::remove_dir_all(&replaced);
        std::fs::rename(&from_rel, &replaced).map_err(InstallError::Io)?;
    }
    std::fs::create_dir_all(&from_rel).map_err(InstallError::Io)?;
    let binary = restore_dir.join("landscape-webserver");
    if !binary.is_file() {
        return Err(InstallError::InvalidBackup(
            "backup is missing landscape-webserver".into(),
        ));
    }
    std::fs::copy(&binary, from_rel.join(super::pipeline::WEBSERVER_BINARY))
        .map_err(InstallError::Io)?;
    std::fs::set_permissions(
        from_rel.join(super::pipeline::WEBSERVER_BINARY),
        std::fs::Permissions::from_mode(0o755),
    )
    .map_err(InstallError::Io)?;
    let static_source = restore_dir.join(super::pipeline::STATIC_DIR);
    if !static_source.is_dir() {
        return Err(InstallError::InvalidBackup(
            "backup is missing the static directory".into(),
        ));
    }
    copy_tree(&static_source, &from_rel.join(super::pipeline::STATIC_DIR))?;
    let static_archive = restore_dir.join("static.zip");
    if !static_archive.is_file() {
        return Err(InstallError::InvalidBackup(
            "backup is missing static.zip".into(),
        ));
    }
    std::fs::copy(&static_archive, from_rel.join("static.zip")).map_err(InstallError::Io)?;
    Ok(())
}

fn build_restored_state(
    root: &InstallRoot,
    transaction: &TransactionFile,
    snapshot: &InstallState,
    metadata: &BackupMetadata,
    restore_dir: &Path,
) -> Result<InstallState, InstallError> {
    let from_version = transaction.from_version.as_deref().ok_or_else(|| {
        InstallError::CorruptedTransaction("switch transaction is missing from_version".into())
    })?;
    let binary = restore_dir.join("landscape-webserver");
    let (webserver_sha256, webserver_size) = hash_file(&binary)?;
    let architecture = metadata.architecture;
    let (initialization, service) = match snapshot.service.manager {
        StateServiceManager::Systemd => {
            let unit_sha = hash_str(
                &std::fs::read_to_string(root.canonical.join("service/landscape-router.service"))
                    .map_err(InstallError::Io)?,
            );
            (
                state::InitializationState {
                    status: state::InitStatus::Complete,
                    lock_present: true,
                    initialized_at: snapshot.initialization.initialized_at,
                },
                state::ServiceState {
                    manager: StateServiceManager::Systemd,
                    registered: true,
                    enabled: true,
                    verified: true,
                    definition_path: Some("service/landscape-router.service".into()),
                    definition_sha256: Some(unit_sha),
                },
            )
        }
        StateServiceManager::None => (
            snapshot.initialization.clone(),
            state::ServiceState {
                manager: StateServiceManager::None,
                registered: false,
                enabled: false,
                verified: false,
                definition_path: None,
                definition_sha256: None,
            },
        ),
    };
    Ok(InstallState {
        schema_version: state::STATE_SCHEMA_VERSION,
        layout_version: state::STATE_LAYOUT_VERSION,
        install_root: root.install_root.display().to_string(),
        canonical_install_root: root.canonical.display().to_string(),
        active_version: from_version.to_string(),
        assets: state::Assets {
            webserver: state::WebserverAsset {
                architecture: match architecture {
                    backup::BackupArchitecture::X86_64 => state::StateArchitecture::X86_64,
                    backup::BackupArchitecture::Aarch64 => state::StateArchitecture::Aarch64,
                },
                sha256: webserver_sha256,
                size: webserver_size,
            },
            static_archive: snapshot.assets.static_archive.clone(),
        },
        initialization,
        service,
        last_transaction_id: Some(transaction.transaction_id.clone()),
        committed_at: Some(chrono::Utc::now()),
    })
}

pub(crate) fn restore_current(root: &InstallRoot, target: &str) -> Result<(), InstallError> {
    let current = root.canonical.join("current");
    let tmp = root.canonical.join("run/.current.tmp");
    std::fs::create_dir_all(tmp.parent().expect("run dir has a parent"))
        .map_err(InstallError::Io)?;
    let _ = std::fs::remove_file(&tmp);
    std::os::unix::fs::symlink(target, &tmp).map_err(InstallError::Io)?;
    std::fs::rename(&tmp, &current).map_err(InstallError::Io)?;
    Ok(())
}

fn copy_tree(source: &Path, target: &Path) -> Result<(), InstallError> {
    copy_tree_into(source, target)
}

/// 将 `source` 目录内容完整复制到 `target`。只允许普通文件与目录,
/// 遇到符号链接、设备、FIFO 或 socket 时失败。失败时 `target` 可能不完整。
pub(crate) fn copy_tree_into(source: &Path, target: &Path) -> Result<(), InstallError> {
    std::fs::create_dir_all(target).map_err(InstallError::Io)?;
    for entry in std::fs::read_dir(source).map_err(InstallError::Io)? {
        let entry = entry.map_err(InstallError::Io)?;
        let file_type = entry.file_type().map_err(InstallError::Io)?;
        let target_path = target.join(entry.file_name());
        if file_type.is_dir() {
            copy_tree(&entry.path(), &target_path)?;
        } else if file_type.is_file() {
            std::fs::copy(entry.path(), target_path).map_err(InstallError::Io)?;
        } else {
            return Err(InstallError::InvalidBackup(format!(
                "backup contains an unsupported entry {}",
                entry.path().display()
            )));
        }
    }
    Ok(())
}

pub(crate) fn write_file_atomic(path: &Path, bytes: &[u8], mode: u32) -> Result<(), InstallError> {
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(&tmp)
        .map_err(InstallError::Io)?;
    file.write_all(bytes).map_err(InstallError::Io)?;
    file.sync_all().map_err(InstallError::Io)?;
    std::fs::rename(&tmp, path).map_err(InstallError::Io)?;
    Ok(())
}

fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> Result<(), InstallError> {
    write_file_atomic(path, bytes, mode)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::systemd::Systemd;

    struct AlwaysHealthy;

    impl DocsProbe for AlwaysHealthy {
        async fn docs_ok(&self) -> bool {
            true
        }
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("lkit-rollback-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn moves_data_aside_on_first_attempt() {
        let dir = temp_dir("first");
        let data = dir.join("data");
        let failed = dir.join("tx/failed-data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("landscape_db.sqlite"), b"db").unwrap();
        move_data_aside(&data, &failed).unwrap();
        assert!(failed.join("landscape_db.sqlite").is_file());
        assert!(data.is_dir());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reuses_failed_data_when_previous_attempt_interrupted() {
        let dir = temp_dir("retry");
        let data = dir.join("data");
        let failed = dir.join("tx/failed-data");
        std::fs::create_dir_all(&failed).unwrap();
        std::fs::write(failed.join("landscape_db.sqlite"), b"db").unwrap();
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("partial"), b"partial").unwrap();
        move_data_aside(&data, &failed).unwrap();
        assert!(failed.join("landscape_db.sqlite").is_file());
        assert!(!data.join("partial").exists());
        assert!(data.is_dir());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn handles_missing_data() {
        let dir = temp_dir("missing");
        let data = dir.join("data");
        let failed = dir.join("tx/failed-data");
        move_data_aside(&data, &failed).unwrap();
        assert!(data.is_dir());
        assert!(!failed.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn restores_release_config_and_geo_from_lkb_instead_of_live_files() {
        let dir = temp_dir("lkb-source");
        let root = InstallRoot {
            install_root: dir.clone(),
            canonical: dir.clone(),
        };
        let from_version = semver::Version::new(1, 2, 3);
        let target_version = semver::Version::new(2, 0, 0);

        let backup_source = dir.join("backup-source");
        let backup_binary = backup_source.join("landscape-webserver");
        let backup_static = backup_source.join("static");
        let backup_zip = backup_source.join("static.zip");
        let backup_geo = backup_source.join("geo_tmp");
        std::fs::create_dir_all(backup_static.join("assets")).unwrap();
        std::fs::create_dir_all(backup_geo.join("ip")).unwrap();
        std::fs::write(&backup_binary, b"binary-from-lkb").unwrap();
        std::fs::write(&backup_zip, b"zip-from-lkb").unwrap();
        std::fs::write(backup_static.join("index.html"), b"static-from-lkb").unwrap();
        std::fs::write(backup_static.join("assets/app.js"), b"asset-from-lkb").unwrap();
        std::fs::write(backup_geo.join("ip/geo.dat"), b"geo-from-lkb").unwrap();
        let backup_ref = backup::create_backup(
            &dir.join("backups"),
            &from_version,
            "x86_64",
            &backup_binary,
            "init_config_from_lkb = true\n",
            &backup_static,
            &backup_zip,
            &backup_geo,
            "",
            true,
            None,
        )
        .unwrap();

        let old_release = dir.join("releases/1.2.3");
        std::fs::create_dir_all(old_release.join("static/assets")).unwrap();
        std::fs::write(old_release.join("landscape-webserver"), b"polluted-binary").unwrap();
        std::fs::write(old_release.join("static/index.html"), b"polluted-static").unwrap();
        std::fs::write(old_release.join("static/assets/app.js"), b"polluted-asset").unwrap();
        std::fs::create_dir_all(dir.join("releases/2.0.0")).unwrap();
        std::os::unix::fs::symlink("releases/2.0.0", dir.join("current")).unwrap();

        let live_geo = dir.join("data/geo_tmp/ip");
        std::fs::create_dir_all(&live_geo).unwrap();
        std::fs::write(live_geo.join("geo.dat"), b"polluted-geo").unwrap();
        std::fs::write(dir.join("data/landscape_init.toml"), b"polluted-init\n").unwrap();

        let (webserver_sha256, webserver_size) = hash_file(&backup_binary).unwrap();
        let snapshot = state::InstallState {
            schema_version: state::STATE_SCHEMA_VERSION,
            layout_version: state::STATE_LAYOUT_VERSION,
            install_root: dir.display().to_string(),
            canonical_install_root: dir.display().to_string(),
            active_version: from_version.to_string(),
            assets: state::Assets {
                webserver: state::WebserverAsset {
                    architecture: state::StateArchitecture::X86_64,
                    sha256: webserver_sha256,
                    size: webserver_size,
                },
                static_archive: state::ArchiveAsset {
                    sha256: "a".repeat(64),
                    size: 1,
                },
            },
            initialization: state::InitializationState {
                status: state::InitStatus::Pending,
                lock_present: false,
                initialized_at: None,
            },
            service: state::ServiceState {
                manager: StateServiceManager::None,
                registered: false,
                enabled: false,
                verified: false,
                definition_path: None,
                definition_sha256: None,
            },
            last_transaction_id: None,
            committed_at: Some(chrono::Utc::now()),
        };

        let mut transaction =
            TransactionFile::new_switch(&root, &from_version, &target_version).unwrap();
        transaction.backup = Some(backup_ref);
        super::super::transaction::begin(&root, &transaction).unwrap();
        let health = HealthOptions {
            docs: AlwaysHealthy,
            ports: Vec::new(),
            startup_timeout: std::time::Duration::from_secs(1),
            stable_duration: std::time::Duration::from_millis(1),
        };

        rollback_switch(&root, &transaction, &snapshot, &Systemd::host(), &health)
            .await
            .unwrap();

        assert_eq!(
            std::fs::read(old_release.join("landscape-webserver")).unwrap(),
            b"binary-from-lkb"
        );
        assert_eq!(
            std::fs::read(old_release.join("static/index.html")).unwrap(),
            b"static-from-lkb"
        );
        assert_eq!(
            std::fs::read(old_release.join("static/assets/app.js")).unwrap(),
            b"asset-from-lkb"
        );
        assert_eq!(
            std::fs::read(old_release.join("static.zip")).unwrap(),
            b"zip-from-lkb"
        );
        assert_eq!(
            std::fs::read(dir.join("data/geo_tmp/ip/geo.dat")).unwrap(),
            b"geo-from-lkb"
        );
        assert_eq!(
            std::fs::read(dir.join("data/landscape_init.toml")).unwrap(),
            b"init_config_from_lkb = true\n"
        );
        assert_eq!(
            std::fs::read_link(dir.join("current")).unwrap(),
            std::path::PathBuf::from("releases/1.2.3")
        );

        let tx_dir = dir.join("transactions").join(&transaction.transaction_id);
        assert_eq!(
            std::fs::read(tx_dir.join("replaced-release/landscape-webserver")).unwrap(),
            b"polluted-binary"
        );
        assert_eq!(
            std::fs::read(tx_dir.join("failed-data/geo_tmp/ip/geo.dat")).unwrap(),
            b"polluted-geo"
        );
        let recorded: TransactionFile = serde_json::from_slice(
            &std::fs::read(
                dir.join("transactions")
                    .join(format!("{}.json", transaction.transaction_id)),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(recorded.phase, Phase::RolledBack);
        assert!(
            super::super::transaction::find_unfinished(&root)
                .unwrap()
                .is_none()
        );
        let restored = state::load_state(&root).unwrap().unwrap();
        assert_eq!(restored.active_version, "1.2.3");
        assert_eq!(
            restored.last_transaction_id,
            Some(transaction.transaction_id)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
