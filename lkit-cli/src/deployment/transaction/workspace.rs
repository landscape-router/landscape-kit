//! Live directories move only within the installation filesystem. Journals,
//! state snapshots and portable backups remain in the lkit territory.
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

use super::{TransactionFile, validate_transaction};
use crate::deployment::{layout, plan::InstallError, root::InstallRoot};

pub(crate) fn select(transaction: &mut TransactionFile) {
    transaction.install_workspace = Some(format!(
        "releases/.lkit-transactions/{}",
        transaction.transaction_id
    ));
}

fn invalid(message: &str) -> InstallError {
    InstallError::CorruptedTransaction(message.into())
}

pub(crate) fn directory(
    root: &InstallRoot,
    transaction: &TransactionFile,
) -> Result<PathBuf, InstallError> {
    validate_transaction(transaction)?;
    if Path::new(&transaction.canonical_install_root) != root.canonical {
        return Err(invalid("installation workspace belongs to another root"));
    }
    let Some(relative) = &transaction.install_workspace else {
        return Ok(layout::territory_transactions_dir().join(&transaction.transaction_id));
    };
    let releases = root.canonical.join("releases");
    let canonical = releases.canonicalize().map_err(InstallError::Io)?;
    if !canonical.starts_with(&root.canonical) {
        return Err(invalid("installation releases escape the managed root"));
    }
    let directory = root.canonical.join(relative);
    for path in [directory.parent().unwrap(), directory.as_path()] {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Ok(_) => return Err(invalid("installation workspace must be a real directory")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(InstallError::Io(error)),
        }
    }
    Ok(directory)
}

pub(crate) fn prepare(
    root: &InstallRoot,
    transaction: &TransactionFile,
) -> Result<PathBuf, InstallError> {
    let directory = directory(root, transaction)?;
    if transaction.install_workspace.is_none() {
        std::fs::create_dir_all(&directory).map_err(InstallError::Io)?;
        return Ok(directory);
    }
    for path in [directory.parent().unwrap(), directory.as_path()] {
        match std::fs::DirBuilder::new().mode(0o700).create(path) {
            Ok(()) => {
                std::fs::File::open(path.parent().unwrap())
                    .and_then(|parent| parent.sync_all())
                    .map_err(InstallError::Io)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(InstallError::Io(error)),
        }
    }
    // Recheck all components before callers may mutate active directories.
    self::directory(root, transaction)
}

pub(crate) fn cleanup_preparing(
    root: &InstallRoot,
    transaction: &TransactionFile,
) -> Result<(), InstallError> {
    if transaction.install_workspace.is_some() {
        match std::fs::remove_dir_all(directory(root, transaction)?) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(InstallError::Io(error)),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn installation_workspace_is_private_and_legacy_layout_remains_readable() {
        let temp = std::env::temp_dir().join(format!("lkit-workspace-{}", uuid::Uuid::now_v7()));
        let root = InstallRoot {
            install_root: temp.join("landscape"),
            canonical: temp.join("landscape"),
        };
        std::fs::create_dir_all(root.canonical.join("releases")).unwrap();
        let _territory = layout::test_territory(&temp.join("territory"));
        let mut tx = TransactionFile::new_repair_static(&root).unwrap();
        assert_eq!(
            directory(&root, &tx).unwrap(),
            layout::territory_transactions_dir().join(&tx.transaction_id)
        );
        select(&mut tx);
        let local = prepare(&root, &tx).unwrap();
        assert!(local.starts_with(root.canonical.join("releases")));
        assert_eq!(
            std::fs::metadata(&local).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let bytes = serde_json::to_vec(&tx).unwrap();
        let recovered = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(directory(&root, &recovered).unwrap(), local);
        std::fs::remove_dir_all(&temp).unwrap();
    }

    #[test]
    fn rejects_workspace_escape_foreign_identity_and_symlink() {
        let temp = std::env::temp_dir().join(format!("lkit-workspace-{}", uuid::Uuid::now_v7()));
        let root = InstallRoot {
            install_root: temp.clone(),
            canonical: temp.clone(),
        };
        std::fs::create_dir_all(temp.join("releases")).unwrap();
        let mut tx = TransactionFile::new_repair_static(&root).unwrap();
        tx.install_workspace = Some("../outside".into());
        assert!(directory(&root, &tx).is_err());
        select(&mut tx);
        let mut foreign = tx.clone();
        foreign.canonical_install_root = temp.join("other").to_string_lossy().into_owned();
        assert!(directory(&root, &foreign).is_err());
        symlink(
            temp.join("outside"),
            temp.join("releases/.lkit-transactions"),
        )
        .unwrap();
        assert!(prepare(&root, &tx).is_err());
        assert!(!temp.join("outside").exists());
        std::fs::remove_dir_all(&temp).unwrap();
    }
}
