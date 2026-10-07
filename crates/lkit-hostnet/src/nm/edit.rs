//! NetworkManager drop-in 计划:生成(或覆盖)`conf.d/lkit-unmanage.conf`,
//! 内容为 `[device] unmanaged-devices=interface-name:<if>;...`。新建文件的属主
//! 继承 conf.d 目录、权限 0644;已存在的 drop-in 保持其原元数据逐字备份。

use std::os::unix::fs::MetadataExt;

use crate::error::HostNetError;
use crate::ifupdown::capture_metadata;
use crate::model::{EditPlan, FileEdit, FileMetadata, FileSet};

use super::UNMANAGE_CONF;

/// NM 的 `interface-name` 匹配器支持 glob 字符;选中名含这些字符会扩大匹配
/// 范围,保守拒绝。
const GLOB_META_CHARS: [char; 4] = ['*', '?', '[', ']'];

pub(super) fn plan_unmanage(
    file_set: &FileSet,
    selected: &[String],
) -> Result<EditPlan, HostNetError> {
    let Some(conf_d) = &file_set.conf_d else {
        return Ok(EditPlan { edits: Vec::new() });
    };
    if selected.is_empty() {
        return Ok(EditPlan { edits: Vec::new() });
    }
    for name in selected {
        if name.is_empty()
            || !name.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b':')
            })
            || name.chars().any(|char| GLOB_META_CHARS.contains(&char))
        {
            return Err(HostNetError::UnsupportedSyntax {
                path: conf_d.join(UNMANAGE_CONF),
                line: 0,
                reason: format!("interface name {name:?} is not usable in unmanaged-devices"),
            });
        }
    }

    let drop_in = conf_d.join(UNMANAGE_CONF);
    let existing = file_set.files.iter().find(|path| **path == drop_in);
    let mut names = selected.to_vec();
    names.sort();
    names.dedup();
    let mut content = String::new();
    content.push_str("# lkit network takeover: these interfaces are managed by Landscape.\n");
    content.push_str("# Removed automatically by rollback and uninstall.\n");
    content.push_str("[device]\n");
    content.push_str("unmanaged-devices=");
    for (index, name) in names.iter().enumerate() {
        if index > 0 {
            content.push(';');
        }
        content.push_str("interface-name:");
        content.push_str(name);
    }
    content.push('\n');

    let (original_content, metadata, created) = match existing {
        Some(path) => {
            let bytes = std::fs::read(path).map_err(|source| HostNetError::UnreadableFile {
                path: path.clone(),
                source,
            })?;
            (bytes, capture_metadata(path)?, false)
        }
        None => {
            // 新建文件继承 conf.d 目录的属主,权限固定 0644。
            let metadata = std::fs::symlink_metadata(conf_d).map_err(|source| {
                HostNetError::UnreadableFile {
                    path: conf_d.clone(),
                    source,
                }
            })?;
            (
                Vec::new(),
                FileMetadata {
                    mode: 0o644,
                    uid: metadata.uid(),
                    gid: metadata.gid(),
                },
                true,
            )
        }
    };

    Ok(EditPlan {
        edits: vec![FileEdit {
            path: drop_in,
            original_content,
            content,
            metadata,
            created,
        }],
    })
}
