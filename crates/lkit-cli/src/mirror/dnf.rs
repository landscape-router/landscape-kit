use std::fs;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use super::{ApplyReport, Family, Host, MirrorError, MirrorName, backup_dir, paths};

/// 受管 dnf/yum 仓库文件：`/etc/yum.repos.d/*.repo`。
pub(crate) fn managed_files() -> Result<Vec<PathBuf>, MirrorError> {
    let entries = fs::read_dir(&paths().dnf_repos_dir).map_err(|_| {
        MirrorError::Message(crate::tr!(crate::keys::mirror::MIRROR_NO_SOURCE_FILES).into())
    })?;
    let mut files: Vec<_> = entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry.file_type().is_ok_and(|kind| kind.is_file())
                && entry.file_name().to_string_lossy().ends_with(".repo")
        })
        .map(|entry| entry.path())
        .collect();
    if files.is_empty() {
        return Err(MirrorError::Message(
            crate::tr!(crate::keys::mirror::MIRROR_NO_SOURCE_FILES).into(),
        ));
    }
    files.sort();
    Ok(files)
}

/// 各家族官方主机到镜像路径的映射。`from` 是官方 URL 中主机+路径的起始，
fn dnf_paths(family: Family) -> &'static [(&'static str, &'static str)] {
    match family {
        Family::Centos7 => &[("mirror.centos.org/centos", "centos")],
        Family::CentosStream => &[("mirror.stream.centos.org", "centos-stream")],
        Family::Fedora => &[
            ("download.fedoraproject.org/pub/epel", "epel"),
            ("download.fedoraproject.org/pub/fedora", "fedora"),
        ],
        Family::Rocky => &[("dl.rockylinux.org/$contentdir", "rockylinux")],
        Family::Alma => &[("repo.almalinux.org/almalinux", "almalinux")],
        _ => &[],
    }
}

/// 生成替换对：
///
/// - 官方主机 URL → 目标镜像；
/// - 其他已识别镜像（`RECOGNIZED_MIRROR_HOSTS`）的同路径 URL → 目标镜像，
///   实现 TUNA/阿里云/USTC 之间互转；
/// - `Official` 走 [`official_pairs`]，把所有已识别镜像映射回官方主机。
fn replacement_pairs(family: Family, mirror: MirrorName) -> Vec<(String, String)> {
    let Some(target) = super::mirror_host(mirror) else {
        return official_pairs(family);
    };
    let paths = dnf_paths(family);
    let mut pairs: Vec<(String, String)> = paths
        .iter()
        .map(|(from, path)| ((*from).to_string(), format!("{target}/{path}")))
        .collect();
    for other in super::RECOGNIZED_MIRROR_HOSTS {
        if other == target {
            continue;
        }
        pairs.extend(
            paths
                .iter()
                .map(|(_, path)| (format!("{other}/{path}"), format!("{target}/{path}"))),
        );
    }
    pairs
}

fn official_pairs(family: Family) -> Vec<(String, String)> {
    let targets = dnf_paths(family);
    super::RECOGNIZED_MIRROR_HOSTS
        .iter()
        .flat_map(|mirror| {
            targets
                .iter()
                .map(move |(official, path)| (format!("{mirror}/{path}"), official.to_string()))
        })
        .collect()
}

/// 内容是否已处于目标状态：目标镜像（或官方）主机路径已存在。
fn already_target(content: &str, family: Family, mirror: MirrorName) -> bool {
    let Some(target) = super::mirror_host(mirror) else {
        // Official：内容中已有官方主机即视为已处于官方源。
        return dnf_paths(family)
            .iter()
            .any(|(from, _)| super::apt::contains_host(content, from));
    };
    dnf_paths(family)
        .iter()
        .any(|(_, path)| super::apt::contains_host(content, &format!("{target}/{path}")))
}

/// 一个仓库块的摘要。`has_baseurl` 决定该块是否可安全转换。
#[derive(Debug)]
struct Block {
    has_baseurl: bool,
}

/// `[section]` 边界判定：`trimmed` 形如 `[name]`，或其注释形式 `# [name]`
/// （整块被禁用时常注释掉节头）。返回该行是否被注释。
fn is_section_header(trimmed: &str) -> Option<bool> {
    let body = trimmed
        .strip_prefix('#')
        .map(str::trim_start)
        .unwrap_or(trimmed);
    (body.starts_with('[') && body.ends_with(']')).then(|| trimmed.starts_with('#'))
}

/// 解析 `.repo` 文件为块列表。`[section]` 之前的行属于空名块（全局配置）；
/// 被注释的 `# [section]` 开启一个禁用块，其中的 `# baseurl=` 不参与转换。
fn parse_blocks(content: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut has_baseurl = false;
    let mut commented_section = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(commented) = is_section_header(trimmed) {
            blocks.push(Block { has_baseurl });
            has_baseurl = false;
            commented_section = commented;
        } else if !commented_section && is_baseurl_line(trimmed) {
            has_baseurl = true;
        }
    }
    blocks.push(Block { has_baseurl });
    blocks
}

fn is_baseurl_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    let stripped = trimmed.strip_prefix('#').unwrap_or(trimmed).trim_start();
    stripped.starts_with("baseurl=")
}

/// 一行是否是需要禁用的 mirrorlist/metalink。
fn is_mirrorlist_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("mirrorlist=") || trimmed.starts_with("metalink=")
}

/// 对 `.repo` 文件做镜像转换。
///
/// 只转换包含 `baseurl=`（含被注释的 `# baseurl=`）的块：解注释并重写
/// baseurl 主机，同时注释掉 `mirrorlist=`/`metalink=`。没有 baseurl 的块
/// 原样保留并计入跳过，避免把仓库改成空配置。
pub(crate) fn rewrite(content: &str, family: Family, mirror: MirrorName) -> Option<DnfRewrite> {
    let blocks = parse_blocks(content);
    let pairs = replacement_pairs(family, mirror);
    let mut current_block = 0usize;
    let mut rewritten = String::new();
    let mut changed = false;
    let mut skipped = 0usize;
    for line in content.lines() {
        let trimmed = line.trim();
        if is_section_header(trimmed).is_some() {
            current_block += 1;
        }
        let (next, modified) = match blocks.get(current_block) {
            Some(block) if block.has_baseurl => transform_line(line, &pairs),
            _ => (line.to_string(), false),
        };
        changed |= modified;
        rewritten.push_str(&next);
        rewritten.push('\n');
    }
    for block in blocks.iter().skip(1) {
        if !block.has_baseurl {
            skipped += 1;
        }
    }
    changed.then_some(DnfRewrite {
        content: rewritten,
        skipped_repositories: skipped,
    })
}

pub(crate) struct DnfRewrite {
    pub content: String,
    pub skipped_repositories: usize,
}

/// 转换单行：解注释并重写 baseurl，或注释 mirrorlist/metalink。
fn transform_line(line: &str, pairs: &[(String, String)]) -> (String, bool) {
    let trimmed = line.trim_start();
    if is_baseurl_line(trimmed) {
        let indent = &line[..line.len() - trimmed.len()];
        let uncommented = trimmed.trim_start_matches('#').trim_start();
        let Some((key, url)) = uncommented.split_once('=') else {
            return (line.to_string(), false);
        };
        let rewritten_url = replace_host(url, pairs);
        let changed = url != rewritten_url;
        return (format!("{indent}{key}={rewritten_url}"), changed);
    }
    if is_mirrorlist_line(trimmed) {
        return (format!("#lkit-mirror: {line}"), true);
    }
    (line.to_string(), false)
}

/// 依次替换 `url` 中出现的所有 `from`（要求主机边界），`to` 来自映射表。
fn replace_host(url: &str, pairs: &[(String, String)]) -> String {
    let mut rewritten = url.to_string();
    for (from, to) in pairs {
        if rewritten.contains(from.as_str()) {
            rewritten = super::apt::replace_host(&rewritten, from, to);
        }
    }
    rewritten
}

/// 备份当前全部受管 repo 文件。
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

/// 把 dnf/yum 仓库切换到指定镜像。失败时从刚创建的备份回滚已修改的文件。
pub(crate) fn apply(host: &Host, mirror: MirrorName) -> Result<ApplyReport, MirrorError> {
    let backup_path = backup(host)?;
    let files = managed_files()?;
    let mut report = ApplyReport::default();
    let mut already_matched = false;
    for file in files {
        let original = match fs::read_to_string(&file) {
            Ok(content) => content,
            Err(error) => {
                let _ = super::apt::rollback(&backup_path);
                return Err(error.into());
            }
        };
        let Some(rewritten) = rewrite(&original, host.family, mirror) else {
            already_matched |= already_target(&original, host.family, mirror);
            continue;
        };
        if let Err(error) = write_atomic(&file, &rewritten.content) {
            let _ = super::apt::rollback(&backup_path);
            return Err(error);
        }
        report.changed_files += 1;
        report.skipped_repositories += rewritten.skipped_repositories;
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
pub(crate) fn restore(host: &Host) -> Result<(), MirrorError> {
    let dir = backup_dir(host.family);
    if !dir.exists() {
        return Err(MirrorError::Message(
            crate::tr!(crate::keys::mirror::MIRROR_NO_BACKUP).into(),
        ));
    }
    super::apt::restore_files(&dir, &paths().restore_root)?;
    fs::remove_dir_all(&dir)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_blocks_and_flags_baseurl() {
        let content = concat!(
            "name=global\n",
            "[baseos]\n",
            "baseurl=http://mirror.stream.centos.org/$stream/BaseOS/$basearch/os/\n",
            "[appstream]\n",
            "mirrorlist=https://mirrorlist.centos.org/?repo=appstream\n",
            "#baseurl=http://mirror.stream.centos.org/$stream/AppStream/$basearch/os/\n",
        );
        let blocks = parse_blocks(content);
        assert_eq!(blocks.len(), 3);
        assert!(!blocks[0].has_baseurl);
        assert!(blocks[1].has_baseurl);
        assert!(blocks[2].has_baseurl);
    }

    #[test]
    fn rewrites_centos_stream_to_tuna_and_disables_mirrorlist() {
        let content = concat!(
            "[baseos]\n",
            "name=CentOS Stream $releasever - BaseOS\n",
            "#baseurl=http://mirror.stream.centos.org/$stream/BaseOS/$basearch/os/\n",
            "metalink=https://mirrors.centos.org/metalink?repo=centos-baseos-$stream\n",
            "[appstream]\n",
            "name=CentOS Stream $releasever - AppStream\n",
            "mirrorlist=https://mirrorlist.centos.org/?repo=appstream\n",
            "#baseurl=http://mirror.stream.centos.org/$stream/AppStream/$basearch/os/\n",
        );
        let rewritten = rewrite(content, Family::CentosStream, MirrorName::Tuna).unwrap();
        assert!(rewritten.content.contains(
            "baseurl=http://mirrors.tuna.tsinghua.edu.cn/centos-stream/$stream/BaseOS/$basearch/os/"
        ));
        assert!(
            rewritten
                .content
                .contains("#lkit-mirror: metalink=https://mirrors.centos.org/metalink")
        );
        assert!(
            rewritten
                .content
                .contains("#lkit-mirror: mirrorlist=https://mirrorlist.centos.org/")
        );
        assert!(!rewritten.content.contains("mirror.stream.centos.org"));
        assert_eq!(rewritten.skipped_repositories, 0);
    }

    #[test]
    fn skips_blocks_without_baseurl() {
        let content = concat!(
            "[extras]\n",
            "name=Extra\n",
            "mirrorlist=https://mirrorlist.centos.org/?repo=extras\n",
        );
        let rewritten = rewrite(content, Family::Centos7, MirrorName::Aliyun);
        assert!(
            rewritten.is_none(),
            "no baseurl anywhere means no file change"
        );
    }

    #[test]
    fn leaves_commented_sections_untouched() {
        let content = concat!(
            "[baseos]\n",
            "#baseurl=http://mirror.stream.centos.org/$stream/BaseOS/$basearch/os/\n",
            "# [disabled]\n",
            "# baseurl=https://repo.internal.example.com/disabled/os/\n",
        );
        let rewritten = rewrite(content, Family::CentosStream, MirrorName::Tuna).unwrap();
        assert!(
            rewritten.content.contains(
                "baseurl=http://mirrors.tuna.tsinghua.edu.cn/centos-stream/$stream/BaseOS/"
            )
        );
        assert!(
            rewritten.content.contains(
                "# [disabled]\n# baseurl=https://repo.internal.example.com/disabled/os/\n"
            ),
            "a commented-out section must keep its commented baseurl"
        );
    }

    #[test]
    fn rewrites_fedora_epel_and_fedora() {
        let content = concat!(
            "[fedora]\n",
            "metalink=https://mirrors.fedoraproject.org/metalink?repo=fedora-$releasever\n",
            "#baseurl=https://download.fedoraproject.org/pub/fedora/linux/releases/$releasever/Everything/$basearch/os/\n",
            "[epel]\n",
            "metalink=https://mirrors.fedoraproject.org/metalink?repo=epel-$releasever\n",
            "# baseurl=https://download.fedoraproject.org/pub/epel/$releasever/Everything/$basearch/\n",
        );
        let rewritten = rewrite(content, Family::Fedora, MirrorName::Ustc).unwrap();
        assert!(
            rewritten
                .content
                .contains("baseurl=https://mirrors.ustc.edu.cn/fedora/linux/releases/$releasever")
        );
        assert!(
            rewritten
                .content
                .contains("baseurl=https://mirrors.ustc.edu.cn/epel/$releasever")
        );
        assert!(rewritten.content.contains("#lkit-mirror: metalink="));
    }

    #[test]
    fn rewrites_rocky_and_alma() {
        let rocky = concat!(
            "[baseos]\n",
            "mirrorlist=https://mirrorlist.rockylinux.org/mirrorlist?repo=baseos-$releasever\n",
            "#baseurl=http://dl.rockylinux.org/$contentdir/$releasever/BaseOS/$basearch/os/\n",
        );
        let rewritten = rewrite(rocky, Family::Rocky, MirrorName::Aliyun).unwrap();
        assert!(rewritten.content.contains(
            "baseurl=http://mirrors.aliyun.com/rockylinux/$releasever/BaseOS/$basearch/os/"
        ));

        let alma = concat!(
            "[baseos]\n",
            "mirrorlist=https://mirrors.almalinux.org/mirrorlist?repo=baseos-$releasever\n",
            "# baseurl=https://repo.almalinux.org/almalinux/$releasever/BaseOS/$basearch/os/\n",
        );
        let rewritten = rewrite(alma, Family::Alma, MirrorName::Tuna).unwrap();
        assert!(rewritten
            .content
            .contains("baseurl=https://mirrors.tuna.tsinghua.edu.cn/almalinux/$releasever/BaseOS/$basearch/os/"));
    }

    #[test]
    fn official_restores_centos7_host() {
        let content = concat!(
            "[base]\n",
            "baseurl=https://mirrors.aliyun.com/centos/$releasever/os/$basearch/\n",
        );
        let rewritten = rewrite(content, Family::Centos7, MirrorName::Official).unwrap();
        assert!(
            rewritten
                .content
                .contains("baseurl=https://mirror.centos.org/centos/$releasever/os/$basearch/")
        );
    }

    #[test]
    fn leaves_custom_hosts_untouched() {
        let content = concat!(
            "[custom]\n",
            "baseurl=https://repo.example.com/centos/$releasever/os/$basearch/\n",
        );
        let rewritten = rewrite(content, Family::Centos7, MirrorName::Tuna);
        assert!(
            rewritten.is_none(),
            "no known official host means no change"
        );
    }

    #[test]
    fn mirror_on_mirror_is_a_noop() {
        let content = concat!(
            "[baseos]\n",
            "baseurl=https://mirrors.tuna.tsinghua.edu.cn/centos-stream/$stream/BaseOS/$basearch/os/\n",
        );
        let rewritten = rewrite(content, Family::CentosStream, MirrorName::Tuna);
        assert!(rewritten.is_none());
    }

    #[test]
    fn switches_between_recognized_mirrors() {
        let content = concat!(
            "[baseos]\n",
            "baseurl=https://mirrors.ustc.edu.cn/centos-stream/$stream/BaseOS/$basearch/os/\n",
            "[appstream]\n",
            "baseurl=https://mirrors.ustc.edu.cn/centos-stream/$stream/AppStream/$basearch/os/\n",
        );
        let rewritten = rewrite(content, Family::CentosStream, MirrorName::Aliyun).unwrap();
        assert!(
            rewritten
                .content
                .contains("baseurl=https://mirrors.aliyun.com/centos-stream/$stream/BaseOS/")
        );
        assert!(
            rewritten
                .content
                .contains("baseurl=https://mirrors.aliyun.com/centos-stream/$stream/AppStream/")
        );
        assert!(!rewritten.content.contains("mirrors.ustc.edu.cn"));
    }

    #[test]
    fn keeps_custom_hosts_when_switching_between_mirrors() {
        let content = concat!(
            "[baseos]\n",
            "baseurl=https://mirrors.tuna.tsinghua.edu.cn/rockylinux/$releasever/BaseOS/$basearch/os/\n",
            "[custom]\n",
            "baseurl=https://repo.internal.example.com/rockylinux/$releasever/BaseOS/$basearch/os/\n",
        );
        let rewritten = rewrite(content, Family::Rocky, MirrorName::Aliyun).unwrap();
        assert!(
            rewritten
                .content
                .contains("baseurl=https://mirrors.aliyun.com/rockylinux/$releasever/")
        );
        assert!(
            rewritten
                .content
                .contains("repo.internal.example.com/rockylinux")
        );
    }

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
            let report = apply(&rocky_host(), MirrorName::Tuna).unwrap();
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
            assert!(apply(&rocky_host(), MirrorName::Tuna).is_err());
            assert_eq!(fs::read_to_string(&repo).unwrap(), content);
            assert!(!temp.join("var/lib/lkit/mirror-backup/rocky").exists());
            let _ = fs::remove_dir_all(&temp);
        }
    }
}
