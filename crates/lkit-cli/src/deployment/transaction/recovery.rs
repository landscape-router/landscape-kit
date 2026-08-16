use std::path::Path;

use super::super::health::{DocsProbe, HealthOptions};
use super::super::layout;
use super::super::manager::{ManagedService, ServiceManager};
use super::super::plan::InstallError;
use super::super::root::InstallRoot;
use super::{self as transaction, Operation, Phase, TransactionFile};

pub(crate) async fn recover_interrupted<P: DocsProbe>(
    root: &InstallRoot,
    transaction: &TransactionFile,
    systemd: &dyn ServiceManager,
    health: &HealthOptions<P>,
) -> Result<(), InstallError> {
    match transaction.operation {
        Operation::Install => {
            if transaction.network_takeover.is_some()
                && matches!(
                    transaction.phase,
                    Phase::AwaitingNetworkConfirmation | Phase::Finalizing | Phase::RollingBack
                )
            {
                return Err(InstallError::BlockedByTransaction(format!(
                    "network takeover {} is {}; use `lkit network confirm` or `lkit network rollback`",
                    transaction.transaction_id,
                    transaction.phase.key()
                )));
            }
            let install_completed = matches!(
                super::super::state::load_state(root).ok().flatten(),
                Some(state) if state.active_version
                    == transaction.target_version.as_deref().unwrap_or_default()
            );
            if install_completed {
                transaction::mark_phase(root, transaction, Phase::Committed)?;
                return Ok(());
            }
            if let Err(error) =
                transaction::cleanup_failed_first_install(root, transaction, systemd)
            {
                if transaction.network_takeover.is_none() {
                    let _ = transaction::mark_phase(root, transaction, Phase::Failed);
                }
                return Err(error);
            }
            if let Some(network) = transaction.network_takeover.as_ref()
                && let Err(error) =
                    crate::network::takeover::cleanup_failed_takeover(root, network, systemd)
            {
                return Err(error);
            }
            transaction::mark_phase(root, transaction, Phase::Failed)?;
            Ok(())
        }
        Operation::Switch => recover_switch(root, transaction, systemd, health).await,
        Operation::Repair => recover_repair(root, transaction, systemd, health).await,
        Operation::Restore => recover_restore(root, transaction, systemd, health).await,
        Operation::Uninstall => recover_uninstall(root, transaction, systemd),
        Operation::Reinit => recover_reinit(root, transaction, systemd, health).await,
        Operation::Migrate => recover_migrate(root, transaction, systemd),
    }
}

/// 中断的迁移事务恢复:
/// - `preparing`:尚未停止旧实例,标记 `failed`(迁移 `.lkb` 保留在 `backups/`);
/// - `prepared`/`stopping`:恢复旧 unit 与事务前 systemd 状态后标记 `failed`;
/// - `activating`/`verifying`:停止新受管 unit,恢复旧 unit,清理新根后标记 `failed`。
fn recover_migrate(
    root: &InstallRoot,
    transaction: &TransactionFile,
    systemd: &dyn ServiceManager,
) -> Result<(), InstallError> {
    match transaction.phase {
        Phase::Preparing => {
            transaction::mark_phase(root, transaction, Phase::Failed)?;
            Ok(())
        }
        Phase::Prepared | Phase::Stopping => {
            crate::workflows::migrate::restore_legacy_unit(root, transaction, systemd)?;
            restore_pre_activation_systemd(root, transaction, systemd)?;
            transaction::mark_phase(root, transaction, Phase::Failed)?;
            Ok(())
        }
        Phase::Activating | Phase::Verifying | Phase::RollingBack => {
            crate::workflows::migrate::rollback_migrate(root, transaction, systemd)
        }
        phase => Err(InstallError::BlockedByTransaction(format!(
            "migrate transaction in terminal phase {} cannot be recovered",
            phase.key()
        ))),
    }
}

/// 中断的 reinit 事务恢复:
/// - `awaiting_network_confirmation`/`finalizing`/`rolling_back`:阻断并提示使用
///   `lkit network confirm` 或 `lkit network rollback`,与首次接管一致;
/// - `preparing`:清理事务临时内容并标记 `failed`;
/// - `prepared`/`stopping`:恢复事务前 systemd 状态后清理并标记 `failed`;
/// - `activating`/`verifying`:执行 reinit 回滚,优先用事务目录中的旧 data 现场。
async fn recover_reinit<P: DocsProbe>(
    root: &InstallRoot,
    transaction: &TransactionFile,
    systemd: &dyn ServiceManager,
    health: &HealthOptions<P>,
) -> Result<(), InstallError> {
    if matches!(
        transaction.phase,
        Phase::AwaitingNetworkConfirmation | Phase::Finalizing | Phase::RollingBack
    ) {
        return Err(InstallError::BlockedByTransaction(format!(
            "network reinit {} is {}; use `lkit network confirm` or `lkit network rollback`",
            transaction.transaction_id,
            transaction.phase.key()
        )));
    }
    match transaction.phase {
        Phase::Preparing => {
            let _ = std::fs::remove_dir_all(transaction_dir(root, transaction));
            transaction::mark_phase(root, transaction, Phase::Failed)?;
            Ok(())
        }
        Phase::Prepared | Phase::Stopping => {
            restore_pre_activation_systemd(root, transaction, systemd)?;
            if let Some(backup_path) = &transaction.resolv_conf_backup {
                let backup_dir = layout::territory_relative(backup_path);
                super::super::resolv::restore(systemd.resolv_conf(), &backup_dir)?;
            }
            let _ = std::fs::remove_dir_all(transaction_dir(root, transaction));
            transaction::mark_phase(root, transaction, Phase::Failed)?;
            Ok(())
        }
        Phase::Activating | Phase::Verifying => {
            crate::workflows::reinit::rollback_reinit(root, transaction, systemd, health).await
        }
        phase => Err(InstallError::BlockedByTransaction(format!(
            "reinit transaction in terminal phase {} cannot be recovered",
            phase.key()
        ))),
    }
}

/// 中断的卸载事务恢复:卸载是用户明确请求的终态,采用**前向完成**语义,不自动回滚。
/// - `preparing`:尚未改变运行状态,清理临时内容并标记 `failed`;
/// - `prepared`/`stopping`/`activating`:继续完成服务注销、受管内容删除并标记
///   `committed`;恢复再次失败标记 `failed` 并保留保护 `.lkb` 与事务现场。
fn recover_uninstall(
    root: &InstallRoot,
    transaction: &TransactionFile,
    systemd: &dyn ServiceManager,
) -> Result<(), InstallError> {
    match transaction.phase {
        Phase::Preparing => {
            transaction::mark_phase(root, transaction, Phase::Failed)?;
            Ok(())
        }
        Phase::Prepared | Phase::Stopping | Phase::Activating => {
            crate::workflows::uninstall::complete_uninstall(root, transaction, systemd)?;
            transaction::mark_phase(root, transaction, Phase::Committed)?;
            crate::workflows::uninstall::cleanup_runtime_dirs(root)?;
            Ok(())
        }
        phase => Err(InstallError::BlockedByTransaction(format!(
            "uninstall transaction in terminal phase {} cannot be recovered",
            phase.key()
        ))),
    }
}

/// 中断的 restore 事务恢复:
/// - `preparing`:清理事务临时内容并标记 `failed`;
/// - `prepared`/`stopping`:恢复事务前 systemd 状态后清理并标记 `failed`;
/// - `activating`/`verifying`/`rolling_back`:执行 restore 回滚,
///   优先用事务目录中的旧 data 现场,必要时使用保护 `.lkb`。
async fn recover_restore<P: DocsProbe>(
    root: &InstallRoot,
    transaction: &TransactionFile,
    systemd: &dyn ServiceManager,
    health: &HealthOptions<P>,
) -> Result<(), InstallError> {
    match transaction.phase {
        Phase::Preparing => {
            let _ = std::fs::remove_dir_all(transaction_dir(root, transaction));
            transaction::mark_phase(root, transaction, Phase::Failed)?;
            Ok(())
        }
        Phase::Prepared | Phase::Stopping => {
            restore_pre_activation_systemd(root, transaction, systemd)?;
            let _ = std::fs::remove_dir_all(transaction_dir(root, transaction));
            transaction::mark_phase(root, transaction, Phase::Failed)?;
            Ok(())
        }
        Phase::Activating | Phase::Verifying | Phase::RollingBack => {
            crate::workflows::restore::rollback_restore(root, transaction, systemd, health).await
        }
        phase => Err(InstallError::BlockedByTransaction(format!(
            "restore transaction in terminal phase {} cannot be recovered",
            phase.key()
        ))),
    }
}

/// 中断的 repair 事务恢复:
/// - `preparing`:尚未触及运行态,清理事务临时内容并标记 `failed`;
/// - `prepared`/`stopping`:恢复事务前 systemd 状态后清理并标记 `failed`;
/// - `activating`/`verifying`:按事务记录的备份类型恢复;
///   - 含 `.lkb` 的 systemd 后端修复:按 `.lkb` 配置级回滚;
///   - 含 `.lkb` 的无 systemd 后端修复:只恢复修复前二进制,不重建 data、不启动;
///   - 含 `static_backup` 的静态修复:从备份恢复 `static/`;
///   - 纯观测 repair:不修改任何资产,直接标记 `failed`。
async fn recover_repair<P: DocsProbe>(
    root: &InstallRoot,
    transaction: &TransactionFile,
    systemd: &dyn ServiceManager,
    health: &HealthOptions<P>,
) -> Result<(), InstallError> {
    match transaction.phase {
        Phase::Preparing => {
            let _ = std::fs::remove_dir_all(transaction_dir(root, transaction));
            transaction::mark_phase(root, transaction, Phase::Failed)?;
            Ok(())
        }
        Phase::Prepared | Phase::Stopping => {
            restore_pre_activation_systemd(root, transaction, systemd)?;
            let _ = std::fs::remove_dir_all(transaction_dir(root, transaction));
            transaction::mark_phase(root, transaction, Phase::Failed)?;
            Ok(())
        }
        Phase::Activating | Phase::Verifying | Phase::RollingBack => {
            if transaction.backup.is_some() {
                recover_binary_repair(root, transaction, systemd, health).await
            } else if transaction.static_backup.is_some() {
                recover_static_repair(root, transaction)
            } else {
                transaction::mark_phase(root, transaction, Phase::Failed)?;
                Ok(())
            }
        }
        phase => Err(InstallError::BlockedByTransaction(format!(
            "repair transaction in terminal phase {} cannot be recovered",
            phase.key()
        ))),
    }
}

async fn recover_binary_repair<P: DocsProbe>(
    root: &InstallRoot,
    transaction: &TransactionFile,
    systemd: &dyn ServiceManager,
    health: &HealthOptions<P>,
) -> Result<(), InstallError> {
    let snapshot = super::super::rollback::read_state_snapshot(root, &transaction.transaction_id)?;
    if snapshot.service.manager == super::super::state::StateServiceManager::Systemd {
        super::super::rollback::rollback_switch(root, transaction, &snapshot, systemd, health).await
    } else {
        let version = transaction
            .from_version
            .as_deref()
            .ok_or_else(|| corrupted("binary repair transaction is missing from_version".into()))?;
        let target = root
            .canonical
            .join("releases")
            .join(version)
            .join(super::super::artifacts::WEBSERVER_BINARY);
        let saved = transaction_dir(root, transaction).join("repaired-binary");
        restore_binary(&saved, &target)?;
        transaction::mark_phase(root, transaction, Phase::Failed)?;
        Ok(())
    }
}

fn recover_static_repair(
    root: &InstallRoot,
    transaction: &TransactionFile,
) -> Result<(), InstallError> {
    let backup = transaction
        .static_backup
        .as_ref()
        .ok_or_else(|| corrupted("static repair transaction is missing static_backup".into()))?;
    let backup_dir = layout::territory_relative(&backup.path);
    let target = root.canonical.join(&backup.target);
    let _ = std::fs::remove_dir_all(&target);
    super::super::rollback::copy_tree_into(&backup_dir, &target)?;
    transaction::mark_phase(root, transaction, Phase::Failed)?;
    Ok(())
}

/// 中断的 service manager 迁移恢复:
/// - `preparing`:尚未激活,标记 `failed`;
/// - `prepared`/`stopping`:恢复事务前 systemd 状态并标记 `failed`;
/// - `activating`/`verifying`:停止本次受管状态,按 `systemd_before` 恢复注册链接
///   与 enabled/active 状态,并按事务恢复 `/etc/resolv.conf`;不修改 `current` 或 data。
fn recover_migration(
    root: &InstallRoot,
    transaction: &TransactionFile,
    systemd: &dyn ServiceManager,
) -> Result<(), InstallError> {
    match transaction.phase {
        Phase::Preparing => {
            transaction::mark_phase(root, transaction, Phase::Failed)?;
            Ok(())
        }
        Phase::Prepared | Phase::Stopping => {
            restore_pre_activation_systemd(root, transaction, systemd)?;
            transaction::mark_phase(root, transaction, Phase::Failed)?;
            Ok(())
        }
        Phase::Activating | Phase::Verifying | Phase::RollingBack => {
            let before = transaction.systemd_before.as_ref().ok_or_else(|| {
                corrupted("service migration transaction is missing systemd_before".into())
            })?;
            let unit_origin = root
                .canonical
                .join("service")
                .join(systemd.service_name(ManagedService::LandscapeRouter));
            if let Err(restore_error) =
                systemd.restore_before(ManagedService::LandscapeRouter, before, &unit_origin)
            {
                let _ = transaction::mark_phase(root, transaction, Phase::Failed);
                return Err(restore_error);
            }
            if let Some(backup_path) = &transaction.resolv_conf_backup {
                let backup_dir = layout::territory_relative(backup_path);
                if let Err(restore_error) =
                    super::super::resolv::restore(systemd.resolv_conf(), &backup_dir)
                {
                    let _ = transaction::mark_phase(root, transaction, Phase::Failed);
                    return Err(restore_error);
                }
            }
            transaction::mark_phase(root, transaction, Phase::Failed)?;
            Ok(())
        }
        phase => Err(InstallError::BlockedByTransaction(format!(
            "service migration transaction in terminal phase {} cannot be recovered",
            phase.key()
        ))),
    }
}

fn transaction_dir(_root: &InstallRoot, transaction: &TransactionFile) -> std::path::PathBuf {
    layout::territory_transactions_dir().join(&transaction.transaction_id)
}

/// 用保存的二进制原子恢复 `releases/<version>/landscape-webserver`。
fn restore_binary(saved: &Path, target: &Path) -> Result<(), InstallError> {
    use std::os::unix::fs::PermissionsExt;
    let tmp = target.with_file_name(".landscape-webserver.tmp");
    std::fs::copy(saved, &tmp).map_err(InstallError::Io)?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))
        .map_err(InstallError::Io)?;
    std::fs::rename(&tmp, target).map_err(|error| {
        let _ = std::fs::remove_file(&tmp);
        InstallError::Io(error)
    })
}

async fn recover_switch<P: DocsProbe>(
    root: &InstallRoot,
    transaction: &TransactionFile,
    systemd: &dyn ServiceManager,
    health: &HealthOptions<P>,
) -> Result<(), InstallError> {
    match transaction.phase {
        Phase::Preparing => {
            // 尚未修改 current:清理本次目标 release,保持当前版本并标记 failed。
            if let Some(target_release) = transaction.target_release.as_deref() {
                let _ = std::fs::remove_dir_all(root.canonical.join(target_release));
            }
            if let Some(target_version) = transaction.target_version.as_deref() {
                let _ = std::fs::remove_dir_all(
                    root.canonical
                        .join("releases")
                        .join(format!(".install-{target_version}.tmp")),
                );
            }
            transaction::mark_phase(root, transaction, Phase::Failed)?;
            Ok(())
        }
        Phase::Prepared | Phase::Stopping => {
            // `stopping` 表示 stop 可能已经发生;`prepared` 的 v1 事务也可能落在
            // 同一崩溃窗口。恢复操作是幂等的,因此两者统一恢复事务前运行态。
            restore_pre_activation_systemd(root, transaction, systemd)?;
            if let Some(target_release) = transaction.target_release.as_deref() {
                let _ = std::fs::remove_dir_all(root.canonical.join(target_release));
            }
            if let Some(target_version) = transaction.target_version.as_deref() {
                let _ = std::fs::remove_dir_all(
                    root.canonical
                        .join("releases")
                        .join(format!(".install-{target_version}.tmp")),
                );
            }
            transaction::mark_phase(root, transaction, Phase::Failed)?;
            Ok(())
        }
        Phase::Activating | Phase::Verifying | Phase::RollingBack => {
            if transaction.backup.is_some() {
                let snapshot =
                    super::super::rollback::read_state_snapshot(root, &transaction.transaction_id)?;
                if snapshot.service.manager == super::super::state::StateServiceManager::Systemd {
                    super::super::rollback::rollback_switch(
                        root,
                        transaction,
                        &snapshot,
                        systemd,
                        health,
                    )
                    .await?;
                    Ok(())
                } else {
                    // 无 systemd:只恢复尚未提交的 current,不重建 data、不启动、不检查。
                    let previous_current =
                        transaction.previous_current.as_deref().ok_or_else(|| {
                            InstallError::CorruptedTransaction(
                                "switch transaction is missing previous_current".into(),
                            )
                        })?;
                    restore_current_link(root, previous_current)?;
                    transaction::mark_phase(root, transaction, Phase::Failed)?;
                    Ok(())
                }
            } else if transaction.no_backup {
                // 无 `.lkb` 切换:状态文件仍是切换前的旧状态,直接按旧状态回滚。
                let previous = super::super::state::load_state(root)?.ok_or_else(|| {
                    InstallError::CorruptedState(
                        "install state disappeared during the no-backup switch".into(),
                    )
                })?;
                super::super::rollback::rollback_no_backup(
                    root,
                    transaction,
                    &previous,
                    systemd,
                    health,
                )
                .await?;
                Ok(())
            } else {
                Err(InstallError::CorruptedTransaction(
                    "switch transaction in a non-terminal phase must record a .lkb backup or no_backup"
                        .into(),
                ))
            }
        }
        phase => Err(InstallError::BlockedByTransaction(format!(
            "switch transaction in terminal phase {} cannot be recovered",
            phase.key()
        ))),
    }
}

fn restore_current_link(root: &InstallRoot, target: &str) -> Result<(), InstallError> {
    let current = root.canonical.join("current");
    let tmp = layout::territory_run_dir().join(".current.tmp");
    std::fs::create_dir_all(tmp.parent().expect("run dir has a parent"))
        .map_err(InstallError::Io)?;
    let _ = std::fs::remove_file(&tmp);
    std::os::unix::fs::symlink(target, &tmp).map_err(InstallError::Io)?;
    std::fs::rename(&tmp, &current).map_err(InstallError::Io)?;
    Ok(())
}

fn restore_pre_activation_systemd(
    root: &InstallRoot,
    transaction: &TransactionFile,
    systemd: &dyn ServiceManager,
) -> Result<(), InstallError> {
    let Some(before) = transaction.systemd_before.as_ref() else {
        return Ok(());
    };
    let unit_origin = root
        .canonical
        .join("service")
        .join(systemd.service_name(ManagedService::LandscapeRouter));
    systemd.restore_before(ManagedService::LandscapeRouter, before, &unit_origin)
}

/// 首次安装失败清理:恢复 systemd 注册与 enabled/active 状态、恢复
/// `/etc/resolv.conf`、移除本次创建的 `current`、release、初始化文件和状态文件。
fn corrupted(reason: String) -> InstallError {
    InstallError::CorruptedTransaction(reason)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::deployment::layout;
    use crate::service::systemd::Systemd;

    use super::super::{
        BackupRef, NetworkTakeoverTransaction, Registration, RegistrationKind, ServiceBefore,
        begin, find_unfinished, load_transaction_file, mark_phase,
    };
    use chrono::Utc;

    /// 建立隔离测试现场:返回 (守卫, 地盘, landscape 根)。
    fn setup(
        name: &str,
    ) -> (
        layout::TerritoryOverride,
        std::path::PathBuf,
        std::path::PathBuf,
    ) {
        let temp = std::env::temp_dir().join(format!("lkit-tx-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();
        let territory = temp.join("territory");
        std::fs::create_dir_all(&territory).unwrap();
        let guard = layout::test_territory(&territory);
        let root = temp.join("landscape");
        std::fs::create_dir_all(&root).unwrap();
        (guard, territory, root)
    }

    fn new_root(path: &std::path::Path) -> InstallRoot {
        InstallRoot {
            install_root: path.to_path_buf(),
            canonical: path.to_path_buf(),
        }
    }
    struct FakeDocs;

    impl super::super::super::health::DocsProbe for FakeDocs {
        async fn docs_ok(&self) -> bool {
            true
        }
    }

    fn test_health() -> super::super::super::health::HealthOptions<FakeDocs> {
        super::super::super::health::HealthOptions {
            docs: FakeDocs,
            ports: Vec::new(),
            startup_timeout: std::time::Duration::from_secs(5),
            stable_duration: std::time::Duration::from_millis(100),
        }
    }

    fn install_transaction(root: &InstallRoot) -> TransactionFile {
        TransactionFile::new_install(root, &semver::Version::new(1, 2, 3)).unwrap()
    }

    #[test]
    fn recovers_interrupted_reinit_in_preparing() {
        let (_guard, territory, root) = setup("reinit-recover");
        let root = new_root(&root);
        let transaction =
            TransactionFile::new_reinit(&root, &semver::Version::new(1, 2, 3)).unwrap();
        begin(&root, &transaction).unwrap();
        let tx = find_unfinished(&root).unwrap().unwrap();
        let health = test_health();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            recover_interrupted(&root, &tx, &Systemd::host(), &health)
                .await
                .unwrap()
        });
        let tx = load_finished(&root, &territory);
        assert_eq!(tx.phase, Phase::Failed);
        assert!(find_unfinished(&root).unwrap().is_none());
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    #[test]
    fn blocks_reinit_awaiting_confirmation_on_recovery() {
        let (_guard, territory, root) = setup("reinit-blocked");
        let root = new_root(&root);
        let mut transaction =
            TransactionFile::new_reinit(&root, &semver::Version::new(1, 2, 3)).unwrap();
        transaction.network_takeover = Some(NetworkTakeoverTransaction {
            plan: crate::network::config::NetworkPlan {
                mode: crate::network::config::NetworkMode::WanDhcp { wan: "ens3".into() },
                selected_macs: vec![crate::network::config::SelectedInterface {
                    name: "ens3".into(),
                    mac: "02:00:00:00:00:03".into(),
                }],
            },
            host_services: Vec::new(),
            confirmation_deadline: Utc::now(),
            rollback_service: "lkit-network-tx-rollback.service".into(),
            rollback_timer: "lkit-network-tx-rollback.timer".into(),
            boot_rollback_service: "lkit-network-tx-boot-rollback.service".into(),
            recovery_binary: "service/lkit-network-recovery".into(),
            pending_state: "transactions/tx/pending-install-state.json".into(),
        });
        transaction.backup = Some(BackupRef {
            backup_id: "b".into(),
            path: "backups/b.lkb".into(),
            sha256: "a".repeat(64),
        });
        begin(&root, &transaction).unwrap();
        mark_phase(&root, &transaction, Phase::AwaitingNetworkConfirmation).unwrap();
        let tx = find_unfinished(&root).unwrap().unwrap();
        let health = test_health();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let error = runtime
            .block_on(async { recover_interrupted(&root, &tx, &Systemd::host(), &health).await });
        assert!(matches!(error, Err(InstallError::BlockedByTransaction(_))));
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    #[test]
    fn recovers_interrupted_uninstall_by_forward_completion() {
        let (_guard, territory, root_path) = setup("uninstall-recover");
        let root = new_root(&root_path);
        let mut transaction =
            TransactionFile::new_uninstall(&root, &semver::Version::new(1, 2, 3)).unwrap();
        transaction.backup = Some(BackupRef {
            backup_id: "b".into(),
            path: "backups/b.lkb".into(),
            sha256: "a".repeat(64),
        });
        transaction.no_backup = false;
        begin(&root, &transaction).unwrap();
        mark_phase(&root, &transaction, Phase::Activating).unwrap();

        std::fs::create_dir_all(root_path.join("releases/1.2.3")).unwrap();
        std::os::unix::fs::symlink("releases/1.2.3", root_path.join("current")).unwrap();
        std::fs::create_dir_all(root_path.join("data")).unwrap();
        std::fs::create_dir_all(root_path.join("service")).unwrap();
        std::fs::create_dir_all(territory.join("state")).unwrap();
        std::fs::write(territory.join("state/install-state.json"), b"{}").unwrap();
        std::fs::create_dir_all(territory.join("backups")).unwrap();
        std::fs::write(territory.join("config.toml"), b"[repository]\n").unwrap();

        let tx = find_unfinished(&root).unwrap().unwrap();
        let health = test_health();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            recover_interrupted(&root, &tx, &Systemd::host(), &health)
                .await
                .unwrap()
        });
        assert!(!root_path.join("releases").exists());
        assert!(!root_path.join("current").exists());
        assert!(!root_path.join("data").exists());
        assert!(!root_path.join("service").exists());
        assert!(
            !territory.join("state/install-state.json").exists(),
            "the territory install state must be removed by the forward completion"
        );
        assert_eq!(
            std::fs::read_to_string(territory.join("config.toml")).unwrap(),
            "[repository]\n"
        );
        assert!(territory.join("backups").is_dir());
        assert!(territory.join("transactions").is_dir());
        let tx = load_finished(&root, &territory);
        assert_eq!(tx.phase, Phase::Committed);
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    #[test]
    fn keeps_uninstall_in_preparing_on_recovery() {
        let (_guard, territory, root) = setup("uninstall-preparing");
        let root = new_root(&root);
        let transaction =
            TransactionFile::new_uninstall(&root, &semver::Version::new(1, 2, 3)).unwrap();
        begin(&root, &transaction).unwrap();
        let tx = find_unfinished(&root).unwrap().unwrap();
        let health = test_health();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            recover_interrupted(&root, &tx, &Systemd::host(), &health)
                .await
                .unwrap()
        });
        let tx = load_finished(&root, &territory);
        assert_eq!(tx.phase, Phase::Failed);
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    #[test]
    fn recovers_interrupted_install() {
        let (_guard, territory, root_path) = setup("recover");
        let root = new_root(&root_path);
        let transaction = install_transaction(&root);
        begin(&root, &transaction).unwrap();
        mark_phase(&root, &transaction, Phase::Activating).unwrap();

        std::fs::create_dir_all(root_path.join("releases/1.2.3/static")).unwrap();
        std::os::unix::fs::symlink("releases/1.2.3", root_path.join("current")).unwrap();
        std::fs::create_dir_all(root_path.join("data")).unwrap();
        std::fs::write(
            root_path.join("data/landscape_init.toml"),
            b"version = \"1.2.3\"",
        )
        .unwrap();
        std::fs::create_dir_all(territory.join("state")).unwrap();
        std::fs::write(territory.join("state/install-state.json"), b"{}").unwrap();

        let tx = find_unfinished(&root).unwrap().unwrap();
        let health = test_health();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            recover_interrupted(&root, &tx, &Systemd::host(), &health)
                .await
                .unwrap()
        });
        assert!(!root_path.join("releases/1.2.3").exists());
        assert!(!root_path.join("current").exists());
        assert!(!root_path.join("data/landscape_init.toml").exists());
        assert!(!territory.join("state/install-state.json").exists());
        assert!(find_unfinished(&root).unwrap().is_none());
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    #[test]
    fn keeps_completed_install_on_recovery() {
        let (_guard, territory, root_path) = setup("keep");
        let root = new_root(&root_path);
        let transaction = install_transaction(&root);
        begin(&root, &transaction).unwrap();
        mark_phase(&root, &transaction, Phase::Activating).unwrap();

        std::fs::create_dir_all(root_path.join("releases/1.2.3")).unwrap();
        std::os::unix::fs::symlink("releases/1.2.3", root_path.join("current")).unwrap();
        std::fs::create_dir_all(root_path.join("service")).unwrap();
        std::fs::write(
            root_path.join("service/landscape-router.service"),
            b"[Unit]\n",
        )
        .unwrap();
        std::fs::create_dir_all(territory.join("state")).unwrap();
        let state = serde_json::json!({
            "schema_version": 1,
            "layout_version": 2,
            "install_root": root_path.display().to_string(),
            "canonical_install_root": std::fs::canonicalize(&root_path).unwrap().display().to_string(),
            "active_version": "1.2.3",
            "repository": {"kind": "http", "location": "https://example.com/"},
            "assets": {
                "webserver": {"architecture": "x86_64", "sha256": "a".repeat(64), "size": 1},
                "static_archive": {"sha256": "b".repeat(64), "size": 1}
            },
            "initialization": {"status": "pending", "lock_present": false, "initialized_at": null},
            "service": {"manager": "systemd", "registered": true, "enabled": true, "verified": true, "definition_path": "service/landscape-router.service", "definition_sha256": "c".repeat(64)},
            "last_transaction_id": null,
            "committed_at": null
        });
        std::fs::write(
            territory.join("state/install-state.json"),
            state.to_string(),
        )
        .unwrap();

        let tx = find_unfinished(&root).unwrap().unwrap();
        let health = test_health();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            recover_interrupted(&root, &tx, &Systemd::host(), &health)
                .await
                .unwrap()
        });
        assert!(root_path.join("releases/1.2.3").exists());
        assert!(root_path.join("current").exists());
        assert!(territory.join("state/install-state.json").exists());
        let tx = load_finished(&root, &territory);
        assert_eq!(tx.phase, Phase::Committed);
        assert!(find_unfinished(&root).unwrap().is_none());
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    fn load_finished(root: &InstallRoot, territory: &std::path::Path) -> TransactionFile {
        let entries: Vec<_> = std::fs::read_dir(territory.join("transactions"))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(entries.len(), 1);
        load_transaction_file(root, &entries[0].path()).unwrap()
    }

    #[test]
    fn recovers_failed_switch_before_backup_creation() {
        let (_guard, territory, root_path) = setup("switch-recover");
        let root = new_root(&root_path);
        let transaction = TransactionFile::new_switch(
            &root,
            &semver::Version::new(1, 1, 0),
            &semver::Version::new(1, 2, 3),
        )
        .unwrap();
        begin(&root, &transaction).unwrap();
        mark_phase(&root, &transaction, Phase::Failed).unwrap();
        assert!(find_unfinished(&root).unwrap().is_none());

        let transaction = TransactionFile::new_switch(
            &root,
            &semver::Version::new(1, 1, 0),
            &semver::Version::new(1, 3, 0),
        )
        .unwrap();
        begin(&root, &transaction).unwrap();
        std::fs::create_dir_all(root_path.join("releases/1.3.0/static")).unwrap();

        let tx = find_unfinished(&root).unwrap().unwrap();
        let health = test_health();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            recover_interrupted(&root, &tx, &Systemd::host(), &health)
                .await
                .unwrap()
        });
        assert!(!root_path.join("releases/1.3.0").exists());
        assert!(find_unfinished(&root).unwrap().is_none());
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    #[test]
    fn recovers_repair_transaction_in_preparing() {
        let (_guard, territory, root) = setup("repair-recover");
        let root = new_root(&root);
        let tx = TransactionFile::new_repair_binary(&root, &semver::Version::new(1, 1, 0)).unwrap();
        begin(&root, &tx).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            recover_interrupted(&root, &tx, &Systemd::host(), &test_health())
                .await
                .unwrap()
        });
        assert!(find_unfinished(&root).unwrap().is_none());
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }
}
