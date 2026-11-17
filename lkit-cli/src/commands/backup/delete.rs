use std::process::ExitCode;

use crate::backup::lkb::backup_id_format_ok;
use crate::deployment::lock;
use crate::deployment::plan;
use crate::deployment::plan::InstallError;
use crate::deployment::root::InstallRoot;
use crate::workflows::restore::validate_backup_file;

use super::BackupDelete;
use super::discover_root;
use super::exit_code;

pub(super) fn run_delete(args: &BackupDelete) -> ExitCode {
    if !backup_id_format_ok(&args.backup) {
        eprintln!(
            "backup: {}",
            crate::tr!(crate::keys::BACKUP_DELETE_INVALID_ID, id = args.backup)
        );
        return ExitCode::from(2);
    }
    if !args.yes {
        if crate::interaction::interactive::is_non_interactive() {
            eprintln!(
                "backup: {}",
                crate::tr!(crate::keys::BACKUP_DELETE_REQUIRES_YES)
            );
            return ExitCode::from(2);
        }
        let accepted = match crate::interaction::interactive::confirm(&crate::tr!(
            crate::keys::BACKUP_DELETE_CONFIRM,
            backup_id = args.backup
        )) {
            Ok(accepted) => accepted,
            Err(error) => {
                eprintln!("backup: {error}");
                return exit_code(&error);
            }
        };
        if !accepted {
            eprintln!("backup: {}", crate::tr!(crate::keys::BACKUP_DELETE_REFUSED));
            return ExitCode::FAILURE;
        }
    }
    let root = match discover_root() {
        Ok(Some(root)) => root,
        Ok(None) => {
            eprintln!(
                "backup: {}",
                crate::tr!(crate::keys::BACKUP_REQUIRES_EXISTING_INSTALLATION)
            );
            return ExitCode::from(2);
        }
        Err(error) => {
            eprintln!("backup: {error}");
            return exit_code(&error);
        }
    };
    let _lock = match lock::acquire_install_lock() {
        Ok(lock) => lock,
        Err(error) => {
            eprintln!("backup: {error}");
            return exit_code(&error);
        }
    };
    match delete_backup(&root, &args.backup) {
        Ok(()) => {
            println!(
                "backup: {}",
                crate::tr!(crate::keys::BACKUP_DELETED, backup_id = args.backup)
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("backup: {error}");
            exit_code(&error)
        }
    }
}

/// 删除 lkit 地盘备份目录下的备份：ID 必须已通过格式校验，目标必须是
/// root 所有、权限不宽于 `0600` 的普通文件（不跟随符号链接）。CLI 与
/// 交互控制台共用；调用方必须持有安装锁直到删除完成。
pub(crate) fn delete_backup(_root: &InstallRoot, backup_id: &str) -> Result<(), InstallError> {
    if !backup_id_format_ok(backup_id) {
        return Err(InstallError::ParameterUsage("invalid backup ID".into()));
    }
    let path = crate::deployment::layout::territory_backups_dir().join(format!("{backup_id}.lkb"));
    if !path.is_file() {
        return Err(plan::InstallError::InvalidBackup(format!(
            "backup {backup_id} not found under {}",
            crate::deployment::layout::territory_backups_dir().display()
        )));
    }
    validate_backup_file(&path)?;
    ensure_unreferenced(backup_id)?;
    std::fs::remove_file(&path).map_err(plan::InstallError::Io)
}

fn ensure_unreferenced(backup_id: &str) -> Result<(), InstallError> {
    use crate::deployment::{layout, transaction};
    let entries = match std::fs::read_dir(layout::territory_transactions_dir()) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(InstallError::Io(error)),
    };
    let backup_path = format!("backups/{backup_id}.lkb");
    for entry in entries {
        let entry = entry.map_err(InstallError::Io)?;
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        if !entry.file_type().map_err(InstallError::Io)?.is_file() {
            return Err(InstallError::CorruptedTransaction(format!(
                "{} is not a regular transaction file; cannot safely delete backups",
                path.display()
            )));
        }
        let bytes = std::fs::read(&path).map_err(InstallError::Io)?;
        let tx: transaction::TransactionFile = serde_json::from_slice(&bytes).map_err(|error| {
            InstallError::CorruptedTransaction(format!(
                "{}: {error}; cannot safely delete backups",
                path.display()
            ))
        })?;
        transaction::validate_transaction(&tx)?;
        if path.file_stem().and_then(|name| name.to_str()) != Some(&tx.transaction_id) {
            return Err(InstallError::CorruptedTransaction(
                "transaction filename identity mismatch".into(),
            ));
        }
        if !tx.phase.is_terminal()
            && [tx.backup.as_ref(), tx.restore_backup.as_ref()]
                .into_iter()
                .flatten()
                .any(|reference| reference.backup_id == backup_id || reference.path == backup_path)
        {
            return Err(InstallError::InvalidBackup(format!(
                "backup {backup_id} is required by unfinished transaction {} ({})",
                tx.transaction_id,
                tx.phase.key()
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn recovery_references_protect_backups_until_transaction_finishes() {
        use crate::deployment::transaction::{BackupRef, Phase, TransactionFile};
        let (_guard, territory) = setup("delete-recovery");
        let root = discover_root().unwrap().unwrap();
        let backup_id = "20260807-131500-ab12cd34";
        let file = territory.join(format!("backups/{backup_id}.lkb"));
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, b"protected backup").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        let mut tx = TransactionFile::new_switch(
            &root,
            &semver::Version::new(1, 2, 3),
            &semver::Version::new(2, 0, 0),
        )
        .unwrap();
        tx.phase = Phase::RollingBack;
        tx.backup = Some(BackupRef {
            backup_id: backup_id.into(),
            path: format!("backups/{backup_id}.lkb"),
            sha256: "a".repeat(64),
        });
        crate::deployment::transaction::validate_transaction(&tx).unwrap();
        let tx_path = territory.join(format!("transactions/{}.json", tx.transaction_id));
        std::fs::create_dir_all(tx_path.parent().unwrap()).unwrap();
        std::fs::write(&tx_path, serde_json::to_vec(&tx).unwrap()).unwrap();
        let error = delete_backup(&root, backup_id).unwrap_err().to_string();
        assert!(error.contains(&tx.transaction_id));
        assert!(file.exists());

        tx.phase = Phase::RolledBack;
        std::fs::write(&tx_path, serde_json::to_vec(&tx).unwrap()).unwrap();
        delete_backup(&root, backup_id).unwrap();
        assert!(!file.exists());
        std::fs::remove_dir_all(territory.parent().unwrap()).unwrap();
    }

    #[test]
    fn restore_targets_and_unreadable_transaction_history_block_deletion() {
        use crate::deployment::transaction::{BackupRef, TransactionFile};
        let (_guard, territory) = setup("delete-restore-reference");
        let root = discover_root().unwrap().unwrap();
        let backup_id = "20260807-131500-ab12cd34";
        let file = territory.join(format!("backups/{backup_id}.lkb"));
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, b"target backup").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        let mut tx = TransactionFile::new_switch(
            &root,
            &semver::Version::new(1, 2, 3),
            &semver::Version::new(2, 0, 0),
        )
        .unwrap();
        tx.operation = crate::deployment::transaction::Operation::Restore;
        tx.restore_backup = Some(BackupRef {
            backup_id: backup_id.into(),
            path: format!("backups/{backup_id}.lkb"),
            sha256: "a".repeat(64),
        });
        crate::deployment::transaction::validate_transaction(&tx).unwrap();
        let tx_path = territory.join(format!("transactions/{}.json", tx.transaction_id));
        std::fs::create_dir_all(tx_path.parent().unwrap()).unwrap();
        std::fs::write(&tx_path, serde_json::to_vec(&tx).unwrap()).unwrap();
        assert!(
            delete_backup(&root, backup_id)
                .unwrap_err()
                .to_string()
                .contains("required by unfinished")
        );
        std::fs::write(&tx_path, b"broken transaction").unwrap();
        assert!(matches!(
            delete_backup(&root, backup_id),
            Err(InstallError::CorruptedTransaction(_))
        ));
        assert!(file.exists());
        std::fs::remove_dir_all(territory.parent().unwrap()).unwrap();
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "lkit-backup-cmd-test-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    /// 建立隔离测试现场并写入有效状态(discover 需要),返回 (守卫, 地盘)。
    fn setup(
        name: &str,
    ) -> (
        crate::deployment::layout::TerritoryOverride,
        std::path::PathBuf,
    ) {
        let temp = temp_dir(name);
        let territory = temp.join("territory");
        std::fs::create_dir_all(&territory).unwrap();
        let guard = crate::deployment::layout::test_territory(&territory);
        let install = temp.join("install");
        std::fs::create_dir_all(&install).unwrap();
        let state = crate::deployment::state::InstallState {
            schema_version: 1,
            layout_version: 2,
            install_root: install.display().to_string(),
            canonical_install_root: install.display().to_string(),
            active_version: "1.2.3".into(),
            assets: crate::deployment::state::Assets {
                webserver: crate::deployment::state::WebserverAsset {
                    architecture: crate::deployment::state::StateArchitecture::X86_64,
                    sha256: "a".repeat(64),
                    size: 1,
                },
                static_archive: crate::deployment::state::ArchiveAsset {
                    sha256: "b".repeat(64),
                    size: 1,
                },
            },
            initialization: crate::deployment::state::InitializationState {
                status: crate::deployment::state::InitStatus::Complete,
                lock_present: true,
                initialized_at: Some(chrono::Utc::now()),
            },
            service: crate::deployment::state::ServiceState {
                manager: crate::deployment::state::StateServiceManager::Systemd,
                registered: true,
                enabled: true,
                verified: true,
                definition_path: Some("service/landscape-router.service".into()),
                definition_sha256: Some("c".repeat(64)),
            },
            last_transaction_id: None,
            committed_at: Some(chrono::Utc::now()),
        };
        crate::deployment::state::write_state(
            &crate::deployment::root::InstallRoot {
                install_root: install.clone(),
                canonical: install.clone(),
            },
            &state,
        )
        .unwrap();
        (guard, territory)
    }

    #[test]
    fn deletes_a_valid_backup_file() {
        let (_guard, territory) = setup("delete-ok");
        let backups = territory.join("backups");
        std::fs::create_dir_all(&backups).unwrap();
        let path = backups.join("20260807-131500-ab12cd34.lkb");
        std::fs::write(&path, b"lkb bytes").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let root = crate::deployment::root::InstallRoot {
            install_root: territory.clone(),
            canonical: territory.clone(),
        };
        delete_backup(&root, "20260807-131500-ab12cd34").unwrap();
        assert!(!path.exists(), "the backup file must be removed");
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    #[test]
    fn refuses_unsafe_or_missing_backups_on_delete() {
        let (_guard, territory) = setup("delete-refuse");
        let backups = territory.join("backups");
        std::fs::create_dir_all(&backups).unwrap();
        let root = crate::deployment::root::InstallRoot {
            install_root: territory.clone(),
            canonical: territory.clone(),
        };

        assert!(delete_backup(&root, "20260807-131500-ab12cd34").is_err());

        let loose = backups.join("20260807-131500-ab12cd34.lkb");
        std::fs::write(&loose, b"lkb bytes").unwrap();
        std::fs::set_permissions(&loose, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(
            delete_backup(&root, "20260807-131500-ab12cd34").is_err(),
            "permission-unsafe backups must not be deleted"
        );
        assert!(loose.exists());

        std::fs::remove_file(&loose).unwrap();
        std::os::unix::fs::symlink(territory.join("outside"), &loose).unwrap();
        assert!(
            delete_backup(&root, "20260807-131500-ab12cd34").is_err(),
            "symbolic links must not be deleted"
        );
        assert!(std::fs::symlink_metadata(&loose).is_ok());
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    #[test]
    fn delete_requires_yes_in_non_interactive_mode() {
        let _interactive_guard = crate::interaction::interactive::test_guard();
        crate::interaction::interactive::configure(true);
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                crate::interaction::interactive::configure(false);
            }
        }
        let _reset = Reset;
        let dir = temp_dir("delete-noyes");
        let args = BackupDelete {
            backup: "20260807-131500-ab12cd34".into(),
            yes: false,
        };
        assert_eq!(run_delete(&args), ExitCode::from(2));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_rejects_malformed_ids_and_missing_files() {
        let _interactive_guard = crate::interaction::interactive::test_guard();
        crate::interaction::interactive::configure(true);
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                crate::interaction::interactive::configure(false);
            }
        }
        let _reset = Reset;
        let (_guard, territory) = setup("delete-bad");
        let args = BackupDelete {
            backup: "../escape".into(),
            yes: true,
        };
        assert_eq!(run_delete(&args), ExitCode::from(2));
        let args = BackupDelete {
            backup: "20260807-131500-ab12cd34".into(),
            yes: true,
        };
        assert_eq!(run_delete(&args), ExitCode::FAILURE);
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }
}
