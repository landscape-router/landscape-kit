use std::fs;
use std::path::PathBuf;

use parse::{already_target, rewrite};

use super::backend::SourcesBackend;
use super::common;
use super::{ApplyReport, Family, Host, MirrorError, MirrorName, backup_dir, paths};

pub(crate) mod parse;

/// dnf/yum 家族（Fedora、CentOS 7/Stream、Rocky、AlmaLinux）软件源后端。
pub(crate) struct DnfBackend {
    family: Family,
}

impl DnfBackend {
    pub(crate) fn new(host: &Host) -> Self {
        Self {
            family: host.family,
        }
    }
}

impl SourcesBackend for DnfBackend {
    fn show(&self) -> Result<String, MirrorError> {
        show()
    }

    fn apply(
        &self,
        mirror: MirrorName,
        _replace_security: bool,
    ) -> Result<ApplyReport, MirrorError> {
        apply(&self.family, mirror)
    }

    fn restore(&self) -> Result<(), MirrorError> {
        restore(&self.family)
    }
}

/// 受管 dnf/yum 仓库文件：`/etc/yum.repos.d/*.repo`。
pub(crate) fn managed_files() -> Result<Vec<PathBuf>, MirrorError> {
    let entries = fs::read_dir(&paths().dnf_repos_dir).map_err(|_| {
        MirrorError::Message(crate::tr!(crate::keys::mirror::MIRROR_NO_SOURCE_FILES))
    })?;
    let mut files: Vec<_> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            // `fs::metadata` 跟随符号链接：dnf 会读取 symlink 指向的 repo 文件。
            fs::metadata(entry.path()).is_ok_and(|metadata| metadata.is_file())
                && entry.file_name().to_string_lossy().ends_with(".repo")
        })
        .map(|entry| entry.path())
        .collect();
    if files.is_empty() {
        return Err(MirrorError::Message(crate::tr!(
            crate::keys::mirror::MIRROR_NO_SOURCE_FILES
        )));
    }
    files.sort();
    Ok(files)
}

/// 备份当前全部受管 repo 文件。
fn backup(family: Family) -> Result<PathBuf, MirrorError> {
    let dir = backup_dir(family);
    if dir.exists() {
        fs::remove_dir_all(&dir)?;
    }
    fs::create_dir_all(&dir)?;
    for file in managed_files()? {
        // 相对 `restore_root` 保存，恢复时原样写回（生产 root 为 `/`）。
        let relative = file.strip_prefix(&paths().restore_root).unwrap_or(&file);
        let target = dir.join(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(&file, &target)?;
    }
    Ok(dir)
}

/// 把 dnf/yum 仓库切换到指定镜像。失败时从刚创建的备份回滚已修改的文件。
/// 先在内存里计算全部重写结果，只有确实要改动文件时才创建备份；no-op 不触碰
/// 已有的备份目录。
fn apply(family: &Family, mirror: MirrorName) -> Result<ApplyReport, MirrorError> {
    let files = managed_files()?;
    // 只读阶段：先算出每个文件的重写结果，不写盘。
    let mut rewrites: Vec<(PathBuf, String)> = Vec::new();
    let mut already_matched = false;
    let mut skipped_repositories = 0usize;
    for file in &files {
        let original = fs::read_to_string(file)?;
        match rewrite(&original, *family, mirror) {
            Some(rewritten) => {
                skipped_repositories += rewritten.skipped_repositories;
                rewrites.push((file.clone(), rewritten.content));
            }
            None => already_matched |= already_target(&original, *family, mirror),
        }
    }
    if rewrites.is_empty() && already_matched {
        return Ok(ApplyReport {
            skipped_repositories,
            ..Default::default()
        });
    }
    if rewrites.is_empty() {
        return Err(MirrorError::Message(crate::tr!(
            crate::keys::mirror::MIRROR_NO_OFFICIAL_SOURCE
        )));
    }
    let backup_path = backup(*family)?;
    let mut report = ApplyReport {
        skipped_repositories,
        ..Default::default()
    };
    for (file, rewritten) in &rewrites {
        if let Err(error) = common::write_atomic(file, rewritten) {
            let _ = common::rollback(&backup_path);
            return Err(error);
        }
        report.changed_files += 1;
    }
    report.backup_path = Some(backup_path);
    Ok(report)
}

/// 显示受管 repo 文件内容。
pub(crate) fn show() -> Result<String, MirrorError> {
    let mut out = String::new();
    for file in managed_files()? {
        out.push_str(&format!("# {}\n", file.display()));
        out.push_str(&fs::read_to_string(&file)?);
        out.push('\n');
    }
    Ok(out)
}

/// 从备份恢复原 repo 文件，成功后删除备份目录。
fn restore(family: &Family) -> Result<(), MirrorError> {
    let dir = backup_dir(*family);
    if !dir.exists() {
        return Err(MirrorError::Message(crate::tr!(
            crate::keys::mirror::MIRROR_NO_BACKUP
        )));
    }
    common::restore_files(&dir, &paths().restore_root)?;
    fs::remove_dir_all(&dir)?;
    Ok(())
}

#[cfg(all(test, feature = "test-support"))]
mod apply_tests {
    use std::path::Path;

    use super::*;
    use crate::mirror::test_support::TestPathsGuard;

    fn temp_root(tag: &str) -> PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH};
        let temp = std::env::temp_dir().join(format!(
            "lkit-mirror-{tag}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&temp);
        temp
    }

    fn paths_guard(temp: &Path, repos_dir: &Path) -> TestPathsGuard {
        TestPathsGuard::set(crate::mirror::MirrorPaths {
            os_release: temp.join("etc/os-release"),
            backup_root: temp.join("var/lib/lkit/mirror-backup"),
            apt_sources_list: temp.join("etc/apt/sources.list"),
            apt_sources_list_d: temp.join("etc/apt/sources.list.d"),
            dnf_repos_dir: repos_dir.to_path_buf(),
            pacman_mirrorlist: temp.join("etc/pacman.d/mirrorlist"),
            restore_root: temp.to_path_buf(),
            allow_non_root: true,
        })
    }

    fn rocky_host() -> Host {
        Host {
            family: Family::Rocky,
            codename: None,
        }
    }

    fn repo_file(temp: &Path, content: &str) -> PathBuf {
        let repo = temp.join("etc/yum.repos.d/rocky.repo");
        fs::create_dir_all(repo.parent().unwrap()).unwrap();
        fs::write(&repo, content).unwrap();
        repo
    }

    #[test]
    fn apply_on_same_mirror_is_a_successful_noop() {
        let temp = temp_root("dnf-mirror-noop");
        let content = concat!(
            "[baseos]\n",
            "baseurl=https://mirrors.tuna.tsinghua.edu.cn/rockylinux/$releasever/BaseOS/$basearch/os/\n",
        );
        let repo = repo_file(&temp, content);
        let _guard = paths_guard(&temp, repo.parent().unwrap());
        let host = rocky_host();
        let report = apply(&host.family, MirrorName::Tuna).unwrap();
        assert_eq!(report.changed_files, 0);
        assert!(report.backup_path.is_none());
        assert_eq!(fs::read_to_string(&repo).unwrap(), content);
        assert!(
            !temp.join("var/lib/lkit/mirror-backup/rocky").exists(),
            "a no-op must not keep a backup"
        );
        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn apply_without_recognized_urls_is_an_error() {
        let temp = temp_root("dnf-unknown-only");
        let content = concat!(
            "[custom]\n",
            "baseurl=https://repo.internal.example.com/rockylinux/$releasever/BaseOS/$basearch/os/\n",
        );
        let repo = repo_file(&temp, content);
        let _guard = paths_guard(&temp, repo.parent().unwrap());
        let host = rocky_host();
        assert!(apply(&host.family, MirrorName::Tuna).is_err());
        assert_eq!(fs::read_to_string(&repo).unwrap(), content);
        assert!(!temp.join("var/lib/lkit/mirror-backup/rocky").exists());
        let _ = fs::remove_dir_all(&temp);
    }
}
