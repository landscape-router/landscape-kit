use std::path::Path;

use super::super::plan::InstallError;
use super::{Operation, Phase, RegistrationKind, TRANSACTION_SCHEMA_VERSION, TransactionFile};

pub(crate) fn validate_transaction(transaction: &TransactionFile) -> Result<(), InstallError> {
    if !(1..=TRANSACTION_SCHEMA_VERSION).contains(&transaction.schema_version) {
        return Err(corrupted(format!(
            "unsupported transaction schema version {}",
            transaction.schema_version
        )));
    }
    if transaction.schema_version == 1 && transaction.phase == Phase::Stopping {
        return Err(corrupted(
            "transaction phase stopping requires schema version 2".into(),
        ));
    }
    if transaction.started_at > transaction.updated_at {
        return Err(corrupted("started_at must not be after updated_at".into()));
    }
    if transaction.log_path != format!("logs/{}.log", transaction.transaction_id) {
        return Err(corrupted(format!(
            "log_path must be logs/<transaction-id>.log, got {}",
            transaction.log_path
        )));
    }
    for value in [
        transaction.previous_current.as_deref(),
        transaction.target_release.as_deref(),
        transaction
            .backup
            .as_ref()
            .map(|backup| backup.path.as_str()),
        transaction
            .static_backup
            .as_ref()
            .map(|backup| backup.path.as_str()),
        transaction
            .static_backup
            .as_ref()
            .map(|backup| backup.target.as_str()),
        transaction.resolv_conf_backup.as_deref(),
        Some(transaction.log_path.as_str()),
    ]
    .into_iter()
    .flatten()
    {
        if !is_safe_relative(value) {
            return Err(corrupted(format!("unsafe transaction path {value}")));
        }
    }
    if let Some(backup) = &transaction.backup
        && !is_sha256(&backup.sha256)
    {
        return Err(corrupted(
            "backup sha256 must be 64 lowercase hex characters".into(),
        ));
    }
    if let Some(systemd_before) = &transaction.systemd_before
        && systemd_before.registration.kind == RegistrationKind::Symlink
        && systemd_before.registration.target.is_none()
    {
        return Err(corrupted(
            "symlink registration must record its target".into(),
        ));
    }
    let has_backup = transaction.backup.is_some();
    let has_static_backup = transaction.static_backup.is_some();
    let has_managers =
        transaction.from_service_manager.is_some() || transaction.target_service_manager.is_some();
    let has_versions = transaction.from_version.is_some()
        || transaction.target_version.is_some()
        || transaction.previous_current.is_some()
        || transaction.target_release.is_some();
    match transaction.operation {
        Operation::Install => {
            if has_backup || has_static_backup || has_managers {
                return Err(corrupted(
                    "install transaction must not record backups, static backups, or service managers"
                        .into(),
                ));
            }
            if transaction.from_version.is_some() || transaction.previous_current.is_some() {
                return Err(corrupted(
                    "install transaction must not record a previous version".into(),
                ));
            }
            if transaction.target_version.is_none() || transaction.target_release.is_none() {
                return Err(corrupted(
                    "install transaction must record the target version and release".into(),
                ));
            }
        }
        Operation::Switch => {
            if transaction.no_backup && has_backup {
                return Err(corrupted(
                    "no-backup switch must not record a .lkb backup".into(),
                ));
            }
            if !has_backup
                && !transaction.no_backup
                && !matches!(transaction.phase, Phase::Preparing | Phase::Failed)
            {
                return Err(corrupted(
                    "switch transaction must record a .lkb backup unless it is still preparing, already failed, or an explicitly allowed no-backup switch"
                        .into(),
                ));
            }
            if has_static_backup || has_managers {
                return Err(corrupted(
                    "switch transaction must not record static backups or service managers".into(),
                ));
            }
            if transaction.from_version.is_none()
                || transaction.target_version.is_none()
                || transaction.previous_current.is_none()
                || transaction.target_release.is_none()
            {
                return Err(corrupted(
                    "switch transaction must record both versions and both release paths".into(),
                ));
            }
        }
        Operation::Repair => {
            if transaction.no_backup {
                return Err(corrupted(
                    "repair transaction must not record no_backup".into(),
                ));
            }
            if has_managers {
                return Err(corrupted(
                    "repair transaction must not record service managers".into(),
                ));
            }
            let observation = !has_backup
                && !has_static_backup
                && transaction.from_version.is_none()
                && transaction.target_version.is_none()
                && transaction.previous_current.is_none()
                && transaction.target_release.is_none();
            if !observation
                && has_backup == has_static_backup
                && !matches!(transaction.phase, Phase::Preparing | Phase::Failed)
            {
                return Err(corrupted(
                    "repair transaction must record exactly one of a .lkb backup or a static backup unless it is still preparing, already failed, or a pure observation repair with neither"
                        .into(),
                ));
            }
            if has_backup
                && (transaction.from_version.is_none() || transaction.target_version.is_none())
            {
                return Err(corrupted(
                    "repair with a .lkb backup must record both versions".into(),
                ));
            }
            if has_static_backup
                && (transaction.from_version.is_some() || transaction.target_version.is_some())
            {
                return Err(corrupted(
                    "static repair must not record version changes".into(),
                ));
            }
        }
        Operation::ServiceMigration => {
            if has_backup || has_static_backup || has_versions {
                return Err(corrupted(
                    "service migration must not record backups, static backups, or version changes"
                        .into(),
                ));
            }
            let (Some(from), Some(target)) = (
                transaction.from_service_manager,
                transaction.target_service_manager,
            ) else {
                return Err(corrupted(
                    "service migration must record both service managers".into(),
                ));
            };
            if from == target {
                return Err(corrupted(
                    "service migration must record two different service managers".into(),
                ));
            }
            if transaction.systemd_before.is_none() {
                return Err(corrupted(
                    "service migration must record systemd_before".into(),
                ));
            }
        }
    }
    Ok(())
}

fn corrupted(reason: String) -> InstallError {
    InstallError::CorruptedTransaction(reason)
}

fn is_safe_relative(value: &str) -> bool {
    !value.is_empty()
        && !Path::new(value).is_absolute()
        && value
            .split('/')
            .all(|part| !matches!(part, "" | "." | ".."))
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
