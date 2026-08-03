use std::path::Path;

use super::super::health::{DocsProbe, HealthOptions};
use super::super::plan::InstallError;
use super::super::root::InstallRoot;
use super::super::systemd::Systemd;
use super::{self as transaction, Operation, Phase, TransactionFile};

pub(crate) async fn recover_interrupted<P: DocsProbe>(
    root: &InstallRoot,
    transaction: &TransactionFile,
    systemd: &Systemd,
    health: &HealthOptions<P>,
) -> Result<(), InstallError> {
    match transaction.operation {
        Operation::Install => {
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
                let _ = transaction::mark_phase(root, transaction, Phase::Failed);
                return Err(error);
            }
            transaction::mark_phase(root, transaction, Phase::Failed)?;
            Ok(())
        }
        Operation::Switch => recover_switch(root, transaction, systemd, health).await,
        Operation::Repair => recover_repair(root, transaction, systemd, health).await,
        Operation::ServiceMigration => recover_migration(root, transaction, systemd),
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
    systemd: &Systemd,
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
    systemd: &Systemd,
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
    let backup_dir = root.canonical.join(&backup.path);
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
    systemd: &Systemd,
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
            let unit_origin = root.canonical.join("service/landscape-router.service");
            if let Err(restore_error) =
                super::super::systemd::restore_systemd_before(systemd, before, &unit_origin)
            {
                let _ = transaction::mark_phase(root, transaction, Phase::Failed);
                return Err(restore_error);
            }
            if let Some(backup_path) = &transaction.resolv_conf_backup {
                let backup_dir = root.canonical.join(backup_path);
                if let Err(restore_error) =
                    super::super::resolv::restore(&systemd.resolv_conf, &backup_dir)
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

fn transaction_dir(root: &InstallRoot, transaction: &TransactionFile) -> std::path::PathBuf {
    root.canonical
        .join("transactions")
        .join(&transaction.transaction_id)
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
    systemd: &Systemd,
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
    let tmp = root.canonical.join("run/.current.tmp");
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
    systemd: &Systemd,
) -> Result<(), InstallError> {
    let Some(before) = transaction.systemd_before.as_ref() else {
        return Ok(());
    };
    let unit_origin = root.canonical.join("service/landscape-router.service");
    super::super::systemd::restore_systemd_before(systemd, before, &unit_origin)
}

/// 首次安装失败清理:恢复 systemd 注册与 enabled/active 状态、恢复
/// `/etc/resolv.conf`、移除本次创建的 `current`、release、初始化文件和状态文件。
fn corrupted(reason: String) -> InstallError {
    InstallError::CorruptedTransaction(reason)
}
