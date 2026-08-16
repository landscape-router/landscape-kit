use std::path::Path;

use super::super::artifacts::hash_file;
use super::super::backup as lkb;
use super::super::backup::{BackupArchitecture, BackupMetadata};
use super::super::plan::InstallError;
use super::super::root::InstallRoot;
use super::super::state::{InstallState, StateArchitecture};
use super::RestoreArgs;
use crate::deployment::layout;

/// 解析目标备份:ID 只解析安装根目录 `backups/`,外部文件必须 root 所有、
/// 权限不宽于 `0600` 的普通文件。返回完整字节与文件级 SHA-256。
pub(super) fn resolve_target_backup(
    _root: &InstallRoot,
    args: &RestoreArgs,
) -> Result<(Vec<u8>, String), InstallError> {
    match (&args.backup_id, &args.file_path) {
        (Some(id), None) => {
            if !lkb::backup_id_format_ok(id) {
                return Err(InstallError::ParameterUsage(format!(
                    "--backup {id} does not match the backup ID format YYYYMMDD-HHMMSS-<8 lowercase hex>"
                )));
            }
            let path = layout::territory_backups_dir().join(format!("{id}.lkb"));
            if !path.is_file() {
                return Err(InstallError::InvalidBackup(format!(
                    "backup {id} not found under {}",
                    layout::territory_backups_dir().display()
                )));
            }
            validate_backup_file(&path)?;
            let bytes = std::fs::read(&path).map_err(InstallError::Io)?;
            let (sha256, _) = hash_file(&path)?;
            Ok((bytes, sha256))
        }
        (None, Some(path)) => {
            validate_backup_file(path)?;
            let bytes = std::fs::read(path).map_err(InstallError::Io)?;
            let (sha256, _) = hash_file(path)?;
            Ok((bytes, sha256))
        }
        _ => Err(InstallError::ParameterUsage(
            "--backup and --file cannot be combined; one of them is required".into(),
        )),
    }
}

/// `.lkb` 文件:必须是 root 所有、权限不宽于 `0600` 的普通文件,
/// 不跟随符号链接。用于外部文件与安装根目录内的备份。
pub(crate) fn validate_backup_file(path: &Path) -> Result<(), InstallError> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        InstallError::InvalidBackup(format!(
            "{} is not a readable regular file: {error}",
            path.display()
        ))
    })?;
    if !metadata.file_type().is_file() {
        return Err(InstallError::InvalidBackup(format!(
            "{} is not a regular file",
            path.display()
        )));
    }
    let uid = unsafe { libc::geteuid() };
    if metadata.uid() != uid {
        return Err(InstallError::InvalidBackup(format!(
            "{} must be owned by uid {uid}",
            path.display()
        )));
    }
    let mode = metadata.mode() & 0o777;
    if mode & !0o600 != 0 {
        return Err(InstallError::InvalidBackup(format!(
            "{} must not be broader than 0600",
            path.display()
        )));
    }
    Ok(())
}

pub(super) fn check_architecture(
    state: &InstallState,
    metadata: &BackupMetadata,
) -> Result<(), InstallError> {
    let host_arch = std::env::consts::ARCH;
    let backup_arch = match metadata.architecture {
        BackupArchitecture::X86_64 => "x86_64",
        BackupArchitecture::Aarch64 => "aarch64",
    };
    if host_arch != backup_arch {
        return Err(InstallError::InvalidBackup(format!(
            "backup architecture {backup_arch} does not match the host {host_arch}"
        )));
    }
    let state_arch = match state.assets.webserver.architecture {
        StateArchitecture::X86_64 => "x86_64",
        StateArchitecture::Aarch64 => "aarch64",
    };
    if state_arch != backup_arch {
        return Err(InstallError::InvalidBackup(format!(
            "backup architecture {backup_arch} does not match the installation {state_arch}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use super::super::tests::{
        PAYLOAD_1_3_0, TOKEN, YES, ZIP_1_3_0, activate_version, create_target_backup,
        export_server, install_state, interactive_guard, none_health, setup_current, temp_root,
    };
    use super::super::{RestoreArgs, RestoreOptions, restore_version};
    use crate::deployment::state::write_state;
    use crate::deployment::transaction::find_unfinished;
    use crate::service::systemd::Systemd;

    #[tokio::test]
    async fn rejects_malformed_backup_ids_before_creating_a_transaction() {
        let _guard = interactive_guard().await;
        crate::interaction::interactive::configure(false);
        let root = temp_root("bad-id");
        let install_root = InstallRoot {
            install_root: root.clone(),
            canonical: root.clone(),
        };
        activate_version(&install_root, "1.3.0", PAYLOAD_1_3_0, ZIP_1_3_0);
        setup_current(&install_root);
        let state = install_state(&install_root, "1.3.0", PAYLOAD_1_3_0, ZIP_1_3_0);
        write_state(&install_root, &state).unwrap();
        let server = export_server("1.3.0".into());
        let options = RestoreOptions {
            export_base_url: server.base.clone(),
            token: &TOKEN,
            confirm: &YES,
            health: &none_health(),
        };
        for id in [
            "../escape",
            "20260801-163000",
            "20260801-163000-A1B2C3D4",
            "notevenclose",
        ] {
            let args = RestoreArgs {
                backup_id: Some(id.into()),
                file_path: None,
                allow_no_backup: false,
                yes: true,
                console_confirmed: false,
            };
            assert!(
                matches!(
                    restore_version(&install_root, &state, &Systemd::host(), &args, &options).await,
                    Err(InstallError::ParameterUsage(_))
                ),
                "backup id {id:?} must be rejected"
            );
        }
        assert!(find_unfinished(&install_root).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
