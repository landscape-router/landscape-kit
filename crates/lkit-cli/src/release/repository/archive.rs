use std::collections::HashSet;
use std::io::{BufWriter, Read, Write};
use std::path::Path;

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
