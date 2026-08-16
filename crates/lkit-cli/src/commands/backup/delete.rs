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

/// 删除安装根目录 `backups/` 下的备份：ID 必须已通过格式校验，目标必须是
/// root 所有、权限不宽于 `0600` 的普通文件（不跟随符号链接）。CLI 与
/// 交互控制台共用。
pub(crate) fn delete_backup(_root: &InstallRoot, backup_id: &str) -> Result<(), InstallError> {
    let path = crate::deployment::layout::territory_backups_dir().join(format!("{backup_id}.lkb"));
    if !path.is_file() {
        return Err(plan::InstallError::InvalidBackup(format!(
            "backup {backup_id} not found under {}",
            crate::deployment::layout::territory_backups_dir().display()
        )));
    }
    validate_backup_file(&path)?;
    std::fs::remove_file(&path).map_err(plan::InstallError::Io)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

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
