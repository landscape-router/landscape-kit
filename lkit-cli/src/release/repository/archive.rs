use std::collections::HashSet;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use lkit_repository::zip_path_parts;
use semver::Version;

use super::RepositoryError;

pub(crate) const MAX_DECOMPRESSED_BYTES: u64 = 1024 * 1024 * 1024;

/// 使用受限流式解压：只接受单个 zstd frame，拒绝尾随数据和损坏帧，
/// 解压结果字节数不得超过 `max_decompressed`。返回实际解压字节数。
pub(crate) fn decompress_zstd(
    version: &Version,
    compressed: &Path,
    output: &Path,
    max_decompressed: u64,
) -> Result<u64, RepositoryError> {
    let input = std::fs::File::open(compressed).map_err(RepositoryError::Io)?;
    let decoder = zstd::stream::read::Decoder::new(input)
        .map_err(|error| RepositoryError::Decompress {
            version: version.clone(),
            reason: error.to_string(),
        })?
        .single_frame();
    let output_file = std::fs::File::create(output).map_err(RepositoryError::Io)?;
    let mut writer = BufWriter::new(output_file);

    let mut written: u64 = 0;
    let mut buffer = [0u8; 128 * 1024];
    let mut decoder = decoder;
    loop {
        let read = decoder
            .read(&mut buffer)
            .map_err(|error| RepositoryError::Decompress {
                version: version.clone(),
                reason: error.to_string(),
            })?;
        if read == 0 {
            break;
        }
        written = written.saturating_add(read as u64);
        if written > max_decompressed {
            return Err(RepositoryError::Decompress {
                version: version.clone(),
                reason: format!("decompressed result exceeds the {max_decompressed} byte limit"),
            });
        }
        writer
            .write_all(&buffer[..read])
            .map_err(RepositoryError::Io)?;
    }

    let mut remaining = decoder.finish();
    let mut probe = [0u8; 1];
    if remaining.read(&mut probe).map_err(RepositoryError::Io)? != 0 {
        return Err(RepositoryError::Decompress {
            version: version.clone(),
            reason: "compressed data contains trailing content".into(),
        });
    }
    writer.flush().map_err(RepositoryError::Io)?;
    Ok(written)
}

/// 解包 `static.zip`：所有条目必须位于 `static/` 前缀下，只允许目录和普通文件，
/// 拒绝绝对路径、`..` 穿越、设备文件和符号链接。去掉前缀后解压到 `target_dir`。
/// 解压总字节数不得超过压缩资产声明大小的 20 倍和 1 GiB 中较小者。
pub(crate) fn extract_static_archive(
    version: &Version,
    archive: &Path,
    declared_size: u64,
    target_dir: &Path,
) -> Result<(), RepositoryError> {
    let limit = std::cmp::min(declared_size.saturating_mul(20), MAX_DECOMPRESSED_BYTES);
    let file = std::fs::File::open(archive).map_err(RepositoryError::Io)?;
    let mut zip = zip::ZipArchive::new(file).map_err(|error| RepositoryError::Extract {
        version: version.clone(),
        reason: error.to_string(),
    })?;
    std::fs::create_dir(target_dir).map_err(|error| RepositoryError::Extract {
        version: version.clone(),
        reason: format!(
            "target directory must not exist and must be creatable exclusively: {error}"
        ),
    })?;

    let mut total_written: u64 = 0;
    let mut output_paths = HashSet::new();
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|error| RepositoryError::Extract {
                version: version.clone(),
                reason: error.to_string(),
            })?;
        if entry.is_symlink() {
            return Err(RepositoryError::Extract {
                version: version.clone(),
                reason: format!("entry {} is a symbolic link", entry.name()),
            });
        }
        if let Some(mode) = entry.unix_mode() {
            const S_IFMT: u32 = 0o170000;
            if matches!(mode & S_IFMT, 0o020000 | 0o060000 | 0o010000 | 0o140000) {
                return Err(RepositoryError::Extract {
                    version: version.clone(),
                    reason: format!("entry {} is a device file or special file", entry.name()),
                });
            }
        }
        let name = entry.name();
        if !name.starts_with("static/") {
            return Err(RepositoryError::Extract {
                version: version.clone(),
                reason: format!("entry {name} is not under the static/ prefix"),
            });
        }
        let relative = &name["static/".len()..];
        let parts = zip_path_parts(relative).map_err(|reason| RepositoryError::Extract {
            version: version.clone(),
            reason: format!("entry {name} has an unsafe path: {reason}"),
        })?;
        if parts.is_empty() {
            if entry.is_dir() && name == "static/" {
                continue;
            }
            return Err(RepositoryError::Extract {
                version: version.clone(),
                reason: format!("entry {name} has no valid relative path"),
            });
        }
        let normalized = parts.join("/");
        if !output_paths.insert(normalized.clone()) {
            return Err(RepositoryError::Extract {
                version: version.clone(),
                reason: format!("entry {name} duplicates existing path {normalized}"),
            });
        }
        if entry.is_dir() {
            let mut path = target_dir.to_path_buf();
            for part in parts {
                path.push(part);
            }
            std::fs::create_dir_all(path).map_err(RepositoryError::Io)?;
            continue;
        }
        if entry.size() > limit.saturating_sub(total_written) {
            return Err(RepositoryError::Extract {
                version: version.clone(),
                reason: format!("entry {name} exceeds the decompression byte limit"),
            });
        }
        let mut path = target_dir.to_path_buf();
        for part in parts {
            path.push(part);
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(RepositoryError::Io)?;
        }
        let mut output = std::fs::File::create(&path).map_err(RepositoryError::Io)?;
        let mut buffer = [0u8; 128 * 1024];
        loop {
            let read = entry
                .read(&mut buffer)
                .map_err(|error| RepositoryError::Extract {
                    version: version.clone(),
                    reason: error.to_string(),
                })?;
            if read == 0 {
                break;
            }
            total_written = total_written.saturating_add(read as u64);
            if total_written > limit {
                return Err(RepositoryError::Extract {
                    version: version.clone(),
                    reason: "total decompressed bytes exceed the limit".into(),
                });
            }
            output
                .write_all(&buffer[..read])
                .map_err(RepositoryError::Io)?;
        }
    }
    if !target_dir.join("index.html").is_file() {
        return Err(RepositoryError::Extract {
            version: version.clone(),
            reason: "decompressed result is missing static/index.html".into(),
        });
    }
    Ok(())
}

/// 从解压后的 static 目录现场打包 `static.zip`（条目带 `static/` 前缀,只允许
/// 目录与普通文件；发现符号链接、设备文件等非法条目时失败）。打包后按仓库解包
/// 规则自校验,保证恢复时可被正常消费。
pub(crate) fn pack_static_zip(
    static_dir: &Path,
    target: &Path,
) -> Result<PathBuf, RepositoryError> {
    let file = std::fs::File::create(target).map_err(RepositoryError::Io)?;
    let mut writer = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let prefix = Path::new("static");
    pack_entry(&mut writer, &options, prefix, static_dir, static_dir)?;
    writer
        .finish()
        .map_err(|error| RepositoryError::Io(std::io::Error::other(error)))?;

    let size = std::fs::metadata(target)
        .map_err(RepositoryError::Io)?
        .len();

    let check_dir = target
        .parent()
        .expect("packed zip has a parent")
        .join("static-pack-check");
    let _ = std::fs::remove_dir_all(&check_dir);
    extract_static_archive(&semver::Version::new(0, 0, 0), target, size, &check_dir)?;
    let _ = std::fs::remove_dir_all(&check_dir);
    Ok(target.to_path_buf())
}

fn pack_entry<W: std::io::Write + std::io::Seek>(
    writer: &mut zip::ZipWriter<W>,
    options: &zip::write::SimpleFileOptions,
    prefix: &Path,
    root: &Path,
    dir: &Path,
) -> Result<(), RepositoryError> {
    for entry in std::fs::read_dir(dir).map_err(RepositoryError::Io)? {
        let entry = entry.map_err(RepositoryError::Io)?;
        let file_type = entry.file_type().map_err(RepositoryError::Io)?;
        let entry_path = entry.path();
        let relative = entry_path
            .strip_prefix(root)
            .map_err(|error| RepositoryError::Io(std::io::Error::other(error)))?
            .to_path_buf();
        let zip_name = prefix.join(relative);
        if file_type.is_dir() {
            writer
                .add_directory(format!("{}/", zip_name.display()), *options)
                .map_err(|error| RepositoryError::Io(std::io::Error::other(error)))?;
            pack_entry(writer, options, prefix, root, &entry_path)?;
        } else if file_type.is_file() {
            writer
                .start_file(zip_name.display().to_string(), *options)
                .map_err(|error| RepositoryError::Io(std::io::Error::other(error)))?;
            let bytes = std::fs::read(&entry_path).map_err(RepositoryError::Io)?;
            use std::io::Write;
            writer.write_all(&bytes).map_err(RepositoryError::Io)?;
        } else {
            return Err(RepositoryError::Extract {
                version: semver::Version::new(0, 0, 0),
                reason: format!(
                    "the static directory contains an unsupported entry {}",
                    entry_path.display()
                ),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use semver::Version;

    use super::*;

    #[test]
    fn decompresses_zstd_and_rejects_trailing_data() {
        let version = Version::parse("0.19.2").unwrap();
        let compressed = std::env::temp_dir().join("lkit-decompress-test.zst");
        let output = std::env::temp_dir().join("lkit-decompress-test.bin");
        let data = b"landscape-webserver".repeat(1024);
        let mut encoded = zstd::stream::encode_all(Cursor::new(&data), 1).unwrap();
        std::fs::write(&compressed, &encoded).unwrap();
        let written = decompress_zstd(&version, &compressed, &output, 1 << 30).unwrap();
        assert_eq!(written as usize, data.len());
        assert_eq!(std::fs::read(&output).unwrap(), data);

        encoded.extend_from_slice(b"trailing garbage");
        std::fs::write(&compressed, &encoded).unwrap();
        let error = decompress_zstd(&version, &compressed, &output, 1 << 30).unwrap_err();
        assert!(matches!(error, RepositoryError::Decompress { .. }));
        let _ = std::fs::remove_file(&compressed);
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn rejects_decompress_bomb() {
        let version = Version::parse("0.19.2").unwrap();
        let compressed = std::env::temp_dir().join("lkit-decompress-bomb.zst");
        let output = std::env::temp_dir().join("lkit-decompress-bomb.bin");
        let data = vec![0xABu8; 1024 * 1024];
        let encoded = zstd::stream::encode_all(Cursor::new(&data), 1).unwrap();
        std::fs::write(&compressed, &encoded).unwrap();
        let error = decompress_zstd(&version, &compressed, &output, 4096).unwrap_err();
        assert!(matches!(error, RepositoryError::Decompress { .. }));
        let _ = std::fs::remove_file(&compressed);
        let _ = std::fs::remove_file(&output);
    }

    fn make_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, content) in entries {
            if content.is_empty() {
                writer
                    .add_directory(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
            } else {
                writer
                    .start_file(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                writer.write_all(content).unwrap();
            }
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn extracts_static_archive() {
        let version = Version::parse("0.19.2").unwrap();
        let archive = std::env::temp_dir().join("lkit-static-test.zip");
        let target = std::env::temp_dir().join("lkit-static-test-out");
        let _ = std::fs::remove_dir_all(&target);
        let zip = make_zip(&[
            ("static/", b""),
            ("static/index.html", b"<html></html>"),
            ("static/assets/app.js", b"console.log(1)"),
        ]);
        std::fs::write(&archive, &zip).unwrap();
        extract_static_archive(&version, &archive, zip.len() as u64, &target).unwrap();
        assert_eq!(
            std::fs::read(target.join("index.html")).unwrap(),
            b"<html></html>"
        );
        assert_eq!(
            std::fs::read(target.join("assets/app.js")).unwrap(),
            b"console.log(1)"
        );
        let _ = std::fs::remove_dir_all(&target);
        let _ = std::fs::remove_file(&archive);
    }

    #[test]
    fn rejects_zip_path_traversal() {
        let version = Version::parse("0.19.2").unwrap();
        let archive = std::env::temp_dir().join("lkit-static-evil.zip");
        let target = std::env::temp_dir().join("lkit-static-evil-out");
        let _ = std::fs::remove_dir_all(&target);
        let zip = make_zip(&[
            ("static/", b""),
            ("static/../evil", b"evil"),
            ("static/index.html", b"<html></html>"),
        ]);
        std::fs::write(&archive, &zip).unwrap();
        let error =
            extract_static_archive(&version, &archive, zip.len() as u64, &target).unwrap_err();
        assert!(matches!(error, RepositoryError::Extract { .. }));
        assert!(!target.join("evil").exists());
        let _ = std::fs::remove_dir_all(&target);
        let _ = std::fs::remove_file(&archive);
    }

    #[test]
    fn rejects_zip_without_static_prefix() {
        let version = Version::parse("0.19.2").unwrap();
        let archive = std::env::temp_dir().join("lkit-static-prefix.zip");
        let target = std::env::temp_dir().join("lkit-static-prefix-out");
        let _ = std::fs::remove_dir_all(&target);
        let zip = make_zip(&[("index.html", b"<html></html>")]);
        std::fs::write(&archive, &zip).unwrap();
        let error =
            extract_static_archive(&version, &archive, zip.len() as u64, &target).unwrap_err();
        assert!(matches!(error, RepositoryError::Extract { .. }));
        let _ = std::fs::remove_dir_all(&target);
        let _ = std::fs::remove_file(&archive);
    }

    #[test]
    fn rejects_zip_without_index_html() {
        let version = Version::parse("0.19.2").unwrap();
        let archive = std::env::temp_dir().join("lkit-static-noindex.zip");
        let target = std::env::temp_dir().join("lkit-static-noindex-out");
        let _ = std::fs::remove_dir_all(&target);
        let zip = make_zip(&[("static/", b""), ("static/assets/app.js", b"x")]);
        std::fs::write(&archive, &zip).unwrap();
        let error =
            extract_static_archive(&version, &archive, zip.len() as u64, &target).unwrap_err();
        assert!(matches!(error, RepositoryError::Extract { .. }));
        let _ = std::fs::remove_dir_all(&target);
        let _ = std::fs::remove_file(&archive);
    }

    #[test]
    fn rejects_existing_target_directory() {
        let version = Version::parse("0.19.2").unwrap();
        let archive = std::env::temp_dir().join("lkit-static-existing.zip");
        let target = std::env::temp_dir().join("lkit-static-existing-out");
        let _ = std::fs::remove_dir_all(&target);
        std::fs::create_dir(&target).unwrap();
        let zip = make_zip(&[("static/index.html", b"<html></html>")]);
        std::fs::write(&archive, &zip).unwrap();
        let error =
            extract_static_archive(&version, &archive, zip.len() as u64, &target).unwrap_err();
        assert!(matches!(error, RepositoryError::Extract { .. }));
        let _ = std::fs::remove_dir_all(&target);
        let _ = std::fs::remove_file(&archive);
    }

    #[test]
    fn rejects_duplicate_normalized_zip_paths() {
        let version = Version::parse("0.19.2").unwrap();
        let archive = std::env::temp_dir().join("lkit-static-duplicate.zip");
        let target = std::env::temp_dir().join("lkit-static-duplicate-out");
        let _ = std::fs::remove_dir_all(&target);
        let zip = make_zip(&[("static/assets/", b""), ("static/assets", b"second")]);
        std::fs::write(&archive, &zip).unwrap();
        let error =
            extract_static_archive(&version, &archive, zip.len() as u64, &target).unwrap_err();
        assert!(matches!(error, RepositoryError::Extract { .. }));
        let _ = std::fs::remove_dir_all(&target);
        let _ = std::fs::remove_file(&archive);
    }

    #[test]
    fn rejects_dot_zip_path_component() {
        let version = Version::parse("0.19.2").unwrap();
        let archive = std::env::temp_dir().join("lkit-static-dot.zip");
        let target = std::env::temp_dir().join("lkit-static-dot-out");
        let _ = std::fs::remove_dir_all(&target);
        let zip = make_zip(&[("static/./index.html", b"<html></html>")]);
        std::fs::write(&archive, &zip).unwrap();
        let error =
            extract_static_archive(&version, &archive, zip.len() as u64, &target).unwrap_err();
        assert!(matches!(error, RepositoryError::Extract { .. }));
        let _ = std::fs::remove_dir_all(&target);
        let _ = std::fs::remove_file(&archive);
    }

    #[test]
    fn rejects_zip_symlink() {
        let version = Version::parse("0.19.2").unwrap();
        let archive = std::env::temp_dir().join("lkit-static-symlink.zip");
        let target = std::env::temp_dir().join("lkit-static-symlink-out");
        let _ = std::fs::remove_dir_all(&target);
        let zip = build_raw_zip(&[
            ("static/", b"", 0o040755u32 << 16),
            ("static/index.html", b"<html></html>", 0o100644u32 << 16),
            ("static/link", b"index.html", 0o120777u32 << 16),
        ]);
        std::fs::write(&archive, &zip).unwrap();
        let error =
            extract_static_archive(&version, &archive, zip.len() as u64, &target).unwrap_err();
        assert!(matches!(error, RepositoryError::Extract { .. }));
        assert!(!target.join("link").exists());
        let _ = std::fs::remove_dir_all(&target);
        let _ = std::fs::remove_file(&archive);
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc: u32 = 0xFFFF_FFFF;
        for byte in data {
            crc ^= *byte as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    /// 手工构造 Store 模式 zip，允许写入带 Unix 文件类型位的条目。
    fn build_raw_zip(entries: &[(&str, &[u8], u32)]) -> Vec<u8> {
        let mut locals = Vec::new();
        let mut central = Vec::new();
        let mut offset = 0u32;
        for (name, data, external) in entries {
            let crc = crc32(data);
            locals.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
            locals.extend_from_slice(&20u16.to_le_bytes());
            locals.extend_from_slice(&0u16.to_le_bytes());
            locals.extend_from_slice(&0u16.to_le_bytes());
            locals.extend_from_slice(&0u16.to_le_bytes());
            locals.extend_from_slice(&0x21u16.to_le_bytes());
            locals.extend_from_slice(&crc.to_le_bytes());
            locals.extend_from_slice(&(data.len() as u32).to_le_bytes());
            locals.extend_from_slice(&(data.len() as u32).to_le_bytes());
            locals.extend_from_slice(&(name.len() as u16).to_le_bytes());
            locals.extend_from_slice(&0u16.to_le_bytes());
            locals.extend_from_slice(name.as_bytes());
            locals.extend_from_slice(data);

            central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            central.extend_from_slice(&(20u16 | (3u16 << 8)).to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0x21u16.to_le_bytes());
            central.extend_from_slice(&crc.to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&external.to_le_bytes());
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name.as_bytes());

            offset += 30 + name.len() as u32 + data.len() as u32;
        }
        let central_offset = offset;
        let central_len = central.len() as u32;
        let count = entries.len() as u16;
        let mut out = locals;
        out.append(&mut central);
        out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&central_len.to_le_bytes());
        out.extend_from_slice(&central_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }
}
