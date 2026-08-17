use std::fs::OpenOptions;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::Path;

use chrono::Utc;
use serde::{Deserialize, Serialize};

use super::plan::InstallError;

pub(crate) const RESOLV_CONF: &str = "/etc/resolv.conf";
pub(crate) const MAX_RESOLV_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum FileType {
    Regular,
    Symlink,
    Missing,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct ResolvMetadata {
    pub schema_version: u64,
    pub path: String,
    pub file_type: FileType,
    pub symlink_target: Option<String>,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub content_saved: bool,
    pub captured_at: chrono::DateTime<Utc>,
}

/// 备份 `source` 到 `backup_dir`(形如 `backups/<tx-id>/host/resolv.conf`),
/// 写入 `metadata.json`;普通文件同时保存 `content`。
pub(crate) fn backup(source: &Path, backup_dir: &Path) -> Result<ResolvMetadata, InstallError> {
    std::fs::create_dir_all(backup_dir).map_err(InstallError::Io)?;
    let metadata = capture(source)?;
    let metadata_json = serde_json::to_vec_pretty(&metadata).map_err(InstallError::StateWrite)?;
    write_atomic(&backup_dir.join("metadata.json"), &metadata_json, 0o600)?;
    if metadata.file_type == FileType::Regular {
        let content = read_limited(source)?;
        write_atomic(&backup_dir.join("content"), &content, 0o600)?;
    }
    Ok(metadata)
}

fn capture(source: &Path) -> Result<ResolvMetadata, InstallError> {
    let stat = match std::fs::symlink_metadata(source) {
        Ok(stat) => stat,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ResolvMetadata {
                schema_version: 1,
                path: source.display().to_string(),
                file_type: FileType::Missing,
                symlink_target: None,
                mode: 0,
                uid: 0,
                gid: 0,
                content_saved: false,
                captured_at: Utc::now(),
            });
        }
        Err(error) => return Err(InstallError::Io(error)),
    };
    if stat.file_type().is_symlink() {
        let target = std::fs::read_link(source).map_err(InstallError::Io)?;
        return Ok(ResolvMetadata {
            schema_version: 1,
            path: source.display().to_string(),
            file_type: FileType::Symlink,
            symlink_target: Some(target.display().to_string()),
            mode: 0,
            uid: 0,
            gid: 0,
            content_saved: false,
            captured_at: Utc::now(),
        });
    }
    if !stat.file_type().is_file() {
        return Err(resolv_backup(format!(
            "{} is a directory, device, or other unsupported type",
            source.display()
        )));
    }
    Ok(ResolvMetadata {
        schema_version: 1,
        path: source.display().to_string(),
        file_type: FileType::Regular,
        symlink_target: None,
        mode: stat.mode() & 0o7777,
        uid: stat.uid(),
        gid: stat.gid(),
        content_saved: true,
        captured_at: Utc::now(),
    })
}

fn read_limited(path: &Path) -> Result<Vec<u8>, InstallError> {
    let metadata = std::fs::metadata(path).map_err(InstallError::Io)?;
    if metadata.len() > MAX_RESOLV_BYTES {
        return Err(resolv_backup(format!(
            "{} exceeds the {} byte limit",
            path.display(),
            MAX_RESOLV_BYTES
        )));
    }
    std::fs::read(path).map_err(InstallError::Io)
}

fn write_atomic(path: &Path, bytes: &[u8], mode: u32) -> Result<(), InstallError> {
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(&tmp)
        .map_err(InstallError::Io)?;
    file.write_all(bytes).map_err(InstallError::Io)?;
    file.sync_all().map_err(InstallError::Io)?;
    std::fs::rename(&tmp, path).map_err(InstallError::Io)?;
    Ok(())
}

/// 按 `metadata.json` 恢复 `source` 的原始状态:普通文件恢复内容/权限/所有者,
/// 软链接恢复目标,原本不存在时删除本次产生的文件。全部使用原子替换。
pub(crate) fn restore(source: &Path, backup_dir: &Path) -> Result<(), InstallError> {
    let bytes = std::fs::read(backup_dir.join("metadata.json")).map_err(InstallError::Io)?;
    let metadata: ResolvMetadata = serde_json::from_slice(&bytes).map_err(|error| {
        InstallError::ResolvBackup(format!(
            "{} is not valid backup metadata: {error}",
            backup_dir.display()
        ))
    })?;
    if metadata.schema_version != 1 {
        return Err(resolv_backup(format!(
            "unsupported metadata schema version {}",
            metadata.schema_version
        )));
    }
    match metadata.file_type {
        FileType::Missing => {
            if std::fs::symlink_metadata(source).is_ok() {
                std::fs::remove_file(source).map_err(InstallError::Io)?;
            }
        }
        FileType::Symlink => {
            let target = metadata
                .symlink_target
                .as_deref()
                .ok_or_else(|| resolv_backup("symlink backup is missing symlink_target".into()))?;
            let tmp = source.with_extension(format!("tmp.{}", std::process::id()));
            let _ = std::fs::remove_file(&tmp);
            std::os::unix::fs::symlink(target, &tmp).map_err(InstallError::Io)?;
            std::fs::rename(&tmp, source).map_err(|error| {
                let _ = std::fs::remove_file(&tmp);
                InstallError::Io(error)
            })?;
        }
        FileType::Regular => {
            let content = match metadata.content_saved {
                true => std::fs::read(backup_dir.join("content")).map_err(InstallError::Io)?,
                false => Vec::new(),
            };
            let tmp = source.with_extension(format!("tmp.{}", std::process::id()));
            let mut file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(metadata.mode & 0o7777)
                .open(&tmp)
                .map_err(InstallError::Io)?;
            file.write_all(&content).map_err(InstallError::Io)?;
            file.sync_all().map_err(InstallError::Io)?;
            std::fs::set_permissions(
                &tmp,
                std::fs::Permissions::from_mode(metadata.mode & 0o7777),
            )
            .map_err(InstallError::Io)?;
            let result = unsafe { libc::fchown(file.as_raw_fd(), metadata.uid, metadata.gid) };
            if result != 0 {
                let _ = std::fs::remove_file(&tmp);
                return Err(InstallError::Io(std::io::Error::last_os_error()));
            }
            drop(file);
            std::fs::rename(&tmp, source).map_err(|error| {
                let _ = std::fs::remove_file(&tmp);
                InstallError::Io(error)
            })?;
        }
    }
    Ok(())
}

fn resolv_backup(reason: String) -> InstallError {
    InstallError::ResolvBackup(reason)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("lkit-resolv-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn round_trips_regular_file() {
        let dir = temp_dir("regular");
        let source = dir.join("resolv.conf");
        std::fs::write(&source, b"nameserver 127.0.0.1\n").unwrap();
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o644)).unwrap();
        let backup_dir = dir.join("backup");
        let metadata = backup(&source, &backup_dir).unwrap();
        assert_eq!(metadata.file_type, FileType::Regular);
        assert!(metadata.content_saved);
        assert!(backup_dir.join("content").is_file());

        std::fs::write(&source, b"nameserver 8.8.8.8\n").unwrap();
        restore(&source, &backup_dir).unwrap();
        assert_eq!(std::fs::read(&source).unwrap(), b"nameserver 127.0.0.1\n");
        let mode = std::fs::metadata(&source).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o644);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn round_trips_symlink() {
        let dir = temp_dir("symlink");
        let source = dir.join("resolv.conf");
        std::os::unix::fs::symlink("../run/systemd/resolve/stub-resolv.conf", &source).unwrap();
        let backup_dir = dir.join("backup");
        let metadata = backup(&source, &backup_dir).unwrap();
        assert_eq!(metadata.file_type, FileType::Symlink);
        assert_eq!(
            metadata.symlink_target.as_deref(),
            Some("../run/systemd/resolve/stub-resolv.conf")
        );

        std::fs::remove_file(&source).unwrap();
        std::fs::write(&source, b"changed\n").unwrap();
        restore(&source, &backup_dir).unwrap();
        assert_eq!(
            std::fs::read_link(&source).unwrap().display().to_string(),
            "../run/systemd/resolve/stub-resolv.conf"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn round_trips_missing() {
        let dir = temp_dir("missing");
        let source = dir.join("resolv.conf");
        let backup_dir = dir.join("backup");
        let metadata = backup(&source, &backup_dir).unwrap();
        assert_eq!(metadata.file_type, FileType::Missing);

        std::fs::write(&source, b"created later\n").unwrap();
        restore(&source, &backup_dir).unwrap();
        assert!(!source.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_directories_and_oversize() {
        let dir = temp_dir("invalid");
        let source = dir.join("resolv.conf");
        std::fs::create_dir_all(&source).unwrap();
        assert!(backup(&source, &dir.join("b1")).is_err());

        std::fs::remove_dir_all(&source).unwrap();
        std::fs::write(&source, vec![b'x'; 1024 * 1024 + 1]).unwrap();
        assert!(backup(&source, &dir.join("b2")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn restore_rejects_corrupted_metadata() {
        let dir = temp_dir("corrupt");
        let backup_dir = dir.join("backup");
        std::fs::create_dir_all(&backup_dir).unwrap();
        std::fs::write(backup_dir.join("metadata.json"), b"not json").unwrap();
        assert!(matches!(
            restore(&dir.join("resolv.conf"), &backup_dir),
            Err(InstallError::ResolvBackup(_))
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
