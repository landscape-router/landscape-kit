//! 逐字备份与恢复:改写前把计划涉及的文件逐字复制到备份根目录并写 manifest.json,
//! 恢复按 manifest 逐字覆盖(幂等)。备份与恢复均原子写回。

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use crate::error::HostNetError;
use crate::model::{EditPlan, MANIFEST_SCHEMA_VERSION, Manifest, ManifestFile};

use super::edit;

pub(crate) fn backup(plan: &EditPlan, dest: &Path) -> Result<Manifest, HostNetError> {
    if !dest.is_absolute() {
        return Err(HostNetError::PathSafety {
            path: dest.to_path_buf(),
            reason: "backup destination must be absolute".into(),
        });
    }
    if std::fs::symlink_metadata(dest).is_ok() {
        return Err(HostNetError::PathSafety {
            path: dest.to_path_buf(),
            reason: "backup destination already exists".into(),
        });
    }
    if plan.edits.is_empty() {
        return Ok(Manifest {
            schema_version: MANIFEST_SCHEMA_VERSION,
            files: Vec::new(),
        });
    }
    for file_edit in &plan.edits {
        if !file_edit.path.is_absolute() {
            return Err(HostNetError::PathSafety {
                path: file_edit.path.clone(),
                reason: "original path must be absolute".into(),
            });
        }
        edit::verify_edit(file_edit)?;
    }

    let parent = dest.parent().ok_or_else(|| HostNetError::PathSafety {
        path: dest.to_path_buf(),
        reason: "backup destination has no parent directory".into(),
    })?;
    std::fs::create_dir_all(parent).map_err(|source| HostNetError::UnreadableFile {
        path: parent.to_path_buf(),
        source,
    })?;
    std::fs::create_dir(dest).map_err(|source| HostNetError::UnreadableFile {
        path: dest.to_path_buf(),
        source,
    })?;
    std::fs::set_permissions(dest, std::fs::Permissions::from_mode(0o700)).map_err(|source| {
        HostNetError::UnreadableFile {
            path: dest.to_path_buf(),
            source,
        }
    })?;

    let result = (|| {
        let mut files = Vec::new();
        for (index, file_edit) in plan.edits.iter().enumerate() {
            let original = &file_edit.path;
            let file_name = original
                .file_name()
                .ok_or_else(|| HostNetError::PathSafety {
                    path: original.clone(),
                    reason: "original has no file name".into(),
                })?
                .to_string_lossy()
                .to_string();
            let backup_dir = dest.join(index.to_string());
            std::fs::create_dir(&backup_dir).map_err(|source| HostNetError::UnreadableFile {
                path: backup_dir.clone(),
                source,
            })?;
            std::fs::set_permissions(&backup_dir, std::fs::Permissions::from_mode(0o700)).map_err(
                |source| HostNetError::UnreadableFile {
                    path: backup_dir.clone(),
                    source,
                },
            )?;
            let backup_path = backup_dir.join(file_name);
            edit::write_atomic(&backup_path, &file_edit.original_content, None)?;
            files.push(ManifestFile {
                original: original.clone(),
                backup: backup_path,
                metadata: file_edit.metadata.clone(),
            });
        }
        let manifest = Manifest {
            schema_version: MANIFEST_SCHEMA_VERSION,
            files,
        };
        let manifest_bytes = serde_json::to_vec_pretty(&manifest).map_err(|source| {
            HostNetError::InvalidManifest {
                path: dest.to_path_buf(),
                reason: source.to_string(),
            }
        })?;
        edit::write_atomic(&dest.join("manifest.json"), &manifest_bytes, None)?;
        Ok(manifest)
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(dest);
    }
    result
}

pub(crate) fn restore(manifest: &Manifest) -> Result<(), HostNetError> {
    let restore_files = read_backups(manifest)?;
    for (file, bytes) in restore_files {
        edit::write_atomic(&file.original, &bytes, Some(&file.metadata))?;
    }
    Ok(())
}

/// Roll back a failed transaction without overwriting an external edit.
///
/// A file is restored only if it is still at the content and metadata produced
/// by this plan. Files that are still at their original snapshot are skipped;
/// files with any other state are preserved and reported as concurrent changes.
pub(crate) fn restore_if_unchanged(
    manifest: &Manifest,
    plan: &EditPlan,
) -> Result<(), HostNetError> {
    let restore_files = read_backups(manifest)?;
    if restore_files.len() != plan.edits.len()
        || restore_files
            .iter()
            .zip(&plan.edits)
            .any(|((file, bytes), edit)| {
                file.original != edit.path
                    || file.metadata != edit.metadata
                    || bytes != &edit.original_content
            })
    {
        return Err(HostNetError::InvalidManifest {
            path: Path::new("<manifest>").to_path_buf(),
            reason: "manifest does not match the edit plan".into(),
        });
    }

    let mut pending = Vec::new();
    let mut conflict = None;
    for ((file, original), edit) in restore_files.iter().zip(&plan.edits) {
        let metadata = edit::capture_metadata(&edit.path)?;
        let current = std::fs::read(&edit.path).map_err(|source| HostNetError::UnreadableFile {
            path: edit.path.clone(),
            source,
        })?;
        if metadata == edit.metadata && current == edit.original_content {
            continue;
        }
        if metadata == edit.metadata && current == edit.content.as_bytes() {
            pending.push((file.original.clone(), original.clone(), edit.clone()));
        } else if conflict.is_none() {
            conflict = Some(HostNetError::ConcurrentModification {
                path: edit.path.clone(),
            });
        }
    }

    for (path, original, edit) in pending {
        edit::write_atomic_checked(
            &path,
            &original,
            Some(&edit.metadata),
            Some((edit.content.as_bytes(), &edit.metadata)),
        )?;
    }

    match conflict {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn read_backups(manifest: &Manifest) -> Result<Vec<(&ManifestFile, Vec<u8>)>, HostNetError> {
    if manifest.schema_version != MANIFEST_SCHEMA_VERSION {
        return Err(HostNetError::InvalidManifest {
            path: Path::new("<manifest>").to_path_buf(),
            reason: format!(
                "unsupported manifest schema version {}",
                manifest.schema_version
            ),
        });
    }
    let mut restore_files = Vec::new();
    for file in &manifest.files {
        if !file.original.is_absolute() || !file.backup.is_absolute() {
            return Err(HostNetError::InvalidManifest {
                path: file.backup.clone(),
                reason: "manifest paths must be absolute".into(),
            });
        }
        let backup_metadata = std::fs::symlink_metadata(&file.backup).map_err(|source| {
            HostNetError::UnreadableFile {
                path: file.backup.clone(),
                source,
            }
        })?;
        if backup_metadata.file_type().is_symlink() || !backup_metadata.file_type().is_file() {
            return Err(HostNetError::InvalidManifest {
                path: file.backup.clone(),
                reason: "backup must be a regular non-symlink file".into(),
            });
        }
        let bytes = std::fs::read(&file.backup).map_err(|source| HostNetError::UnreadableFile {
            path: file.backup.clone(),
            source,
        })?;
        restore_files.push((file, bytes));
    }
    Ok(restore_files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{EditPlan, FileEdit, FileMetadata};
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("lkit-hostnet-backup-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn plan_for(original: &Path, content: &str) -> EditPlan {
        EditPlan {
            edits: vec![FileEdit {
                path: original.to_path_buf(),
                original_content: std::fs::read(original).unwrap(),
                content: content.into(),
                metadata: edit::capture_metadata(original).unwrap(),
            }],
        }
    }

    #[test]
    fn backup_copies_verbatim_and_restore_restores_exactly() {
        let dir = temp_dir("roundtrip");
        let original = dir.join("interfaces");
        let content = b"auto eth0\niface eth0 inet static\n    address 192.168.1.10\n";
        std::fs::write(&original, content).unwrap();
        let backup_dir = dir.join("backup");
        let plan = plan_for(&original, "iface eth0 inet manual\n");
        let manifest = backup(&plan, &backup_dir).unwrap();
        assert_eq!(manifest.files.len(), 1);
        assert_eq!(manifest.files[0].original, original);
        assert_eq!(
            std::fs::read(manifest.files[0].backup.clone()).unwrap(),
            content
        );
        assert!(backup_dir.join("manifest.json").is_file());
        assert_eq!(
            std::fs::metadata(&manifest.files[0].backup)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );

        std::fs::write(&original, b"iface eth0 inet manual\n").unwrap();
        restore(&manifest).unwrap();
        assert_eq!(std::fs::read(&original).unwrap(), content);
    }

    #[test]
    fn restore_is_idempotent() {
        let dir = temp_dir("idempotent");
        let original = dir.join("interfaces");
        std::fs::write(&original, b"auto eth0\niface eth0 inet static\n").unwrap();
        let backup_dir = dir.join("backup");
        let plan = plan_for(&original, "iface eth0 inet manual\n");
        let manifest = backup(&plan, &backup_dir).unwrap();
        restore(&manifest).unwrap();
        let content = std::fs::read(&original).unwrap();
        restore(&manifest).unwrap();
        assert_eq!(std::fs::read(&original).unwrap(), content);
    }

    #[test]
    fn restore_rejects_unknown_schema_version() {
        let manifest = Manifest {
            schema_version: 99,
            files: Vec::new(),
        };
        let error = restore(&manifest).unwrap_err();
        assert!(matches!(error, HostNetError::InvalidManifest { .. }));
    }

    #[test]
    fn manifest_file_round_trips_through_serde() {
        let manifest = Manifest {
            schema_version: MANIFEST_SCHEMA_VERSION,
            files: vec![ManifestFile {
                original: "/etc/network/interfaces".into(),
                backup: "/tmp/backup/0/interfaces".into(),
                metadata: FileMetadata {
                    mode: 0o640,
                    uid: 0,
                    gid: 0,
                },
            }],
        };
        let json = serde_json::to_vec(&manifest).unwrap();
        let decoded: Manifest = serde_json::from_slice(&json).unwrap();
        assert_eq!(decoded, manifest);
    }

    #[test]
    fn backup_rejects_existing_destination_and_stale_plan() {
        let dir = temp_dir("backup-safety");
        let original = dir.join("interfaces");
        std::fs::write(&original, b"iface eth0 inet dhcp\n").unwrap();
        let plan = plan_for(&original, "iface eth0 inet manual\n");
        let existing = dir.join("existing");
        std::fs::create_dir(&existing).unwrap();
        assert!(matches!(
            backup(&plan, &existing),
            Err(HostNetError::PathSafety { .. })
        ));

        std::fs::write(&original, b"iface eth0 inet static\n").unwrap();
        assert!(matches!(
            backup(&plan, &dir.join("stale")),
            Err(HostNetError::ConcurrentModification { .. })
        ));
        assert!(!dir.join("stale").exists());
    }

    #[test]
    fn empty_plan_does_not_create_backup_directory() {
        let dir = temp_dir("empty-plan");
        let dest = dir.join("backup");
        let manifest = backup(&EditPlan { edits: Vec::new() }, &dest).unwrap();
        assert_eq!(manifest.files, Vec::new());
        assert!(!dest.exists());
    }

    #[test]
    fn restore_rejects_symlink_backup_before_writing_any_original() {
        let dir = temp_dir("restore-symlink");
        let original = dir.join("interfaces");
        let backup_target = dir.join("backup-target");
        let backup_link = dir.join("backup-link");
        std::fs::write(&original, b"current\n").unwrap();
        std::fs::write(&backup_target, b"original\n").unwrap();
        std::os::unix::fs::symlink(&backup_target, &backup_link).unwrap();
        let manifest = Manifest {
            schema_version: MANIFEST_SCHEMA_VERSION,
            files: vec![ManifestFile {
                original: original.clone(),
                backup: backup_link,
                metadata: edit::capture_metadata(&original).unwrap(),
            }],
        };
        assert!(matches!(
            restore(&manifest),
            Err(HostNetError::InvalidManifest { .. })
        ));
        assert_eq!(std::fs::read(&original).unwrap(), b"current\n");
    }

    #[test]
    fn guarded_restore_preserves_external_change() {
        let dir = temp_dir("guarded-external-change");
        let original = dir.join("interfaces");
        std::fs::write(&original, b"original\n").unwrap();
        let plan = plan_for(&original, "edited\n");
        let manifest = backup(&plan, &dir.join("backup")).unwrap();

        std::fs::write(&original, b"external\n").unwrap();
        let error = restore_if_unchanged(&manifest, &plan).unwrap_err();
        assert!(matches!(error, HostNetError::ConcurrentModification { .. }));
        assert_eq!(std::fs::read(&original).unwrap(), b"external\n");
    }

    #[test]
    fn guarded_restore_reverts_our_edit() {
        let dir = temp_dir("guarded-our-edit");
        let original = dir.join("interfaces");
        std::fs::write(&original, b"original\n").unwrap();
        let plan = plan_for(&original, "edited\n");
        let manifest = backup(&plan, &dir.join("backup")).unwrap();

        edit::apply(&plan).unwrap();
        restore_if_unchanged(&manifest, &plan).unwrap();
        assert_eq!(std::fs::read(&original).unwrap(), b"original\n");
    }

    #[test]
    fn guarded_restore_reverts_safe_files_and_preserves_conflicts() {
        let dir = temp_dir("guarded-partial");
        let first = dir.join("interfaces");
        let second = dir.join("fragment");
        std::fs::write(&first, b"first original\n").unwrap();
        std::fs::write(&second, b"second original\n").unwrap();
        let mut plan = plan_for(&first, "first edited\n");
        plan.edits
            .extend(plan_for(&second, "second edited\n").edits);
        let manifest = backup(&plan, &dir.join("backup")).unwrap();

        std::fs::write(&first, b"first edited\n").unwrap();
        std::fs::write(&second, b"second external\n").unwrap();
        let error = restore_if_unchanged(&manifest, &plan).unwrap_err();
        assert!(matches!(error, HostNetError::ConcurrentModification { .. }));
        assert_eq!(std::fs::read(&first).unwrap(), b"first original\n");
        assert_eq!(std::fs::read(&second).unwrap(), b"second external\n");
    }
}
