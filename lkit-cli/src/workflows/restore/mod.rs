use std::path::PathBuf;

use super::artifacts::{hash_file, hash_str};
use super::backup::{BackupMetadata, create_secure_dir, extract_lkb, verify_lkb};
use super::health::{DocsProbe, HealthOptions, StartupOptions};
use super::manager::{ManagedService, ServiceManager};
use super::pipeline;
use super::plan::InstallError;
use super::rollback as rollback_util;
use super::root::InstallRoot;
use super::state::{InstallState, StateServiceManager};
use super::transaction::{BackupRef, Phase, TransactionFile};
use crate::deployment::layout;
use crate::interaction::presentation::{OperationPhase, operation_progress};

mod backup;
mod rollback;
mod state;

pub(crate) use self::backup::validate_backup_file;
use self::backup::{check_architecture, resolve_target_backup};
pub(crate) use self::rollback::{restore_previous_data, rollback_restore, write_file_atomic};
use self::state::{build_restore_state, create_protection_backup};
/// restore 运行参数。
pub(crate) struct RestoreArgs {
    /// `--backup <ID>` 只解析 lkit 地盘 `backups/` 下的备份 ID。
    pub backup_id: Option<String>,
    /// `--file <PATH>` 用于外部复制的备份,先复制进事务目录再校验。
    pub file_path: Option<PathBuf>,
    /// 允许在保护备份无法创建时继续,不产生可移植的当前配置快照。
    pub allow_no_backup: bool,
    /// 非交互模式必须显式 `--yes`,否则返回参数错误。
    pub yes: bool,
    /// 交互控制台已确认恢复计划,跳过 `/dev/tty` 二次确认(worker 进程无法读取 TUI 输入)。
    pub console_confirmed: bool,
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
    manager: &dyn ServiceManager,
    args: &RestoreArgs,
    options: &RestoreOptions<'_, P>,
) -> Result<RestoreOutcome, InstallError> {
    let is_systemd = state.service.manager == StateServiceManager::Systemd;
    let (bytes, file_sha256) = resolve_target_backup(root, args)?;
    let metadata = verify_lkb(&bytes)?;
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
    let tx_dir = layout::territory_transactions_dir().join(&transaction.transaction_id);

    // 外部备份先复制进事务目录并重新自校验,事务只记录 lkit 地盘相对路径。
    let target_backup = match (&args.backup_id, &args.file_path) {
        (Some(id), None) => BackupRef {
            backup_id: id.clone(),
            path: format!("backups/{id}.lkb"),
            sha256: file_sha256,
        },
        (None, Some(_)) => {
            create_secure_dir(&tx_dir, 0o700)?;
            let copied = tx_dir.join("target-backup.lkb");
            write_file_atomic(&copied, &bytes, 0o600)?;
            let copied_bytes = std::fs::read(&copied).map_err(InstallError::Io)?;
            verify_lkb(&copied_bytes)?;
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
        transaction.systemd_before = Some(pipeline::capture_before(
            manager,
            ManagedService::LandscapeRouter,
        )?);
        let backup_dir = layout::territory_backups_dir()
            .join(&transaction.transaction_id)
            .join("host/resolv.conf");
        let _ = super::resolv::backup(manager.resolv_conf(), &backup_dir)?;
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
    rollback_util::write_state_snapshot(root, &transaction.transaction_id, state)?;

    // 停止服务前完成安全解包与完整内容校验(必需条目、权限 0700/0600):
    // 解包失败时服务与现场均未改变,事务直接标记 failed。
    let restore_dir = tx_dir.join("restore");
    let _ = std::fs::remove_dir_all(&restore_dir);
    if let Err(error) = extract_lkb(&bytes, &restore_dir) {
        let _ = super::transaction::mark_phase(root, &transaction, Phase::Failed);
        return Err(error);
    }
    super::transaction::mark_phase(root, &transaction, Phase::Prepared)?;
    let steps = if is_systemd { 4 } else { 2 };
    operation_progress(OperationPhase::Preparing, Some((1, steps)));

    let mut activated = false;
    let result: Result<(), InstallError> = async {
        if is_systemd {
            super::transaction::mark_phase(root, &transaction, Phase::Stopping)?;
            operation_progress(OperationPhase::Stopping, Some((2, steps)));
            manager.stop_and_wait(
                ManagedService::LandscapeRouter,
                &(|| {
                    manager
                        .active_state(ManagedService::LandscapeRouter)
                        .map(|value| value != "active")
                        .unwrap_or(true)
                }),
            )?;
        } else if !crate::interaction::interactive::is_non_interactive() && !args.console_confirmed
        {
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
        operation_progress(
            OperationPhase::Activating,
            Some((if is_systemd { 3 } else { 2 }, steps)),
        );
        activated = true;
        std::fs::create_dir_all(&tx_dir).map_err(InstallError::Io)?;
        rollback_util::move_data_aside(
            &root.canonical.join("data"),
            &tx_dir.join("previous-data"),
        )?;
        rollback_util::rebuild_release_from_backup(
            root,
            &tx_dir,
            &target_version.to_string(),
            &restore_dir,
        )?;
        let data = root.canonical.join("data");
        let geo_tmp_source = restore_dir.join("geo_tmp");
        if geo_tmp_source.is_dir() {
            rollback_util::copy_tree_into(&geo_tmp_source, &data.join("geo_tmp"))?;
        }
        let init_config =
            std::fs::read(restore_dir.join("landscape_init.toml")).map_err(InstallError::Io)?;
        write_file_atomic(&data.join("landscape_init.toml"), &init_config, 0o600)?;
        rollback_util::restore_current(root, &format!("releases/{target_version}"))?;
        if is_systemd {
            manager.register(
                ManagedService::LandscapeRouter,
                &root.canonical.join("service/landscape-router.service"),
            )?;
            manager.enable(ManagedService::LandscapeRouter)?;
            manager.start(ManagedService::LandscapeRouter)?;
            super::transaction::mark_phase(root, &transaction, Phase::Verifying)?;
            operation_progress(OperationPhase::Verifying, Some((4, steps)));
            let pid = manager.main_pid(ManagedService::LandscapeRouter)?;
            if pid == 0 {
                return Err(InstallError::Systemd(
                    "service did not produce a main pid after start".into(),
                ));
            }
            let startup = StartupOptions {
                ports: &options.health.ports,
                expected_pid: pid,
                docs: &options.health.docs,
                unit_state: Some(&(|| manager.active_state(ManagedService::LandscapeRouter).ok())),
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
            match rollback_restore(root, &transaction, manager, options.health).await {
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
                    let unit_origin = root
                        .canonical
                        .join("service")
                        .join(manager.service_name(ManagedService::LandscapeRouter));
                    let restore_error = manager
                        .restore_before(ManagedService::LandscapeRouter, before, &unit_origin)
                        .and_then(|()| {
                            if let Some(backup_path) = &transaction.resolv_conf_backup {
                                let backup_dir = layout::territory_relative(backup_path);
                                super::resolv::restore(manager.resolv_conf(), &backup_dir)
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
    // 交互控制台的分发路径在 TUI 确认层已完成确认;worker 进程无法读取 TUI 输入,
    // 继续请求 `/dev/tty` 会死锁,因此直接跳过。
    if args.console_confirmed {
        return Ok(());
    }
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

#[cfg(test)]
#[allow(clippy::await_holding_lock)]
mod tests {
    // 交互模式测试通过 std::sync::Mutex 串行化,锁故意跨 await 持有。
    use std::os::unix::fs::PermissionsExt;

    use super::super::health::HealthOptions;
    use super::super::repository::test_server::{TestResponse, TestServer};
    use super::super::state::{
        ArchiveAsset, Assets, InitStatus, InitializationState, ServiceState, StateArchitecture,
        StateServiceManager, WebserverAsset,
    };
    use super::*;
    use crate::service::systemd::Systemd;

    use super::super::backup as lkb;
    use super::super::export;
    use chrono::Utc;

    pub(super) const PAYLOAD_1_2_3: &[u8] = b"webserver payload 1.2.3";
    pub(super) const PAYLOAD_1_3_0: &[u8] = b"webserver payload 1.3.0";
    pub(super) const ZIP_1_2_3: &[u8] = b"zip payload 1.2.3";
    pub(super) const ZIP_1_3_0: &[u8] = b"zip payload 1.3.0";

    pub(super) fn temp_root(name: &str) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("lkit-restore-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    pub(super) fn sha256_bytes(bytes: &[u8]) -> (String, u64) {
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

    pub(super) fn activate_version(root: &InstallRoot, version: &str, payload: &[u8], zip: &[u8]) {
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

    pub(super) fn install_state(
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
                manager: StateServiceManager::Systemd,
                registered: true,
                enabled: true,
                verified: true,
                definition_path: Some("service/landscape-router.service".into()),
                definition_sha256: Some("d".repeat(64)),
            },
            last_transaction_id: None,
            committed_at: Some(Utc::now()),
        }
    }

    static BACKUP_SOURCE_COUNTER: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(0);

    pub(super) fn create_target_backup(_root: &InstallRoot) -> (BackupRef, Vec<u8>) {
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
        let backup_ref = lkb::create_backup(
            &layout::territory_backups_dir(),
            &semver::Version::new(1, 2, 3),
            "x86_64",
            &binary,
            "version = \"1.2.3\"\n",
            &static_dir,
            &zip,
            &geo,
            "manual backup",
            false,
            None,
        )
        .unwrap();
        let bytes = std::fs::read(
            layout::territory_backups_dir().join(format!("{}.lkb", backup_ref.backup_id)),
        )
        .unwrap();
        let _ = std::fs::remove_dir_all(&source);
        (backup_ref, bytes)
    }

    pub(super) struct FakeDocs;

    impl DocsProbe for FakeDocs {
        async fn docs_ok(&self) -> bool {
            true
        }
    }

    pub(super) fn none_health() -> HealthOptions<FakeDocs> {
        HealthOptions {
            docs: FakeDocs,
            ports: Vec::new(),
            startup_timeout: std::time::Duration::from_secs(5),
            stable_duration: std::time::Duration::from_millis(100),
        }
    }

    pub(super) static YES: fn(&str) -> Result<bool, InstallError> = |_| Ok(true);
    pub(super) static TOKEN: fn() -> Result<String, InstallError> = || Ok("tok".into());

    /// `is_non_interactive()` 是进程级全局状态,并发 tokio 测试会互相干扰;
    /// 涉及交互模式的测试必须串行执行。
    pub(super) async fn interactive_guard() -> std::sync::MutexGuard<'static, ()> {
        crate::interaction::interactive::test_guard()
    }

    pub(super) fn export_server(version: String) -> TestServer {
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

    pub(super) fn setup_current(root: &InstallRoot) {
        std::fs::create_dir_all(root.canonical.join("data")).unwrap();
        std::fs::write(root.canonical.join("data/landscape_init.lock"), b"").unwrap();
        std::fs::write(root.canonical.join("data/landscape.toml"), b"").unwrap();
    }

    /// 受管服务 unit 源文件(restore 的 systemd 路径会读取并记录其 sha256)。
    pub(super) fn write_unit_origin(root: &InstallRoot) {
        std::fs::create_dir_all(root.canonical.join("service")).unwrap();
        std::fs::write(
            root.canonical.join("service/landscape-router.service"),
            b"[Unit]\nDescription=Landscape Router\n",
        )
        .unwrap();
    }

    /// 有状态假 systemctl:stop/start 维护 state 文件,stop 后 ActiveState 为 inactive。
    pub(super) fn fake_systemd_stateful(dir: &std::path::Path) -> Systemd {
        std::fs::create_dir_all(dir.join("units")).unwrap();
        std::fs::create_dir_all(dir.join("run")).unwrap();
        std::fs::write(dir.join("state"), b"active").unwrap();
        let script = dir.join("systemctl");
        std::fs::write(
            &script,
            format!(
                r#"#!/bin/sh
STATE_FILE="{}"
case "$*" in
  "start landscape-router.service") echo active > "$STATE_FILE"; exit 0;;
  "stop landscape-router.service") echo inactive > "$STATE_FILE"; exit 0;;
  "show --property=ActiveState --value landscape-router.service") cat "$STATE_FILE";;
  "show --property=MainPID --value landscape-router.service") echo {};;
  "is-enabled landscape-router.service") echo enabled;;
  "is-active landscape-router.service") cat "$STATE_FILE";;
  *) exit 0;;
esac
"#,
                dir.join("state").display(),
                std::process::id()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        Systemd {
            systemctl: script,
            system_unit_dir: dir.join("units"),
            run_systemd_dir: dir.join("run"),
            pid1_is_systemd: true,
            resolv_conf: dir.join("resolv.conf"),
        }
    }

    /// 初始化 watcher:landscape_init.toml 出现后写入 lock 与 landscape.toml
    /// (restore 的 systemd 启动检查要求;与 move_data_aside 竞态下容错重试)。
    pub(super) fn init_watcher(
        data_dir: std::path::PathBuf,
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                if data_dir.join("landscape_init.toml").is_file() {
                    let _ = std::fs::write(data_dir.join("landscape_init.lock"), b"");
                    let _ = std::fs::write(data_dir.join("landscape.toml"), b"");
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        })
    }

    #[tokio::test]
    async fn restores_cross_version_with_systemd() {
        let _guard = interactive_guard().await;
        crate::interaction::interactive::configure(false);
        let root = temp_root("cross-version");
        let territory = root.join("territory");
        std::fs::create_dir_all(&territory).unwrap();
        let _territory_guard = crate::deployment::layout::test_territory(&territory);
        let install_root = InstallRoot {
            install_root: root.clone(),
            canonical: root.clone(),
        };
        activate_version(&install_root, "1.3.0", PAYLOAD_1_3_0, ZIP_1_3_0);
        setup_current(&install_root);
        write_unit_origin(&install_root);
        let systemd = fake_systemd_stateful(&root.join("fake-systemd"));
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let watcher = init_watcher(install_root.canonical.join("data"), stop.clone());
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
            console_confirmed: false,
        };
        let outcome = restore_version(&install_root, &state, &systemd, &args, &options)
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
            super::super::config::load_repository().unwrap().is_none(),
            "restore must not write the repository record"
        );
        let (webserver_sha, webserver_size) = sha256_bytes(PAYLOAD_1_2_3);
        assert_eq!(updated.assets.webserver.sha256, webserver_sha);
        assert_eq!(updated.assets.webserver.size, webserver_size);
        let (static_sha, static_size) = sha256_bytes(ZIP_1_2_3);
        assert_eq!(updated.assets.static_archive.sha256, static_sha);
        assert_eq!(updated.assets.static_archive.size, static_size);
        assert_eq!(updated.initialization.status, InitStatus::Complete);
        assert!(updated.service.verified);
        assert_eq!(updated.service.manager, StateServiceManager::Systemd);

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
        let lkb_count = std::fs::read_dir(layout::territory_backups_dir())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("lkb"))
            .count();
        assert_eq!(lkb_count, 2, "target backup plus the protection backup");
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        watcher.join().unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    pub(super) struct NonInteractiveGuard;

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
        let territory = root.join("territory");
        std::fs::create_dir_all(&territory).unwrap();
        let _territory_guard = crate::deployment::layout::test_territory(&territory);
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
            console_confirmed: false,
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
        assert!(!layout::territory_transactions_dir().join(".tmp").exists());
        assert_eq!(
            std::fs::read_dir(layout::territory_transactions_dir())
                .map(|entries| entries.count())
                .unwrap_or(0),
            0,
            "missing --yes must not leave transaction files behind"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn restore_proceeds_with_non_interactive_yes() {
        let _guard = interactive_guard().await;
        crate::interaction::interactive::configure(true);
        let _reset = NonInteractiveGuard;
        let root = temp_root("restore-yes");
        let territory = root.join("territory");
        std::fs::create_dir_all(&territory).unwrap();
        let _territory_guard = crate::deployment::layout::test_territory(&territory);
        let install_root = InstallRoot {
            install_root: root.clone(),
            canonical: root.clone(),
        };
        activate_version(&install_root, "1.3.0", PAYLOAD_1_3_0, ZIP_1_3_0);
        setup_current(&install_root);
        write_unit_origin(&install_root);
        let systemd = fake_systemd_stateful(&root.join("fake-systemd"));
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let watcher = init_watcher(install_root.canonical.join("data"), stop.clone());
        let state = install_state(&install_root, "1.3.0", PAYLOAD_1_3_0, ZIP_1_3_0);
        super::super::state::write_state(&install_root, &state).unwrap();
        let (backup_ref, _) = create_target_backup(&install_root);
        let server = export_server("1.3.0".into());
        let options = RestoreOptions {
            export_base_url: server.base.clone(),
            token: &TOKEN,
            confirm: &|_| panic!("systemd restore with --yes must not open a TTY confirmation"),
            health: &none_health(),
        };
        let args = RestoreArgs {
            backup_id: Some(backup_ref.backup_id),
            file_path: None,
            allow_no_backup: false,
            yes: true,
            console_confirmed: false,
        };
        assert!(matches!(
            restore_version(&install_root, &state, &systemd, &args, &options).await,
            Ok(RestoreOutcome::Committed { .. })
        ));
        assert_eq!(
            super::super::state::load_state(&install_root)
                .unwrap()
                .unwrap()
                .active_version,
            "1.2.3"
        );
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        watcher.join().unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn console_confirmed_skips_interactive_confirmations() {
        // 控制台分发路径:交互模式下 console_confirmed 使确认闭包不被调用
        // (worker 进程无法读取 TUI 输入,tty 确认会死锁),恢复正常提交。
        let _guard = interactive_guard().await;
        crate::interaction::interactive::configure(false);
        let root = temp_root("console-confirmed");
        let territory = root.join("territory");
        std::fs::create_dir_all(&territory).unwrap();
        let _territory_guard = crate::deployment::layout::test_territory(&territory);
        let install_root = InstallRoot {
            install_root: root.clone(),
            canonical: root.clone(),
        };
        activate_version(&install_root, "1.3.0", PAYLOAD_1_3_0, ZIP_1_3_0);
        setup_current(&install_root);
        write_unit_origin(&install_root);
        let systemd = fake_systemd_stateful(&root.join("fake-systemd"));
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let watcher = init_watcher(install_root.canonical.join("data"), stop.clone());
        let state = install_state(&install_root, "1.3.0", PAYLOAD_1_3_0, ZIP_1_3_0);
        super::super::state::write_state(&install_root, &state).unwrap();
        let (backup_ref, _) = create_target_backup(&install_root);
        let server = export_server("1.3.0".into());
        let options = RestoreOptions {
            export_base_url: server.base.clone(),
            token: &TOKEN,
            confirm: &|_| panic!("console-confirmed restore must not open a TTY confirmation"),
            health: &none_health(),
        };
        let args = RestoreArgs {
            backup_id: Some(backup_ref.backup_id),
            file_path: None,
            allow_no_backup: false,
            yes: true,
            console_confirmed: true,
        };
        assert!(matches!(
            restore_version(&install_root, &state, &systemd, &args, &options).await,
            Ok(RestoreOutcome::Committed { .. })
        ));
        assert_eq!(
            super::super::state::load_state(&install_root)
                .unwrap()
                .unwrap()
                .active_version,
            "1.2.3"
        );
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        watcher.join().unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }
}
