use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::backup::lkb::{BackupMetadata, read_backup_metadata_streamed, verify_lkb};
use crate::deployment::plan;
use crate::deployment::root::InstallRoot;
use crate::workflows::restore::validate_backup_file;

use super::BackupList;
use super::architecture_key;
use super::exit_code;
use super::resolve_root;
use super::scope_key;

pub(super) fn run_list(args: &BackupList) -> ExitCode {
    let root = match resolve_root(args.install_dir.as_deref()) {
        Ok(root) => root,
        Err(error) => {
            eprintln!("backup: {error}");
            return exit_code(&error);
        }
    };
    let backups_dir = root.canonical.join("backups");
    let rows = match list_backups(&root) {
        Ok(rows) => rows,
        Err(error) => {
            eprintln!("backup: {error}");
            return exit_code(&error);
        }
    };
    for (parsed, path) in &rows {
        match parsed {
            Some(metadata) => {
                println!(
                    "{} {} {} {} auto={} scope={} remark={} status=valid",
                    metadata.backup_id,
                    metadata.created_at,
                    metadata.landscape_version,
                    architecture_key(metadata.architecture),
                    metadata.auto,
                    scope_key(metadata.scope),
                    metadata.remark,
                );
            }
            None => {
                println!(
                    "{} status=invalid",
                    path.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .trim_end_matches(".lkb")
                );
            }
        }
    }
    let invalid = rows
        .iter()
        .filter(|(metadata, _)| metadata.is_none())
        .count();
    if invalid > 0 {
        eprintln!(
            "backup: {}",
            crate::tr!(crate::keys::BACKUP_LIST_INVALID, count = invalid)
        );
        return ExitCode::FAILURE;
    }
    if rows.is_empty() {
        eprintln!(
            "backup: {}",
            crate::tr!(crate::keys::BACKUP_NONE_FOUND, dir = backups_dir.display())
        );
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// 读取安装根目录 `backups/` 下的 `.lkb` 文件并完整校验,按创建时间降序排列。
/// 目录缺失时返回空列表;校验失败的条目 metadata 为 `None`(视为损坏)。
/// 临时目录或解包写入失败等环境错误直接返回,不得把全部备份误报为损坏。
pub(crate) fn list_backups(
    root: &InstallRoot,
) -> Result<Vec<(Option<BackupMetadata>, PathBuf)>, plan::InstallError> {
    list_backups_with(root, BackupListCheck::Full)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BackupListCheck {
    /// 完整校验：checksum + 安全解包并检查必需条目。CLI `backup list` 使用。
    Full,
    /// 只读容器 Header 与 metadata 区（前 1 MiB），不读取归档体。控制台列表
    /// 使用，保证切换面板时的响应速度；完整校验由 V 与恢复流程按需执行。
    Metadata,
}

pub(crate) fn list_backups_with(
    root: &InstallRoot,
    check: BackupListCheck,
) -> Result<Vec<(Option<BackupMetadata>, PathBuf)>, plan::InstallError> {
    let backups_dir = root.canonical.join("backups");
    let entries = match std::fs::read_dir(&backups_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(plan::InstallError::Io(error)),
    };
    let mut rows: Vec<(Option<BackupMetadata>, PathBuf)> = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("lkb") {
            continue;
        }
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if !metadata.file_type().is_file() || validate_backup_file(&path).is_err() {
            rows.push((None, path));
            continue;
        }
        let parsed = match check {
            BackupListCheck::Metadata => read_metadata_only(&path)?,
            BackupListCheck::Full => read_verified(&path)?,
        };
        rows.push((parsed, path));
    }
    rows.sort_by(|a, b| match (&a.0, &b.0) {
        (Some(a), Some(b)) => b.created_at.cmp(&a.created_at),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    });
    Ok(rows)
}

fn read_verified(path: &Path) -> Result<Option<BackupMetadata>, plan::InstallError> {
    let bytes = std::fs::read(path)?;
    let parsed = match verify_lkb(&bytes) {
        Ok(parsed) => parsed,
        Err(_) => return Ok(None),
    };
    // 内容完整性校验与 verify 相同:归档必须包含全部必需条目。
    let verify_dir =
        std::env::temp_dir().join(format!("lkit-backup-list-{}", uuid::Uuid::now_v7()));
    let content = crate::backup::lkb::create_secure_dir(&verify_dir, 0o700)
        .and_then(|()| crate::backup::lkb::extract_lkb(&bytes, &verify_dir));
    let result = match content {
        Ok(_) => Ok(Some(parsed)),
        Err(plan::InstallError::InvalidBackup(_)) => Ok(None),
        Err(error) => Err(error),
    };
    let _ = std::fs::remove_dir_all(&verify_dir);
    result
}

fn read_metadata_only(path: &Path) -> Result<Option<BackupMetadata>, plan::InstallError> {
    let file_len = std::fs::metadata(path)
        .map_err(plan::InstallError::Io)?
        .len();
    let mut file = std::fs::File::open(path).map_err(plan::InstallError::Io)?;
    Ok(read_backup_metadata_streamed(&mut file, file_len).ok())
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
    fn list_marks_symlinks_and_unsafe_permissions_invalid() {
        let dir = temp_dir("list");
        let source = dir.join("source");
        std::fs::create_dir_all(source.join("static/assets")).unwrap();
        let webserver = source.join("landscape-webserver");
        std::fs::write(&webserver, b"binary").unwrap();
        std::fs::write(source.join("static/index.html"), b"<h1>x</h1>").unwrap();
        std::fs::write(source.join("static.zip"), b"zip").unwrap();
        std::fs::create_dir_all(source.join("geo_tmp")).unwrap();
        let backups = dir.join("backups");
        std::fs::create_dir_all(&backups).unwrap();
        let backup = crate::backup::lkb::create_backup(
            &backups,
            &semver::Version::new(1, 2, 3),
            "x86_64",
            &webserver,
            "version = \"1.2.3\"\n",
            &source.join("static"),
            &source.join("static.zip"),
            &source.join("geo_tmp"),
            "",
            true,
            None,
        )
        .unwrap();
        let valid = backups.join(format!("{}.lkb", backup.backup_id));
        std::os::unix::fs::symlink(&valid, backups.join("link.lkb")).unwrap();
        let loose = backups.join("loose.lkb");
        std::fs::copy(&valid, &loose).unwrap();
        std::fs::set_permissions(&loose, std::fs::Permissions::from_mode(0o644)).unwrap();

        #[cfg(feature = "test-support")]
        let args = BackupList {
            install_dir: Some(dir.clone()),
            test_runtime: None,
        };
        #[cfg(not(feature = "test-support"))]
        let args = BackupList {
            install_dir: Some(dir.clone()),
        };
        assert_eq!(run_list(&args), ExitCode::FAILURE);
        let _ = std::fs::remove_dir_all(&dir);
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

    #[test]
    fn list_marks_content_incomplete_backups_invalid() {
        // 构造 checksum 有效、但缺少 landscape_init.toml 的归档:
        // verify_lkb 通过,内容完整性校验必须拒绝。
        let tar_gz = gzip_tar(&raw_tar(&[
            ("landscape-webserver", b'0', b"bin"),
            ("static.zip", b'0', b"zip"),
            ("static", b'5', b""),
            ("geo_tmp", b'5', b""),
        ]));
        let dir = temp_dir("list-incomplete");
        let backups = dir.join("backups");
        std::fs::create_dir_all(&backups).unwrap();
        std::fs::write(backups.join("20260801-163000-a1b2c3d4.lkb"), wrap(&tar_gz)).unwrap();
        std::fs::set_permissions(
            backups.join("20260801-163000-a1b2c3d4.lkb"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();

        #[cfg(feature = "test-support")]
        let args = BackupList {
            install_dir: Some(dir.clone()),
            test_runtime: None,
        };
        #[cfg(not(feature = "test-support"))]
        let args = BackupList {
            install_dir: Some(dir.clone()),
        };
        assert_eq!(run_list(&args), ExitCode::FAILURE);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn metadata_list_mode_reads_only_the_metadata_region() {
        let dir = temp_dir("metadata-list");
        let source = dir.join("source");
        std::fs::create_dir_all(source.join("static/assets")).unwrap();
        let webserver = source.join("landscape-webserver");
        std::fs::write(&webserver, b"binary").unwrap();
        std::fs::write(source.join("static/index.html"), b"<h1>x</h1>").unwrap();
        std::fs::write(source.join("static.zip"), b"zip").unwrap();
        std::fs::create_dir_all(source.join("geo_tmp")).unwrap();
        std::fs::write(source.join("geo_tmp/geo.dat"), b"geo").unwrap();
        let backups = dir.join("backups");
        std::fs::create_dir_all(&backups).unwrap();
        let backup = crate::backup::lkb::create_backup(
            &backups,
            &semver::Version::new(1, 2, 3),
            "x86_64",
            &webserver,
            "version = \"1.2.3\"\n",
            &source.join("static"),
            &source.join("static.zip"),
            &source.join("geo_tmp"),
            "",
            true,
            None,
        )
        .unwrap();
        let path = backups.join(format!("{}.lkb", backup.backup_id));
        let root = crate::deployment::root::InstallRoot {
            install_root: dir.clone(),
            canonical: dir.clone(),
        };

        let rows = list_backups_with(&root, BackupListCheck::Full).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].0.is_some());

        // 归档体被篡改但 header/metadata 完好：Full 标记 invalid，
        // Metadata 快速模式仍能展示（V 键与 restore 做完整校验）。
        let bytes = std::fs::read(&path).unwrap();
        let mut bad = bytes.clone();
        let end = bad.len();
        bad[end - 1] ^= 0xFF;
        std::fs::write(&path, &bad).unwrap();
        let rows = list_backups_with(&root, BackupListCheck::Full).unwrap();
        assert!(rows[0].0.is_none());
        let rows = list_backups_with(&root, BackupListCheck::Metadata).unwrap();
        assert!(rows[0].0.is_some());

        // 结构性损坏（magic 被改）：两种模式都标记 invalid。
        let mut bad = bytes.clone();
        bad[0] = b'X';
        std::fs::write(&path, &bad).unwrap();
        for check in [BackupListCheck::Full, BackupListCheck::Metadata] {
            let rows = list_backups_with(&root, check).unwrap();
            assert!(rows[0].0.is_none(), "{check:?} must reject a bad header");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
