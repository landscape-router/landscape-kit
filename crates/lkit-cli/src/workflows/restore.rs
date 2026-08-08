use std::path::{Path, PathBuf};

use chrono::Utc;

use super::artifacts::{WEBSERVER_BINARY, hash_file, hash_str};
use super::backup::{self, BackupArchitecture, BackupMetadata};
use super::export;
use super::health::{DocsProbe, HealthOptions, StartupOptions};
use super::pipeline;
use super::plan::InstallError;
use super::rollback;
use super::root::InstallRoot;
use super::state::{
    self, ArchiveAsset, Assets, InitStatus, InitializationState, InstallState, ServiceState,
    StateArchitecture, StateServiceManager, WebserverAsset,
};
use super::systemd::Systemd;
use super::transaction::{BackupRef, Phase, TransactionFile};

/// restore 运行参数。
pub(crate) struct RestoreArgs {
    /// `--backup <ID>` 只解析安装根目录 `backups/` 下的备份 ID。
    pub backup_id: Option<String>,
    /// `--file <PATH>` 用于外部复制的备份,先复制进事务目录再校验。
    pub file_path: Option<PathBuf>,
    /// 允许在保护备份无法创建时继续,不产生可移植的当前配置快照。
    pub allow_no_backup: bool,
    /// 非交互模式必须显式 `--yes`,否则返回参数错误。
    pub yes: bool,
}

#[derive(Debug)]
pub(crate) enum RestoreOutcome {
    Committed {
        version: semver::Version,
        backup_id: String,
    },
    RolledBack {
        version: semver::Version,
    },
    RollbackFailed {
        version: semver::Version,
        reason: String,
    },
}

/// restore 运行参数(测试可注入)。
pub(crate) struct RestoreOptions<'a, P: DocsProbe> {
    pub export_base_url: String,
    pub token: &'a dyn Fn() -> Result<String, InstallError>,
    pub confirm: &'a dyn Fn(&str) -> Result<bool, InstallError>,
    pub health: &'a HealthOptions<P>,
}

/// 从 `.lkb` 恢复指定版本。目标备份在停止服务前完整校验;
/// 默认创建当前实例的保护 `.lkb`。失败回滚优先使用事务目录中的旧 data 现场,
/// 必要时使用保护备份。
pub(crate) async fn restore_version<P: DocsProbe>(
    root: &InstallRoot,
    state: &InstallState,
    systemd: &Systemd,
    args: &RestoreArgs,
    options: &RestoreOptions<'_, P>,
) -> Result<RestoreOutcome, InstallError> {
    let is_systemd = state.service.manager == StateServiceManager::Systemd;
    let (bytes, file_sha256) = resolve_target_backup(root, args)?;
    let metadata = backup::verify_lkb(&bytes)?;
    check_architecture(state, &metadata)?;
    let from_version = pipeline::parse_stable_version(&state.active_version).map_err(|error| {
        InstallError::CorruptedState(format!("invalid active version: {error}"))
    })?;
    let target_version = pipeline::parse_stable_version(&metadata.landscape_version)
        .map_err(|error| InstallError::InvalidBackup(format!("invalid backup version: {error}")))?;

    // 确认先于事务创建:拒绝或缺少 `--yes` 时不创建事务、不写任何文件,
    // `--file` 也不产生暂存拷贝,现场保持不变。
    confirm_restore(options, args, state, &metadata)?;

    let mut transaction = TransactionFile::new_restore(root, &from_version, &target_version)?;
    super::transaction::begin(root, &transaction)?;
    let tx_dir = root
        .canonical
        .join("transactions")
        .join(&transaction.transaction_id);

    // 外部备份先复制进事务目录并重新自校验,事务只记录安装根目录内的相对路径。
    let target_backup = match (&args.backup_id, &args.file_path) {
        (Some(id), None) => BackupRef {
            backup_id: id.clone(),
            path: format!("backups/{id}.lkb"),
            sha256: file_sha256,
        },
        (None, Some(_)) => {
            backup::create_secure_dir(&tx_dir, 0o700)?;
            let copied = tx_dir.join("target-backup.lkb");
            write_file_atomic(&copied, &bytes, 0o600)?;
            let copied_bytes = std::fs::read(&copied).map_err(InstallError::Io)?;
            backup::verify_lkb(&copied_bytes)?;
            let (copied_sha256, _) = hash_file(&copied)?;
            BackupRef {
                backup_id: metadata.backup_id.clone(),
                path: format!(
                    "transactions/{}/target-backup.lkb",
                    transaction.transaction_id
                ),
                sha256: copied_sha256,
            }
        }
        _ => {
            return Err(InstallError::ParameterUsage(
                "--backup and --file cannot be combined".into(),
            ));
        }
    };
    transaction.restore_backup = Some(target_backup);
    super::transaction::persist(root, &transaction)?;

    // 保护备份失败时保持现场不变;`--allow-no-backup` 才允许继续。
    if let Err(error) = create_protection_backup(root, state, &mut transaction, options).await {
        if !args.allow_no_backup {
            let _ = super::transaction::mark_phase(root, &transaction, Phase::Failed);
            return Err(error);
        }
        transaction.no_backup = true;
        eprintln!(
            "install: {}",
            crate::tr!(
                crate::keys::RESTORE_WARNING_NO_PROTECTION_BACKUP,
                error = error
            )
        );
    }
    super::transaction::persist(root, &transaction)?;

    let unit_sha = if is_systemd {
        transaction.systemd_before = Some(pipeline::capture_systemd_before(systemd)?);
        let backup_dir = root
            .canonical
            .join("backups")
            .join(&transaction.transaction_id)
            .join("host/resolv.conf");
        let _ = super::resolv::backup(&systemd.resolv_conf, &backup_dir)?;
        transaction.resolv_conf_backup = Some(format!(
            "backups/{}/host/resolv.conf",
            transaction.transaction_id
        ));
        super::transaction::persist(root, &transaction)?;
        Some(hash_str(
            &std::fs::read_to_string(root.canonical.join("service/landscape-router.service"))
                .map_err(InstallError::Io)?,
        ))
    } else {
        None
    };
    rollback::write_state_snapshot(root, &transaction.transaction_id, state)?;

    // 停止服务前完成安全解包与完整内容校验(必需条目、权限 0700/0600):
    // 解包失败时服务与现场均未改变,事务直接标记 failed。
    let restore_dir = tx_dir.join("restore");
    let _ = std::fs::remove_dir_all(&restore_dir);
    if let Err(error) = backup::extract_lkb(&bytes, &restore_dir) {
        let _ = super::transaction::mark_phase(root, &transaction, Phase::Failed);
        return Err(error);
    }
    super::transaction::mark_phase(root, &transaction, Phase::Prepared)?;

    let mut activated = false;
    let result: Result<(), InstallError> = async {
        if is_systemd {
            super::transaction::mark_phase(root, &transaction, Phase::Stopping)?;
            super::systemd::stop_and_wait(systemd, || {
                super::systemd::active_state(systemd)
                    .map(|value| value != "active")
                    .unwrap_or(true)
            })?;
        } else if !crate::interaction::interactive::is_non_interactive() {
            // 非交互模式的「外部实例已停止」确认由 `--yes` 覆盖(见 confirm_restore)。
            let accepted = (options.confirm)(&crate::tr!(
                crate::keys::RESTORE_CONFIRM_STOP_WITH_OWN_MANAGER
            ))?;
            if !accepted {
                return Err(InstallError::UserRefused(
                    "user refused to stop the running instance".into(),
                ));
            }
        }
        super::transaction::mark_phase(root, &transaction, Phase::Activating)?;
        activated = true;
        std::fs::create_dir_all(&tx_dir).map_err(InstallError::Io)?;
        rollback::move_data_aside(&root.canonical.join("data"), &tx_dir.join("previous-data"))?;
        rollback::rebuild_release_from_backup(
            root,
            &tx_dir,
            &target_version.to_string(),
            &restore_dir,
        )?;
        let data = root.canonical.join("data");
        let geo_tmp_source = restore_dir.join("geo_tmp");
        if geo_tmp_source.is_dir() {
            rollback::copy_tree_into(&geo_tmp_source, &data.join("geo_tmp"))?;
        }
        let init_config =
            std::fs::read(restore_dir.join("landscape_init.toml")).map_err(InstallError::Io)?;
        write_file_atomic(&data.join("landscape_init.toml"), &init_config, 0o600)?;
        rollback::restore_current(root, &format!("releases/{target_version}"))?;
        if is_systemd {
            super::systemd::register(
                systemd,
                &root.canonical.join("service/landscape-router.service"),
            )?;
            super::systemd::enable(systemd)?;
            super::systemd::start(systemd)?;
            super::transaction::mark_phase(root, &transaction, Phase::Verifying)?;
            let pid = super::systemd::main_pid(systemd)?;
            if pid == 0 {
                return Err(InstallError::Systemd(
                    "service did not produce a main pid after start".into(),
                ));
            }
            let startup = StartupOptions {
                ports: &options.health.ports,
                expected_pid: pid,
                docs: &options.health.docs,
                unit_state: Some(&(|| super::systemd::active_state(systemd).ok())),
                init_required: true,
                data_dir: &data,
                startup_timeout: options.health.startup_timeout,
                stable_duration: options.health.stable_duration,
            };
            super::health::wait_for_startup(&startup).await?;
            super::health::observe_stable(&startup).await?;
        }
        let new_state =
            build_restore_state(root, state, &transaction, &metadata, &restore_dir, unit_sha)?;
        super::state::write_state(root, &new_state)?;
        super::transaction::mark_phase(root, &transaction, Phase::Committed)?;
        Ok(())
    }
    .await;

    match result {
        Ok(()) => Ok(RestoreOutcome::Committed {
            version: target_version,
            backup_id: metadata.backup_id,
        }),
        Err(error) if is_systemd && activated => {
            match rollback_restore(root, &transaction, systemd, options.health).await {
                Ok(()) => Ok(RestoreOutcome::RolledBack {
                    version: from_version,
                }),
                Err(rollback_error) => {
                    eprintln!(
                        "install: {}",
                        crate::tr!(
                            crate::keys::RESTORE_ROLLBACK_FAILED,
                            rollback_error = rollback_error
                        )
                    );
                    Ok(RestoreOutcome::RollbackFailed {
                        version: from_version,
                        reason: error.to_string(),
                    })
                }
            }
        }
        Err(error) => {
            if !activated {
                // 激活前失败:可能发生在停止服务阶段,服务状态可能已改变;
                // 先按 systemd_before 恢复 unit 注册与 enabled/active 状态,再标记 failed。
                let mut systemd_restored = true;
                if let Some(before) = &transaction.systemd_before {
                    let unit_origin = root.canonical.join("service/landscape-router.service");
                    let restore_error =
                        super::systemd::restore_systemd_before(systemd, before, &unit_origin)
                            .and_then(|()| {
                                if let Some(backup_path) = &transaction.resolv_conf_backup {
                                    let backup_dir = root.canonical.join(backup_path);
                                    super::resolv::restore(&systemd.resolv_conf, &backup_dir)
                                } else {
                                    Ok(())
                                }
                            });
                    if let Err(restore_error) = restore_error {
                        systemd_restored = false;
                        eprintln!(
                            "install: {}",
                            crate::tr!(
                                crate::keys::RESTORE_ROLLBACK_FAILED,
                                rollback_error = restore_error
                            )
                        );
                    }
                }
                let _ = super::transaction::mark_phase(root, &transaction, Phase::Failed);
                if !systemd_restored {
                    // 服务状态恢复也失败:事务已终结且服务可能未恢复,
                    // 按自动恢复失败处理(退出码 6),不能按普通失败返回。
                    return Ok(RestoreOutcome::RollbackFailed {
                        version: from_version,
                        reason: error.to_string(),
                    });
                }
            }
            Err(error)
        }
    }
}

/// 解析目标备份:ID 只解析安装根目录 `backups/`,外部文件必须 root 所有、
/// 权限不宽于 `0600` 的普通文件。返回完整字节与文件级 SHA-256。
fn resolve_target_backup(
    root: &InstallRoot,
    args: &RestoreArgs,
) -> Result<(Vec<u8>, String), InstallError> {
    match (&args.backup_id, &args.file_path) {
        (Some(id), None) => {
            if !backup::backup_id_format_ok(id) {
                return Err(InstallError::ParameterUsage(format!(
                    "--backup {id} does not match the backup ID format YYYYMMDD-HHMMSS-<8 lowercase hex>"
                )));
            }
            let path = root.canonical.join("backups").join(format!("{id}.lkb"));
            if !path.is_file() {
                return Err(InstallError::InvalidBackup(format!(
                    "backup {id} not found under {}",
                    root.canonical.join("backups").display()
                )));
            }
            validate_backup_file(&path)?;
            let bytes = std::fs::read(&path).map_err(InstallError::Io)?;
            let (sha256, _) = hash_file(&path)?;
            Ok((bytes, sha256))
        }
        (None, Some(path)) => {
            validate_backup_file(path)?;
            let bytes = std::fs::read(path).map_err(InstallError::Io)?;
            let (sha256, _) = hash_file(path)?;
            Ok((bytes, sha256))
        }
        _ => Err(InstallError::ParameterUsage(
            "--backup and --file cannot be combined; one of them is required".into(),
        )),
    }
}

/// `.lkb` 文件:必须是 root 所有、权限不宽于 `0600` 的普通文件,
/// 不跟随符号链接。用于外部文件与安装根目录内的备份。
pub(crate) fn validate_backup_file(path: &Path) -> Result<(), InstallError> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        InstallError::InvalidBackup(format!(
            "{} is not a readable regular file: {error}",
            path.display()
        ))
    })?;
    if !metadata.file_type().is_file() {
        return Err(InstallError::InvalidBackup(format!(
            "{} is not a regular file",
            path.display()
        )));
    }
    let uid = unsafe { libc::geteuid() };
    if metadata.uid() != uid {
        return Err(InstallError::InvalidBackup(format!(
            "{} must be owned by uid {uid}",
            path.display()
        )));
    }
    let mode = metadata.mode() & 0o777;
    if mode & !0o600 != 0 {
        return Err(InstallError::InvalidBackup(format!(
            "{} must not be broader than 0600",
            path.display()
        )));
    }
    Ok(())
}

fn check_architecture(state: &InstallState, metadata: &BackupMetadata) -> Result<(), InstallError> {
    let host_arch = std::env::consts::ARCH;
    let backup_arch = match metadata.architecture {
        BackupArchitecture::X86_64 => "x86_64",
        BackupArchitecture::Aarch64 => "aarch64",
    };
    if host_arch != backup_arch {
        return Err(InstallError::InvalidBackup(format!(
            "backup architecture {backup_arch} does not match the host {host_arch}"
        )));
    }
    let state_arch = match state.assets.webserver.architecture {
        StateArchitecture::X86_64 => "x86_64",
        StateArchitecture::Aarch64 => "aarch64",
    };
    if state_arch != backup_arch {
        return Err(InstallError::InvalidBackup(format!(
            "backup architecture {backup_arch} does not match the installation {state_arch}"
        )));
    }
    Ok(())
}

/// 交互模式确认当前版本、目标版本、备份 ID 和 minimal scope 的数据损失;
/// 非交互模式必须显式 `--yes`。
fn confirm_restore<P: DocsProbe>(
    options: &RestoreOptions<'_, P>,
    args: &RestoreArgs,
    state: &InstallState,
    metadata: &BackupMetadata,
) -> Result<(), InstallError> {
    let current = state.active_version.clone();
    let target = metadata.landscape_version.clone();
    if crate::interaction::interactive::is_non_interactive() {
        if !args.yes {
            return Err(InstallError::ParameterUsage(
                "--yes is required in non-interactive mode to confirm the restore".into(),
            ));
        }
        return Ok(());
    }
    let accepted = (options.confirm)(&crate::tr!(
        crate::keys::RESTORE_CONFIRM_PLAN,
        current = current,
        target = target,
        backup_id = metadata.backup_id
    ))?;
    if !accepted {
        return Err(InstallError::UserRefused(
            "user refused the restore plan".into(),
        ));
    }
    let accepted = (options.confirm)(&crate::tr!(crate::keys::RESTORE_CONFIRM_MINIMAL_SCOPE))?;
    if !accepted {
        return Err(InstallError::UserRefused(
            "user refused the restore plan".into(),
        ));
    }
    Ok(())
}

async fn create_protection_backup<P: DocsProbe>(
    root: &InstallRoot,
    state: &InstallState,
    transaction: &mut TransactionFile,
    options: &RestoreOptions<'_, P>,
) -> Result<(), InstallError> {
    pipeline::check_initialization(root, state)?;
    pipeline::verify_current_backend(root, state)?;
    let token = (options.token)()?;
    let exported = export::export_config(&options.export_base_url, &token).await?;
    if exported.version != state.active_version {
        return Err(InstallError::ExportFailed(format!(
            "exported version {} does not match the running version {}",
            exported.version, state.active_version
        )));
    }
    let architecture = match state.assets.webserver.architecture {
        StateArchitecture::X86_64 => "x86_64",
        StateArchitecture::Aarch64 => "aarch64",
    };
    let version = pipeline::parse_stable_version(&state.active_version).map_err(|error| {
        InstallError::CorruptedState(format!("invalid active version: {error}"))
    })?;
    let webserver = root
        .canonical
        .join("releases")
        .join(&state.active_version)
        .join(WEBSERVER_BINARY);
    let static_dir = root.canonical.join("current/static");
    let static_archive = root
        .canonical
        .join("releases")
        .join(&state.active_version)
        .join("static.zip");
    let geo_tmp = root.canonical.join("data/geo_tmp");
    let backup_ref = backup::create_backup(
        &root.canonical.join("backups"),
        &version,
        architecture,
        &webserver,
        &exported.content,
        &static_dir,
        &static_archive,
        &geo_tmp,
        "",
        true,
    )?;
    transaction.backup = Some(backup_ref);
    Ok(())
}

/// 恢复提交的 state:`repository` 沿用当前安装,`webserver` 与 `static_archive`
/// 身份分别从解包二进制和备份内压缩包现场计算。
fn build_restore_state(
    root: &InstallRoot,
    previous: &InstallState,
    transaction: &TransactionFile,
    metadata: &BackupMetadata,
    restore_dir: &Path,
    unit_sha: Option<String>,
) -> Result<InstallState, InstallError> {
    let binary = restore_dir.join("landscape-webserver");
    let (webserver_sha256, webserver_size) = hash_file(&binary)?;
    let static_zip = restore_dir.join("static.zip");
    let (static_sha256, static_size) = hash_file(&static_zip)?;
    let architecture = match metadata.architecture {
        BackupArchitecture::X86_64 => StateArchitecture::X86_64,
        BackupArchitecture::Aarch64 => StateArchitecture::Aarch64,
    };
    let (initialization, service) = match previous.service.manager {
        StateServiceManager::Systemd => (
            InitializationState {
                status: InitStatus::Complete,
                lock_present: true,
                initialized_at: Some(Utc::now()),
            },
            ServiceState {
                manager: StateServiceManager::Systemd,
                registered: true,
                enabled: true,
                verified: true,
                definition_path: Some("service/landscape-router.service".into()),
                definition_sha256: unit_sha,
            },
        ),
        StateServiceManager::None => (
            InitializationState {
                status: InitStatus::Pending,
                lock_present: false,
                initialized_at: None,
            },
            ServiceState {
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
        active_version: metadata.landscape_version.clone(),
        assets: Assets {
            webserver: WebserverAsset {
                architecture,
                sha256: webserver_sha256,
                size: webserver_size,
            },
            static_archive: ArchiveAsset {
                sha256: static_sha256,
                size: static_size,
            },
        },
        initialization,
        service,
        last_transaction_id: Some(transaction.transaction_id.clone()),
        committed_at: Some(Utc::now()),
    })
}

/// restore 失败回滚:优先用事务目录中的旧 `data/`、previous-state、
/// `previous_current`、`systemd_before` 和 `resolv_conf_backup` 恢复原安装,
/// 必要时使用保护 `.lkb` 做配置级重建。
///
/// systemd 回滚顺序固定为:停止服务 → 恢复注册/enabled(不启动) → 同版本时移回
/// 原 release → 恢复 `current` → 恢复 `data/` → 恢复前服务活跃时启动并做健康检查
/// → 重新提交恢复前 state。启动必须发生在 current/data 恢复之后。
///
/// 回滚任一步失败时统一把事务标记为 `failed`(退出码 `6` 语义),不留在 `rolling_back`
/// 让下一条命令再次自动尝试回滚。
pub(crate) async fn rollback_restore<P: DocsProbe>(
    root: &InstallRoot,
    transaction: &TransactionFile,
    systemd: &Systemd,
    health: &HealthOptions<P>,
) -> Result<(), InstallError> {
    super::transaction::mark_phase(root, transaction, Phase::RollingBack)?;
    let result = rollback_restore_inner(root, transaction, systemd, health).await;
    if result.is_err() {
        let _ = super::transaction::mark_phase(root, transaction, Phase::Failed);
    }
    result
}

async fn rollback_restore_inner<P: DocsProbe>(
    root: &InstallRoot,
    transaction: &TransactionFile,
    systemd: &Systemd,
    health: &HealthOptions<P>,
) -> Result<(), InstallError> {
    let snapshot = rollback::read_state_snapshot(root, &transaction.transaction_id)?;
    let is_systemd = snapshot.service.manager == StateServiceManager::Systemd;
    if is_systemd {
        super::systemd::stop_and_wait(systemd, || {
            super::systemd::active_state(systemd)
                .map(|value| value != "active")
                .unwrap_or(true)
        })?;
    }
    if let Some(before) = &transaction.systemd_before {
        let unit_origin = root.canonical.join("service/landscape-router.service");
        super::systemd::restore_systemd_registration(systemd, before, &unit_origin)?;
        if let Some(backup_path) = &transaction.resolv_conf_backup {
            let backup_dir = root.canonical.join(backup_path);
            super::resolv::restore(&systemd.resolv_conf, &backup_dir)?;
        }
    }

    let tx_dir = root
        .canonical
        .join("transactions")
        .join(&transaction.transaction_id);
    let data = root.canonical.join("data");
    let previous_data = tx_dir.join("previous-data");
    if previous_data.exists() || data.exists() {
        // 原现场存在(previous-data 未消费),或上次回滚已经恢复 data
        // (previous-data 已消费、data 在):两种情况下都只做幂等恢复。
        restore_replaced_release_if_same_version(root, transaction)?;
        let previous_current = transaction.previous_current.as_deref().ok_or_else(|| {
            InstallError::CorruptedTransaction(
                "restore transaction is missing previous_current".into(),
            )
        })?;
        rollback::restore_current(root, previous_current)?;
        if previous_data.exists() {
            restore_previous_data(&data, &previous_data)?;
        }
        let was_active = transaction
            .systemd_before
            .as_ref()
            .is_some_and(|before| before.active);
        if is_systemd && was_active {
            super::systemd::start(systemd)?;
            let pid = super::systemd::main_pid(systemd)?;
            if pid == 0 {
                return Err(InstallError::Systemd(
                    "restored service did not produce a main pid".into(),
                ));
            }
            let startup = StartupOptions {
                ports: &health.ports,
                expected_pid: pid,
                docs: &health.docs,
                unit_state: Some(&(|| super::systemd::active_state(systemd).ok())),
                init_required: true,
                data_dir: &data,
                startup_timeout: health.startup_timeout,
                stable_duration: health.stable_duration,
            };
            super::health::wait_for_startup(&startup).await?;
            super::health::observe_stable(&startup).await?;
        }
        let mut restored = snapshot.clone();
        restored.last_transaction_id = Some(transaction.transaction_id.clone());
        restored.committed_at = Some(Utc::now());
        super::state::write_state(root, &restored)?;
        super::transaction::mark_phase(root, transaction, Phase::RolledBack)?;
        Ok(())
    } else {
        // 事务现场损坏(previous-data 与 data 均不存在):只能使用保护快照或报损坏。
        if transaction.backup.is_some() {
            super::rollback::rollback_switch(root, transaction, &snapshot, systemd, health).await
        } else {
            super::rollback::rollback_no_backup(root, transaction, &snapshot, systemd, health).await
        }
    }
}

/// 同版本 restore(`previous_current` 与 `target_release` 相同)回滚时,
/// 把被 `rebuild_release_from_backup` 移入事务目录 `replaced-release` 的原 release
/// 移回 `releases/<版本>`,确保回滚后的 release 内容与回滚前完全一致。
/// 必须在 `restore_current` 之前调用。
fn restore_replaced_release_if_same_version(
    root: &InstallRoot,
    transaction: &TransactionFile,
) -> Result<(), InstallError> {
    if transaction.previous_current.as_deref() != transaction.target_release.as_deref() {
        return Ok(());
    }
    let tx_dir = root
        .canonical
        .join("transactions")
        .join(&transaction.transaction_id);
    let replaced = tx_dir.join("replaced-release");
    if !replaced.is_dir() {
        return Ok(());
    }
    let target = transaction.target_release.as_deref().ok_or_else(|| {
        InstallError::CorruptedTransaction("restore transaction is missing target_release".into())
    })?;
    let release_dir = root.canonical.join(target);
    if release_dir.exists() {
        std::fs::remove_dir_all(&release_dir).map_err(InstallError::Io)?;
    }
    std::fs::rename(&replaced, &release_dir).map_err(InstallError::Io)?;
    Ok(())
}

/// 将事务目录中的旧 data 恢复为当前 data。幂等:
/// 丢弃回滚中断时残留的部分新 data,再把 previous-data 移回原位;
/// previous-data 已被消费、data 已恢复时直接视为已完成,不得再次删除 data。
fn restore_previous_data(data: &Path, previous_data: &Path) -> Result<(), InstallError> {
    if previous_data.exists() {
        if data.exists() {
            std::fs::remove_dir_all(data).map_err(InstallError::Io)?;
        }
        std::fs::rename(previous_data, data).map_err(InstallError::Io)?;
    } else if !data.exists() {
        return Err(InstallError::CorruptedTransaction(format!(
            "neither {} nor {} exists; cannot restore previous data",
            data.display(),
            previous_data.display()
        )));
    }
    std::fs::create_dir_all(data).map_err(InstallError::Io)?;
    Ok(())
}

fn write_file_atomic(path: &Path, bytes: &[u8], mode: u32) -> Result<(), InstallError> {
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
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

#[cfg(test)]
mod tests {
    use super::super::health::HealthOptions;
    use super::super::repository::test_server::{TestResponse, TestServer};
    use super::super::state::{
        ArchiveAsset, Assets, InitializationState, ServiceState, StateArchitecture,
        StateServiceManager, WebserverAsset,
    };
    use super::*;

    const PAYLOAD_1_2_3: &[u8] = b"webserver payload 1.2.3";
    const PAYLOAD_1_3_0: &[u8] = b"webserver payload 1.3.0";
    const ZIP_1_2_3: &[u8] = b"zip payload 1.2.3";
    const ZIP_1_3_0: &[u8] = b"zip payload 1.3.0";

    fn temp_root(name: &str) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("lkit-restore-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn sha256_bytes(bytes: &[u8]) -> (String, u64) {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let hex = hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        (hex, bytes.len() as u64)
    }

    fn activate_version(root: &InstallRoot, version: &str, payload: &[u8], zip: &[u8]) {
        let release = root.canonical.join("releases").join(version);
        std::fs::create_dir_all(release.join("static")).unwrap();
        std::fs::write(release.join("landscape-webserver"), payload).unwrap();
        std::fs::write(release.join("static.zip"), zip).unwrap();
        std::fs::write(
            release.join("static/index.html"),
            format!("static {version}"),
        )
        .unwrap();
        let _ = std::fs::remove_file(root.canonical.join("current"));
        std::os::unix::fs::symlink(
            format!("releases/{version}"),
            root.canonical.join("current"),
        )
        .unwrap();
    }

    fn install_state(
        root: &InstallRoot,
        version: &str,
        payload: &[u8],
        zip: &[u8],
    ) -> InstallState {
        let (webserver_sha, webserver_size) = sha256_bytes(payload);
        let (static_sha, static_size) = sha256_bytes(zip);
        InstallState {
            schema_version: 1,
            layout_version: 1,
            install_root: root.install_root.display().to_string(),
            canonical_install_root: root.canonical.display().to_string(),
            active_version: version.into(),
            assets: Assets {
                webserver: WebserverAsset {
                    architecture: StateArchitecture::X86_64,
                    sha256: webserver_sha,
                    size: webserver_size,
                },
                static_archive: ArchiveAsset {
                    sha256: static_sha,
                    size: static_size,
                },
            },
            initialization: InitializationState {
                status: InitStatus::Complete,
                lock_present: true,
                initialized_at: Some(Utc::now()),
            },
            service: ServiceState {
                manager: StateServiceManager::None,
                registered: false,
                enabled: false,
                verified: false,
                definition_path: None,
                definition_sha256: None,
            },
            last_transaction_id: None,
            committed_at: Some(Utc::now()),
        }
    }

    static BACKUP_SOURCE_COUNTER: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(0);

    fn create_target_backup(root: &InstallRoot) -> (BackupRef, Vec<u8>) {
        let source = temp_root(&format!(
            "backup-source-{}",
            BACKUP_SOURCE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let binary = source.join("landscape-webserver");
        let static_dir = source.join("static");
        let zip = source.join("static.zip");
        let geo = source.join("geo_tmp");
        std::fs::create_dir_all(static_dir.join("assets")).unwrap();
        std::fs::create_dir_all(geo.join("ip")).unwrap();
        std::fs::write(&binary, PAYLOAD_1_2_3).unwrap();
        std::fs::write(&zip, ZIP_1_2_3).unwrap();
        std::fs::write(static_dir.join("index.html"), "static 1.2.3").unwrap();
        std::fs::write(geo.join("ip/geo.dat"), "geo 1.2.3").unwrap();
        let backup_ref = backup::create_backup(
            &root.canonical.join("backups"),
            &semver::Version::new(1, 2, 3),
            "x86_64",
            &binary,
            "version = \"1.2.3\"\n",
            &static_dir,
            &zip,
            &geo,
            "manual backup",
            false,
        )
        .unwrap();
        let bytes = std::fs::read(
            root.canonical
                .join("backups")
                .join(format!("{}.lkb", backup_ref.backup_id)),
        )
        .unwrap();
        let _ = std::fs::remove_dir_all(&source);
        (backup_ref, bytes)
    }

    struct FakeDocs;

    impl DocsProbe for FakeDocs {
        async fn docs_ok(&self) -> bool {
            true
        }
    }

    fn none_health() -> HealthOptions<FakeDocs> {
        HealthOptions {
            docs: FakeDocs,
            ports: Vec::new(),
            startup_timeout: std::time::Duration::from_secs(5),
            stable_duration: std::time::Duration::from_millis(100),
        }
    }

    static YES: fn(&str) -> Result<bool, InstallError> = |_| Ok(true);
    static TOKEN: fn() -> Result<String, InstallError> = || Ok("tok".into());

    /// `is_non_interactive()` 是进程级全局状态,并发 tokio 测试会互相干扰;
    /// 涉及交互模式的测试必须串行执行。
    static INTERACTIVE_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    async fn interactive_guard() -> tokio::sync::MutexGuard<'static, ()> {
        INTERACTIVE_MUTEX.lock().await
    }

    fn export_server(version: String) -> TestServer {
        TestServer::start(move |path| {
            if path == export::EXPORT_PATH {
                TestResponse::ok(format!(
                    r#"{{"data":{{"filename":"landscape_init_v{version}.toml","version":"{version}","content":"version = \"{version}\"\n"}}}}"#
                ).into_bytes())
            } else {
                TestResponse::status(404, "Not Found", Vec::new())
            }
        })
    }

    fn setup_current(root: &InstallRoot) {
        std::fs::create_dir_all(root.canonical.join("data")).unwrap();
        std::fs::write(root.canonical.join("data/landscape_init.lock"), b"").unwrap();
        std::fs::write(root.canonical.join("data/landscape.toml"), b"").unwrap();
    }

    #[tokio::test]
    async fn restores_cross_version_without_systemd() {
        let _guard = interactive_guard().await;
        crate::interaction::interactive::configure(false);
        let root = temp_root("cross-version");
        let install_root = InstallRoot {
            install_root: root.clone(),
            canonical: root.clone(),
        };
        activate_version(&install_root, "1.3.0", PAYLOAD_1_3_0, ZIP_1_3_0);
        setup_current(&install_root);
        let state = install_state(&install_root, "1.3.0", PAYLOAD_1_3_0, ZIP_1_3_0);
        super::super::state::write_state(&install_root, &state).unwrap();
        let (backup_ref, _) = create_target_backup(&install_root);
        let server = export_server("1.3.0".into());

        let options = RestoreOptions {
            export_base_url: server.base.clone(),
            token: &TOKEN,
            confirm: &YES,
            health: &none_health(),
        };
        let args = RestoreArgs {
            backup_id: Some(backup_ref.backup_id.clone()),
            file_path: None,
            allow_no_backup: false,
            yes: false,
        };
        let outcome = restore_version(&install_root, &state, &Systemd::host(), &args, &options)
            .await
            .unwrap();
        assert!(
            matches!(outcome, RestoreOutcome::Committed { version, .. } if version == semver::Version::new(1, 2, 3))
        );

        let updated = super::super::state::load_state(&install_root)
            .unwrap()
            .unwrap();
        assert_eq!(updated.active_version, "1.2.3");
        assert!(
            super::super::config::load_repository(&install_root)
                .unwrap()
                .is_none(),
            "restore must not write the repository record"
        );
        let (webserver_sha, webserver_size) = sha256_bytes(PAYLOAD_1_2_3);
        assert_eq!(updated.assets.webserver.sha256, webserver_sha);
        assert_eq!(updated.assets.webserver.size, webserver_size);
        let (static_sha, static_size) = sha256_bytes(ZIP_1_2_3);
        assert_eq!(updated.assets.static_archive.sha256, static_sha);
        assert_eq!(updated.assets.static_archive.size, static_size);
        assert_eq!(updated.initialization.status, InitStatus::Pending);
        assert!(!updated.service.verified);

        let release = install_root.canonical.join("releases/1.2.3");
        assert_eq!(
            std::fs::read(release.join("landscape-webserver")).unwrap(),
            PAYLOAD_1_2_3
        );
        assert_eq!(
            std::fs::read_to_string(release.join("static/index.html")).unwrap(),
            "static 1.2.3"
        );
        assert_eq!(
            std::fs::read(release.join("static.zip")).unwrap(),
            ZIP_1_2_3
        );
        assert_eq!(
            std::fs::read_to_string(install_root.canonical.join("data/landscape_init.toml"))
                .unwrap(),
            "version = \"1.2.3\"\n"
        );
        assert_eq!(
            std::fs::read(install_root.canonical.join("data/geo_tmp/ip/geo.dat")).unwrap(),
            b"geo 1.2.3"
        );
        assert_eq!(
            std::fs::read_link(install_root.canonical.join("current")).unwrap(),
            std::path::PathBuf::from("releases/1.2.3")
        );
        assert!(
            super::super::transaction::find_unfinished(&install_root)
                .unwrap()
                .is_none()
        );
        let lkb_count = std::fs::read_dir(install_root.canonical.join("backups"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("lkb"))
            .count();
        assert_eq!(lkb_count, 2, "target backup plus the protection backup");
        let _ = std::fs::remove_dir_all(&root);
    }

    struct NonInteractiveGuard;

    impl Drop for NonInteractiveGuard {
        fn drop(&mut self) {
            crate::interaction::interactive::configure(false);
        }
    }

    #[tokio::test]
    async fn restore_requires_yes_in_non_interactive_mode() {
        let _guard = interactive_guard().await;
        crate::interaction::interactive::configure(true);
        let _reset = NonInteractiveGuard;
        let root = temp_root("non-interactive");
        let install_root = InstallRoot {
            install_root: root.clone(),
            canonical: root.clone(),
        };
        activate_version(&install_root, "1.3.0", PAYLOAD_1_3_0, ZIP_1_3_0);
        setup_current(&install_root);
        let state = install_state(&install_root, "1.3.0", PAYLOAD_1_3_0, ZIP_1_3_0);
        super::super::state::write_state(&install_root, &state).unwrap();
        let (backup_ref, _) = create_target_backup(&install_root);
        let server = export_server("1.3.0".into());
        let options = RestoreOptions {
            export_base_url: server.base.clone(),
            token: &TOKEN,
            confirm: &YES,
            health: &none_health(),
        };
        let args = RestoreArgs {
            backup_id: Some(backup_ref.backup_id),
            file_path: None,
            allow_no_backup: false,
            yes: false,
        };
        assert!(matches!(
            restore_version(&install_root, &state, &Systemd::host(), &args, &options).await,
            Err(InstallError::ParameterUsage(_))
        ));
        assert!(
            super::super::transaction::find_unfinished(&install_root)
                .unwrap()
                .is_none(),
            "missing --yes must not create a transaction"
        );
        assert!(
            !install_root
                .canonical
                .join("transactions")
                .join(".tmp")
                .exists()
        );
        assert_eq!(
            std::fs::read_dir(install_root.canonical.join("transactions"))
                .map(|entries| entries.count())
                .unwrap_or(0),
            0,
            "missing --yes must not leave transaction files behind"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn rejects_malformed_backup_ids_before_creating_a_transaction() {
        let _guard = interactive_guard().await;
        crate::interaction::interactive::configure(false);
        let root = temp_root("bad-id");
        let install_root = InstallRoot {
            install_root: root.clone(),
            canonical: root.clone(),
        };
        activate_version(&install_root, "1.3.0", PAYLOAD_1_3_0, ZIP_1_3_0);
        setup_current(&install_root);
        let state = install_state(&install_root, "1.3.0", PAYLOAD_1_3_0, ZIP_1_3_0);
        super::super::state::write_state(&install_root, &state).unwrap();
        let server = export_server("1.3.0".into());
        let options = RestoreOptions {
            export_base_url: server.base.clone(),
            token: &TOKEN,
            confirm: &YES,
            health: &none_health(),
        };
        for id in [
            "../escape",
            "20260801-163000",
            "20260801-163000-A1B2C3D4",
            "notevenclose",
        ] {
            let args = RestoreArgs {
                backup_id: Some(id.into()),
                file_path: None,
                allow_no_backup: false,
                yes: true,
            };
            assert!(
                matches!(
                    restore_version(&install_root, &state, &Systemd::host(), &args, &options).await,
                    Err(InstallError::ParameterUsage(_))
                ),
                "backup id {id:?} must be rejected"
            );
        }
        assert!(
            super::super::transaction::find_unfinished(&install_root)
                .unwrap()
                .is_none()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn restore_blocks_without_allow_no_backup_when_protection_fails() {
        let _guard = interactive_guard().await;
        crate::interaction::interactive::configure(false);
        let root = temp_root("protection-blocked");
        let install_root = InstallRoot {
            install_root: root.clone(),
            canonical: root.clone(),
        };
        activate_version(&install_root, "1.3.0", PAYLOAD_1_3_0, ZIP_1_3_0);
        setup_current(&install_root);
        let state = install_state(&install_root, "1.3.0", PAYLOAD_1_3_0, ZIP_1_3_0);
        super::super::state::write_state(&install_root, &state).unwrap();
        let (backup_ref, _) = create_target_backup(&install_root);
        let server = TestServer::start(|_| TestResponse::status(500, "boom", Vec::new()));
        let options = RestoreOptions {
            export_base_url: server.base.clone(),
            token: &TOKEN,
            confirm: &YES,
            health: &none_health(),
        };
        let args = RestoreArgs {
            backup_id: Some(backup_ref.backup_id),
            file_path: None,
            allow_no_backup: false,
            yes: false,
        };
        assert!(
            restore_version(&install_root, &state, &Systemd::host(), &args, &options)
                .await
                .is_err()
        );
        assert_eq!(
            std::fs::read_link(install_root.canonical.join("current")).unwrap(),
            std::path::PathBuf::from("releases/1.3.0")
        );
        assert_eq!(
            super::super::state::load_state(&install_root)
                .unwrap()
                .unwrap()
                .active_version,
            "1.3.0"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn restore_continues_with_allow_no_backup_when_protection_fails() {
        // 保护备份失败时默认阻断;显式 --allow-no-backup 才允许继续,
        // 事务记录 no_backup: true 且不记录保护 .lkb。
        let _guard = interactive_guard().await;
        crate::interaction::interactive::configure(true);
        let _reset = NonInteractiveGuard;
        let root = temp_root("protection-allow");
        let install_root = InstallRoot {
            install_root: root.clone(),
            canonical: root.clone(),
        };
        activate_version(&install_root, "1.3.0", PAYLOAD_1_3_0, ZIP_1_3_0);
        setup_current(&install_root);
        let state = install_state(&install_root, "1.3.0", PAYLOAD_1_3_0, ZIP_1_3_0);
        super::super::state::write_state(&install_root, &state).unwrap();
        let (backup_ref, _) = create_target_backup(&install_root);
        let server = TestServer::start(|_| TestResponse::status(500, "boom", Vec::new()));
        let options = RestoreOptions {
            export_base_url: server.base.clone(),
            token: &TOKEN,
            confirm: &YES,
            health: &none_health(),
        };
        let args = RestoreArgs {
            backup_id: Some(backup_ref.backup_id),
            file_path: None,
            allow_no_backup: true,
            yes: true,
        };
        assert!(matches!(
            restore_version(&install_root, &state, &Systemd::host(), &args, &options).await,
            Ok(RestoreOutcome::Committed { .. })
        ));
        let transaction = super::super::transaction::find_unfinished(&install_root).unwrap();
        assert!(
            transaction.is_none(),
            "the restore must commit successfully"
        );
        let tx_id = super::super::state::load_state(&install_root)
            .unwrap()
            .unwrap()
            .last_transaction_id
            .unwrap();
        let path = install_root
            .canonical
            .join("transactions")
            .join(format!("{tx_id}.json"));
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(value["no_backup"], true);
        assert!(value["backup"].is_null());
        assert!(value["restore_backup"].is_object());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn none_mode_activation_failure_is_recovered_by_next_command() {
        // none 模式激活后失败不内联回滚,返回普通失败;previous-data 与事务现场
        // 保留在事务目录,由下次命令的 phase 恢复入口恢复原 data、current 与 state。
        let _guard = interactive_guard().await;
        crate::interaction::interactive::configure(true);
        let _reset = NonInteractiveGuard;
        let root = temp_root("none-activation-fail");
        let install_root = InstallRoot {
            install_root: root.clone(),
            canonical: root.clone(),
        };
        activate_version(&install_root, "1.3.0", PAYLOAD_1_3_0, ZIP_1_3_0);
        setup_current(&install_root);
        std::fs::write(
            install_root.canonical.join("data/landscape_db.sqlite"),
            b"db",
        )
        .unwrap();
        let state = install_state(&install_root, "1.3.0", PAYLOAD_1_3_0, ZIP_1_3_0);
        super::super::state::write_state(&install_root, &state).unwrap();
        let (backup_ref, _) = create_target_backup(&install_root);
        let server = export_server("1.3.0".into());
        let options = RestoreOptions {
            export_base_url: server.base.clone(),
            token: &TOKEN,
            confirm: &YES,
            health: &none_health(),
        };
        // current 被替换为普通目录(内含 static 副本与额外条目):保护备份仍能读取
        // current/static,但激活阶段 restore_current 的 rename 必然失败,失败发生在
        // data 移入 previous-data 之后、提交 state 之前。
        let current = install_root.canonical.join("current");
        std::fs::remove_file(&current).unwrap();
        std::fs::create_dir_all(current.join("static")).unwrap();
        std::fs::write(
            current.join("static/index.html"),
            std::fs::read(
                install_root
                    .canonical
                    .join("releases/1.3.0/static/index.html"),
            )
            .unwrap(),
        )
        .unwrap();
        std::fs::create_dir_all(current.join("occupied")).unwrap();
        let args = RestoreArgs {
            backup_id: Some(backup_ref.backup_id),
            file_path: None,
            allow_no_backup: false,
            yes: true,
        };
        let restore_error = restore_version(
            &install_root,
            &state,
            &Systemd::host(),
            &args,
            &options,
        )
        .await
        .expect_err(
            "none-mode activation failure must return a plain error, not an inline rollback",
        );
        assert!(
            !matches!(
                restore_error,
                InstallError::InvalidBackup(_) | InstallError::ExportFailed(_)
            ),
            "the failure must come from the activation phase, got {restore_error:?}"
        );
        let unfinished = super::super::transaction::find_unfinished(&install_root)
            .unwrap()
            .expect("failed none-mode restore must leave an unfinished transaction");
        assert_eq!(
            unfinished.phase,
            super::super::transaction::Phase::Activating
        );
        let tx_dir = install_root
            .canonical
            .join("transactions")
            .join(&unfinished.transaction_id);
        assert_eq!(
            std::fs::read(tx_dir.join("previous-data/landscape_db.sqlite")).unwrap(),
            b"db",
            "the original data must be preserved in the transaction directory"
        );

        // 现场修复后(操作员处理),下次命令经恢复入口完成回滚。
        std::fs::remove_dir_all(&current).unwrap();
        std::os::unix::fs::symlink("releases/1.3.0", &current).unwrap();
        super::super::transaction::recovery::recover_interrupted(
            &install_root,
            &unfinished,
            &Systemd::host(),
            &none_health(),
        )
        .await
        .unwrap();

        assert_eq!(
            std::fs::read(install_root.canonical.join("data/landscape_db.sqlite")).unwrap(),
            b"db",
            "recovery must restore the original data"
        );
        assert_eq!(
            std::fs::read_link(install_root.canonical.join("current")).unwrap(),
            std::path::PathBuf::from("releases/1.3.0")
        );
        let restored = super::super::state::load_state(&install_root)
            .unwrap()
            .unwrap();
        assert_eq!(restored.active_version, "1.3.0");
        assert_eq!(
            restored.last_transaction_id.as_deref(),
            Some(unfinished.transaction_id.as_str())
        );
        assert!(
            super::super::transaction::find_unfinished(&install_root)
                .unwrap()
                .is_none()
        );
        let path = install_root
            .canonical
            .join("transactions")
            .join(format!("{}.json", unfinished.transaction_id));
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(value["phase"], "rolled_back");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn rollback_restores_previous_data_from_transaction_dir() {
        let root = temp_root("rollback");
        let install_root = InstallRoot {
            install_root: root.clone(),
            canonical: root.clone(),
        };
        activate_version(&install_root, "1.3.0", PAYLOAD_1_3_0, ZIP_1_3_0);
        setup_current(&install_root);
        std::fs::write(
            install_root.canonical.join("data/landscape_db.sqlite"),
            b"db",
        )
        .unwrap();
        let state = install_state(&install_root, "1.3.0", PAYLOAD_1_3_0, ZIP_1_3_0);

        let mut transaction = TransactionFile::new_restore(
            &install_root,
            &semver::Version::new(1, 3, 0),
            &semver::Version::new(1, 2, 3),
        )
        .unwrap();
        super::super::transaction::begin(&install_root, &transaction).unwrap();
        transaction.restore_backup = Some(BackupRef {
            backup_id: "t".into(),
            path: "backups/t.lkb".into(),
            sha256: "a".repeat(64),
        });
        super::super::transaction::persist(&install_root, &transaction).unwrap();
        rollback::write_state_snapshot(&install_root, &transaction.transaction_id, &state).unwrap();
        let tx_dir = install_root
            .canonical
            .join("transactions")
            .join(&transaction.transaction_id);
        std::fs::create_dir_all(tx_dir.join("previous-data")).unwrap();
        std::fs::write(tx_dir.join("previous-data/landscape_db.sqlite"), b"old-db").unwrap();
        super::super::transaction::mark_phase(&install_root, &transaction, Phase::Activating)
            .unwrap();
        std::fs::write(install_root.canonical.join("data/partial"), b"partial").unwrap();

        rollback_restore(
            &install_root,
            &transaction,
            &Systemd::host(),
            &none_health(),
        )
        .await
        .unwrap();

        assert_eq!(
            std::fs::read(install_root.canonical.join("data/landscape_db.sqlite")).unwrap(),
            b"old-db"
        );
        assert!(!install_root.canonical.join("data/partial").exists());
        assert_eq!(
            std::fs::read_link(install_root.canonical.join("current")).unwrap(),
            std::path::PathBuf::from("releases/1.3.0")
        );
        let restored = super::super::state::load_state(&install_root)
            .unwrap()
            .unwrap();
        assert_eq!(restored.active_version, "1.3.0");
        assert_eq!(
            restored.last_transaction_id.as_deref(),
            Some(transaction.transaction_id.as_str())
        );
        assert!(
            super::super::transaction::find_unfinished(&install_root)
                .unwrap()
                .is_none()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn rollback_treats_consumed_previous_data_as_already_restored() {
        // 模拟:上次回滚已完成 previous-data -> data 重命名,但写 state 前崩溃。
        // 重试不得再次删除 data,必须直接按已恢复状态提交。
        let root = temp_root("already-restored");
        let install_root = InstallRoot {
            install_root: root.clone(),
            canonical: root.clone(),
        };
        activate_version(&install_root, "1.3.0", PAYLOAD_1_3_0, ZIP_1_3_0);
        setup_current(&install_root);
        std::fs::write(
            install_root.canonical.join("data/landscape_db.sqlite"),
            b"db",
        )
        .unwrap();
        let state = install_state(&install_root, "1.3.0", PAYLOAD_1_3_0, ZIP_1_3_0);

        let mut transaction = TransactionFile::new_restore(
            &install_root,
            &semver::Version::new(1, 3, 0),
            &semver::Version::new(1, 2, 3),
        )
        .unwrap();
        super::super::transaction::begin(&install_root, &transaction).unwrap();
        transaction.restore_backup = Some(BackupRef {
            backup_id: "t".into(),
            path: "backups/t.lkb".into(),
            sha256: "a".repeat(64),
        });
        super::super::transaction::persist(&install_root, &transaction).unwrap();
        rollback::write_state_snapshot(&install_root, &transaction.transaction_id, &state).unwrap();
        let tx_dir = install_root
            .canonical
            .join("transactions")
            .join(&transaction.transaction_id);
        // previous-data 已被上次回滚消费:data 里放旧数据库,previous-data 不存在。
        std::fs::write(
            install_root.canonical.join("data/landscape_db.sqlite"),
            b"old-db",
        )
        .unwrap();
        assert!(!tx_dir.join("previous-data").exists());
        super::super::transaction::mark_phase(&install_root, &transaction, Phase::Activating)
            .unwrap();

        rollback_restore(
            &install_root,
            &transaction,
            &Systemd::host(),
            &none_health(),
        )
        .await
        .unwrap();

        assert_eq!(
            std::fs::read(install_root.canonical.join("data/landscape_db.sqlite")).unwrap(),
            b"old-db",
            "already-restored data must not be deleted or replaced"
        );
        assert_eq!(
            std::fs::read_link(install_root.canonical.join("current")).unwrap(),
            std::path::PathBuf::from("releases/1.3.0")
        );
        let restored = super::super::state::load_state(&install_root)
            .unwrap()
            .unwrap();
        assert_eq!(restored.active_version, "1.3.0");
        assert!(
            super::super::transaction::find_unfinished(&install_root)
                .unwrap()
                .is_none()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn same_version_rollback_restores_the_original_release() {
        // 同版本 restore:rebuild_release_from_backup 会把原 release 移入
        // replaced-release;回滚必须把它移回,保证 release 内容与回滚前一致。
        let root = temp_root("same-version");
        let install_root = InstallRoot {
            install_root: root.clone(),
            canonical: root.clone(),
        };
        activate_version(&install_root, "1.3.0", PAYLOAD_1_3_0, ZIP_1_3_0);
        setup_current(&install_root);
        std::fs::write(
            install_root.canonical.join("data/landscape_db.sqlite"),
            b"db",
        )
        .unwrap();
        let state = install_state(&install_root, "1.3.0", PAYLOAD_1_3_0, ZIP_1_3_0);

        let mut transaction = TransactionFile::new_restore(
            &install_root,
            &semver::Version::new(1, 3, 0),
            &semver::Version::new(1, 3, 0),
        )
        .unwrap();
        super::super::transaction::begin(&install_root, &transaction).unwrap();
        transaction.restore_backup = Some(BackupRef {
            backup_id: "t".into(),
            path: "backups/t.lkb".into(),
            sha256: "a".repeat(64),
        });
        super::super::transaction::persist(&install_root, &transaction).unwrap();
        rollback::write_state_snapshot(&install_root, &transaction.transaction_id, &state).unwrap();
        let tx_dir = install_root
            .canonical
            .join("transactions")
            .join(&transaction.transaction_id);
        std::fs::create_dir_all(tx_dir.join("previous-data")).unwrap();
        std::fs::write(tx_dir.join("previous-data/landscape_db.sqlite"), b"old-db").unwrap();
        // 模拟 rebuild:原 release 被移入 replaced-release,releases/1.3.0 现在是
        // 备份重建版本(内容不同)。
        std::fs::create_dir_all(tx_dir.join("replaced-release")).unwrap();
        std::fs::write(
            tx_dir.join("replaced-release/landscape-webserver"),
            PAYLOAD_1_3_0,
        )
        .unwrap();
        std::fs::write(tx_dir.join("replaced-release/static.zip"), ZIP_1_3_0).unwrap();
        std::fs::write(
            install_root
                .canonical
                .join("releases/1.3.0/landscape-webserver"),
            b"rebuilt-from-lkb",
        )
        .unwrap();
        super::super::transaction::mark_phase(&install_root, &transaction, Phase::Activating)
            .unwrap();

        rollback_restore(
            &install_root,
            &transaction,
            &Systemd::host(),
            &none_health(),
        )
        .await
        .unwrap();

        assert_eq!(
            std::fs::read(
                install_root
                    .canonical
                    .join("releases/1.3.0/landscape-webserver")
            )
            .unwrap(),
            PAYLOAD_1_3_0,
            "the original release must be moved back after a same-version rollback"
        );
        assert!(!tx_dir.join("replaced-release").exists());
        assert!(
            super::super::transaction::find_unfinished(&install_root)
                .unwrap()
                .is_none()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn rollback_failure_marks_the_transaction_failed() {
        // 回滚任一步失败(这里让 restore_current 失败)必须把事务标记为 failed,
        // 不能留在 rolling_back 让下一条命令反复自动回滚。
        let root = temp_root("rollback-failed");
        let install_root = InstallRoot {
            install_root: root.clone(),
            canonical: root.clone(),
        };
        activate_version(&install_root, "1.3.0", PAYLOAD_1_3_0, ZIP_1_3_0);
        setup_current(&install_root);
        std::fs::write(
            install_root.canonical.join("data/landscape_db.sqlite"),
            b"db",
        )
        .unwrap();
        let state = install_state(&install_root, "1.3.0", PAYLOAD_1_3_0, ZIP_1_3_0);

        let mut transaction = TransactionFile::new_restore(
            &install_root,
            &semver::Version::new(1, 3, 0),
            &semver::Version::new(1, 2, 3),
        )
        .unwrap();
        super::super::transaction::begin(&install_root, &transaction).unwrap();
        transaction.restore_backup = Some(BackupRef {
            backup_id: "t".into(),
            path: "backups/t.lkb".into(),
            sha256: "a".repeat(64),
        });
        super::super::transaction::persist(&install_root, &transaction).unwrap();
        rollback::write_state_snapshot(&install_root, &transaction.transaction_id, &state).unwrap();
        let tx_dir = install_root
            .canonical
            .join("transactions")
            .join(&transaction.transaction_id);
        std::fs::create_dir_all(tx_dir.join("previous-data")).unwrap();
        std::fs::write(tx_dir.join("previous-data/landscape_db.sqlite"), b"old-db").unwrap();
        super::super::transaction::mark_phase(&install_root, &transaction, Phase::Activating)
            .unwrap();
        // current 变成普通目录:restore_current 的 rename 必然失败。
        let current = install_root.canonical.join("current");
        std::fs::remove_file(&current).unwrap();
        std::fs::create_dir_all(current.join("occupied")).unwrap();

        assert!(
            rollback_restore(
                &install_root,
                &transaction,
                &Systemd::host(),
                &none_health(),
            )
            .await
            .is_err()
        );

        let phase_file = install_root
            .canonical
            .join("transactions")
            .join(format!("{}.json", transaction.transaction_id));
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&phase_file).unwrap()).unwrap();
        assert_eq!(
            value["phase"], "failed",
            "a failed rollback must leave the transaction in the failed phase"
        );
        assert!(
            super::super::transaction::find_unfinished(&install_root)
                .unwrap()
                .is_none()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn none_mode_proceeds_with_non_interactive_yes() {
        let _guard = interactive_guard().await;
        crate::interaction::interactive::configure(true);
        let _reset = NonInteractiveGuard;
        let root = temp_root("none-yes");
        let install_root = InstallRoot {
            install_root: root.clone(),
            canonical: root.clone(),
        };
        activate_version(&install_root, "1.3.0", PAYLOAD_1_3_0, ZIP_1_3_0);
        setup_current(&install_root);
        let state = install_state(&install_root, "1.3.0", PAYLOAD_1_3_0, ZIP_1_3_0);
        super::super::state::write_state(&install_root, &state).unwrap();
        let (backup_ref, _) = create_target_backup(&install_root);
        let server = export_server("1.3.0".into());
        let options = RestoreOptions {
            export_base_url: server.base.clone(),
            token: &TOKEN,
            confirm: &|_| panic!("none mode with --yes must not open a TTY confirmation"),
            health: &none_health(),
        };
        let args = RestoreArgs {
            backup_id: Some(backup_ref.backup_id),
            file_path: None,
            allow_no_backup: false,
            yes: true,
        };
        assert!(matches!(
            restore_version(&install_root, &state, &Systemd::host(), &args, &options).await,
            Ok(RestoreOutcome::Committed { .. })
        ));
        assert_eq!(
            super::super::state::load_state(&install_root)
                .unwrap()
                .unwrap()
                .active_version,
            "1.2.3"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
