use std::path::Path;

use super::super::manager::{ManagedService, ServiceManager};
use super::super::plan::InstallError;
use super::super::root::InstallRoot;
use super::{Operation, Phase, TransactionFile};

pub(crate) fn cleanup_failed_first_install(
    root: &InstallRoot,
    transaction: &TransactionFile,
    manager: &dyn ServiceManager,
) -> Result<(), InstallError> {
    if let Some(before) = &transaction.systemd_before {
        let unit_origin = root
            .canonical
            .join("service")
            .join(manager.service_name(ManagedService::LandscapeRouter));
        manager.restore_before(ManagedService::LandscapeRouter, before, &unit_origin)?;
        if let Some(backup_path) = &transaction.resolv_conf_backup {
            let backup_dir = root.canonical.join(backup_path);
            super::super::resolv::restore(manager.resolv_conf(), &backup_dir)?;
        }
    }
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
    let _ = std::fs::remove_file(root.canonical.join("run/.current.tmp"));
    if let Some(target_release) = transaction.target_release.as_deref()
        && let Ok(target) = std::fs::read_link(root.canonical.join("current"))
        && target == Path::new(target_release)
    {
        let _ = std::fs::remove_file(root.canonical.join("current"));
    }
    let _ = std::fs::remove_file(root.canonical.join("data/landscape_init.toml"));
    let _ = std::fs::remove_file(root.canonical.join("state/install-state.json"));
    Ok(())
}

/// Strict cleanup for an uncommitted network takeover install.
///
/// Unlike ordinary first-install failure cleanup, this path may remove the
/// entire Landscape data directory. It is only valid while the install has no
/// previous version, backup, or committed state.
pub(crate) fn cleanup_uncommitted_network_install(
    root: &InstallRoot,
    transaction: &TransactionFile,
) -> Result<(), InstallError> {
    validate_network_takeover_rollback(root, transaction)?;
    let current_present = validate_current_for_target(root, transaction.target_release.as_deref())?;

    if current_present {
        std::fs::remove_file(root.canonical.join("current")).map_err(InstallError::Io)?;
    }
    if let Some(target_release) = transaction.target_release.as_deref() {
        remove_path_if_present(&root.canonical.join(target_release))?;
    }
    if let Some(target_version) = transaction.target_version.as_deref() {
        remove_path_if_present(
            &root
                .canonical
                .join("releases")
                .join(format!(".install-{target_version}.tmp")),
        )?;
    }
    remove_path_if_present(&root.canonical.join("run/.current.tmp"))?;
    remove_path_if_present(&root.canonical.join("state/install-state.json"))?;
    if let Some(network) = &transaction.network_takeover {
        remove_path_if_present(&root.canonical.join(&network.pending_state))?;
    }
    remove_path_if_present(&root.canonical.join("data"))?;
    Ok(())
}

pub(crate) fn restore_uncommitted_network_systemd(
    root: &InstallRoot,
    transaction: &TransactionFile,
    manager: &dyn ServiceManager,
) -> Result<(), InstallError> {
    validate_network_takeover_rollback(root, transaction)?;
    if let Some(before) = &transaction.systemd_before {
        let unit_origin = root
            .canonical
            .join("service")
            .join(manager.service_name(ManagedService::LandscapeRouter));
        manager.restore_before(ManagedService::LandscapeRouter, before, &unit_origin)?;
        if let Some(backup_path) = &transaction.resolv_conf_backup {
            let backup_dir = root.canonical.join(backup_path);
            super::super::resolv::restore(manager.resolv_conf(), &backup_dir)?;
        }
    }
    Ok(())
}

fn validate_network_takeover_rollback(
    root: &InstallRoot,
    transaction: &TransactionFile,
) -> Result<(), InstallError> {
    if transaction.operation != Operation::Install
        || transaction.network_takeover.is_none()
        || !matches!(
            transaction.phase,
            Phase::AwaitingNetworkConfirmation | Phase::Finalizing | Phase::RollingBack
        )
    {
        return Err(InstallError::BlockedByTransaction(format!(
            "transaction {} is not an uncommitted network takeover install",
            transaction.transaction_id
        )));
    }
    if transaction.from_version.is_some()
        || transaction.previous_current.is_some()
        || transaction.backup.is_some()
        || super::super::state::load_state(root)?.is_some()
    {
        return Err(InstallError::CorruptedTransaction(
            "network takeover rollback would affect an already committed installation".into(),
        ));
    }
    let data = root.canonical.join("data");
    if let Ok(metadata) = std::fs::symlink_metadata(&data) {
        if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
            return Err(InstallError::DangerousDirectory(format!(
                "{} is not a real data directory",
                data.display()
            )));
        }
    }
    Ok(())
}

fn validate_current_for_target(
    root: &InstallRoot,
    target_release: Option<&str>,
) -> Result<bool, InstallError> {
    let current = root.canonical.join("current");
    let metadata = match std::fs::symlink_metadata(&current) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(InstallError::Io(error)),
    };
    if !metadata.file_type().is_symlink() {
        return Err(InstallError::CorruptedTransaction(
            "current is not a symbolic link during network rollback".into(),
        ));
    }
    let target = std::fs::read_link(&current).map_err(InstallError::Io)?;
    if Some(target.as_path()) != target_release.map(Path::new) {
        return Err(InstallError::CorruptedTransaction(format!(
            "current points to {} instead of the network takeover target",
            target.display()
        )));
    }
    Ok(true)
}

fn remove_path_if_present(path: &Path) -> Result<(), InstallError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(InstallError::Io(error)),
    };
    if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
        std::fs::remove_dir_all(path).map_err(InstallError::Io)
    } else {
        std::fs::remove_file(path).map_err(InstallError::Io)
    }
}
