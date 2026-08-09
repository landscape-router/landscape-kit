use std::fs;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use super::{ApplyReport, Family, Host, MirrorError, MirrorName, backup_dir, paths};

/// 受管 apt 源文件：`sources.list` 与 `sources.list.d/` 下的全部普通文件。
pub(crate) fn managed_files() -> Result<Vec<PathBuf>, MirrorError> {
    let mut files = Vec::new();
    if paths().apt_sources_list.is_file() {
        files.push(paths().apt_sources_list.clone());
    }
    if let Ok(entries) = fs::read_dir(&paths().apt_sources_list_d) {
        let mut names: Vec<_> = entries
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .map(|entry| entry.path())
            .collect();
        names.sort();
        files.extend(names);
    }
    if files.is_empty() {
        return Err(MirrorError::Message(
            crate::tr!(crate::keys::mirror::MIRROR_NO_SOURCE_FILES).into(),
        ));
    }
    Ok(files)
}

/// 各家族主仓库官方主机+路径到镜像路径的映射。
///
/// 官方与镜像主机可能互为前缀（如 `.../ubuntu` 是 `.../ubuntu-ports` 的前缀），
/// 因此列表顺序即替换顺序：具体路径（`-ports`）必须排在通用路径之前。
/// Ubuntu 的 security 内容与主仓库合并镜像，`security.ubuntu.com/ubuntu` 归入
/// 主仓库，始终替换。
fn apt_paths(family: Family) -> Vec<(&'static str, &'static str)> {
    match family {
        Family::Debian => vec![
            ("deb.debian.org/debian-backports", "/debian-backports"),
            ("deb.debian.org/debian-ports", "/debian-ports"),
            ("deb.debian.org/debian", "/debian"),
        ],
        Family::Ubuntu => vec![
            ("ports.ubuntu.com/ubuntu-ports", "/ubuntu-ports"),
            ("archive.ubuntu.com/ubuntu-ports", "/ubuntu-ports"),
            ("archive.ubuntu.com/ubuntu", "/ubuntu"),
            ("security.ubuntu.com/ubuntu", "/ubuntu"),
        ],
        _ => Vec::new(),
    }
}

/// Debian 独立的 security 仓库。默认不替换（安全补丁时效性、部分镜像站不镜像
/// security），仅在显式要求时替换。Ubuntu 没有独立 security 路径。
fn apt_security_paths(family: Family) -> Vec<(&'static str, &'static str)> {
    match family {
        Family::Debian => vec![
            ("deb.debian.org/debian-security", "/debian-security"),
            ("security.debian.org/debian-security", "/debian-security"),
        ],
        _ => Vec::new(),
    }
}

/// 生成替换对：
///
/// - 官方主机 URL → 目标镜像（`deb.debian.org/debian` → `mirrors.tuna.../debian`）；
/// - 其他已识别镜像（`RECOGNIZED_MIRROR_HOSTS`）的同路径 URL → 目标镜像，
///   实现 TUNA/阿里云/USTC 之间互转；
/// - `replace_security` 时额外包含 Debian security 仓库（官方与已识别镜像的
///   `debian-security` 路径）；
/// - `Official` 走 [`official_pairs`]，把所有已识别镜像（含 security）映射回官方主机。
fn replacement_pairs(
    family: Family,
    mirror: MirrorName,
    replace_security: bool,
) -> Vec<(String, String)> {
    let Some(target) = super::mirror_host(mirror) else {
        return official_pairs(family);
    };
    let security = if replace_security {
        apt_security_paths(family)
    } else {
        Vec::new()
    };
    // security 先于主仓库（`mirrors.x/debian` 是 `mirrors.x/debian-security` 的前缀）。
    let paths: Vec<(&str, &str)> = security
        .iter()
        .chain(apt_paths(family).iter())
        .map(|(from, path)| (*from, *path))
        .collect();
    let mut pairs: Vec<(String, String)> = paths
        .iter()
        .map(|(from, path)| ((*from).to_string(), format!("{target}{path}")))
        .collect();
    for other in super::RECOGNIZED_MIRROR_HOSTS {
        if other == target {
            continue;
        }
        pairs.extend(
            paths
                .iter()
                .map(|(_, path)| (format!("{other}{path}"), format!("{target}{path}"))),
        );
    }
    pairs
}

/// 官方源恢复：把所有已识别的镜像主机路径（含 security）映射回官方主机。
fn official_pairs(family: Family) -> Vec<(String, String)> {
    let paths = apt_security_paths(family)
        .into_iter()
        .chain(apt_paths(family))
        .collect::<Vec<_>>();
    let mut pairs = Vec::new();
    for mirror in super::RECOGNIZED_MIRROR_HOSTS {
        for (official, path) in &paths {
            pairs.push((format!("{mirror}{path}"), (*official).to_string()));
        }
    }
    pairs
}

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

fn is_boundary(character: char) -> bool {
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

/// 内容是否已处于目标状态：目标镜像主机（或官方主机）的 URL 已存在。
pub(crate) fn already_target(content: &str, family: Family, mirror: MirrorName) -> bool {
    let Some(target) = super::mirror_host(mirror) else {
        // Official：内容中已有官方主机路径即视为已处于官方源。
        return apt_paths(family)
            .iter()
            .chain(apt_security_paths(family).iter())
            .any(|(from, _)| contains_host(content, from));
    };
    apt_paths(family)
        .iter()
        .chain(apt_security_paths(family).iter())
        .any(|(_, path)| contains_host(content, &format!("{target}{path}")))
}

/// 对整个源文件做 URL 重写。没有可替换内容时返回 `None`。
pub(crate) fn rewrite(
    content: &str,
    family: Family,
    mirror: MirrorName,
    replace_security: bool,
) -> Option<String> {
    let pairs = replacement_pairs(family, mirror, replace_security);
    let mut rewritten = content.to_string();
    let mut changed = false;
    for (from, to) in &pairs {
        let next = replace_host(&rewritten, from, to);
        changed |= next != rewritten;
        rewritten = next;
    }
    changed.then_some(rewritten)
}

/// 备份当前全部受管 apt 源文件到 `/var/lib/lkit/mirror-backup/<family>/`。
fn backup(host: &Host) -> Result<PathBuf, MirrorError> {
    let dir = backup_dir(host.family);
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

/// 把源文件切换到指定镜像。失败时从刚创建的备份回滚已修改的文件。
/// `replace_security` 控制 Debian 独立 security 仓库是否一并替换（默认不替换）。
pub(crate) fn apply(
    host: &Host,
    mirror: MirrorName,
    replace_security: bool,
) -> Result<ApplyReport, MirrorError> {
    let backup_path = backup(host)?;
    let files = managed_files()?;
    let mut report = ApplyReport::default();
    let mut already_matched = false;
    for file in files {
        let original = match fs::read_to_string(&file) {
            Ok(content) => content,
            Err(error) => {
                let _ = rollback(&backup_path);
                return Err(error.into());
            }
        };
        let Some(rewritten) = rewrite(&original, host.family, mirror, replace_security) else {
            already_matched |= already_target(&original, host.family, mirror);
            continue;
        };
        if let Err(error) = write_atomic(&file, &rewritten) {
            let _ = rollback(&backup_path);
            return Err(error);
        }
        report.changed_files += 1;
    }
    if report.changed_files == 0 {
        let _ = fs::remove_dir_all(&backup_path);
        if already_matched {
            return Ok(report);
        }
        return Err(MirrorError::Message(
            crate::tr!(crate::keys::mirror::MIRROR_NO_OFFICIAL_SOURCE).into(),
        ));
    }
    report.backup_path = Some(backup_path);
    Ok(report)
}

/// 把备份文件写回根目录（apply 失败时回滚）。
pub(crate) fn rollback(backup_path: &Path) -> Result<(), MirrorError> {
    restore_files(backup_path, &paths().restore_root)?;
    fs::remove_dir_all(backup_path)?;
    Ok(())
}

/// 原子写入：同目录临时文件 + rename，保留原文件权限位。
fn write_atomic(path: &Path, content: &str) -> Result<(), MirrorError> {
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

/// 显示受管 apt 源文件内容。
pub(crate) fn show() -> Result<String, MirrorError> {
    let mut out = String::new();
    for file in managed_files()? {
        out.push_str(&format!("# {}\n", file.display()));
        out.push_str(&fs::read_to_string(&file)?);
        out.push('\n');
    }
    Ok(out)
}

/// 从备份恢复原 apt 源文件，成功后删除备份目录。
pub(crate) fn restore(host: &Host) -> Result<(), MirrorError> {
    let dir = backup_dir(host.family);
    if !dir.exists() {
        return Err(MirrorError::Message(
            crate::tr!(crate::keys::mirror::MIRROR_NO_BACKUP).into(),
        ));
    }
    restore_files(&dir, &paths().restore_root)?;
    fs::remove_dir_all(&dir)?;
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

fn walk_files(dir: &Path) -> Result<Vec<PathBuf>, MirrorError> {
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
    fn rewrites_debian_one_line_sources_to_tuna() {
        let content = concat!(
            "deb http://deb.debian.org/debian bookworm main contrib\n",
            "deb-src http://deb.debian.org/debian bookworm main\n",
            "deb http://security.debian.org/debian-security bookworm-security main\n",
        );
        let rewritten = rewrite(content, Family::Debian, MirrorName::Tuna, false).unwrap();
        assert!(rewritten.contains("http://mirrors.tuna.tsinghua.edu.cn/debian bookworm"));
        assert!(!rewritten.contains("deb.debian.org"));
        assert!(rewritten.contains("security.debian.org"));
        assert!(rewritten.contains("deb-src"));
        assert_eq!(rewritten.matches("http://").count(), 3);
        assert!(
            !rewritten.contains("https://"),
            "the original scheme must be preserved"
        );
    }

    #[test]
    fn debian_security_is_replaced_only_when_requested() {
        let content = concat!(
            "deb http://deb.debian.org/debian bookworm main\n",
            "deb http://deb.debian.org/debian-security bookworm-security main\n",
        );
        let kept = rewrite(content, Family::Debian, MirrorName::Aliyun, false).unwrap();
        assert!(kept.contains("http://mirrors.aliyun.com/debian bookworm"));
        assert!(
            kept.contains("http://deb.debian.org/debian-security"),
            "security must stay official by default"
        );
        let replaced = rewrite(content, Family::Debian, MirrorName::Aliyun, true).unwrap();
        assert!(replaced.contains("http://mirrors.aliyun.com/debian-security"));
        assert!(!replaced.contains("deb.debian.org"));
    }

    #[test]
    fn ubuntu_security_is_always_replaced() {
        let content = concat!(
            "deb http://archive.ubuntu.com/ubuntu noble main\n",
            "deb http://security.ubuntu.com/ubuntu noble-security main\n",
        );
        for replace_security in [false, true] {
            let rewritten =
                rewrite(content, Family::Ubuntu, MirrorName::Tuna, replace_security).unwrap();
            assert!(
                rewritten.contains("http://mirrors.tuna.tsinghua.edu.cn/ubuntu noble-security"),
                "ubuntu security merges into the main mirror path regardless of the flag"
            );
            assert!(!rewritten.contains("security.ubuntu.com"));
        }
    }

    #[test]
    fn rewrites_ubuntu_ports_before_archive() {
        let content = concat!(
            "deb http://archive.ubuntu.com/ubuntu noble main universe\n",
            "deb http://security.ubuntu.com/ubuntu noble-security main\n",
            "deb http://ports.ubuntu.com/ubuntu-ports noble main\n",
        );
        let rewritten = rewrite(content, Family::Ubuntu, MirrorName::Ustc, false).unwrap();
        assert!(rewritten.contains("http://mirrors.ustc.edu.cn/ubuntu noble"));
        assert!(rewritten.contains("http://mirrors.ustc.edu.cn/ubuntu-ports noble"));
        assert!(!rewritten.contains("archive.ubuntu.com"));
        assert!(!rewritten.contains("security.ubuntu.com"));
        assert!(!rewritten.contains("ports.ubuntu.com"));
    }

    #[test]
    fn rewrites_deb822_sources_files() {
        let content = concat!(
            "Types: deb\n",
            "URIs: http://deb.debian.org/debian\n",
            "Suites: bookworm\n",
            "Components: main\n",
            "\n",
            "Types: deb\n",
            "URIs: http://deb.debian.org/debian-security\n",
            "Suites: bookworm-security\n",
        );
        let rewritten = rewrite(content, Family::Debian, MirrorName::Aliyun, true).unwrap();
        assert!(rewritten.contains("URIs: http://mirrors.aliyun.com/debian\n"));
        assert!(rewritten.contains("URIs: http://mirrors.aliyun.com/debian-security\n"));
    }

    #[test]
    fn official_restores_original_hosts() {
        let content = concat!(
            "deb https://mirrors.tuna.tsinghua.edu.cn/debian bookworm main\n",
            "deb https://mirrors.aliyun.com/debian-security bookworm-security main\n",
            "deb https://mirrors.ustc.edu.cn/ubuntu noble main\n",
        );
        let debian = rewrite(content, Family::Debian, MirrorName::Official, true).unwrap();
        assert!(debian.contains("https://deb.debian.org/debian bookworm"));
        assert!(debian.contains("https://deb.debian.org/debian-security"));
        assert!(
            debian.contains("mirrors.ustc.edu.cn/ubuntu"),
            "ubuntu host is untouched by debian rules"
        );
        let ubuntu = rewrite(content, Family::Ubuntu, MirrorName::Official, true).unwrap();
        assert!(ubuntu.contains("https://archive.ubuntu.com/ubuntu noble"));
        assert!(
            ubuntu.contains("mirrors.tuna.tsinghua.edu.cn/debian"),
            "debian host is untouched by ubuntu rules"
        );
    }

    #[test]
    fn official_on_official_content_is_a_noop() {
        let content = "deb https://deb.debian.org/debian bookworm main\n";
        let rewritten = rewrite(content, Family::Debian, MirrorName::Official, true);
        assert!(rewritten.is_none());
    }

    #[test]
    fn mirror_on_mirror_content_is_a_noop() {
        let content = "deb http://mirrors.tuna.tsinghua.edu.cn/debian bookworm main\n";
        let rewritten = rewrite(content, Family::Debian, MirrorName::Tuna, false);
        assert!(rewritten.is_none());
    }

    #[test]
    fn switches_between_recognized_mirrors() {
        let content = concat!(
            "deb http://mirrors.ustc.edu.cn/debian bookworm main\n",
            "deb-src http://mirrors.ustc.edu.cn/debian bookworm main\n",
            "deb http://mirrors.ustc.edu.cn/debian-security bookworm-security main\n",
        );
        let rewritten = rewrite(content, Family::Debian, MirrorName::Tuna, false).unwrap();
        assert!(rewritten.contains("http://mirrors.tuna.tsinghua.edu.cn/debian bookworm"));
        assert!(
            rewritten.contains("http://mirrors.ustc.edu.cn/debian-security"),
            "security stays on the previous mirror by default"
        );
        let replaced = rewrite(content, Family::Debian, MirrorName::Tuna, true).unwrap();
        assert!(replaced.contains("http://mirrors.tuna.tsinghua.edu.cn/debian-security"));
        assert!(!replaced.contains("mirrors.ustc.edu.cn"));
    }

    #[test]
    fn switches_ports_before_main_path_between_mirrors() {
        let content = concat!(
            "deb http://mirrors.aliyun.com/ubuntu-ports noble main\n",
            "deb http://mirrors.aliyun.com/ubuntu noble main\n",
        );
        let rewritten = rewrite(content, Family::Ubuntu, MirrorName::Ustc, false).unwrap();
        assert!(rewritten.contains("http://mirrors.ustc.edu.cn/ubuntu-ports noble"));
        assert!(rewritten.contains("http://mirrors.ustc.edu.cn/ubuntu noble"));
        assert!(!rewritten.contains("mirrors.aliyun.com"));
    }

    #[test]
    fn keeps_custom_hosts_when_switching_between_mirrors() {
        let content = concat!(
            "deb http://mirrors.aliyun.com/debian bookworm main\n",
            "deb http://repo.internal.example.com/debian bookworm main\n",
        );
        let rewritten = rewrite(content, Family::Debian, MirrorName::Ustc, false).unwrap();
        assert!(rewritten.contains("http://mirrors.ustc.edu.cn/debian bookworm"));
        assert!(rewritten.contains("repo.internal.example.com/debian"));
    }

    #[test]
    fn does_not_replace_host_name_substrings() {
        let content = "deb https://www.deb.debian.org/debian bookworm main\n";
        let rewritten = rewrite(content, Family::Debian, MirrorName::Tuna, false);
        assert!(
            rewritten.is_none(),
            "www.deb.debian.org must not match deb.debian.org"
        );
    }

    #[test]
    fn replaces_multiple_occurrences_per_line() {
        let content =
            "deb https://archive.ubuntu.com/ubuntu noble main https://archive.ubuntu.com/ubuntu\n";
        let rewritten = rewrite(content, Family::Ubuntu, MirrorName::Tuna, false).unwrap();
        assert_eq!(
            rewritten
                .matches("https://mirrors.tuna.tsinghua.edu.cn/ubuntu")
                .count(),
            2
        );
        assert_eq!(rewritten.matches("archive.ubuntu.com").count(), 0);
    }

    #[test]
    fn restore_writes_files_back_and_removes_backup() {
        let backup =
            std::env::temp_dir().join(format!("lkit-mirror-restore-{}", std::process::id()));
        let target =
            std::env::temp_dir().join(format!("lkit-mirror-target-{}", std::process::id()));
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

    /// `apply` 的 no-op 语义与 `TestPathsGuard` 全局覆盖必须串行执行（锁在
    #[cfg(all(test, feature = "test-support"))]
    mod apply_tests {
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

        fn paths_guard(temp: &Path, apt_sources: &Path) -> TestPathsGuard {
            TestPathsGuard::set(crate::mirror::MirrorPaths {
                os_release: temp.join("etc/os-release"),
                backup_root: temp.join("var/lib/lkit/mirror-backup"),
                apt_sources_list: apt_sources.to_path_buf(),
                apt_sources_list_d: temp.join("etc/apt/sources.list.d"),
                dnf_repos_dir: temp.join("etc/yum.repos.d"),
                pacman_mirrorlist: temp.join("etc/pacman.d/mirrorlist"),
                restore_root: temp.to_path_buf(),
                allow_non_root: true,
            })
        }

        fn debian_host() -> Host {
            Host {
                family: Family::Debian,
                codename: Some("bookworm".into()),
            }
        }

        fn sources_file(temp: &Path, content: &str) -> PathBuf {
            let sources = temp.join("etc/apt/sources.list");
            fs::create_dir_all(sources.parent().unwrap()).unwrap();
            fs::write(&sources, content).unwrap();
            sources
        }

        #[test]
        fn apply_on_official_sources_is_a_successful_noop() {
            let temp = temp_root("apt-official-noop");
            let content = concat!(
                "deb http://deb.debian.org/debian bookworm main\n",
                "deb http://deb.debian.org/debian-security bookworm-security main\n",
            );
            let sources = sources_file(&temp, content);
            let _guard = paths_guard(&temp, &sources);
            let report = apply(&debian_host(), MirrorName::Official, true).unwrap();
            assert_eq!(report.changed_files, 0);
            assert!(report.backup_path.is_none());
            assert_eq!(fs::read_to_string(&sources).unwrap(), content);
            assert!(
                !temp.join("var/lib/lkit/mirror-backup/debian").exists(),
                "a no-op must not keep a backup"
            );
            let _ = fs::remove_dir_all(&temp);
        }

        #[test]
        fn apply_on_same_mirror_is_a_successful_noop() {
            let temp = temp_root("apt-mirror-noop");
            let content = concat!(
                "deb https://mirrors.tuna.tsinghua.edu.cn/debian bookworm main\n",
                "deb https://mirrors.tuna.tsinghua.edu.cn/debian-security bookworm-security main\n",
            );
            let sources = sources_file(&temp, content);
            let _guard = paths_guard(&temp, &sources);
            let report = apply(&debian_host(), MirrorName::Tuna, false).unwrap();
            assert_eq!(report.changed_files, 0);
            assert!(report.backup_path.is_none());
            assert_eq!(fs::read_to_string(&sources).unwrap(), content);
            assert!(!temp.join("var/lib/lkit/mirror-backup/debian").exists());
            let _ = fs::remove_dir_all(&temp);
        }

        #[test]
        fn apply_without_recognized_urls_is_an_error() {
            let temp = temp_root("apt-unknown-only");
            let content = "deb http://repo.internal.example.com/debian bookworm main\n";
            let sources = sources_file(&temp, content);
            let _guard = paths_guard(&temp, &sources);
            assert!(apply(&debian_host(), MirrorName::Tuna, false).is_err());
            assert_eq!(fs::read_to_string(&sources).unwrap(), content);
            assert!(!temp.join("var/lib/lkit/mirror-backup/debian").exists());
            let _ = fs::remove_dir_all(&temp);
        }
    }
}
