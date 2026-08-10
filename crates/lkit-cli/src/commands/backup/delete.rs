use std::process::ExitCode;

use crate::backup::lkb::backup_id_format_ok;
use crate::deployment::lock;
use crate::deployment::plan;
use crate::deployment::plan::InstallError;
use crate::deployment::root::InstallRoot;
use crate::workflows::restore::validate_backup_file;

use super::BackupDelete;
use super::exit_code;
use super::resolve_root;

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
    let root = match resolve_root(args.install_dir.as_deref()) {
        Ok(root) => root,
        Err(error) => {
            eprintln!("backup: {error}");
            return exit_code(&error);
        }
    };
    let _lock = match lock::acquire_install_lock(&root) {
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
pub(crate) fn delete_backup(root: &InstallRoot, backup_id: &str) -> Result<(), InstallError> {
    let path = root
        .canonical
        .join("backups")
        .join(format!("{backup_id}.lkb"));
    if !path.is_file() {
        return Err(plan::InstallError::InvalidBackup(format!(
            "backup {backup_id} not found under {}",
            root.canonical.join("backups").display()
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

    #[test]
    fn deletes_a_valid_backup_file() {
        let dir = temp_dir("delete-ok");
        let backups = dir.join("backups");
        std::fs::create_dir_all(&backups).unwrap();
        let path = backups.join("20260807-131500-ab12cd34.lkb");
        std::fs::write(&path, b"lkb bytes").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let root = crate::deployment::root::InstallRoot {
            install_root: dir.clone(),
            canonical: dir.clone(),
        };
        delete_backup(&root, "20260807-131500-ab12cd34").unwrap();
        assert!(!path.exists(), "the backup file must be removed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refuses_unsafe_or_missing_backups_on_delete() {
        let dir = temp_dir("delete-refuse");
        let backups = dir.join("backups");
        std::fs::create_dir_all(&backups).unwrap();
        let root = crate::deployment::root::InstallRoot {
            install_root: dir.clone(),
            canonical: dir.clone(),
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
        std::os::unix::fs::symlink(dir.join("outside"), &loose).unwrap();
        assert!(
            delete_backup(&root, "20260807-131500-ab12cd34").is_err(),
            "symbolic links must not be deleted"
        );
        assert!(std::fs::symlink_metadata(&loose).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
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
            install_dir: Some(dir.clone()),
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
        let dir = temp_dir("delete-bad");
        let args = BackupDelete {
            backup: "../escape".into(),
            yes: true,
            install_dir: Some(dir.clone()),
        };
        assert_eq!(run_delete(&args), ExitCode::from(2));
        let args = BackupDelete {
            backup: "20260807-131500-ab12cd34".into(),
            yes: true,
            install_dir: Some(dir.clone()),
        };
        assert_eq!(run_delete(&args), ExitCode::FAILURE);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
