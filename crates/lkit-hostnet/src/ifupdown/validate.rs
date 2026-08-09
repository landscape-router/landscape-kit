//! dry-run 校验:调用注入的 `ifup --no-act --interfaces <main> --all` 验证编辑后的
//! 文件集合。工具缺失或不可执行返回 `Validation::Unavailable`,不阻断调用方;
//! dry-run 非零退出返回 `Validation::Failed`。

use std::process::Command;

use crate::error::HostNetError;
use crate::model::{FileSet, ToolPaths, Validation};

pub(crate) fn validate(file_set: &FileSet, tools: &ToolPaths) -> Result<Validation, HostNetError> {
    if file_set.is_empty() {
        return Ok(Validation::Clean);
    }
    let Some(ifup) = tools.ifup.as_ref() else {
        return Ok(Validation::Unavailable);
    };
    if !ifup.exists() {
        return Ok(Validation::Unavailable);
    }
    let interfaces = format!("--interfaces={}", file_set.interfaces.display());
    let output = match Command::new(ifup)
        .arg("--no-act")
        .arg(interfaces)
        .arg("--all")
        .output()
    {
        Ok(output) => output,
        Err(_) => return Ok(Validation::Unavailable),
    };
    if output.status.success() {
        return Ok(Validation::Clean);
    }
    Ok(Validation::Failed {
        exit: output.status.code(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lkit-hostnet-validate-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn file_set(path: &Path) -> FileSet {
        FileSet {
            interfaces: path.to_path_buf(),
            files: vec![path.to_path_buf()],
        }
    }

    #[test]
    fn missing_tool_is_unavailable() {
        let dir = temp_dir("missing-tool");
        let path = dir.join("interfaces");
        std::fs::write(&path, b"iface eth0 inet manual\n").unwrap();
        let result = validate(&file_set(&path), &ToolPaths::default()).unwrap();
        assert_eq!(result, Validation::Unavailable);
    }

    #[test]
    fn nonexistent_tool_path_is_unavailable() {
        let dir = temp_dir("nonexistent-tool");
        let path = dir.join("interfaces");
        std::fs::write(&path, b"iface eth0 inet manual\n").unwrap();
        let tools = ToolPaths {
            ifup: Some(dir.join("does-not-exist")),
        };
        let result = validate(&file_set(&path), &tools).unwrap();
        assert_eq!(result, Validation::Unavailable);
    }

    #[test]
    fn successful_dry_run_is_clean() {
        let dir = temp_dir("clean");
        let path = dir.join("interfaces");
        std::fs::write(&path, b"iface eth0 inet manual\n").unwrap();
        let tool = dir.join("fake-ifup");
        std::fs::write(&tool, b"#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        let tools = ToolPaths { ifup: Some(tool) };
        let result = validate(&file_set(&path), &tools).unwrap();
        assert_eq!(result, Validation::Clean);
    }

    #[test]
    fn failing_dry_run_reports_exit_and_stderr() {
        let dir = temp_dir("failed");
        let path = dir.join("interfaces");
        std::fs::write(&path, b"iface eth0 inet manual\n").unwrap();
        let tool = dir.join("fake-ifup");
        std::fs::write(&tool, b"#!/bin/sh\necho 'ifup: bad stanza' >&2\nexit 3\n").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        let tools = ToolPaths { ifup: Some(tool) };
        let result = validate(&file_set(&path), &tools).unwrap();
        assert_eq!(
            result,
            Validation::Failed {
                exit: Some(3),
                stderr: "ifup: bad stanza".into()
            }
        );
    }

    #[test]
    fn empty_file_set_validates_clean_without_tools() {
        let dir = temp_dir("empty-set");
        let path = dir.join("interfaces");
        let result = validate(
            &FileSet {
                interfaces: path,
                files: Vec::new(),
            },
            &ToolPaths::default(),
        )
        .unwrap();
        assert_eq!(result, Validation::Clean);
    }
}
