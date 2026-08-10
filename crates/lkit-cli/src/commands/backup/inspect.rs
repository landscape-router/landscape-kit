use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::backup::lkb::{LKB_METADATA_CAPACITY, backup_id_format_ok, verify_lkb};
use crate::deployment::plan;
use crate::workflows::restore::validate_backup_file;

use super::architecture_key;
use super::exit_code;
use super::resolve_root;
use super::scope_key;
use super::{BackupShow, BackupVerify};

pub(super) fn run_show(args: &BackupShow) -> ExitCode {
    let (bytes, label) =
        match resolve_backup_bytes(&args.backup, &args.file, args.install_dir.as_deref()) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("backup: {error}");
                return exit_code(&error);
            }
        };
    let metadata = match verify_lkb(&bytes) {
        Ok(metadata) => metadata,
        Err(error) => {
            eprintln!("backup: {error}");
            return ExitCode::FAILURE;
        }
    };
    let metadata_len = metadata_bytes_len(&bytes).unwrap_or(0);
    println!("{label}");
    println!("backup_id: {}", metadata.backup_id);
    println!("created_at: {}", metadata.created_at);
    println!("landscape_version: {}", metadata.landscape_version);
    println!("lkit_version: {}", metadata.lkit_version);
    println!("architecture: {}", architecture_key(metadata.architecture));
    println!("hostname: {}", metadata.hostname);
    println!("remark: {}", metadata.remark);
    println!("auto: {}", metadata.auto);
    println!("scope: {}", scope_key(metadata.scope));
    println!(
        "contents: binary={} static={} static_archive={} init_config={} geo_cache={}",
        metadata.contents.binary,
        metadata.contents.static_,
        metadata.contents.static_archive,
        metadata.contents.init_config,
        metadata.contents.geo_cache,
    );
    println!("header_bytes: 32");
    println!("metadata_bytes: {metadata_len}");
    println!(
        "archive_bytes: {}",
        bytes.len().saturating_sub(LKB_METADATA_CAPACITY)
    );
    ExitCode::SUCCESS
}

pub(super) fn run_verify(args: &BackupVerify) -> ExitCode {
    let (bytes, label) =
        match resolve_backup_bytes(&args.backup, &args.file, args.install_dir.as_deref()) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("backup: {error}");
                return exit_code(&error);
            }
        };
    let metadata = match verify_lkb(&bytes) {
        Ok(metadata) => metadata,
        Err(error) => {
            eprintln!("backup: {error}");
            return ExitCode::FAILURE;
        }
    };
    let verify_dir =
        std::env::temp_dir().join(format!("lkit-backup-verify-{}", uuid::Uuid::now_v7()));
    if let Err(error) = crate::backup::lkb::create_secure_dir(&verify_dir, 0o700) {
        eprintln!("backup: {error}");
        return exit_code(&error);
    }
    if let Err(error) = crate::backup::lkb::extract_lkb(&bytes, &verify_dir) {
        let _ = std::fs::remove_dir_all(&verify_dir);
        eprintln!("backup: {error}");
        return ExitCode::FAILURE;
    }
    let _ = std::fs::remove_dir_all(&verify_dir);
    println!(
        "backup: {}",
        crate::tr!(
            crate::keys::BACKUP_VERIFIED,
            backup_id = metadata.backup_id,
            label = label
        )
    );
    ExitCode::SUCCESS
}

fn resolve_backup_bytes(
    backup: &Option<String>,
    file: &Option<PathBuf>,
    install_dir: Option<&Path>,
) -> Result<(Vec<u8>, String), plan::InstallError> {
    match (backup, file) {
        (Some(id), None) => {
            if !backup_id_format_ok(id) {
                return Err(plan::InstallError::ParameterUsage(format!(
                    "--backup {id} does not match the backup ID format YYYYMMDD-HHMMSS-<8 lowercase hex>"
                )));
            }
            let root = resolve_root(install_dir)?;
            let path = root.canonical.join("backups").join(format!("{id}.lkb"));
            if !path.is_file() {
                return Err(plan::InstallError::InvalidBackup(format!(
                    "backup {id} not found under {}",
                    root.canonical.join("backups").display()
                )));
            }
            validate_backup_file(&path)?;
            Ok((
                std::fs::read(&path).map_err(plan::InstallError::Io)?,
                format!("backups/{id}.lkb"),
            ))
        }
        (None, Some(path)) => {
            validate_backup_file(path)?;
            Ok((
                std::fs::read(path).map_err(plan::InstallError::Io)?,
                path.display().to_string(),
            ))
        }
        _ => Err(plan::InstallError::ParameterUsage(
            "--backup and --file cannot be combined; one of them is required".into(),
        )),
    }
}

fn metadata_bytes_len(bytes: &[u8]) -> Option<usize> {
    if bytes.len() < 32 {
        return None;
    }
    Some(u32::from_le_bytes(bytes[6..10].try_into().ok()?) as usize)
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

    /// 构造不含 tar checksum 校验的最小 tar 字节流(用于包装成 `.lkb`)。
    fn raw_tar(entries: &[(&str, u8, &[u8])]) -> Vec<u8> {
        let mut tar = Vec::new();
        for (name, kind, content) in entries {
            let mut header = [0u8; 512];
            header[..name.len()].copy_from_slice(name.as_bytes());
            let size = format!("{:011o}", content.len());
            header[124..124 + 11].copy_from_slice(size.as_bytes());
            header[156] = *kind;
            for byte in &mut header[148..156] {
                *byte = b' ';
            }
            let sum: u32 = header.iter().map(|byte| *byte as u32).sum();
            let octal = format!("{sum:06o}");
            header[148..154].copy_from_slice(octal.as_bytes());
            header[154] = 0;
            header[155] = b' ';
            tar.extend_from_slice(&header);
            tar.extend_from_slice(content);
            let pad = (512 - content.len() % 512) % 512;
            tar.extend(std::iter::repeat_n(0, pad));
        }
        tar.extend([0u8; 1024]);
        tar
    }

    /// 把 tar.gz 包装成 checksum 自洽的 `.lkb` 字节(metadata 的 checksum 与归档一致)。
    fn wrap(tar_gz: &[u8]) -> Vec<u8> {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(tar_gz);
        let sha256: String = hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let metadata = crate::backup::lkb::BackupMetadata {
            schema_version: 1,
            backup_id: format!("20260801-163000-{}", &sha256[..8]),
            created_at: chrono::DateTime::parse_from_rfc3339("2026-08-01T16:30:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
            landscape_version: "1.2.3".into(),
            lkit_version: "0.1.0".into(),
            architecture: crate::backup::lkb::BackupArchitecture::X86_64,
            hostname: "test".into(),
            remark: String::new(),
            auto: true,
            scope: crate::backup::lkb::BackupScope::Minimal,
            contents: crate::backup::lkb::BackupContents {
                binary: true,
                static_: true,
                static_archive: true,
                init_config: true,
                geo_cache: true,
            },
            checksum: format!("sha256:{sha256}"),
        };
        let mut bytes = Vec::new();
        let mut header = [0u8; crate::backup::lkb::LKB_HEADER_LEN];
        header[0..4].copy_from_slice(crate::backup::lkb::LKB_MAGIC);
        header[4..6].copy_from_slice(&1u16.to_le_bytes());
        header[6..10]
            .copy_from_slice(&(serde_json::to_vec(&metadata).unwrap().len() as u32).to_le_bytes());
        bytes.extend_from_slice(&header);
        bytes.extend_from_slice(&serde_json::to_vec(&metadata).unwrap());
        bytes.resize(crate::backup::lkb::LKB_METADATA_CAPACITY, 0);
        bytes.extend_from_slice(tar_gz);
        bytes
    }

    /// 写入一个 `root:backups/<id>.lkb` 文件并设为 `0600`。
    #[cfg(feature = "test-support")]
    fn write_backup_file(dir: &std::path::Path, bytes: &[u8]) {
        let backups = dir.join("backups");
        std::fs::create_dir_all(&backups).unwrap();
        std::fs::write(backups.join("20260801-163000-a1b2c3d4.lkb"), bytes).unwrap();
        std::fs::set_permissions(
            backups.join("20260801-163000-a1b2c3d4.lkb"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
    }

    #[cfg(feature = "test-support")]
    fn verify_args(dir: &std::path::Path) -> BackupVerify {
        BackupVerify {
            backup: Some("20260801-163000-a1b2c3d4".into()),
            file: None,
            install_dir: Some(dir.to_path_buf()),
            test_runtime: None,
        }
    }

    #[cfg(feature = "test-support")]
    fn leftover_verify_dirs() -> Vec<std::path::PathBuf> {
        std::fs::read_dir(std::env::temp_dir())
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .filter(|path| {
                        path.file_name()
                            .and_then(|name| name.to_str())
                            .is_some_and(|name| name.starts_with("lkit-backup-verify-"))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn verify_cleans_up_temp_dirs_and_rejects_incomplete_archives() {
        // 完整归档 verify 成功且不留临时解包目录。
        let dir = temp_dir("verify");
        write_backup_file(
            &dir,
            &wrap(&gzip_tar(&raw_tar(&[
                ("landscape-webserver", b'0', b"bin"),
                ("landscape_init.toml", b'0', b"init"),
                ("static.zip", b'0', b"zip"),
                ("static", b'5', b""),
                ("geo_tmp", b'5', b""),
            ]))),
        );
        assert_eq!(run_verify(&verify_args(&dir)), ExitCode::SUCCESS);
        assert!(
            leftover_verify_dirs().is_empty(),
            "verify must clean up its temporary extraction directory"
        );
        // 归档缺少必需条目 landscape_init.toml 时,verify 必须失败且不留临时目录。
        write_backup_file(
            &dir,
            &wrap(&gzip_tar(&raw_tar(&[
                ("landscape-webserver", b'0', b"bin"),
                ("static.zip", b'0', b"zip"),
                ("static", b'5', b""),
                ("geo_tmp", b'5', b""),
            ]))),
        );
        assert_eq!(run_verify(&verify_args(&dir)), ExitCode::FAILURE);
        assert!(
            leftover_verify_dirs().is_empty(),
            "failed verify must not leave temporary extraction directories"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn gzip_tar(mut tar: &[u8]) -> Vec<u8> {
        use flate2::Compression;
        use flate2::write::GzEncoder;
        let mut tar_gz = Vec::new();
        let encoder = GzEncoder::new(&mut tar_gz, Compression::default());
        let mut gz = encoder;
        std::io::copy(&mut tar, &mut gz).unwrap();
        gz.finish().unwrap();
        tar_gz
    }
}
