//! NetworkManager conf.d 目录收集:只关注 lkit 自己的 drop-in 文件。

use std::path::PathBuf;

use crate::error::HostNetError;
use crate::model::{FileSet, FileSources};

use super::UNMANAGE_CONF;

/// `nm_conf_d` 缺失或目录不存在时返回空集合(宿主没有 NM,摘除为 no-op)。
/// 目录必须是非符号链接的绝对路径;已存在的 lkit drop-in 若是符号链接,
/// 在收集阶段以 PathSafety 阻断。
pub(super) fn collect(sources: &FileSources) -> Result<FileSet, HostNetError> {
    let Some(conf_d) = &sources.nm_conf_d else {
        return Ok(FileSet::default());
    };
    if !conf_d.is_absolute() {
        return Err(HostNetError::PathSafety {
            path: conf_d.clone(),
            reason: "NetworkManager conf.d directory must be absolute".into(),
        });
    }
    match std::fs::symlink_metadata(conf_d) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(HostNetError::PathSafety {
                path: conf_d.clone(),
                reason: "refusing to operate through a symbolic conf.d directory".into(),
            });
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(FileSet::default());
        }
        Err(source) => {
            return Err(HostNetError::UnreadableFile {
                path: conf_d.clone(),
                source,
            });
        }
    }
    let drop_in = conf_d.join(UNMANAGE_CONF);
    let mut files = Vec::new();
    match std::fs::symlink_metadata(&drop_in) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(HostNetError::PathSafety {
                path: drop_in,
                reason: "lkit drop-in must be a regular file".into(),
            });
        }
        Ok(_) => files.push(drop_in),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(source) => {
            return Err(HostNetError::UnreadableFile {
                path: drop_in,
                source,
            });
        }
    }
    Ok(FileSet {
        interfaces: PathBuf::new(),
        files,
        conf_d: Some(conf_d.clone()),
    })
}
