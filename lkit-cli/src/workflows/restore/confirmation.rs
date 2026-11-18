//! Backup application is provisional until the operator verifies network access.
use crate::deployment::{
    layout,
    plan::InstallError,
    root::InstallRoot,
    runtime::InstallRuntime,
    state,
    transaction::{self, Phase, RestoreConfirmation, TransactionFile},
};
use crate::service::{health, manager::ManagedService};
use chrono::Utc;

pub(crate) fn prepare(
    id: &str,
    runtime: &InstallRuntime,
) -> Result<RestoreConfirmation, InstallError> {
    crate::network::takeover::require_recovery_daemon(runtime)?;
    Ok(RestoreConfirmation {
        boot_id: crate::network::takeover::current_boot_id()?,
        confirmation_deadline: deadline(runtime)?,
        pending_state: format!("transactions/{id}/pending-restore-state.json"),
    })
}

pub(super) fn deadline(runtime: &InstallRuntime) -> Result<chrono::DateTime<Utc>, InstallError> {
    let duration = chrono::Duration::from_std(runtime.network_confirm_timeout).map_err(|_| {
        InstallError::ParameterUsage("network confirmation timeout is too large".into())
    })?;
    Utc::now().checked_add_signed(duration).ok_or_else(|| {
        InstallError::ParameterUsage("network confirmation timeout is too large".into())
    })
}

pub(crate) fn check_window(guard: &RestoreConfirmation) -> Result<(), InstallError> {
    if Utc::now() >= guard.confirmation_deadline
        || crate::network::takeover::current_boot_id()? != guard.boot_id
    {
        return Err(InstallError::ParameterUsage("restore confirmation expired or host restarted; wait for automatic rollback or run `lkit network rollback`".into()));
    }
    Ok(())
}

pub(super) fn write_pending(
    tx: &TransactionFile,
    installed: &state::InstallState,
) -> Result<(), InstallError> {
    let guard = tx
        .restore_confirmation
        .as_ref()
        .ok_or_else(|| InstallError::CorruptedTransaction("missing restore confirmation".into()))?;
    super::rollback_util::write_file_atomic(
        &layout::territory_relative(&guard.pending_state),
        &serde_json::to_vec_pretty(installed).map_err(InstallError::StateWrite)?,
        0o600,
    )
}

fn read_pending(
    root: &InstallRoot,
    tx: &TransactionFile,
) -> Result<state::InstallState, InstallError> {
    transaction::validate_transaction(tx)?;
    let guard = tx
        .restore_confirmation
        .as_ref()
        .ok_or_else(|| InstallError::CorruptedTransaction("missing restore confirmation".into()))?;
    let bytes = std::fs::read(layout::territory_relative(&guard.pending_state))
        .map_err(InstallError::Io)?;
    let installed: state::InstallState = serde_json::from_slice(&bytes)
        .map_err(|e| InstallError::CorruptedState(format!("invalid pending restore state: {e}")))?;
    state::validate_state(&installed)?;
    if installed.canonical_install_root != root.canonical.to_string_lossy()
        || tx.canonical_install_root != installed.canonical_install_root
        || installed.last_transaction_id.as_deref() != Some(&tx.transaction_id)
        || tx.target_version.as_deref() != Some(&installed.active_version)
    {
        return Err(InstallError::CorruptedState(
            "pending restore state identity mismatch".into(),
        ));
    }
    Ok(installed)
}

pub(crate) async fn confirm(
    root: &InstallRoot,
    tx: &TransactionFile,
    runtime: &InstallRuntime,
) -> Result<(), InstallError> {
    if !matches!(
        tx.phase,
        Phase::AwaitingNetworkConfirmation | Phase::Finalizing
    ) {
        return Err(InstallError::BlockedByTransaction(
            "restore is not awaiting confirmation".into(),
        ));
    }
    let mut installed = read_pending(root, tx)?;
    let manager = runtime.service_manager.as_ref();
    if manager.kind() != installed.service.manager {
        return Err(InstallError::Preflight(
            "restore confirmation backend mismatch".into(),
        ));
    }
    if tx.phase == Phase::AwaitingNetworkConfirmation {
        let guard = tx.restore_confirmation.as_ref().unwrap();
        check_window(guard)?;
        super::pipeline::verify_current_backend(root, &installed)?;
        let health = runtime.health_options()?;
        let pid = manager.main_pid(ManagedService::LandscapeRouter)?;
        if pid == 0 {
            return Err(InstallError::Preflight(
                "restored service has no main pid".into(),
            ));
        }
        let startup = health::StartupOptions {
            ports: &health.ports,
            expected_pid: pid,
            docs: &health.docs,
            unit_state: Some(&(|| manager.active_state(ManagedService::LandscapeRouter).ok())),
            init_required: true,
            data_dir: &root.canonical.join("data"),
            startup_timeout: health.startup_timeout,
            stable_duration: health.stable_duration,
        };
        health::wait_for_startup(&startup).await?;
        health::observe_stable(&startup).await?;
        check_window(guard)?;
        transaction::mark_phase(root, tx, Phase::Finalizing)?;
        #[cfg(feature = "test-support")]
        crate::workflows::operation_control::checkpoint("restore-finalizing")?;
    }
    // Finalizing is the durable user decision: finish it even after a reboot.
    installed.committed_at = Some(Utc::now());
    state::write_state(root, &installed)?;
    #[cfg(feature = "test-support")]
    crate::workflows::operation_control::checkpoint("restore-state-written")?;
    transaction::mark_phase(root, tx, Phase::Committed)?;
    let _ = std::fs::remove_file(layout::territory_relative(
        &tx.restore_confirmation.as_ref().unwrap().pending_state,
    ));
    println!(
        "restore: {}",
        crate::tr!(
            crate::keys::RESTORE_COMMITTED,
            version = installed.active_version,
            backup_id = tx
                .restore_backup
                .as_ref()
                .map(|backup| backup.backup_id.as_str())
                .unwrap_or_default()
        )
    );
    Ok(())
}

pub(crate) async fn rollback(
    root: &InstallRoot,
    tx: &TransactionFile,
    runtime: &InstallRuntime,
) -> Result<(), InstallError> {
    if !matches!(
        tx.phase,
        Phase::AwaitingNetworkConfirmation | Phase::RollingBack
    ) {
        return Err(InstallError::BlockedByTransaction(
            "restore cannot be rolled back after confirmation was accepted".into(),
        ));
    }
    super::rollback_restore(
        root,
        tx,
        runtime.service_manager.as_ref(),
        &runtime.health_options()?,
    )
    .await
}

/// Progress describes lkit's directory moves, never filesystem object identity.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum DataRecovery {
    OriginalPresent,
    OriginalAbsent,
    Saved,
    Restoring,
}

pub(super) fn directory_exists(path: &std::path::Path) -> Result<bool, InstallError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(true),
        Ok(_) => Err(InstallError::Preflight(format!(
            "restore path {} must be a real directory",
            path.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(InstallError::Io(error)),
    }
}

fn data_recovery_path(tx: &TransactionFile) -> std::path::PathBuf {
    layout::territory_transactions_dir()
        .join(&tx.transaction_id)
        .join("data-recovery.json")
}

fn read_data_recovery(tx: &TransactionFile) -> Result<Option<DataRecovery>, InstallError> {
    let path = data_recovery_path(tx);
    match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_file() => {}
        Ok(_) => {
            return Err(InstallError::CorruptedTransaction(
                "restore data progress must be a regular file".into(),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(InstallError::Io(error)),
    }
    serde_json::from_slice(&std::fs::read(path).map_err(InstallError::Io)?)
        .map(Some)
        .map_err(|error| {
            InstallError::CorruptedTransaction(format!("invalid restore data progress: {error}"))
        })
}

fn write_data_recovery(tx: &TransactionFile, progress: DataRecovery) -> Result<(), InstallError> {
    let path = data_recovery_path(tx);
    super::rollback_util::write_file_atomic(
        &path,
        &serde_json::to_vec(&progress).map_err(InstallError::StateWrite)?,
        0o600,
    )?;
    std::fs::File::open(path.parent().unwrap())
        .and_then(|directory| directory.sync_all())
        .map_err(InstallError::Io)
}

pub(super) fn prepare_data_recovery(
    root: &InstallRoot,
    tx: &TransactionFile,
) -> Result<(), InstallError> {
    if tx.restore_confirmation.is_some() {
        let progress = if directory_exists(&root.canonical.join("data"))? {
            DataRecovery::OriginalPresent
        } else {
            DataRecovery::OriginalAbsent
        };
        write_data_recovery(tx, progress)?;
    }
    Ok(())
}

/// Preserve the pre-activation fact before changing an older journal to RollingBack.
pub(super) fn prepare_legacy_data_recovery(
    root: &InstallRoot,
    tx: &TransactionFile,
) -> Result<(), InstallError> {
    if tx.restore_confirmation.is_some()
        && matches!(tx.phase, Phase::Prepared | Phase::Stopping)
        && read_data_recovery(tx)?.is_none()
    {
        prepare_data_recovery(root, tx)?;
    }
    Ok(())
}

pub(super) fn sync_data_directories(
    root: &InstallRoot,
    tx: &TransactionFile,
) -> Result<(), InstallError> {
    let workspace = transaction::workspace::directory(root, tx)?;
    for path in [&root.canonical, &workspace] {
        // Before staging begins, a transaction can still lack its workspace.
        if path == &workspace && !directory_exists(path)? {
            continue;
        }
        std::fs::File::open(path)
            .and_then(|directory| directory.sync_all())
            .map_err(InstallError::Io)?;
    }
    Ok(())
}

/// The original move must be durable before recording that target data can be written.
pub(super) fn record_data_saved(
    root: &InstallRoot,
    tx: &TransactionFile,
) -> Result<(), InstallError> {
    if tx.restore_confirmation.is_some() {
        sync_data_directories(root, tx)?;
        write_data_recovery(tx, DataRecovery::Saved)?;
    }
    Ok(())
}

/// Persist intent before consuming previous-data, so retry can recognize the moved-back data.
pub(super) fn begin_data_restore(tx: &TransactionFile) -> Result<(), InstallError> {
    if tx.restore_confirmation.is_some() {
        read_data_recovery(tx)?;
        write_data_recovery(tx, DataRecovery::Restoring)?;
    }
    Ok(())
}

pub(super) fn data_was_original(
    root: &InstallRoot,
    tx: &TransactionFile,
) -> Result<bool, InstallError> {
    if !directory_exists(&root.canonical.join("data"))? {
        return Ok(false);
    }
    if tx.restore_confirmation.is_none() {
        return Ok(true);
    }
    Ok(matches!(
        read_data_recovery(tx)?,
        Some(DataRecovery::OriginalPresent | DataRecovery::Restoring)
    ))
}

/// Only call after the original release and data have been restored.
pub(crate) fn permit_restored_start(tx: &TransactionFile) -> Result<(), InstallError> {
    if tx.restore_confirmation.is_some() {
        let permit = layout::territory_transactions_dir()
            .join(&tx.transaction_id)
            .join("restored-boot-id");
        super::rollback_util::write_file_atomic(
            &permit,
            crate::network::takeover::current_boot_id()?.as_bytes(),
            0o600,
        )?;
    }
    Ok(())
}

/// A retry may rebuild data again; its previous permit cannot cover partial work.
pub(super) fn revoke_restored_start(tx: &TransactionFile) -> Result<(), InstallError> {
    if tx.restore_confirmation.is_some() {
        let directory = layout::territory_transactions_dir().join(&tx.transaction_id);
        match std::fs::remove_file(directory.join("restored-boot-id")) {
            Ok(()) => std::fs::File::open(directory)
                .and_then(|file| file.sync_all())
                .map_err(InstallError::Io)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(InstallError::Io(error)),
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "confirmation_tests.rs"]
mod tests;
