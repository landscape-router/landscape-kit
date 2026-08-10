use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use chrono::{DateTime, Utc};
use flate2::Compression;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::super::plan::InstallError;
use super::super::transaction::BackupRef;
use super::verify_lkb;
use super::{
    BACKUP_ID_ATTEMPTS, BackupArchitecture, BackupContents, BackupMetadata, BackupProgress,
    BackupScope, LKB_HEADER_LEN, LKB_MAGIC, LKB_METADATA_CAPACITY, hex, invalid_backup,
};

/// 写出时同步计算 SHA-256 与字节数的流式写入器，避免把整个归档保存在内存中。
struct HashingWriter<W: Write> {
    inner: W,
    hasher: Sha256,
    bytes: u64,
}

type TarBuilder<'a> = tar::Builder<GzEncoder<HashingWriter<&'a mut File>>>;

impl<W: Write> HashingWriter<W> {
    fn finish(self) -> (String, u64) {
        (hex(&self.hasher.finalize()), self.bytes)
    }
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(buf)?;
        self.hasher.update(&buf[..written]);
        self.bytes += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// 临时文件守卫：离开作用域时删除，保证失败路径不残留中间文件。
struct TmpCleanup(std::path::PathBuf);

impl Drop for TmpCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
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
    mut progress: Option<&mut dyn FnMut(BackupProgress)>,
) -> Result<BackupRef, InstallError> {
    validate_remark(remark)?;
    let tmp_dir = backups_dir.join(".tmp");
    std::fs::create_dir_all(&tmp_dir).map_err(InstallError::Io)?;
    let archive_tmp = tmp_dir.join(format!("archive-{}.tar.gz", Uuid::now_v7()));
    let _archive_cleanup = TmpCleanup(archive_tmp.clone());
    let (tar_sha256, _) = stream_tar_gz(
        &archive_tmp,
        webserver,
        init_config,
        static_dir,
        static_archive_zip,
        geo_tmp,
        &mut progress,
    )?;
    let now = Utc::now();
    let architecture = parse_architecture(architecture)?;
    if let Some(callback) = progress.as_mut() {
        callback(BackupProgress::Finalizing);
    }

    for _ in 0..BACKUP_ID_ATTEMPTS {
        let backup_id = new_backup_id(now);
        let metadata = BackupMetadata {
            schema_version: 1,
            backup_id: backup_id.clone(),
            created_at: now,
            landscape_version: landscape_version.to_string(),
            lkit_version: env!("CARGO_PKG_VERSION").into(),
            architecture,
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
        let tmp = tmp_dir.join(format!("{backup_id}.tmp"));
        let tmp_cleanup = TmpCleanup(tmp.clone());
        let (file_sha256, _) = write_lkb_container(&tmp, &metadata_json, &archive_tmp)?;
        let written = std::fs::read(&tmp).map_err(InstallError::Io)?;
        verify_lkb(&written)?;
        let final_path = backups_dir.join(format!("{backup_id}.lkb"));
        match publish_no_replace(&tmp, &final_path) {
            Ok(()) => {
                drop(tmp_cleanup);
                return Ok(BackupRef {
                    path: format!("backups/{backup_id}.lkb"),
                    backup_id,
                    sha256: file_sha256,
                });
            }
            Err(InstallError::Io(error)) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }

    Err(invalid_backup(
        "could not allocate a unique backup ID after multiple attempts".into(),
    ))
}

fn new_backup_id(created_at: DateTime<Utc>) -> String {
    let suffix = Uuid::now_v7().as_u128() as u32;
    format!("{}-{suffix:08x}", created_at.format("%Y%m%d-%H%M%S"))
}

/// 把 tar.gz 流式写入 `archive_tmp`（`0600` 新建），压缩过程同步计算 SHA-256；
/// 按文件数报告进度（`total` 只统计文件，目录不计入）。
fn stream_tar_gz(
    archive_tmp: &Path,
    webserver: &Path,
    init_config: &str,
    static_dir: &Path,
    static_archive_zip: &Path,
    geo_tmp: &Path,
    progress: &mut Option<&mut dyn FnMut(BackupProgress)>,
) -> Result<(String, u64), InstallError> {
    let total = count_archive_entries(static_dir, geo_tmp)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(archive_tmp)
        .map_err(InstallError::Io)?;
    let mut done = 0u64;
    let (tar_sha256, tar_size);
    {
        let sink = HashingWriter {
            inner: &mut file,
            hasher: Sha256::new(),
            bytes: 0,
        };
        let encoder = GzEncoder::new(sink, Compression::default());
        let mut builder = tar::Builder::new(encoder);
        append_entry(&mut builder, webserver, "landscape-webserver")?;
        done += 1;
        report_archive(progress, done, total, "landscape-webserver");
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(init_config.len() as u64);
        header.set_mode(0o600);
        header.set_cksum();
        builder
            .append_data(&mut header, "landscape_init.toml", init_config.as_bytes())
            .map_err(|error| invalid_backup(format!("failed to append init config: {error}")))?;
        done += 1;
        report_archive(progress, done, total, "landscape_init.toml");
        if !static_archive_zip.is_file() {
            return Err(invalid_backup(format!(
                "static archive {} is not a readable regular file",
                static_archive_zip.display()
            )));
        }
        append_entry(&mut builder, static_archive_zip, "static.zip")?;
        done += 1;
        report_archive(progress, done, total, "static.zip");
        append_tree(
            &mut builder,
            static_dir,
            "static",
            &mut done,
            total,
            progress,
        )?;
        if geo_tmp.is_dir() {
            append_tree(&mut builder, geo_tmp, "geo_tmp", &mut done, total, progress)?;
        } else {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Directory);
            header.set_size(0);
            header.set_mode(0o755);
            header.set_cksum();
            builder
                .append_data(&mut header, "geo_tmp/", io::empty())
                .map_err(|error| invalid_backup(format!("failed to append geo_tmp: {error}")))?;
            done += 1;
            report_archive(progress, done, total, "geo_tmp");
        }
        builder
            .finish()
            .map_err(|error| invalid_backup(format!("failed to finish tar: {error}")))?;
        let encoder = match builder.into_inner() {
            Ok(encoder) => encoder,
            Err(error) => {
                return Err(invalid_backup(format!("failed to finalize tar: {error}")));
            }
        };
        let sink = encoder
            .finish()
            .map_err(|error| invalid_backup(format!("failed to finalize gzip: {error}")))?;
        let (sha256, size) = sink.finish();
        tar_sha256 = sha256;
        tar_size = size;
    }
    file.sync_all().map_err(InstallError::Io)?;
    Ok((tar_sha256, tar_size))
}

fn append_entry(builder: &mut TarBuilder<'_>, path: &Path, name: &str) -> Result<(), InstallError> {
    builder
        .append_path_with_name(path, name)
        .map_err(|error| invalid_backup(format!("failed to append {name}: {error}")))
}

fn report_archive(
    progress: &mut Option<&mut dyn FnMut(BackupProgress)>,
    done: u64,
    total: u64,
    current: &str,
) {
    if let Some(callback) = progress.as_mut() {
        callback(BackupProgress::Archiving {
            done,
            total,
            current: current.to_string(),
        });
    }
}

/// 统计归档将写入的条目总数：固定 3 个文件 + 两个目录树中的文件数。
/// `geo_tmp` 缺失时按 1 个空目录条目计，与 `stream_tar_gz` 的实际写入一致。
fn count_archive_entries(static_dir: &Path, geo_tmp: &Path) -> Result<u64, InstallError> {
    let mut total = 3u64;
    total += count_tree_files(static_dir)?;
    total += if geo_tmp.is_dir() {
        count_tree_files(geo_tmp)?
    } else {
        1
    };
    Ok(total)
}

fn count_tree_files(dir: &Path) -> Result<u64, InstallError> {
    let mut count = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(path) = stack.pop() {
        for entry in std::fs::read_dir(&path).map_err(InstallError::Io)? {
            let entry = entry.map_err(InstallError::Io)?;
            let file_type = entry.file_type().map_err(InstallError::Io)?;
            if file_type.is_dir() {
                stack.push(entry.path());
            } else {
                count += 1;
            }
        }
    }
    Ok(count)
}

/// 写 Header + metadata JSON + 零填充 + tar.gz 的完整 `.lkb` 容器，
/// 边写边计算整文件 SHA-256。
fn write_lkb_container(
    tmp: &Path,
    metadata_json: &[u8],
    archive_tmp: &Path,
) -> Result<(String, u64), InstallError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(tmp)
        .map_err(InstallError::Io)?;
    let mut sink = HashingWriter {
        inner: &mut file,
        hasher: Sha256::new(),
        bytes: 0,
    };
    let mut header = [0u8; LKB_HEADER_LEN];
    header[0..4].copy_from_slice(LKB_MAGIC);
    header[4..6].copy_from_slice(&1u16.to_le_bytes());
    header[6..10].copy_from_slice(&(metadata_json.len() as u32).to_le_bytes());
    sink.write_all(&header).map_err(InstallError::Io)?;
    sink.write_all(metadata_json).map_err(InstallError::Io)?;
    let padding = vec![0u8; LKB_METADATA_CAPACITY - LKB_HEADER_LEN - metadata_json.len()];
    sink.write_all(&padding).map_err(InstallError::Io)?;
    let mut archive = File::open(archive_tmp).map_err(InstallError::Io)?;
    io::copy(&mut archive, &mut sink).map_err(InstallError::Io)?;
    let (file_sha256, file_size) = sink.finish();
    file.sync_all().map_err(InstallError::Io)?;
    Ok((file_sha256, file_size))
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

fn parse_architecture(value: &str) -> Result<BackupArchitecture, InstallError> {
    match value {
        "x86_64" => Ok(BackupArchitecture::X86_64),
        "aarch64" => Ok(BackupArchitecture::Aarch64),
        _ => Err(invalid_backup(format!("unsupported architecture {value}"))),
    }
}

fn append_tree(
    builder: &mut TarBuilder<'_>,
    dir: &Path,
    prefix: &str,
    done: &mut u64,
    total: u64,
    progress: &mut Option<&mut dyn FnMut(BackupProgress)>,
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
                *done += 1;
                report_archive(progress, *done, total, &rel_path);
            } else {
                return Err(invalid_backup(format!(
                    "{rel_path} is not a regular file or directory"
                )));
            }
        }
    }
    Ok(())
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
    fn identical_archives_created_back_to_back_get_distinct_ids() {
        let temp = temp_dir("distinct-ids");
        let source = temp.join("source");
        std::fs::create_dir_all(&source).unwrap();
        let (webserver, static_dir, static_zip, geo_tmp) = backup_source(&source);
        let backups = temp.join("backups");
        std::fs::create_dir_all(&backups).unwrap();

        let first = create_backup(
            &backups,
            &semver::Version::new(1, 2, 3),
            "x86_64",
            &webserver,
            "version = \"1.2.3\"",
            &static_dir,
            &static_zip,
            &geo_tmp,
            "first",
            false,
            None,
        )
        .unwrap();
        let second = create_backup(
            &backups,
            &semver::Version::new(1, 2, 3),
            "x86_64",
            &webserver,
            "version = \"1.2.3\"",
            &static_dir,
            &static_zip,
            &geo_tmp,
            "second",
            false,
            None,
        )
        .unwrap();

        assert_ne!(first.backup_id, second.backup_id);
        assert!(backups.join(format!("{}.lkb", first.backup_id)).is_file());
        assert!(backups.join(format!("{}.lkb", second.backup_id)).is_file());
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn reports_file_count_progress_while_creating() {
        let temp = temp_dir("progress");
        let source = temp.join("source");
        std::fs::create_dir_all(&source).unwrap();
        let (webserver, static_dir, static_zip, geo_tmp) = backup_source(&source);
        let backups = temp.join("backups");
        std::fs::create_dir_all(&backups).unwrap();
        let mut events = Vec::new();
        {
            let mut sink = |progress: BackupProgress| events.push(progress);
            create_backup(
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
                Some(&mut sink),
            )
            .unwrap();
        }
        // 固定 3 个文件 + static 2 个 + geo_tmp 1 个。
        let archiving: Vec<_> = events
            .iter()
            .filter(|event| matches!(event, BackupProgress::Archiving { .. }))
            .collect();
        assert_eq!(archiving.len(), 6);
        let last = match archiving.last().unwrap() {
            BackupProgress::Archiving { done, total, .. } => (*done, *total),
            _ => unreachable!(),
        };
        assert_eq!(last, (6, 6), "the last archive event must reach the total");
        assert_eq!(events.last(), Some(&BackupProgress::Finalizing));
        assert!(events.iter().all(|event| {
            matches!(
                event,
                BackupProgress::Exporting
                    | BackupProgress::Archiving { .. }
                    | BackupProgress::Finalizing
            )
        }));
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
                None,
            )
            .is_err()
        );
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
                None,
            )
            .is_err()
        );
        let _ = std::fs::remove_dir_all(&temp);
    }
}
