use std::io::Read;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;

use chrono::{DateTime, Utc};
use flate2::read::GzDecoder;
use lkit_repository::{parse_stable_version, zip_path_parts};
use sha2::{Digest, Sha256};

use super::super::plan::InstallError;
use super::{
    BackupMetadata, BackupScope, LKB_HEADER_LEN, LKB_MAGIC, LKB_METADATA_CAPACITY, LKB_MIN_LEN,
    hex, invalid_backup,
};

pub(crate) fn verify_lkb(bytes: &[u8]) -> Result<BackupMetadata, InstallError> {
    if bytes.len() as u64 <= LKB_MIN_LEN {
        return Err(invalid_backup(format!(
            "file length {} must be greater than 1 MiB",
            bytes.len()
        )));
    }
    let json_len = validate_lkb_header(bytes)?;
    let metadata = parse_metadata_json(&bytes[LKB_HEADER_LEN..LKB_HEADER_LEN + json_len])?;
    let actual = hex(&Sha256::digest(&bytes[LKB_METADATA_CAPACITY..]));
    let expected = metadata
        .checksum
        .strip_prefix("sha256:")
        .ok_or_else(|| invalid_backup("checksum must use the sha256: prefix".into()))?;
    if actual != expected {
        return Err(invalid_backup(format!(
            "tar.gz checksum mismatch: expected {expected}, got {actual}"
        )));
    }
    Ok(metadata)
}

/// 只校验容器 Header 与 metadata 区（前 1 MiB），不读取也不校验归档校验和。
/// 用于列表展示等需要快速读取的场景；完整校验必须使用 `verify_lkb`。
pub(crate) fn read_backup_metadata(bytes: &[u8]) -> Result<BackupMetadata, InstallError> {
    if bytes.len() < LKB_METADATA_CAPACITY {
        return Err(invalid_backup(format!(
            "file length {} must be at least 1 MiB",
            bytes.len()
        )));
    }
    let json_len = validate_lkb_header(bytes)?;
    parse_metadata_json(&bytes[LKB_HEADER_LEN..LKB_HEADER_LEN + json_len])
}

/// 流式快速读取：只读 32 字节 Header 和 `json_len` 字节的 metadata JSON，
/// 不读取 1 MiB 零填充区，也不校验归档校验和。用于列表展示；
/// 完整校验必须使用 `verify_lkb`。
pub(crate) fn read_backup_metadata_streamed(
    reader: &mut impl Read,
    file_len: u64,
) -> Result<BackupMetadata, InstallError> {
    if file_len <= LKB_MIN_LEN {
        return Err(invalid_backup(format!(
            "file length {file_len} must be greater than 1 MiB"
        )));
    }
    let mut header = [0u8; LKB_HEADER_LEN];
    reader
        .read_exact(&mut header)
        .map_err(|_| invalid_backup("file is shorter than the LKB1 header".into()))?;
    if &header[..4] != LKB_MAGIC {
        return Err(invalid_backup("missing LKB1 magic".into()));
    }
    let container_version = u16::from_le_bytes(header[4..6].try_into().unwrap());
    if container_version != 1 {
        return Err(invalid_backup(format!(
            "unsupported container version {container_version}"
        )));
    }
    let json_len = u32::from_le_bytes(header[6..10].try_into().unwrap()) as usize;
    if json_len == 0 {
        return Err(invalid_backup("empty metadata length".into()));
    }
    if json_len > LKB_METADATA_CAPACITY - LKB_HEADER_LEN {
        return Err(invalid_backup(
            "metadata length exceeds the 1 MiB capacity".into(),
        ));
    }
    if header[10..].iter().any(|byte| *byte != 0) {
        return Err(invalid_backup("non-zero reserved header bytes".into()));
    }
    let mut json = vec![0u8; json_len];
    reader
        .read_exact(&mut json)
        .map_err(|_| invalid_backup("file is shorter than the declared metadata length".into()))?;
    parse_metadata_json(&json)
}

fn validate_lkb_header(bytes: &[u8]) -> Result<usize, InstallError> {
    let header = &bytes[..LKB_HEADER_LEN];
    if &header[..4] != LKB_MAGIC {
        return Err(invalid_backup("missing LKB1 magic".into()));
    }
    let container_version = u16::from_le_bytes(header[4..6].try_into().unwrap());
    if container_version != 1 {
        return Err(invalid_backup(format!(
            "unsupported container version {container_version}"
        )));
    }
    let json_len = u32::from_le_bytes(header[6..10].try_into().unwrap()) as usize;
    if json_len == 0 {
        return Err(invalid_backup("empty metadata length".into()));
    }
    if json_len > LKB_METADATA_CAPACITY - LKB_HEADER_LEN {
        return Err(invalid_backup(
            "metadata length exceeds the 1 MiB capacity".into(),
        ));
    }
    if header[10..].iter().any(|byte| *byte != 0) {
        return Err(invalid_backup("non-zero reserved header bytes".into()));
    }
    if bytes[LKB_HEADER_LEN + json_len..LKB_METADATA_CAPACITY]
        .iter()
        .any(|byte| *byte != 0)
    {
        return Err(invalid_backup("non-zero metadata padding".into()));
    }
    Ok(json_len)
}

fn parse_metadata_json(json: &[u8]) -> Result<BackupMetadata, InstallError> {
    let metadata: BackupMetadata = serde_json::from_slice(json)
        .map_err(|error| invalid_backup(format!("invalid metadata JSON: {error}")))?;
    validate_metadata(&metadata)?;
    Ok(metadata)
}

pub(crate) fn extract_lkb(bytes: &[u8], target_dir: &Path) -> Result<BackupMetadata, InstallError> {
    let metadata = verify_lkb(bytes)?;
    create_secure_dir(target_dir, 0o700)?;
    let decoder = GzDecoder::new(&bytes[LKB_METADATA_CAPACITY..]);
    let mut archive = tar::Archive::new(decoder);
    let mut extracted_files = std::collections::HashSet::new();
    let mut extracted_dirs = std::collections::HashSet::new();
    for entry in archive
        .entries()
        .map_err(|error| invalid_backup(format!("tar decode failed: {error}")))?
    {
        let mut entry =
            entry.map_err(|error| invalid_backup(format!("tar entry failed: {error}")))?;
        let name = entry
            .path()
            .map_err(|error| invalid_backup(format!("tar entry path failed: {error}")))?
            .into_owned()
            .to_string_lossy()
            .into_owned();
        if name.starts_with('/') || name.contains('\\') {
            return Err(invalid_backup(format!(
                "tar entry {name} is not a safe relative path"
            )));
        }
        let parts = zip_path_parts(&name)
            .map_err(|_| invalid_backup(format!("tar entry {name} has an unsafe path")))?;
        if parts.is_empty() {
            continue;
        }
        let entry_type = entry.header().entry_type();
        if !entry_type.is_dir() && !entry_type.is_file() {
            return Err(invalid_backup(format!(
                "tar entry {name} is not a regular file or directory"
            )));
        }
        let normalized = parts.join("/");
        if normalized == "static.zip" && !entry_type.is_file() {
            return Err(invalid_backup("static.zip must be a regular file".into()));
        }
        let target = target_dir.join(&normalized);
        if entry_type.is_dir() {
            create_secure_dir(&target, 0o700)?;
            extracted_dirs.insert(normalized);
        } else if entry_type.is_file() {
            if let Some(parent) = target.parent() {
                create_secure_dir(parent, 0o700)?;
            }
            let mut output = create_file_mode(&target, 0o600)?;
            std::io::copy(&mut entry, &mut output).map_err(InstallError::Io)?;
            output.sync_all().map_err(InstallError::Io)?;
            extracted_files.insert(normalized);
        } else {
            return Err(invalid_backup(format!(
                "tar entry {name} has an unsupported type"
            )));
        }
    }
    for required in ["landscape-webserver", "landscape_init.toml", "static.zip"] {
        if !extracted_files.contains(required) {
            return Err(invalid_backup(format!(
                "archive is missing the {required} entry"
            )));
        }
    }
    for required in ["static", "geo_tmp"] {
        if !extracted_dirs.contains(required) {
            return Err(invalid_backup(format!(
                "archive is missing the {required} directory"
            )));
        }
    }
    Ok(metadata)
}

/// 递归创建目录并强制权限 `mode`,只影响新建的目录。
pub(crate) fn create_secure_dir(path: &Path, mode: u32) -> Result<(), InstallError> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(mode)
        .create(path)
        .map_err(InstallError::Io)
}

/// 独占创建文件并强制权限 `mode`。已存在时失败,避免覆盖或跟随预置的符号链接。
pub(crate) fn create_file_mode(path: &Path, mode: u32) -> Result<std::fs::File, InstallError> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)
        .map_err(InstallError::Io)
}

fn validate_metadata(metadata: &BackupMetadata) -> Result<(), InstallError> {
    if metadata.schema_version != 1 {
        return Err(invalid_backup(format!(
            "unsupported metadata schema version {}",
            metadata.schema_version
        )));
    }
    if metadata.scope != BackupScope::Minimal {
        return Err(invalid_backup(format!(
            "unsupported backup scope {:?}",
            metadata.scope
        )));
    }
    if !metadata.contents.binary
        || !metadata.contents.static_
        || !metadata.contents.static_archive
        || !metadata.contents.init_config
        || !metadata.contents.geo_cache
    {
        return Err(invalid_backup("backup contents must all be true".into()));
    }
    if !is_sha256(
        metadata
            .checksum
            .strip_prefix("sha256:")
            .unwrap_or_default(),
    ) {
        return Err(invalid_backup(
            "checksum must be sha256: followed by 64 lowercase hex characters".into(),
        ));
    }
    if let Err(error) = parse_stable_version(&metadata.landscape_version) {
        return Err(invalid_backup(format!(
            "invalid landscape version {:?}: {error}",
            metadata.landscape_version
        )));
    }
    if !valid_backup_id(&metadata.backup_id, &metadata.created_at) {
        return Err(invalid_backup(format!(
            "invalid backup_id {}",
            metadata.backup_id
        )));
    }
    Ok(())
}

fn valid_backup_id(value: &str, created_at: &DateTime<Utc>) -> bool {
    if !backup_id_format_ok(value) {
        return false;
    }
    let expected = created_at.format("%Y%m%d-%H%M%S").to_string();
    let mut parts = value.split('-');
    let (Some(date), Some(time), _) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    date == &expected[..8] && time == &expected[9..]
}

/// 只校验备份 ID 的字符结构 `YYYYMMDD-HHMMSS-<8位小写hex>`,不校验时间与
/// `created_at` 的一致性。用于校验用户提供的 `--backup <ID>` 参数。
pub(crate) fn backup_id_format_ok(value: &str) -> bool {
    let mut parts = value.split('-');
    let (Some(date), Some(time), Some(suffix)) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    if parts.next().is_some() {
        return false;
    }
    if date.len() != 8 || !date.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    if time.len() != 6 || !time.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    suffix.len() == 8
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn hash_bytes(bytes: &[u8]) -> (String, u64) {
    (hex(&Sha256::digest(bytes)), bytes.len() as u64)
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::super::{BackupArchitecture, BackupContents, create_backup};
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("lkit-backup-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn backup_source(
        root: &std::path::Path,
    ) -> (
        std::path::PathBuf,
        std::path::PathBuf,
        std::path::PathBuf,
        std::path::PathBuf,
    ) {
        let webserver = root.join("landscape-webserver");
        std::fs::write(&webserver, b"binary payload").unwrap();
        let static_dir = root.join("static");
        std::fs::create_dir_all(static_dir.join("assets")).unwrap();
        std::fs::write(static_dir.join("index.html"), b"<h1>hello</h1>").unwrap();
        std::fs::write(static_dir.join("assets/app.js"), b"console.log(1);").unwrap();
        let static_zip = root.join("static.zip");
        std::fs::write(&static_zip, b"zip payload").unwrap();
        let geo_tmp = root.join("geo_tmp");
        std::fs::create_dir_all(geo_tmp.join("ip")).unwrap();
        std::fs::write(geo_tmp.join("ip/geo.dat"), b"geo").unwrap();
        (webserver, static_dir, static_zip, geo_tmp)
    }

    #[test]
    fn creates_verifies_and_extracts_backup() {
        let temp = temp_dir("roundtrip");
        let source = temp.join("source");
        std::fs::create_dir_all(&source).unwrap();
        let (webserver, static_dir, static_zip, geo_tmp) = backup_source(&source);
        let backups = temp.join("backups");
        std::fs::create_dir_all(&backups).unwrap();
        let backup = create_backup(
            &backups,
            &semver::Version::new(1, 2, 3),
            "x86_64",
            &webserver,
            "version = \"1.2.3\"",
            &static_dir,
            &static_zip,
            &geo_tmp,
            "manual backup",
            false,
            None,
        )
        .unwrap();
        assert!(backups.join(format!("{}.lkb", backup.backup_id)).is_file());
        assert_eq!(backup.path, format!("backups/{}.lkb", backup.backup_id));

        let bytes = std::fs::read(backups.join(format!("{}.lkb", backup.backup_id))).unwrap();
        let metadata = verify_lkb(&bytes).unwrap();
        assert_eq!(metadata.backup_id, backup.backup_id);
        assert_eq!(metadata.landscape_version, "1.2.3");
        assert_eq!(metadata.scope, BackupScope::Minimal);
        assert!(!metadata.auto);
        assert_eq!(metadata.remark, "manual backup");
        assert!(metadata.contents.static_);
        assert!(metadata.contents.static_archive);

        let target = temp.join("extracted");
        extract_lkb(&bytes, &target).unwrap();
        assert_eq!(
            std::fs::read(target.join("landscape-webserver")).unwrap(),
            b"binary payload"
        );
        assert_eq!(
            std::fs::read(target.join("static.zip")).unwrap(),
            b"zip payload"
        );
        assert_eq!(
            std::fs::read_to_string(target.join("static/index.html")).unwrap(),
            "<h1>hello</h1>"
        );
        assert_eq!(
            std::fs::read_to_string(target.join("static/assets/app.js")).unwrap(),
            "console.log(1);"
        );
        assert_eq!(
            std::fs::read_to_string(target.join("geo_tmp/ip/geo.dat")).unwrap(),
            "geo"
        );
        assert!(target.join("landscape_init.toml").is_file());
        use std::os::unix::fs::MetadataExt;
        assert_eq!(
            std::fs::metadata(&target).unwrap().mode() & 0o777,
            0o700,
            "extract root must be 0700"
        );
        assert_eq!(
            std::fs::metadata(target.join("landscape_init.toml"))
                .unwrap()
                .mode()
                & 0o777,
            0o600,
            "extracted files must be 0600"
        );
        assert_eq!(
            std::fs::metadata(target.join("landscape-webserver"))
                .unwrap()
                .mode()
                & 0o777,
            0o600,
            "extracted binary must be 0600"
        );
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn streamed_metadata_reader_skips_the_archive_but_keeps_header_checks() {
        let temp = temp_dir("streamed");
        let source = temp.join("source");
        std::fs::create_dir_all(&source).unwrap();
        let (webserver, static_dir, static_zip, geo_tmp) = backup_source(&source);
        let backups = temp.join("backups");
        std::fs::create_dir_all(&backups).unwrap();
        let backup = create_backup(
            &backups,
            &semver::Version::new(1, 2, 3),
            "x86_64",
            &webserver,
            "version = \"1.2.3\"",
            &static_dir,
            &static_zip,
            &geo_tmp,
            "",
            true,
            None,
        )
        .unwrap();
        let path = backups.join(format!("{}.lkb", backup.backup_id));

        let mut file = std::fs::File::open(&path).unwrap();
        let parsed =
            read_backup_metadata_streamed(&mut file, std::fs::metadata(&path).unwrap().len())
                .unwrap();
        assert_eq!(parsed.backup_id, backup.backup_id);
        assert_eq!(parsed.landscape_version, "1.2.3");

        // 归档体被篡改（checksum 失效）时流式读取仍返回 metadata；
        // 完整校验 verify_lkb 必须拒绝。
        let bytes = std::fs::read(&path).unwrap();
        let mut bad = bytes.clone();
        let end = bad.len();
        bad[end - 1] ^= 0xFF;
        std::fs::write(&path, &bad).unwrap();
        let mut file = std::fs::File::open(&path).unwrap();
        let parsed =
            read_backup_metadata_streamed(&mut file, std::fs::metadata(&path).unwrap().len())
                .unwrap();
        assert_eq!(parsed.backup_id, backup.backup_id);
        assert!(verify_lkb(&std::fs::read(&path).unwrap()).is_err());

        // Header 损坏时流式读取必须拒绝。
        let mut bad = bytes.clone();
        bad[0] = b'X';
        std::fs::write(&path, &bad).unwrap();
        let mut file = std::fs::File::open(&path).unwrap();
        assert!(read_backup_metadata_streamed(&mut file, bad.len() as u64).is_err());
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn includes_empty_geo_tmp_when_missing() {
        let temp = temp_dir("nogeo");
        let source = temp.join("source");
        std::fs::create_dir_all(&source).unwrap();
        let (webserver, static_dir, static_zip, _) = backup_source(&source);
        let missing_geo = temp.join("missing-geo");
        let backups = temp.join("backups");
        std::fs::create_dir_all(&backups).unwrap();
        let backup = create_backup(
            &backups,
            &semver::Version::new(1, 2, 3),
            "aarch64",
            &webserver,
            "version = \"1.2.3\"",
            &static_dir,
            &static_zip,
            &missing_geo,
            "",
            true,
            None,
        )
        .unwrap();
        let bytes = std::fs::read(backups.join(format!("{}.lkb", backup.backup_id))).unwrap();
        let target = temp.join("extracted");
        extract_lkb(&bytes, &target).unwrap();
        assert!(target.join("geo_tmp").is_dir());
        assert_eq!(
            std::fs::read_dir(target.join("geo_tmp")).unwrap().count(),
            0
        );
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn rejects_tampered_files() {
        let temp = temp_dir("tamper");
        let source = temp.join("source");
        std::fs::create_dir_all(&source).unwrap();
        let (webserver, static_dir, static_zip, geo_tmp) = backup_source(&source);
        let backups = temp.join("backups");
        std::fs::create_dir_all(&backups).unwrap();
        let backup = create_backup(
            &backups,
            &semver::Version::new(1, 2, 3),
            "x86_64",
            &webserver,
            "version = \"1.2.3\"",
            &static_dir,
            &static_zip,
            &geo_tmp,
            "",
            true,
            None,
        )
        .unwrap();
        let bytes = std::fs::read(backups.join(format!("{}.lkb", backup.backup_id))).unwrap();

        let mut bad = bytes.clone();
        bad[0] = b'X';
        assert!(verify_lkb(&bad).is_err());

        let mut bad = bytes.clone();
        bad[4] = 2;
        assert!(verify_lkb(&bad).is_err());

        let mut bad = bytes.clone();
        bad[10] = 1;
        assert!(verify_lkb(&bad).is_err());

        let mut bad = bytes.clone();
        bad[LKB_HEADER_LEN + 40] = 1;
        assert!(verify_lkb(&bad).is_err());

        let mut bad = bytes.clone();
        let end = bad.len();
        bad[end - 1] ^= 0xFF;
        assert!(verify_lkb(&bad).is_err());

        assert!(verify_lkb(&bytes[..1024 * 1024]).is_err());
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn rejects_escaping_tar_entries() {
        fn raw_tar(name: &str, content: &[u8]) -> Vec<u8> {
            let mut header = [0u8; 512];
            header[..name.len()].copy_from_slice(name.as_bytes());
            let size = format!("{:011o}", content.len());
            header[124..124 + 11].copy_from_slice(size.as_bytes());
            header[156] = b'0';
            for byte in &mut header[148..156] {
                *byte = b' ';
            }
            let sum: u32 = header.iter().map(|byte| *byte as u32).sum();
            let octal = format!("{sum:06o}");
            header[148..154].copy_from_slice(octal.as_bytes());
            header[154] = 0;
            header[155] = b' ';
            let mut tar = header.to_vec();
            tar.extend_from_slice(content);
            let pad = (512 - content.len() % 512) % 512;
            tar.extend(std::iter::repeat_n(0, pad));
            tar.extend([0u8; 1024]);
            tar
        }
        let tar_gz = {
            let mut tar_gz = Vec::new();
            let encoder = GzEncoder::new(&mut tar_gz, Compression::default());
            let mut gz = encoder;
            std::io::copy(&mut raw_tar("../evil", b"boom").as_slice(), &mut gz).unwrap();
            gz.finish().unwrap();
            tar_gz
        };
        let (sha256, _) = hash_bytes(&tar_gz);
        let metadata = BackupMetadata {
            schema_version: 1,
            backup_id: format!("20260801-163000-{}", &sha256[..8]),
            created_at: DateTime::parse_from_rfc3339("2026-08-01T16:30:00Z")
                .unwrap()
                .with_timezone(&Utc),
            landscape_version: "1.2.3".into(),
            lkit_version: "0.1.0".into(),
            architecture: BackupArchitecture::X86_64,
            hostname: "test".into(),
            remark: String::new(),
            auto: true,
            scope: BackupScope::Minimal,
            contents: BackupContents {
                binary: true,
                static_: true,
                static_archive: true,
                init_config: true,
                geo_cache: true,
            },
            checksum: format!("sha256:{sha256}"),
        };
        let mut bytes = Vec::new();
        let mut header = [0u8; LKB_HEADER_LEN];
        header[0..4].copy_from_slice(LKB_MAGIC);
        header[4..6].copy_from_slice(&1u16.to_le_bytes());
        header[6..10]
            .copy_from_slice(&(serde_json::to_vec(&metadata).unwrap().len() as u32).to_le_bytes());
        bytes.extend_from_slice(&header);
        bytes.extend_from_slice(&serde_json::to_vec(&metadata).unwrap());
        bytes.resize(LKB_METADATA_CAPACITY, 0);
        bytes.extend_from_slice(&tar_gz);
        let target = temp_dir("extract-escape").join("out");
        assert!(extract_lkb(&bytes, &target).is_err());
        assert!(!target.join("evil").exists());
        let _ = std::fs::remove_dir_all(target);
    }

    #[test]
    fn validates_backup_ids() {
        let time = DateTime::parse_from_rfc3339("2026-08-01T16:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert!(valid_backup_id("20260801-163000-a1b2c3d4", &time));
        assert!(!valid_backup_id("20260801-163000-a1b2c3d", &time));
        assert!(!valid_backup_id("20260801-163000-a1b2c3d40", &time));
        assert!(!valid_backup_id("20260801-16300-a1b2c3d4", &time));
        assert!(!valid_backup_id("20260801-163000-A1B2C3D4", &time));
        assert!(!valid_backup_id("20260701-163000-a1b2c3d4", &time));
        assert!(!valid_backup_id("20260801-163000-a1b2c3d4-extra", &time));
        assert!(!valid_backup_id("..%2f..%2fescape", &time));
    }

    #[test]
    fn validates_backup_id_format_only() {
        for (value, expected) in [
            ("20260801-163000-a1b2c3d4", true),
            ("20260801-163000-a1b2c3d", false),
            ("20260801-163000-a1b2c3d40", false),
            ("20260801-16300-a1b2c3d4", false),
            ("20260801-163000-A1B2C3D4", false),
            ("20260801-163000-a1b2c3d4-extra", false),
            ("../escape", false),
            ("20260801-163000-a1b2c3d4/", false),
            ("", false),
        ] {
            assert_eq!(backup_id_format_ok(value), expected, "id {value:?}");
        }
    }

    #[test]
    fn rejects_directory_entry_named_static_zip() {
        fn raw_dir_tar(name: &str) -> Vec<u8> {
            let mut header = [0u8; 512];
            header[..name.len()].copy_from_slice(name.as_bytes());
            let size = format!("{:011o}", 0usize);
            header[124..124 + 11].copy_from_slice(size.as_bytes());
            header[156] = b'5';
            for byte in &mut header[148..156] {
                *byte = b' ';
            }
            let sum: u32 = header.iter().map(|byte| *byte as u32).sum();
            let octal = format!("{sum:06o}");
            header[148..154].copy_from_slice(octal.as_bytes());
            header[154] = 0;
            header[155] = b' ';
            let mut tar = header.to_vec();
            tar.extend([0u8; 1024]);
            tar
        }
        let tar_gz = {
            let mut tar_gz = Vec::new();
            let encoder = GzEncoder::new(&mut tar_gz, Compression::default());
            let mut gz = encoder;
            std::io::copy(&mut raw_dir_tar("static.zip").as_slice(), &mut gz).unwrap();
            gz.finish().unwrap();
            tar_gz
        };
        let (sha256, _) = hash_bytes(&tar_gz);
        let metadata = BackupMetadata {
            schema_version: 1,
            backup_id: format!("20260801-163000-{}", &sha256[..8]),
            created_at: DateTime::parse_from_rfc3339("2026-08-01T16:30:00Z")
                .unwrap()
                .with_timezone(&Utc),
            landscape_version: "1.2.3".into(),
            lkit_version: "0.1.0".into(),
            architecture: BackupArchitecture::X86_64,
            hostname: "test".into(),
            remark: String::new(),
            auto: true,
            scope: BackupScope::Minimal,
            contents: BackupContents {
                binary: true,
                static_: true,
                static_archive: true,
                init_config: true,
                geo_cache: true,
            },
            checksum: format!("sha256:{sha256}"),
        };
        let mut bytes = Vec::new();
        let mut header = [0u8; LKB_HEADER_LEN];
        header[0..4].copy_from_slice(LKB_MAGIC);
        header[4..6].copy_from_slice(&1u16.to_le_bytes());
        header[6..10]
            .copy_from_slice(&(serde_json::to_vec(&metadata).unwrap().len() as u32).to_le_bytes());
        bytes.extend_from_slice(&header);
        bytes.extend_from_slice(&serde_json::to_vec(&metadata).unwrap());
        bytes.resize(LKB_METADATA_CAPACITY, 0);
        bytes.extend_from_slice(&tar_gz);
        let target = temp_dir("extract-dirzip").join("out");
        assert!(extract_lkb(&bytes, &target).is_err());
        assert!(!target.join("static.zip").exists());
        let _ = std::fs::remove_dir_all(target);
    }

    #[test]
    fn rejects_backups_missing_required_entries() {
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
        fn wrap(tar_gz: &[u8]) -> Vec<u8> {
            let (sha256, _) = hash_bytes(tar_gz);
            let metadata = BackupMetadata {
                schema_version: 1,
                backup_id: format!("20260801-163000-{}", &sha256[..8]),
                created_at: DateTime::parse_from_rfc3339("2026-08-01T16:30:00Z")
                    .unwrap()
                    .with_timezone(&Utc),
                landscape_version: "1.2.3".into(),
                lkit_version: "0.1.0".into(),
                architecture: BackupArchitecture::X86_64,
                hostname: "test".into(),
                remark: String::new(),
                auto: true,
                scope: BackupScope::Minimal,
                contents: BackupContents {
                    binary: true,
                    static_: true,
                    static_archive: true,
                    init_config: true,
                    geo_cache: true,
                },
                checksum: format!("sha256:{sha256}"),
            };
            let mut bytes = Vec::new();
            let mut header = [0u8; LKB_HEADER_LEN];
            header[0..4].copy_from_slice(LKB_MAGIC);
            header[4..6].copy_from_slice(&1u16.to_le_bytes());
            header[6..10].copy_from_slice(
                &(serde_json::to_vec(&metadata).unwrap().len() as u32).to_le_bytes(),
            );
            bytes.extend_from_slice(&header);
            bytes.extend_from_slice(&serde_json::to_vec(&metadata).unwrap());
            bytes.resize(LKB_METADATA_CAPACITY, 0);
            bytes.extend_from_slice(tar_gz);
            bytes
        }
        fn gzip(mut tar: &[u8]) -> Vec<u8> {
            let mut tar_gz = Vec::new();
            let encoder = GzEncoder::new(&mut tar_gz, Compression::default());
            let mut gz = encoder;
            std::io::copy(&mut tar, &mut gz).unwrap();
            gz.finish().unwrap();
            tar_gz
        }
        let complete = raw_tar(&[
            ("landscape-webserver", b'0', b"bin"),
            ("landscape_init.toml", b'0', b"init"),
            ("static.zip", b'0', b"zip"),
            ("static", b'5', b""),
            ("geo_tmp", b'5', b""),
        ]);
        let target = temp_dir("extract-complete").join("out");
        assert!(extract_lkb(&wrap(&gzip(&complete)), &target).is_ok());
        let _ = std::fs::remove_dir_all(target);

        let missing_init = raw_tar(&[
            ("landscape-webserver", b'0', b"bin"),
            ("static.zip", b'0', b"zip"),
            ("static", b'5', b""),
            ("geo_tmp", b'5', b""),
        ]);
        let target = temp_dir("extract-noinit").join("out");
        assert!(extract_lkb(&wrap(&gzip(&missing_init)), &target).is_err());
        let _ = std::fs::remove_dir_all(target);

        let missing_static = raw_tar(&[
            ("landscape-webserver", b'0', b"bin"),
            ("landscape_init.toml", b'0', b"init"),
            ("static.zip", b'0', b"zip"),
            ("geo_tmp", b'5', b""),
        ]);
        let target = temp_dir("extract-nostatic").join("out");
        assert!(extract_lkb(&wrap(&gzip(&missing_static)), &target).is_err());
        let _ = std::fs::remove_dir_all(target);
    }
}
