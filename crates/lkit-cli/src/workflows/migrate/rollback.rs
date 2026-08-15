use std::path::Path;

use super::super::plan::InstallError;
use super::super::root::InstallRoot;
use super::super::systemd::{self, Systemd};
use super::super::transaction::{HostServiceBefore, Phase, TransactionFile};

/// 迁移失败回滚:恢复旧实例并清理新根,使安装根回到迁移前状态。
/// 顺序固定为:注销并停止新受管 unit → 恢复 `/etc/resolv.conf` → 恢复旧 unit
/// (enabled/active)→ 清理新根内容。旧 unit 为前台进程时无法自动重启,由用户负责。
pub(crate) fn rollback_migrate(
    root: &InstallRoot,
    transaction: &TransactionFile,
    systemd: &Systemd,
) -> Result<(), InstallError> {
    super::super::transaction::mark_phase(root, transaction, Phase::RollingBack)?;
    let result = rollback_migrate_inner(root, transaction, systemd);
    if result.is_err() {
        let _ = super::super::transaction::mark_phase(root, transaction, Phase::Failed);
    }
    result
}

fn rollback_migrate_inner(
    root: &InstallRoot,
    transaction: &TransactionFile,
    systemd: &Systemd,
) -> Result<(), InstallError> {
    // 注销/停止新受管 unit(事务前为未注册,幂等),恢复 resolv.conf,
    // 移除本次创建的 release、current、初始化文件与状态文件。
    super::super::transaction::cleanup_failed_first_install(root, transaction, systemd)?;
    restore_legacy_unit(root, transaction, systemd)?;
    cleanup_migrated_root(root)?;
    super::super::transaction::mark_phase(root, transaction, Phase::RolledBack)?;
    Ok(())
}

/// 恢复旧 unit:原件被移入事务目录时放回原位并 daemon-reload,再按事务前
/// enabled/active 状态恢复(与原位 mask 场景共用同一恢复入口)。
/// 前台进程场景(`legacy_unit` 为 None)不执行任何操作。
pub(crate) fn restore_legacy_unit(
    root: &InstallRoot,
    transaction: &TransactionFile,
    systemd: &Systemd,
) -> Result<(), InstallError> {
    let Some(before) = &transaction.legacy_unit else {
        return Ok(());
    };
    if before.file_moved {
        let backup_path = before.file_backup.as_deref().ok_or_else(|| {
            InstallError::CorruptedTransaction("moved legacy unit is missing file_backup".into())
        })?;
        let file_path = before.file_path.as_deref().ok_or_else(|| {
            InstallError::CorruptedTransaction("moved legacy unit is missing file_path".into())
        })?;
        move_file(&root.canonical.join(backup_path), Path::new(file_path))?;
        systemd::daemon_reload(systemd)?;
    }
    systemd::restore_host_service(
        systemd,
        &HostServiceBefore {
            unit: before.unit.clone(),
            installed: true,
            active: before.active,
            enable_state: before.enable_state.clone(),
        },
    )
}

/// 删除迁移创建但未提交的根目录内容。`backups/`(迁移 `.lkb`)与
/// `transactions/`(事务文件)保留;`run/` 与 `logs/` 保留(锁文件与事务日志)。
pub(crate) fn cleanup_migrated_root(root: &InstallRoot) -> Result<(), InstallError> {
    let _ = std::fs::remove_dir_all(root.canonical.join("data"));
    let _ = std::fs::remove_dir_all(root.canonical.join("service"));
    let _ = std::fs::remove_dir_all(root.canonical.join("state"));
    Ok(())
}

/// 跨文件系统安全的文件移动:rename 失败且为 EXDEV 时复制后删除。
pub(crate) fn move_file(source: &Path, target: &Path) -> Result<(), InstallError> {
    match std::fs::rename(source, target) {
        Ok(()) => Ok(()),
        Err(error) if error.raw_os_error() == Some(libc::EXDEV) => {
            std::fs::copy(source, target).map_err(InstallError::Io)?;
            std::fs::remove_file(source).map_err(InstallError::Io)
        }
        Err(error) => Err(InstallError::Io(error)),
    }
}
