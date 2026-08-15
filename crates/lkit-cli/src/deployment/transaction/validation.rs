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
    if transaction.schema_version < 3
        && matches!(
            transaction.phase,
            Phase::AwaitingNetworkConfirmation | Phase::Finalizing
        )
    {
        return Err(corrupted(
            "network confirmation phases require schema version 3".into(),
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
            .restore_backup
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
        transaction
            .network_takeover
            .as_ref()
            .map(|network| network.recovery_binary.as_str()),
        transaction
            .network_takeover
            .as_ref()
            .map(|network| network.pending_state.as_str()),
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
    if let Some(restore_backup) = &transaction.restore_backup
        && !is_sha256(&restore_backup.sha256)
    {
        return Err(corrupted(
            "restore_backup sha256 must be 64 lowercase hex characters".into(),
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
    let has_restore_backup = transaction.restore_backup.is_some();
    let has_static_backup = transaction.static_backup.is_some();
    let has_managers =
        transaction.from_service_manager.is_some() || transaction.target_service_manager.is_some();
    let has_versions = transaction.from_version.is_some()
        || transaction.target_version.is_some()
        || transaction.previous_current.is_some()
        || transaction.target_release.is_some();
    match transaction.operation {
        Operation::Install => {
            if has_backup || has_restore_backup || has_static_backup || has_managers {
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
            if let Some(network) = &transaction.network_takeover {
                network.plan.validate()?;
                if transaction.schema_version < 3 {
                    return Err(corrupted(
                        "network takeover install requires transaction schema version 3".into(),
                    ));
                }
                for unit in [
                    &network.rollback_service,
                    &network.rollback_timer,
                    &network.boot_rollback_service,
                ] {
                    if !is_safe_unit_name(unit) {
                        return Err(corrupted(format!(
                            "invalid network recovery unit name {unit}"
                        )));
                    }
                }
            }
        }
        Operation::Switch => {
            reject_network_takeover(transaction, "switch")?;
            if has_restore_backup {
                return Err(corrupted(
                    "switch transaction must not record restore_backup".into(),
                ));
            }
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
        Operation::Restore => {
            reject_network_takeover(transaction, "restore")?;
            if has_static_backup || has_managers {
                return Err(corrupted(
                    "restore transaction must not record static backups or service managers".into(),
                ));
            }
            if transaction.no_backup && has_backup {
                return Err(corrupted(
                    "no-backup restore must not record a .lkb protection backup".into(),
                ));
            }
            if !has_restore_backup && !matches!(transaction.phase, Phase::Preparing | Phase::Failed)
            {
                return Err(corrupted(
                    "restore transaction must record the target backup unless it is still preparing or already failed"
                        .into(),
                ));
            }
            if transaction.from_version.is_none()
                || transaction.target_version.is_none()
                || transaction.previous_current.is_none()
                || transaction.target_release.is_none()
            {
                return Err(corrupted(
                    "restore transaction must record both versions and both release paths".into(),
                ));
            }
        }
        Operation::Repair => {
            reject_network_takeover(transaction, "repair")?;
            if transaction.no_backup || has_restore_backup {
                return Err(corrupted(
                    "repair transaction must not record no_backup or restore_backup".into(),
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
            reject_network_takeover(transaction, "service migration")?;
            if has_backup || has_restore_backup || has_static_backup || has_versions {
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
        Operation::Uninstall => {
            reject_network_takeover(transaction, "uninstall")?;
            if has_restore_backup || has_static_backup || has_managers {
                return Err(corrupted(
                    "uninstall transaction must not record restore backups, static backups, or service managers"
                        .into(),
                ));
            }
            if transaction.no_backup && has_backup {
                return Err(corrupted(
                    "no-backup uninstall must not record a .lkb protection backup".into(),
                ));
            }
            if !has_backup
                && !transaction.no_backup
                && !matches!(transaction.phase, Phase::Preparing | Phase::Failed)
            {
                return Err(corrupted(
                    "uninstall transaction must record a .lkb protection backup unless it is still preparing, already failed, or an explicitly allowed no-backup uninstall"
                        .into(),
                ));
            }
            if transaction.from_version.is_none() || transaction.previous_current.is_none() {
                return Err(corrupted(
                    "uninstall transaction must record the current version and release".into(),
                ));
            }
            if transaction.target_version.is_some() || transaction.target_release.is_some() {
                return Err(corrupted(
                    "uninstall transaction must not record a target version".into(),
                ));
            }
        }
        Operation::Migrate => {
            reject_network_takeover(transaction, "migrate")?;
            if has_restore_backup || has_static_backup || has_managers {
                return Err(corrupted(
                    "migrate transaction must not record restore backups, static backups, or service managers"
                        .into(),
                ));
            }
            if transaction.no_backup {
                return Err(corrupted(
                    "migrate transaction must not record no_backup".into(),
                ));
            }
            if transaction.from_version.is_some() || transaction.previous_current.is_some() {
                return Err(corrupted(
                    "migrate transaction must not record a previous version".into(),
                ));
            }
            if transaction.target_version.is_none() || transaction.target_release.is_none() {
                return Err(corrupted(
                    "migrate transaction must record the target version and release".into(),
                ));
            }
            if !has_backup && !matches!(transaction.phase, Phase::Preparing | Phase::Failed) {
                return Err(corrupted(
                    "migrate transaction must record the migration backup unless it is still preparing or already failed"
                        .into(),
                ));
            }
            if let Some(unit) = &transaction.legacy_unit {
                if !is_safe_unit_name(&unit.unit) {
                    return Err(corrupted(format!("invalid legacy unit name {}", unit.unit)));
                }
                if let Some(path) = &unit.file_path
                    && !Path::new(path).is_absolute()
                {
                    return Err(corrupted(
                        "legacy unit file_path must be an absolute path".into(),
                    ));
                }
                if let Some(path) = &unit.file_backup
                    && !is_safe_relative(path)
                {
                    return Err(corrupted(format!("unsafe legacy unit backup path {path}")));
                }
                if unit.file_moved && (unit.file_path.is_none() || unit.file_backup.is_none()) {
                    return Err(corrupted(
                        "a moved legacy unit must record both file_path and file_backup".into(),
                    ));
                }
            }
        }
        Operation::Reinit => {
            if has_restore_backup || has_static_backup || has_managers {
                return Err(corrupted(
                    "reinit transaction must not record restore backups, static backups, or service managers"
                        .into(),
                ));
            }
            if transaction.no_backup && has_backup {
                return Err(corrupted(
                    "no-backup reinit must not record a .lkb protection backup".into(),
                ));
            }
            if !has_backup
                && !transaction.no_backup
                && !matches!(transaction.phase, Phase::Preparing | Phase::Failed)
            {
                return Err(corrupted(
                    "reinit transaction must record a .lkb protection backup unless it is still preparing, already failed, or an explicitly allowed no-backup reinit"
                        .into(),
                ));
            }
            if transaction.from_version.is_none() {
                return Err(corrupted(
                    "reinit transaction must record the current version".into(),
                ));
            }
            if transaction.target_version.is_some()
                || transaction.target_release.is_some()
                || transaction.previous_current.is_some()
            {
                return Err(corrupted(
                    "reinit transaction must not record a target version or release".into(),
                ));
            }
            if let Some(network) = &transaction.network_takeover {
                network.plan.validate()?;
                if transaction.schema_version < 3 {
                    return Err(corrupted(
                        "reinit network confirmation requires transaction schema version 3".into(),
                    ));
                }
                for unit in [
                    &network.rollback_service,
                    &network.rollback_timer,
                    &network.boot_rollback_service,
                ] {
                    if !is_safe_unit_name(unit) {
                        return Err(corrupted(format!(
                            "invalid network recovery unit name {unit}"
                        )));
                    }
                }
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

fn reject_network_takeover(
    transaction: &TransactionFile,
    operation: &str,
) -> Result<(), InstallError> {
    if transaction.network_takeover.is_some() {
        return Err(corrupted(format!(
            "{operation} transaction must not record network takeover state"
        )));
    }
    Ok(())
}

fn is_safe_unit_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'_' | b'-' | b'.' | b'@')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    use super::super::{
        BackupRef, Registration, StaticBackupRef, SystemdBefore, TransactionServiceManager,
        find_unfinished,
    };
    use crate::deployment::root::InstallRoot;

    fn temp_root(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("lkit-tx-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn new_root(path: &std::path::Path) -> InstallRoot {
        InstallRoot {
            install_root: path.to_path_buf(),
            canonical: path.to_path_buf(),
        }
    }

    fn install_transaction(root: &InstallRoot) -> TransactionFile {
        TransactionFile::new_install(root, &semver::Version::new(1, 2, 3)).unwrap()
    }

    #[test]
    fn accepts_v1_transactions_and_names_stopping_phase() {
        let temp = temp_root("schema-compatibility");
        let root = new_root(&temp);
        let mut transaction = install_transaction(&root);
        transaction.schema_version = 1;
        assert!(validate_transaction(&transaction).is_ok());
        assert_eq!(Phase::Stopping.key(), "stopping");
        transaction.phase = Phase::Stopping;
        assert!(validate_transaction(&transaction).is_err());
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn rejects_corrupted_transactions() {
        let temp = temp_root("corrupt");
        let root = new_root(&temp);
        std::fs::create_dir_all(temp.join("transactions")).unwrap();
        std::fs::write(temp.join("transactions/bad.json"), b"not json").unwrap();
        assert!(matches!(
            find_unfinished(&root),
            Err(InstallError::CorruptedTransaction(_))
        ));

        let mut transaction = install_transaction(&root);
        transaction.schema_version = 5;
        assert!(validate_transaction(&transaction).is_err());

        let mut transaction = install_transaction(&root);
        transaction.log_path = "../escape.log".into();
        assert!(validate_transaction(&transaction).is_err());

        let mut transaction = install_transaction(&root);
        transaction.target_release = Some("../escape".into());
        assert!(validate_transaction(&transaction).is_err());
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn validates_operation_specific_rules() {
        let temp = temp_root("ops");
        let root = new_root(&temp);
        let mut transaction = install_transaction(&root);

        transaction.operation = Operation::Switch;
        transaction.from_version = Some("1.1.0".into());
        transaction.previous_current = Some("releases/1.1.0".into());
        assert!(validate_transaction(&transaction).is_ok());
        transaction.phase = Phase::Prepared;
        assert!(validate_transaction(&transaction).is_err());
        transaction.phase = Phase::Failed;
        assert!(validate_transaction(&transaction).is_ok());

        transaction.backup = Some(BackupRef {
            backup_id: "b".into(),
            path: "backups/b.lkb".into(),
            sha256: "a".repeat(64),
        });
        assert!(validate_transaction(&transaction).is_ok());

        transaction.operation = Operation::ServiceMigration;
        transaction.backup = None;
        transaction.from_version = None;
        transaction.previous_current = None;
        transaction.target_version = None;
        transaction.target_release = None;
        assert!(validate_transaction(&transaction).is_err());
        transaction.from_service_manager = Some(TransactionServiceManager::Systemd);
        transaction.target_service_manager = Some(TransactionServiceManager::None);
        assert!(validate_transaction(&transaction).is_err());
        transaction.systemd_before = Some(SystemdBefore {
            registration: Registration {
                kind: RegistrationKind::Missing,
                target: None,
            },
            enabled: false,
            active: false,
        });
        assert!(validate_transaction(&transaction).is_ok());
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn validates_restore_transaction_rules() {
        let temp = temp_root("restore");
        let root = new_root(&temp);
        let from = semver::Version::new(1, 1, 0);
        let target = semver::Version::new(1, 2, 3);
        let mut transaction = TransactionFile::new_restore(&root, &from, &target).unwrap();
        assert_eq!(transaction.operation, Operation::Restore);
        assert_eq!(transaction.schema_version, 4);
        assert!(validate_transaction(&transaction).is_ok());

        transaction.phase = Phase::Prepared;
        assert!(validate_transaction(&transaction).is_err());
        transaction.restore_backup = Some(BackupRef {
            backup_id: "rb".into(),
            path: "transactions/tx/target-backup.lkb".into(),
            sha256: "a".repeat(64),
        });
        assert!(validate_transaction(&transaction).is_ok());

        transaction.static_backup = Some(StaticBackupRef {
            path: "backups/static".into(),
            target: "current/static".into(),
        });
        assert!(validate_transaction(&transaction).is_err());
        transaction.static_backup = None;

        transaction.no_backup = true;
        assert!(validate_transaction(&transaction).is_ok());
        transaction.backup = Some(BackupRef {
            backup_id: "p".into(),
            path: "backups/p.lkb".into(),
            sha256: "b".repeat(64),
        });
        assert!(validate_transaction(&transaction).is_err());
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn validates_reinit_transaction_rules() {
        let temp = temp_root("reinit");
        let root = new_root(&temp);
        let mut transaction =
            TransactionFile::new_reinit(&root, &semver::Version::new(1, 2, 3)).unwrap();
        assert_eq!(transaction.operation, Operation::Reinit);
        assert_eq!(transaction.phase, Phase::Preparing);
        assert_eq!(transaction.from_version.as_deref(), Some("1.2.3"));
        assert!(transaction.target_version.is_none());
        assert!(transaction.target_release.is_none());
        assert!(transaction.previous_current.is_none());
        assert!(validate_transaction(&transaction).is_ok());

        // 保护备份记录后(仍在 preparing)合法;进入 prepared 后必须有备份或 no_backup。
        transaction.backup = Some(BackupRef {
            backup_id: "b".into(),
            path: "backups/b.lkb".into(),
            sha256: "a".repeat(64),
        });
        assert!(validate_transaction(&transaction).is_ok());
        transaction.phase = Phase::Prepared;
        assert!(validate_transaction(&transaction).is_ok());
        transaction.backup = None;
        assert!(validate_transaction(&transaction).is_err());
        transaction.no_backup = true;
        assert!(validate_transaction(&transaction).is_ok());
        transaction.no_backup = false;
        transaction.phase = Phase::Failed;
        assert!(validate_transaction(&transaction).is_ok());

        // 非法组合:版本变化、release、previous_current、restore_backup、managers。
        let mut invalid =
            TransactionFile::new_reinit(&root, &semver::Version::new(1, 2, 3)).unwrap();
        invalid.target_version = Some("1.3.0".into());
        assert!(validate_transaction(&invalid).is_err());
        let mut invalid =
            TransactionFile::new_reinit(&root, &semver::Version::new(1, 2, 3)).unwrap();
        invalid.target_release = Some("releases/1.2.3".into());
        assert!(validate_transaction(&invalid).is_err());
        let mut invalid =
            TransactionFile::new_reinit(&root, &semver::Version::new(1, 2, 3)).unwrap();
        invalid.previous_current = Some("releases/1.2.3".into());
        assert!(validate_transaction(&invalid).is_err());
        let mut invalid =
            TransactionFile::new_reinit(&root, &semver::Version::new(1, 2, 3)).unwrap();
        invalid.restore_backup = Some(BackupRef {
            backup_id: "r".into(),
            path: "transactions/tx/target-backup.lkb".into(),
            sha256: "b".repeat(64),
        });
        assert!(validate_transaction(&invalid).is_err());
        let mut invalid =
            TransactionFile::new_reinit(&root, &semver::Version::new(1, 2, 3)).unwrap();
        invalid.from_version = None;
        assert!(validate_transaction(&invalid).is_err());
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn rejects_reinit_network_confirmation_below_schema_three() {
        let temp = temp_root("reinit-network");
        let root = new_root(&temp);
        let mut transaction =
            TransactionFile::new_reinit(&root, &semver::Version::new(1, 2, 3)).unwrap();
        transaction.schema_version = 1;
        transaction.phase = Phase::AwaitingNetworkConfirmation;
        assert!(validate_transaction(&transaction).is_err());
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn validates_uninstall_transaction_rules() {
        let temp = temp_root("uninstall");
        let root = new_root(&temp);
        let mut transaction =
            TransactionFile::new_uninstall(&root, &semver::Version::new(1, 2, 3)).unwrap();
        assert_eq!(transaction.operation, Operation::Uninstall);
        assert_eq!(transaction.phase, Phase::Preparing);
        assert_eq!(transaction.from_version.as_deref(), Some("1.2.3"));
        assert_eq!(
            transaction.previous_current.as_deref(),
            Some("releases/1.2.3")
        );
        assert!(transaction.target_version.is_none());
        assert!(transaction.target_release.is_none());
        assert!(validate_transaction(&transaction).is_ok());

        // 保护备份记录后(仍在 preparing)合法。
        transaction.backup = Some(BackupRef {
            backup_id: "b".into(),
            path: "backups/b.lkb".into(),
            sha256: "a".repeat(64),
        });
        assert!(validate_transaction(&transaction).is_ok());
        // 进入 prepared 后必须有备份或 no_backup。
        transaction.phase = Phase::Prepared;
        assert!(validate_transaction(&transaction).is_ok());
        transaction.backup = None;
        assert!(validate_transaction(&transaction).is_err());
        transaction.no_backup = true;
        assert!(validate_transaction(&transaction).is_ok());
        transaction.no_backup = false;

        // 非法组合:restore_backup、static_backup、managers、目标版本。
        let mut invalid =
            TransactionFile::new_uninstall(&root, &semver::Version::new(1, 2, 3)).unwrap();
        invalid.restore_backup = Some(BackupRef {
            backup_id: "r".into(),
            path: "transactions/tx/target-backup.lkb".into(),
            sha256: "b".repeat(64),
        });
        assert!(validate_transaction(&invalid).is_err());
        let mut invalid =
            TransactionFile::new_uninstall(&root, &semver::Version::new(1, 2, 3)).unwrap();
        invalid.target_version = Some("1.3.0".into());
        assert!(validate_transaction(&invalid).is_err());
        let mut invalid =
            TransactionFile::new_uninstall(&root, &semver::Version::new(1, 2, 3)).unwrap();
        invalid.target_service_manager = Some(TransactionServiceManager::Systemd);
        assert!(validate_transaction(&invalid).is_err());
        let _ = std::fs::remove_dir_all(&temp);
    }
}
