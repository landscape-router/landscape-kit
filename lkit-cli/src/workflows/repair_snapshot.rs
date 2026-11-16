//! Versioned static repair preimages. All three objects are restored together.
use crate::backup::rollback;
use crate::deployment::{
    layout,
    plan::InstallError,
    root::InstallRoot,
    state,
    transaction::{self, Phase, TransactionFile},
};
use serde::{Deserialize, Serialize};
use std::{os::unix::fs::PermissionsExt, path::Path};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    version: u32,
    state: state::InstallState,
    archive: Option<(String, u32)>,
    static_digest: String,
}
fn failure(message: &str) -> InstallError {
    InstallError::CorruptedTransaction(message.into())
}

pub(crate) fn sync_tree(path: &Path) -> Result<(), InstallError> {
    if path.is_dir() {
        for entry in std::fs::read_dir(path).map_err(InstallError::Io)? {
            sync_tree(&entry.map_err(InstallError::Io)?.path())?;
        }
    }
    std::fs::File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(InstallError::Io)
}
pub(crate) fn save(
    root: &InstallRoot,
    tx: &TransactionFile,
    installed: &state::InstallState,
) -> Result<(), InstallError> {
    let dir = layout::territory_transactions_dir().join(&tx.transaction_id);
    let archive = root
        .canonical
        .join("releases")
        .join(&installed.active_version)
        .join("static.zip");
    let saved = dir.join("previous-static.zip");
    let archive = match std::fs::symlink_metadata(&archive) {
        Ok(meta) if meta.is_file() => {
            std::fs::copy(&archive, &saved).map_err(InstallError::Io)?;
            Some((
                crate::release::artifacts::hash_file(&saved)?.0,
                meta.permissions().mode() & 0o777,
            ))
        }
        Ok(_) => return Err(failure("static archive must be a regular file")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(InstallError::Io(e)),
    };
    let snapshot = Snapshot {
        version: 1,
        state: installed.clone(),
        archive,
        static_digest: tree_digest(&dir.join("static-backup"))?,
    };
    super::repair_service::atomic_write(
        &dir.join("static-snapshot.json"),
        &serde_json::to_vec(&snapshot).map_err(InstallError::StateWrite)?,
        0o600,
    )?;
    sync_tree(&dir)
}
pub(crate) fn restore(root: &InstallRoot, tx: &TransactionFile) -> Result<(), InstallError> {
    let dir = layout::territory_transactions_dir().join(&tx.transaction_id);
    let snapshot: Snapshot = serde_json::from_slice(
        &std::fs::read(dir.join("static-snapshot.json")).map_err(InstallError::Io)?,
    )
    .map_err(|e| failure(&format!("invalid static snapshot: {e}")))?;
    state::validate_state(&snapshot.state)?;
    let backup = tx
        .static_backup
        .as_ref()
        .ok_or_else(|| failure("static snapshot reference missing"))?;
    let version = &snapshot.state.active_version;
    if snapshot.version != 1
        || Path::new(&snapshot.state.canonical_install_root) != root.canonical
        || backup.path != format!("transactions/{}/static-backup", tx.transaction_id)
        || backup.target != format!("releases/{version}/static")
    {
        return Err(failure("static snapshot identity mismatch"));
    }
    let source = dir.join("static-backup");
    if !source.is_dir() {
        return Err(failure("static directory snapshot missing"));
    }
    let saved = dir.join("previous-static.zip");
    if let Some((hash, _)) = &snapshot.archive
        && crate::release::artifacts::hash_file(&saved)?.0 != *hash
    {
        return Err(failure("static archive snapshot corrupted"));
    }
    if tree_digest(&source)? != snapshot.static_digest {
        return Err(failure("static directory snapshot corrupted"));
    }
    transaction::mark_phase(root, tx, Phase::RollingBack)?;
    let target = root.canonical.join(&backup.target);
    if target.exists() {
        std::fs::remove_dir_all(&target).map_err(InstallError::Io)?;
    }
    rollback::copy_tree_into(&source, &target)?;
    sync_tree(&target)?;
    let archive = root
        .canonical
        .join("releases")
        .join(version)
        .join("static.zip");
    if let Some((_, mode)) = snapshot.archive {
        super::repair_service::atomic_write(
            &archive,
            &std::fs::read(saved).map_err(InstallError::Io)?,
            mode,
        )?;
    } else {
        match std::fs::remove_file(&archive) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(InstallError::Io(e)),
        }
        std::fs::File::open(archive.parent().unwrap())
            .and_then(|dir| dir.sync_all())
            .map_err(InstallError::Io)?;
    }
    state::write_state(root, &snapshot.state)?;
    transaction::mark_phase(root, tx, Phase::RolledBack)
}

pub(crate) fn tree_digest(root: &Path) -> Result<String, InstallError> {
    fn visit(
        path: &Path,
        root: &Path,
        records: &mut Vec<(Vec<u8>, u32, Option<String>)>,
    ) -> Result<(), InstallError> {
        let meta = std::fs::symlink_metadata(path).map_err(InstallError::Io)?;
        if !meta.is_file() && !meta.is_dir() {
            return Err(failure("static preimage contains a non-regular entry"));
        }
        let digest = if meta.is_file() {
            Some(crate::release::artifacts::hash_file(path)?.0)
        } else {
            None
        };
        records.push((
            path.strip_prefix(root)
                .unwrap()
                .as_os_str()
                .as_encoded_bytes()
                .to_vec(),
            meta.permissions().mode() & 0o777,
            digest,
        ));
        if meta.is_dir() {
            let mut children = std::fs::read_dir(path)
                .map_err(InstallError::Io)?
                .map(|entry| entry.map(|entry| entry.path()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(InstallError::Io)?;
            children.sort();
            for child in children {
                visit(&child, root, records)?;
            }
        }
        Ok(())
    }
    let mut records = Vec::new();
    visit(root, root, &mut records)?;
    Ok(crate::release::artifacts::hash_str(
        &serde_json::to_string(&records).map_err(InstallError::StateWrite)?,
    ))
}
