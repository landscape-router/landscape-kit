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

#[derive(serde::Serialize, serde::Deserialize, PartialEq)]
struct DataIdentity {
    device: u64,
    inode: u64,
}

fn data_identity(path: &std::path::Path) -> Result<Option<DataIdentity>, InstallError> {
    use std::os::unix::fs::MetadataExt;
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            Ok(Some(DataIdentity {
                device: metadata.dev(),
                inode: metadata.ino(),
            }))
        }
        Ok(_) => Err(InstallError::Preflight(
            "restore data must be a real directory".into(),
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(InstallError::Io(e)),
    }
}

fn identity_path(tx: &TransactionFile) -> std::path::PathBuf {
    layout::territory_transactions_dir()
        .join(&tx.transaction_id)
        .join("previous-data-identity.json")
}

pub(super) fn capture_previous_data(
    root: &InstallRoot,
    tx: &TransactionFile,
) -> Result<(), InstallError> {
    if tx.restore_confirmation.is_some() {
        let identity = data_identity(&root.canonical.join("data"))?;
        super::rollback_util::write_file_atomic(
            &identity_path(tx),
            &serde_json::to_vec(&identity).map_err(InstallError::StateWrite)?,
            0o600,
        )?;
    }
    Ok(())
}

/// Rename preserves directory identity even if recovery dies before writing its permit.
pub(super) fn is_previous_data(
    path: &std::path::Path,
    tx: &TransactionFile,
) -> Result<bool, InstallError> {
    if tx.restore_confirmation.is_none() {
        return Ok(path.exists());
    }
    let identity: Option<DataIdentity> =
        serde_json::from_slice(&std::fs::read(identity_path(tx)).map_err(InstallError::Io)?)
            .map_err(|e| {
                InstallError::CorruptedTransaction(format!("invalid previous data identity: {e}"))
            })?;
    Ok(identity.is_some() && data_identity(path)? == identity)
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
