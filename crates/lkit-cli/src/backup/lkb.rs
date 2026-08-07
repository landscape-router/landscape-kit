use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;

use chrono::{DateTime, Utc};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use lkit_repository::{parse_stable_version, zip_path_parts};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::plan::InstallError;
use super::transaction::BackupRef;

pub(crate) const LKB_MAGIC: &[u8; 4] = b"LKB1";
pub(crate) const LKB_HEADER_LEN: usize = 32;
pub(crate) const LKB_METADATA_CAPACITY: usize = 1024 * 1024;
pub(crate) const LKB_MIN_LEN: u64 = 1024 * 1024 + 1;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct BackupMetadata {
    pub schema_version: u64,
    pub backup_id: String,
    pub created_at: DateTime<Utc>,
    pub landscape_version: String,
    pub lkit_version: String,
    pub architecture: BackupArchitecture,
    pub hostname: String,
    pub remark: String,
    pub auto: bool,
    pub scope: BackupScope,
    pub contents: BackupContents,
    pub checksum: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum BackupArchitecture {
    X86_64,
    Aarch64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum BackupScope {
    Minimal,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct BackupContents {
    pub binary: bool,
    #[serde(rename = "static")]
    pub static_: bool,
    pub static_archive: bool,
    pub init_config: bool,
    pub geo_cache: bool,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn create_backup(
    backups_dir: &Path,
    landscape_version: &semver::Version,
    architecture: &str,
    webserver: &Path,
    init_config: &str,
    static_dir: &Path,
    static_archive_zip: &Path,
    geo_tmp: &Path,
    remark: &str,
    auto: bool,
) -> Result<BackupRef, InstallError> {
    validate_remark(remark)?;
    let tar_gz = build_tar_gz(
        webserver,
        init_config,
        static_dir,
        static_archive_zip,
        geo_tmp,
    )?;
    let (tar_sha256, tar_size) = hash_bytes(&tar_gz);
    let now = Utc::now();
    let backup_id = format!("{}-{}", now.format("%Y%m%d-%H%M%S"), &tar_sha256[..8]);
    let metadata = BackupMetadata {
        schema_version: 1,
        backup_id: backup_id.clone(),
        created_at: now,
        landscape_version: landscape_version.to_string(),
        lkit_version: env!("CARGO_PKG_VERSION").into(),
        architecture: parse_architecture(architecture)?,
        hostname: hostname(),
        remark: remark.to_string(),
        auto,
        scope: BackupScope::Minimal,
        contents: BackupContents {
            binary: true,
            static_: true,
            static_archive: true,
            init_config: true,
            geo_cache: true,
        },
        checksum: format!("sha256:{tar_sha256}"),
    };
    let metadata_json = serde_json::to_vec(&metadata).map_err(InstallError::StateWrite)?;
    if metadata_json.len() > LKB_METADATA_CAPACITY - LKB_HEADER_LEN {
        return Err(invalid_backup(
            "backup metadata exceeds the 1 MiB capacity".into(),
        ));
    }
    let mut bytes = Vec::with_capacity(LKB_METADATA_CAPACITY + tar_size as usize);
    let mut header = [0u8; LKB_HEADER_LEN];
    header[0..4].copy_from_slice(LKB_MAGIC);
    header[4..6].copy_from_slice(&1u16.to_le_bytes());
    header[6..10].copy_from_slice(&(metadata_json.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&header);
    bytes.extend_from_slice(&metadata_json);
    bytes.resize(LKB_METADATA_CAPACITY, 0);
    bytes.extend_from_slice(&tar_gz);
    let (file_sha256, _) = hash_bytes(&bytes);

    let tmp_dir = backups_dir.join(".tmp");
    std::fs::create_dir_all(&tmp_dir).map_err(InstallError::Io)?;
    let tmp = tmp_dir.join(format!("{backup_id}.tmp"));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(InstallError::Io)?;
    file.write_all(&bytes).map_err(InstallError::Io)?;
    file.sync_all().map_err(InstallError::Io)?;
    let written = std::fs::read(&tmp).map_err(InstallError::Io)?;
    verify_lkb(&written)?;
    let final_path = backups_dir.join(format!("{backup_id}.lkb"));
    if final_path.exists() {
        let _ = std::fs::remove_file(&tmp);
        return Err(invalid_backup(format!("backup {backup_id} already exists")));
    }
    publish_no_replace(&tmp, &final_path).map_err(|error| {
        if matches!(
            &error,
            InstallError::Io(io) if io.kind() == std::io::ErrorKind::AlreadyExists
        ) {
            invalid_backup(format!("backup {backup_id} already exists"))
        } else {
            error
        }
    })?;
    Ok(BackupRef {
        path: format!("backups/{backup_id}.lkb"),
        backup_id,
        sha256: file_sha256,
    })
}

/// 无覆盖原子发布:hard-link 到目标后删除临时文件。目标已存在时返回
/// `AlreadyExists`,不存在 check-then-act 竞态(rename 会静默覆盖目标)。
/// hard-link 失败时清理临时文件;link 成功后临时文件删除失败则回滚目标。
pub(crate) fn publish_no_replace(tmp: &Path, target: &Path) -> Result<(), InstallError> {
    if let Err(error) = std::fs::hard_link(tmp, target) {
        let _ = std::fs::remove_file(tmp);
        return Err(InstallError::Io(error));
    }
    if let Err(error) = std::fs::remove_file(tmp) {
        let _ = std::fs::remove_file(target);
        return Err(InstallError::Io(error));
    }
    Ok(())
}

pub(crate) fn verify_lkb(bytes: &[u8]) -> Result<BackupMetadata, InstallError> {
    if bytes.len() as u64 <= LKB_MIN_LEN {
        return Err(invalid_backup(format!(
            "file length {} must be greater than 1 MiB",
            bytes.len()
        )));
    }
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
    let metadata: BackupMetadata =
        serde_json::from_slice(&bytes[LKB_HEADER_LEN..LKB_HEADER_LEN + json_len])
            .map_err(|error| invalid_backup(format!("invalid metadata JSON: {error}")))?;
    validate_metadata(&metadata)?;
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

fn parse_architecture(value: &str) -> Result<BackupArchitecture, InstallError> {
    match value {
        "x86_64" => Ok(BackupArchitecture::X86_64),
        "aarch64" => Ok(BackupArchitecture::Aarch64),
        _ => Err(invalid_backup(format!("unsupported architecture {value}"))),
    }
}

fn build_tar_gz(
    webserver: &Path,
    init_config: &str,
    static_dir: &Path,
    static_archive_zip: &Path,
    geo_tmp: &Path,
) -> Result<Vec<u8>, InstallError> {
    let mut tar_gz = Vec::new();
    {
        let encoder = GzEncoder::new(&mut tar_gz, Compression::default());
        let mut builder = tar::Builder::new(encoder);
        builder
            .append_path_with_name(webserver, "landscape-webserver")
            .map_err(|error| invalid_backup(format!("failed to append webserver: {error}")))?;
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(init_config.len() as u64);
        header.set_mode(0o600);
        header.set_cksum();
        builder
            .append_data(&mut header, "landscape_init.toml", init_config.as_bytes())
            .map_err(|error| invalid_backup(format!("failed to append init config: {error}")))?;
        if !static_archive_zip.is_file() {
            return Err(invalid_backup(format!(
                "static archive {} is not a readable regular file",
                static_archive_zip.display()
            )));
        }
        builder
            .append_path_with_name(static_archive_zip, "static.zip")
            .map_err(|error| invalid_backup(format!("failed to append static archive: {error}")))?;
        append_tree(&mut builder, static_dir, "static")?;
        if geo_tmp.is_dir() {
            append_tree(&mut builder, geo_tmp, "geo_tmp")?;
        } else {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Directory);
            header.set_size(0);
            header.set_mode(0o755);
            header.set_cksum();
            builder
                .append_data(&mut header, "geo_tmp/", std::io::empty())
                .map_err(|error| invalid_backup(format!("failed to append geo_tmp: {error}")))?;
        }
        builder
            .finish()
            .map_err(|error| invalid_backup(format!("failed to finish tar: {error}")))?;
        let encoder = builder
            .into_inner()
            .map_err(|error| invalid_backup(format!("failed to finalize tar: {error}")))?;
        encoder
            .finish()
            .map_err(|error| invalid_backup(format!("failed to finalize gzip: {error}")))?;
    }
    Ok(tar_gz)
}

/// remark 是最多 256 个字符的单行说明,不得包含控制字符。
pub(crate) fn validate_remark(remark: &str) -> Result<(), InstallError> {
    if remark.chars().count() > 256 {
        return Err(InstallError::ParameterUsage(
            "remark must be at most 256 characters".into(),
        ));
    }
    if remark.chars().any(char::is_control) {
        return Err(InstallError::ParameterUsage(
            "remark must not contain control characters".into(),
        ));
    }
    Ok(())
}

fn append_tree(
    builder: &mut tar::Builder<GzEncoder<&mut Vec<u8>>>,
    dir: &Path,
    prefix: &str,
) -> Result<(), InstallError> {
    builder
        .append_dir(prefix, dir)
        .map_err(|error| invalid_backup(format!("failed to append {prefix}: {error}")))?;
    let mut stack = vec![(dir.to_path_buf(), prefix.to_string())];
    while let Some((path, rel)) = stack.pop() {
        for entry in std::fs::read_dir(&path).map_err(InstallError::Io)? {
            let entry = entry.map_err(InstallError::Io)?;
            let name = entry.file_name().into_string().map_err(|_| {
                invalid_backup("archive source contains a non-UTF-8 file name".into())
            })?;
            let rel_path = format!("{rel}/{name}");
            let file_type = entry.file_type().map_err(InstallError::Io)?;
            if file_type.is_symlink() {
                return Err(invalid_backup(format!("{rel_path} is a symbolic link")));
            }
            if file_type.is_dir() {
                builder
                    .append_dir(&rel_path, entry.path())
                    .map_err(|error| {
                        invalid_backup(format!("failed to append {rel_path}: {error}"))
                    })?;
                stack.push((entry.path(), rel_path));
            } else if file_type.is_file() {
                builder
                    .append_path_with_name(entry.path(), &rel_path)
                    .map_err(|error| {
                        invalid_backup(format!("failed to append {rel_path}: {error}"))
                    })?;
            } else {
                return Err(invalid_backup(format!(
                    "{rel_path} is not a regular file or directory"
                )));
            }
        }
    }
    Ok(())
}

fn hash_bytes(bytes: &[u8]) -> (String, u64) {
    (hex(&Sha256::digest(bytes)), bytes.len() as u64)
}

fn hostname() -> String {
    let mut buffer = [0u8; 256];
    let result =
        unsafe { libc::gethostname(buffer.as_mut_ptr() as *mut libc::c_char, buffer.len()) };
    if result != 0 {
        return String::new();
    }
    let end = buffer
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(buffer.len());
    String::from_utf8_lossy(&buffer[..end]).into_owned()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn invalid_backup(reason: String) -> InstallError {
    InstallError::InvalidBackup(reason)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn rejects_invalid_remarks() {
        assert!(matches!(
            validate_remark(&"x".repeat(257)),
            Err(InstallError::ParameterUsage(_))
        ));
        assert!(validate_remark("x\nline").is_err());
        assert!(validate_remark("ok remark").is_ok());
        assert!(validate_remark("").is_ok());
    }

    #[test]
    fn publish_no_replace_never_overwrites_an_existing_target() {
        let temp = temp_dir("noreplace");
        let tmp = temp.join("backup.lkb.tmp");
        let target = temp.join("backup.lkb");
        std::fs::write(&tmp, b"payload").unwrap();

        std::fs::write(&target, b"keep").unwrap();
        assert!(matches!(
            publish_no_replace(&tmp, &target),
            Err(InstallError::Io(io)) if io.kind() == std::io::ErrorKind::AlreadyExists
        ));
        assert_eq!(std::fs::read(&target).unwrap(), b"keep");
        assert!(
            !tmp.exists(),
            "failed publish must clean up the temporary file"
        );

        std::fs::remove_file(&target).unwrap();
        std::fs::write(&tmp, b"payload").unwrap();
        publish_no_replace(&tmp, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"payload");
        assert!(
            !tmp.exists(),
            "successful publish must remove the temporary file"
        );
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn rejects_missing_static_archive() {
        let temp = temp_dir("nozip");
        let source = temp.join("source");
        std::fs::create_dir_all(&source).unwrap();
        let (webserver, static_dir, _, geo_tmp) = backup_source(&source);
        let backups = temp.join("backups");
        std::fs::create_dir_all(&backups).unwrap();
        let missing_zip = temp.join("missing.zip");
        assert!(
            create_backup(
                &backups,
                &semver::Version::new(1, 2, 3),
                "x86_64",
                &webserver,
                "version = \"1.2.3\"",
                &static_dir,
                &missing_zip,
                &geo_tmp,
                "",
                true,
            )
            .is_err()
        );
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
    fn rejects_symlinks_in_source_tree() {
        let temp = temp_dir("symlink");
        let source = temp.join("source");
        std::fs::create_dir_all(source.join("static")).unwrap();
        let webserver = source.join("landscape-webserver");
        std::fs::write(&webserver, b"x").unwrap();
        let outside = temp.join("outside");
        std::fs::write(&outside, b"secret").unwrap();
        std::os::unix::fs::symlink(&outside, source.join("static/evil")).unwrap();
        let (webserver, _static_dir, _, geo_tmp) = backup_source(&temp);
        let backups = temp.join("backups");
        std::fs::create_dir_all(&backups).unwrap();
        assert!(
            create_backup(
                &backups,
                &semver::Version::new(1, 2, 3),
                "x86_64",
                &webserver,
                "version = \"1.2.3\"",
                &source.join("static"),
                &temp.join("static.zip"),
                &geo_tmp,
                "",
                true,
            )
            .is_err()
        );
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
