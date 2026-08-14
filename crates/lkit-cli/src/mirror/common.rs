use std::fs;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use super::{MirrorError, paths};

/// 替换文本中所有出现在 URL 主机位置的 `from` 为 `to`。
/// 要求 `from` 前后都是 URL 边界（`/`、空白或行首/行尾），避免误替换主机名子串，
/// 也避免通用路径（`.../debian`）吞掉更具体的路径（`.../debian-security`）。
pub(crate) fn replace_host(text: &str, from: &str, to: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(offset) = rest.find(from) {
        let head = &rest[..offset];
        let after = &rest[offset + from.len()..];
        let boundary = head.chars().next_back().is_none_or(is_boundary)
            && after.chars().next().is_none_or(is_boundary);
        if boundary {
            out.push_str(head);
            out.push_str(to);
            rest = after;
        } else {
            out.push_str(&rest[..offset + from.len()]);
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// `from` 之后的一个字符是否是 URL 边界（非主机名/路径字符）。
pub(crate) fn is_boundary(character: char) -> bool {
    !(character.is_ascii_alphanumeric() || matches!(character, '.' | '-'))
}

/// `host`（主机+路径）是否以 URL 边界出现在文本中。
pub(crate) fn contains_host(text: &str, host: &str) -> bool {
    let mut rest = text;
    while let Some(offset) = rest.find(host) {
        let head = &rest[..offset];
        let after = &rest[offset + host.len()..];
        if head.chars().next_back().is_none_or(is_boundary)
            && after.chars().next().is_none_or(is_boundary)
        {
            return true;
        }
        rest = after;
    }
    false
}

/// 原子写入：同目录临时文件 + rename，保留原文件权限位。
pub(crate) fn write_atomic(path: &Path, content: &str) -> Result<(), MirrorError> {
    let mode = fs::metadata(path)
        .map(|metadata| metadata.permissions().mode())
        .unwrap_or(0o644);
    let parent = path.parent().ok_or_else(|| {
        MirrorError::Message(format!("no parent directory for {}", path.display()))
    })?;
    let temp = parent.join(format!(".lkit-mirror-{}.tmp", std::process::id()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true).mode(mode);
    let mut file = options.open(&temp)?;
    use std::io::Write;
    file.write_all(content.as_bytes())?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temp, path)?;
    Ok(())
}

/// 把备份目录中的文件按相对路径写回 `target_root`。
pub(crate) fn restore_files(dir: &Path, target_root: &Path) -> Result<(), MirrorError> {
    for entry in walk_files(dir)? {
        let relative = entry.strip_prefix(dir).unwrap_or(&entry);
        let target = target_root.join(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(&entry, &target)?;
    }
    Ok(())
}

/// 把备份文件写回根目录（apply 失败时回滚），成功后删除备份。
pub(crate) fn rollback(backup_path: &Path) -> Result<(), MirrorError> {
    restore_files(backup_path, &paths().restore_root)?;
    fs::remove_dir_all(backup_path)?;
    Ok(())
}

/// 递归收集目录下全部文件（排序，保持确定性）。
pub(crate) fn walk_files(dir: &Path) -> Result<Vec<PathBuf>, MirrorError> {
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in fs::read_dir(&current)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                stack.push(entry.path());
            } else {
                files.push(entry.path());
            }
        }
    }
    files.sort();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_host_with_boundary_check() {
        let text = "deb http://deb.debian.org/debian bookworm main\n";
        assert_eq!(
            replace_host(
                text,
                "deb.debian.org/debian",
                "mirrors.tuna.tsinghua.edu.cn/debian"
            ),
            "deb http://mirrors.tuna.tsinghua.edu.cn/debian bookworm main\n"
        );
        assert_eq!(
            replace_host(
                "https://www.deb.debian.org/debian bookworm main",
                "deb.debian.org/debian",
                "mirrors.tuna.tsinghua.edu.cn/debian"
            ),
            "https://www.deb.debian.org/debian bookworm main"
        );
        assert_eq!(
            replace_host(
                "https://mirrors.tuna.tsinghua.edu.cn/debian-security bookworm",
                "mirrors.tuna.tsinghua.edu.cn/debian",
                "mirrors.aliyun.com/debian"
            ),
            "https://mirrors.tuna.tsinghua.edu.cn/debian-security bookworm",
            "`-security` 的后缀不是边界，不得误替换"
        );
    }

    #[test]
    fn restore_writes_files_back_and_removes_backup() {
        let backup =
            std::env::temp_dir().join(format!("lkit-mirror-common-restore-{}", std::process::id()));
        let target =
            std::env::temp_dir().join(format!("lkit-mirror-common-target-{}", std::process::id()));
        let _ = fs::remove_dir_all(&backup);
        let _ = fs::remove_dir_all(&target);
        fs::create_dir_all(backup.join("etc/apt/sources.list.d")).unwrap();
        fs::write(backup.join("etc/apt/sources.list"), "official\n").unwrap();
        fs::write(backup.join("etc/apt/sources.list.d/local.list"), "local\n").unwrap();
        restore_files(&backup, &target).unwrap();
        assert_eq!(
            fs::read_to_string(target.join("etc/apt/sources.list")).unwrap(),
            "official\n"
        );
        assert_eq!(
            fs::read_to_string(target.join("etc/apt/sources.list.d/local.list")).unwrap(),
            "local\n"
        );
        let _ = fs::remove_dir_all(&backup);
        let _ = fs::remove_dir_all(&target);
    }
}
