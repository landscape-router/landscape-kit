use std::path::Path;

use chrono::Utc;

use super::super::health::{
    DocsProbe, HealthOptions, StartupOptions, observe_stable, wait_for_startup,
};
use super::super::manager::{ManagedService, ServiceManager};
use super::super::plan::InstallError;
use super::super::resolv;
use super::super::rollback as rollback_util;
use super::super::root::InstallRoot;
use super::super::state::{self, StateServiceManager};
use super::super::transaction::{Phase, TransactionFile, mark_phase};
use crate::deployment::layout;

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
    manager: &dyn ServiceManager,
    health: &HealthOptions<P>,
) -> Result<(), InstallError> {
    mark_phase(root, transaction, Phase::RollingBack)?;
    let result = rollback_restore_inner(root, transaction, manager, health).await;
    if result.is_err() {
        let _ = mark_phase(root, transaction, Phase::Failed);
    }
    result
}

async fn rollback_restore_inner<P: DocsProbe>(
    root: &InstallRoot,
    transaction: &TransactionFile,
    manager: &dyn ServiceManager,
    health: &HealthOptions<P>,
) -> Result<(), InstallError> {
    let snapshot = rollback_util::read_state_snapshot(root, &transaction.transaction_id)?;
    let is_systemd = snapshot.service.manager == StateServiceManager::Systemd;
    if is_systemd {
        manager.stop_and_wait(
            ManagedService::LandscapeRouter,
            &(|| {
                manager
                    .active_state(ManagedService::LandscapeRouter)
                    .map(|value| value != "active")
                    .unwrap_or(true)
            }),
        )?;
    }
    if let Some(before) = &transaction.systemd_before {
        let unit_origin = root.canonical.join("service/landscape-router.service");
        manager.restore_registration(ManagedService::LandscapeRouter, before, &unit_origin)?;
        if let Some(backup_path) = &transaction.resolv_conf_backup {
            let backup_dir = layout::territory_relative(backup_path);
            resolv::restore(manager.resolv_conf(), &backup_dir)?;
        }
    }

    let tx_dir = layout::territory_transactions_dir().join(&transaction.transaction_id);
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
        rollback_util::restore_current(root, previous_current)?;
        if previous_data.exists() {
            restore_previous_data(&data, &previous_data)?;
        }
        let was_active = transaction
            .systemd_before
            .as_ref()
            .is_some_and(|before| before.active);
        if is_systemd && was_active {
            manager.start(ManagedService::LandscapeRouter)?;
            let pid = manager.main_pid(ManagedService::LandscapeRouter)?;
            if pid == 0 {
                return Err(InstallError::Systemd(
                    "restored service did not produce a main pid".into(),
                ));
            }
            let startup = StartupOptions {
                ports: &health.ports,
                expected_pid: pid,
                docs: &health.docs,
                unit_state: Some(&(|| manager.active_state(ManagedService::LandscapeRouter).ok())),
                init_required: true,
                data_dir: &data,
                startup_timeout: health.startup_timeout,
                stable_duration: health.stable_duration,
            };
            wait_for_startup(&startup).await?;
            observe_stable(&startup).await?;
        }
        let mut restored = snapshot.clone();
        restored.last_transaction_id = Some(transaction.transaction_id.clone());
        restored.committed_at = Some(Utc::now());
        state::write_state(root, &restored)?;
        mark_phase(root, transaction, Phase::RolledBack)?;
        Ok(())
    } else {
        // 事务现场损坏(previous-data 与 data 均不存在):只能使用保护快照或报损坏。
        if transaction.backup.is_some() {
            rollback_util::rollback_switch(root, transaction, &snapshot, manager, health).await
        } else {
            rollback_util::rollback_no_backup(root, transaction, &snapshot, manager, health).await
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
    let tx_dir = layout::territory_transactions_dir().join(&transaction.transaction_id);
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
pub(crate) fn restore_previous_data(data: &Path, previous_data: &Path) -> Result<(), InstallError> {
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

pub(crate) fn write_file_atomic(path: &Path, bytes: &[u8], mode: u32) -> Result<(), InstallError> {
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
    use super::*;

    use super::super::tests::{
        NonInteractiveGuard, PAYLOAD_1_2_3, PAYLOAD_1_3_0, TOKEN, YES, ZIP_1_2_3, ZIP_1_3_0,
        activate_version, create_target_backup, export_server, fake_systemd_stateful,
        install_state, interactive_guard, none_health, setup_current, temp_root,
    };
    use super::super::{RestoreArgs, RestoreOptions, restore_version};
    use crate::backup::rollback::write_state_snapshot;
    use crate::deployment::state::{load_state, write_state};
    use crate::deployment::transaction::recovery::recover_interrupted;
    use crate::deployment::transaction::{
        BackupRef, Phase, TransactionFile, begin, find_unfinished, mark_phase, persist,
    };
    use crate::service::systemd::Systemd;

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
        begin(&install_root, &transaction).unwrap();
        transaction.restore_backup = Some(BackupRef {
            backup_id: "t".into(),
            path: "backups/t.lkb".into(),
            sha256: "a".repeat(64),
        });
        persist(&install_root, &transaction).unwrap();
        write_state_snapshot(&install_root, &transaction.transaction_id, &state).unwrap();
        let tx_dir = install_root
            .canonical
            .join("transactions")
            .join(&transaction.transaction_id);
        std::fs::create_dir_all(tx_dir.join("previous-data")).unwrap();
        std::fs::write(tx_dir.join("previous-data/landscape_db.sqlite"), b"old-db").unwrap();
        mark_phase(&install_root, &transaction, Phase::Activating).unwrap();
        std::fs::write(install_root.canonical.join("data/partial"), b"partial").unwrap();

        let systemd = fake_systemd_stateful(&root.join("fake-systemd"));
        rollback_restore(&install_root, &transaction, &systemd, &none_health())
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
        let restored = load_state(&install_root).unwrap().unwrap();
        assert_eq!(restored.active_version, "1.3.0");
        assert_eq!(
            restored.last_transaction_id.as_deref(),
            Some(transaction.transaction_id.as_str())
        );
        assert!(find_unfinished(&install_root).unwrap().is_none());
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
        begin(&install_root, &transaction).unwrap();
        transaction.restore_backup = Some(BackupRef {
            backup_id: "t".into(),
            path: "backups/t.lkb".into(),
            sha256: "a".repeat(64),
        });
        persist(&install_root, &transaction).unwrap();
        write_state_snapshot(&install_root, &transaction.transaction_id, &state).unwrap();
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
        mark_phase(&install_root, &transaction, Phase::Activating).unwrap();

        let systemd = fake_systemd_stateful(&root.join("fake-systemd"));
        rollback_restore(&install_root, &transaction, &systemd, &none_health())
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
        let restored = load_state(&install_root).unwrap().unwrap();
        assert_eq!(restored.active_version, "1.3.0");
        assert!(find_unfinished(&install_root).unwrap().is_none());
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
        begin(&install_root, &transaction).unwrap();
        transaction.restore_backup = Some(BackupRef {
            backup_id: "t".into(),
            path: "backups/t.lkb".into(),
            sha256: "a".repeat(64),
        });
        persist(&install_root, &transaction).unwrap();
        write_state_snapshot(&install_root, &transaction.transaction_id, &state).unwrap();
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
        mark_phase(&install_root, &transaction, Phase::Activating).unwrap();

        let systemd = fake_systemd_stateful(&root.join("fake-systemd"));
        rollback_restore(&install_root, &transaction, &systemd, &none_health())
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
        assert!(find_unfinished(&install_root).unwrap().is_none());
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
        begin(&install_root, &transaction).unwrap();
        transaction.restore_backup = Some(BackupRef {
            backup_id: "t".into(),
            path: "backups/t.lkb".into(),
            sha256: "a".repeat(64),
        });
        persist(&install_root, &transaction).unwrap();
        write_state_snapshot(&install_root, &transaction.transaction_id, &state).unwrap();
        let tx_dir = install_root
            .canonical
            .join("transactions")
            .join(&transaction.transaction_id);
        std::fs::create_dir_all(tx_dir.join("previous-data")).unwrap();
        std::fs::write(tx_dir.join("previous-data/landscape_db.sqlite"), b"old-db").unwrap();
        mark_phase(&install_root, &transaction, Phase::Activating).unwrap();
        // current 变成普通目录:restore_current 的 rename 必然失败。
        let current = install_root.canonical.join("current");
        std::fs::remove_file(&current).unwrap();
        std::fs::create_dir_all(current.join("occupied")).unwrap();

        let systemd = fake_systemd_stateful(&root.join("fake-systemd"));
        assert!(
            rollback_restore(&install_root, &transaction, &systemd, &none_health(),)
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
        assert!(find_unfinished(&install_root).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
