use std::fs;
use std::path::{Path, PathBuf};

use parse::{
    already_target, comment_cdrom, convert_cdrom, parse_sources, parse_sources_with_diagnostics,
    rewrite, synth_lines,
};

use super::backend::SourcesBackend;
use super::common;
use super::{ApplyReport, Fallback, Family, Host, MirrorError, MirrorName, backup_dir, paths};

pub(crate) mod parse;

/// apt 家族（Debian/Ubuntu）软件源后端。
pub(crate) struct AptBackend {
    family: Family,
    codename: Option<String>,
}

impl AptBackend {
    pub(crate) fn new(host: &Host) -> Self {
        Self {
            family: host.family,
            codename: host.codename.clone(),
        }
    }
}

impl SourcesBackend for AptBackend {
    fn show(&self) -> Result<String, MirrorError> {
        show()
    }

    fn apply(
        &self,
        mirror: MirrorName,
        replace_security: bool,
        disable_cdrom: bool,
    ) -> Result<ApplyReport, MirrorError> {
        apply(
            &self.family,
            &self.codename,
            mirror,
            replace_security,
            disable_cdrom,
        )
    }

    fn restore(&self) -> Result<(), MirrorError> {
        restore(&self.family)
    }
}

/// 受管 apt 源文件：`sources.list` 与 `sources.list.d/` 下的全部普通文件。
pub(crate) fn managed_files() -> Result<Vec<PathBuf>, MirrorError> {
    let mut files = Vec::new();
    if paths().apt_sources_list.is_file() {
        files.push(paths().apt_sources_list.clone());
    }
    if let Ok(entries) = fs::read_dir(&paths().apt_sources_list_d) {
        let mut names: Vec<_> = entries
            .filter_map(|entry| entry.ok())
            // `fs::metadata` 跟随符号链接：apt 会读取 symlink 指向的文件，
            // 这里必须一致（`DirEntry::metadata` 不跟随）。
            .filter(|entry| fs::metadata(entry.path()).is_ok_and(|metadata| metadata.is_file()))
            .map(|entry| entry.path())
            .collect();
        names.sort();
        files.extend(names);
    }
    if files.is_empty() {
        return Err(MirrorError::Message(crate::tr!(
            crate::keys::mirror::MIRROR_NO_SOURCE_FILES
        )));
    }
    Ok(files)
}

/// 备份当前全部受管 apt 源文件到 `/var/lib/lkit/mirror-backup/<family>/`。
fn backup(family: Family, files: &[PathBuf]) -> Result<PathBuf, MirrorError> {
    let dir = backup_dir(family);
    if dir.exists() {
        fs::remove_dir_all(&dir)?;
    }
    fs::create_dir_all(&dir)?;
    for file in files {
        // 相对 `restore_root` 保存，恢复时原样写回（生产 root 为 `/`）。
        let relative = file.strip_prefix(&paths().restore_root).unwrap_or(file);
        let target = dir.join(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(file, &target)?;
    }
    Ok(dir)
}

/// 把源文件切换到指定镜像。失败时从刚创建的备份回滚已修改的文件。
/// `replace_security` 控制 Debian 独立 security 仓库是否一并替换（默认不替换）；
/// `disable_cdrom` 控制是否把启用的 `deb cdrom:` 条目注释掉（默认注释，避免换源后
/// apt 仍提示插入安装介质）。
///
/// 先在内存里计算全部重写结果，只有确实要改动文件时才创建备份；no-op 不触碰
/// 已有的备份目录，因此重复执行同一目标不会丢掉上一轮保存的原源。
///
/// 没有任何条目可重写且未处于目标状态时走 [`fallback`]：未禁用 cdrom 时优先把
/// 启用的 `deb cdrom:` 条目转换为镜像（保留 suites/components）；禁用时注释
/// cdrom 条目并合成新条目追加。sources.list 为空、只有注释、或系统里完全没有
/// 源文件（sources.list 与 sources.list.d 都不存在）都进入该兜底，不再报错。
fn apply(
    family: &Family,
    codename: &Option<String>,
    mirror: MirrorName,
    replace_security: bool,
    disable_cdrom: bool,
) -> Result<ApplyReport, MirrorError> {
    // 没有任何源文件时不报错，交给 fallback 直接创建镜像源条目。
    let files = managed_files().unwrap_or_default();
    // 换源前先做格式检查：统计无法识别的行（只诊断，不改动这些行）。
    let unrecognized_lines = files
        .iter()
        .filter_map(|file| fs::read_to_string(file).ok())
        .map(|content| parse_sources_with_diagnostics(&content).1.len())
        .sum();
    // 只读阶段：先算出每个文件的重写结果，不写盘。
    let mut rewrites: Vec<(PathBuf, String)> = Vec::new();
    let mut already_matched = false;
    let mut cdrom_commented = 0usize;
    for file in &files {
        let original = fs::read_to_string(file)?;
        let mut changed = rewrite(&original, *family, mirror, replace_security);
        if disable_cdrom {
            let base = changed.as_deref().unwrap_or(&original);
            if let Some((commented, count)) = comment_cdrom(base) {
                // 注释后文件仍有启用条目 → 直接写盘；否则（原本只有 cdrom 源）
                // 留给兜底处理（注释 cdrom + 合成镜像条目）。
                if parse_sources(&commented).iter().any(|entry| entry.enabled) {
                    changed = Some(commented);
                    cdrom_commented += count;
                }
            }
        }
        match changed {
            Some(rewritten) => rewrites.push((file.clone(), rewritten)),
            None => already_matched |= already_target(&original, *family, mirror),
        }
    }
    if rewrites.is_empty() && already_matched {
        return Ok(ApplyReport {
            unrecognized_lines,
            ..Default::default()
        });
    }
    let backup_path = backup(*family, &files)?;
    let mut report = ApplyReport {
        unrecognized_lines,
        ..Default::default()
    };
    if !rewrites.is_empty() {
        for (file, rewritten) in &rewrites {
            if let Err(error) = common::write_atomic(file, rewritten) {
                let _ = common::rollback(&backup_path);
                return Err(error);
            }
            report.changed_files += 1;
        }
        report.backup_path = Some(backup_path);
        report.cdrom_commented = cdrom_commented;
        return Ok(report);
    }
    let mut result = fallback(
        *family,
        codename,
        mirror,
        replace_security,
        disable_cdrom,
        &backup_path,
    )?;
    result.unrecognized_lines = unrecognized_lines;
    Ok(result)
}

/// 只读检查所有受管 apt 源文件的格式，返回每个含问题的文件与异常行列表。
/// 没有任何源文件时返回空报告（与 `apply` 的兜底语义一致，不算错误）。
pub(crate) fn check_format() -> Result<Vec<(PathBuf, Vec<parse::ParseIssue>)>, MirrorError> {
    let mut report = Vec::new();
    for file in managed_files().unwrap_or_default() {
        let content = fs::read_to_string(&file)?;
        let issues = parse_sources_with_diagnostics(&content).1;
        if !issues.is_empty() {
            report.push((file, issues));
        }
    }
    Ok(report)
}

/// 受管源文件中是否存在启用的 `deb cdrom:` 条目（交互询问是否注释 CD 源时用）。
pub(crate) fn has_enabled_cdrom() -> bool {
    managed_files()
        .unwrap_or_default()
        .iter()
        .filter_map(|file| fs::read_to_string(file).ok())
        .any(|content| {
            parse_sources(&content)
                .iter()
                .any(|entry| entry.enabled && entry.is_cdrom())
        })
}
/// 兜底：未禁用 cdrom 时把启用的 cdrom 条目转换为镜像（保留 suites/components）；
/// 禁用时注释 cdrom 条目并合成新条目追加；否则用代号合成新条目追加。
fn fallback(
    family: Family,
    codename: &Option<String>,
    mirror: MirrorName,
    replace_security: bool,
    disable_cdrom: bool,
    backup_path: &Path,
) -> Result<ApplyReport, MirrorError> {
    // 1) 处理 cdrom 条目：未禁用时转换为镜像（保留 suites/components）；
    //    禁用时注释掉并合成镜像条目追加（避免注释后系统没有任何可用源）。
    for file in managed_files().unwrap_or_default() {
        let original = fs::read_to_string(&file)?;
        if disable_cdrom {
            let Some((commented, _count)) = comment_cdrom(&original) else {
                continue;
            };
            let Some(codename) = codename else {
                let _ = fs::remove_dir_all(backup_path);
                return Err(MirrorError::Message(crate::tr!(
                    crate::keys::mirror::MIRROR_NO_OFFICIAL_SOURCE
                )));
            };
            let lines = synth_lines(family, mirror, replace_security, codename);
            let mut content = commented;
            if !content.ends_with('\n') {
                content.push('\n');
            }
            content.push_str(&lines);
            if let Err(error) = common::write_atomic(&file, &content) {
                let _ = common::rollback(backup_path);
                return Err(error);
            }
            return Ok(ApplyReport {
                changed_files: 1,
                fallback: Some(Fallback::CdromDisabled),
                backup_path: Some(backup_path.to_path_buf()),
                ..Default::default()
            });
        }
        if let Some(rewritten) = convert_cdrom(&original, family, mirror) {
            if let Err(error) = common::write_atomic(&file, &rewritten) {
                let _ = common::rollback(backup_path);
                return Err(error);
            }
            return Ok(ApplyReport {
                changed_files: 1,
                fallback: Some(Fallback::CdromConverted),
                backup_path: Some(backup_path.to_path_buf()),
                ..Default::default()
            });
        }
    }
    // 2) 合成并追加新条目。
    let Some(codename) = codename else {
        let _ = fs::remove_dir_all(backup_path);
        return Err(MirrorError::Message(crate::tr!(
            crate::keys::mirror::MIRROR_NO_OFFICIAL_SOURCE
        )));
    };
    let lines = synth_lines(family, mirror, replace_security, codename);
    // 写入 sources.list；不存在则创建 sources.list.d/lkit-mirror.list，并在备份
    // 目录放空占位，保证 restore 后不残留（空 .list 对 apt 无影响）。
    let target = if paths().apt_sources_list.is_file() {
        paths().apt_sources_list.clone()
    } else {
        let created = paths().apt_sources_list_d.join("lkit-mirror.list");
        if let Some(parent) = created.parent() {
            fs::create_dir_all(parent)?;
        }
        let relative = created
            .strip_prefix(&paths().restore_root)
            .unwrap_or(&created);
        let placeholder = backup_path.join(relative);
        if let Some(parent) = placeholder.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&placeholder, "")?;
        created
    };
    let mut content = fs::read_to_string(&target).unwrap_or_default();
    if !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(&lines);
    if let Err(error) = common::write_atomic(&target, &content) {
        let _ = common::rollback(backup_path);
        return Err(error);
    }
    Ok(ApplyReport {
        changed_files: 1,
        fallback: Some(Fallback::SourceAdded),
        backup_path: Some(backup_path.to_path_buf()),
        ..Default::default()
    })
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
    fn apply_reports_unrecognized_lines() {
        let temp = temp_root("apt-unrecognized");
        let content = concat!(
            "deb http://deb.debian.org/debian bookworm main\n",
            "this line is not a source\n",
        );
        let sources = sources_file(&temp, content);
        let _guard = paths_guard(&temp, &sources);
        let host = debian_host();
        let report = apply(&host.family, &host.codename, MirrorName::Tuna, false, true).unwrap();
        assert_eq!(report.changed_files, 1);
        assert_eq!(report.unrecognized_lines, 1);
        let rewritten = fs::read_to_string(&sources).unwrap();
        assert!(
            rewritten.contains("mirrors.tuna.tsinghua.edu.cn/debian"),
            "recognized entries are rewritten: {rewritten}"
        );
        assert!(
            rewritten.contains("this line is not a source"),
            "unrecognized lines are kept byte-for-byte: {rewritten}"
        );
        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn check_format_reports_issues_per_file() {
        let temp = temp_root("apt-check-format");
        let sources = sources_file(
            &temp,
            "deb http://deb.debian.org/debian bookworm main\nnot a source line\n",
        );
        let _guard = paths_guard(&temp, &sources);
        let report = check_format().unwrap();
        assert_eq!(report.len(), 1);
        assert_eq!(report[0].0, sources);
        assert_eq!(report[0].1.len(), 1);
        assert_eq!(report[0].1[0].line, 2);
        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn check_format_is_clean_for_valid_files() {
        let temp = temp_root("apt-check-clean");
        let sources = sources_file(&temp, "deb http://deb.debian.org/debian bookworm main\n");
        let _guard = paths_guard(&temp, &sources);
        assert!(check_format().unwrap().is_empty());
        let _ = fs::remove_dir_all(&temp);
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
        let host = debian_host();
        let report = apply(
            &host.family,
            &host.codename,
            MirrorName::Official,
            true,
            true,
        )
        .unwrap();
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
        let host = debian_host();
        let report = apply(&host.family, &host.codename, MirrorName::Tuna, false, true).unwrap();
        assert_eq!(report.changed_files, 0);
        assert!(report.backup_path.is_none());
        assert_eq!(fs::read_to_string(&sources).unwrap(), content);
        assert!(!temp.join("var/lib/lkit/mirror-backup/debian").exists());
        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn apply_without_recognized_urls_synthesizes_a_new_source() {
        let temp = temp_root("apt-unknown-only");
        let content = "deb http://repo.internal.example.com/debian bookworm main\n";
        let sources = sources_file(&temp, content);
        let _guard = paths_guard(&temp, &sources);
        let host = debian_host();
        let report = apply(&host.family, &host.codename, MirrorName::Tuna, false, true).unwrap();
        assert_eq!(report.changed_files, 1);
        assert_eq!(report.fallback, Some(Fallback::SourceAdded));
        let rewritten = fs::read_to_string(&sources).unwrap();
        assert!(
            rewritten.contains("https://mirrors.tuna.tsinghua.edu.cn/debian bookworm main"),
            "a synthesized mirror entry must be appended: {rewritten}"
        );
        assert!(
            rewritten.contains("deb https://deb.debian.org/debian-security bookworm-security main"),
            "security stays official by default: {rewritten}"
        );
        assert!(
            rewritten.contains("repo.internal.example.com"),
            "custom host must be kept"
        );

        // restore 写回原内容。
        restore(&host.family).unwrap();
        assert_eq!(fs::read_to_string(&sources).unwrap(), content);
        assert!(!temp.join("var/lib/lkit/mirror-backup/debian").exists());
        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn apply_with_only_cdrom_source_disables_it_and_adds_the_mirror() {
        let temp = temp_root("apt-cdrom-only");
        let content = concat!(
            "deb cdrom:[Debian GNU/Linux 12.5.0 _Bookworm_ - Official amd64 DVD Binary-1 20240210-10:16]/ bookworm contrib main\n",
            "# deb cdrom:[Debian GNU/Linux 12.5.0 _Bookworm_ - Official amd64 DVD Binary-2 20240210-10:16]/ bookworm contrib main\n",
        );
        let sources = sources_file(&temp, content);
        let _guard = paths_guard(&temp, &sources);
        let host = debian_host();
        let report = apply(&host.family, &host.codename, MirrorName::Tuna, false, true).unwrap();
        assert_eq!(report.fallback, Some(Fallback::CdromDisabled));
        let rewritten = fs::read_to_string(&sources).unwrap();
        assert!(
            rewritten.contains("# deb cdrom:[Debian GNU/Linux 12.5.0 _Bookworm_"),
            "the enabled cdrom line must be commented out by default: {rewritten}"
        );
        assert!(
            rewritten.contains(
                "deb https://mirrors.tuna.tsinghua.edu.cn/debian bookworm main contrib non-free\n"
            ),
            "a mirror entry must be appended so the system keeps sources: {rewritten}"
        );
        assert!(
            rewritten.contains("deb https://deb.debian.org/debian-security bookworm-security main"),
            "security stays official by default: {rewritten}"
        );

        // restore 写回原内容。
        restore(&host.family).unwrap();
        assert_eq!(fs::read_to_string(&sources).unwrap(), content);
        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn apply_with_only_cdrom_source_converts_it_when_cdrom_is_kept() {
        let temp = temp_root("apt-cdrom-only-keep");
        let content = concat!(
            "deb cdrom:[Debian GNU/Linux 12.5.0 _Bookworm_ - Official amd64 DVD Binary-1 20240210-10:16]/ bookworm contrib main\n",
            "# deb cdrom:[Debian GNU/Linux 12.5.0 _Bookworm_ - Official amd64 DVD Binary-2 20240210-10:16]/ bookworm contrib main\n",
        );
        let sources = sources_file(&temp, content);
        let _guard = paths_guard(&temp, &sources);
        let host = debian_host();
        let report = apply(&host.family, &host.codename, MirrorName::Tuna, false, false).unwrap();
        assert_eq!(report.fallback, Some(Fallback::CdromConverted));
        let rewritten = fs::read_to_string(&sources).unwrap();
        assert!(
            rewritten.contains(
                "deb https://mirrors.tuna.tsinghua.edu.cn/debian bookworm contrib main\n"
            ),
            "with cdrom kept, the cdrom line is converted instead: {rewritten}"
        );
        assert!(
            rewritten.contains("# deb cdrom:[Debian GNU/Linux 12.5.0 _Bookworm_"),
            "disabled cdrom entries stay untouched: {rewritten}"
        );

        // restore 写回原内容。
        restore(&host.family).unwrap();
        assert_eq!(fs::read_to_string(&sources).unwrap(), content);
        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn apply_comments_cdrom_alongside_url_rewrites() {
        let temp = temp_root("apt-cdrom-plus-official");
        let content = concat!(
            "deb cdrom:[Debian GNU/Linux 12.5.0 _Bookworm_ - Official amd64 DVD Binary-1 20240210-10:16]/ bookworm contrib main non-free\n",
            "deb http://deb.debian.org/debian bookworm main contrib non-free-firmware\n",
            "deb http://security.debian.org/debian-security bookworm-security main\n",
        );
        let sources = sources_file(&temp, content);
        let _guard = paths_guard(&temp, &sources);
        let host = debian_host();
        let report = apply(&host.family, &host.codename, MirrorName::Tuna, false, true).unwrap();
        assert_eq!(report.changed_files, 1);
        assert_eq!(report.cdrom_commented, 1);
        let rewritten = fs::read_to_string(&sources).unwrap();
        assert!(
            rewritten.contains(
                "# deb cdrom:[Debian GNU/Linux 12.5.0 _Bookworm_ - Official amd64 DVD Binary-1 20240210-10:16]/ bookworm contrib main non-free\n"
            ),
            "the cdrom line must be commented out: {rewritten}"
        );
        assert!(
            rewritten.contains("deb http://mirrors.tuna.tsinghua.edu.cn/debian bookworm"),
            "recognized URLs are rewritten as before: {rewritten}"
        );
        assert!(
            rewritten.contains("security.debian.org"),
            "security stays official by default: {rewritten}"
        );

        // restore 写回原内容（含未注释的 cdrom 行）。
        restore(&host.family).unwrap();
        assert_eq!(fs::read_to_string(&sources).unwrap(), content);
        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn apply_comments_cdrom_even_when_already_on_target() {
        let temp = temp_root("apt-cdrom-on-target");
        let content = concat!(
            "deb https://mirrors.tuna.tsinghua.edu.cn/debian bookworm main\n",
            "deb cdrom:[Debian GNU/Linux 12.5.0 _Bookworm_]/ bookworm main\n",
        );
        let sources = sources_file(&temp, content);
        let _guard = paths_guard(&temp, &sources);
        let host = debian_host();
        let report = apply(&host.family, &host.codename, MirrorName::Tuna, false, true).unwrap();
        assert_eq!(report.changed_files, 1);
        assert_eq!(report.cdrom_commented, 1);
        let rewritten = fs::read_to_string(&sources).unwrap();
        assert!(
            rewritten.contains("# deb cdrom:[Debian GNU/Linux 12.5.0 _Bookworm_]"),
            "commenting the cdrom is a real change even on an already-target file: {rewritten}"
        );
        assert!(
            rewritten.contains("deb https://mirrors.tuna.tsinghua.edu.cn/debian bookworm main\n"),
            "the mirror entries themselves stay untouched"
        );

        // 保留 cdrom（--keep-cdrom）时该文件是 no-op。
        let report = apply(&host.family, &host.codename, MirrorName::Tuna, false, false).unwrap();
        assert_eq!(report.changed_files, 0);
        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn has_enabled_cdrom_detects_only_enabled_entries() {
        let temp = temp_root("apt-cdrom-detect");
        let sources = sources_file(
            &temp,
            concat!(
                "# deb cdrom:[Debian GNU/Linux 12.5.0 _Bookworm_]/ bookworm main\n",
                "deb http://deb.debian.org/debian bookworm main\n",
            ),
        );
        let _guard = paths_guard(&temp, &sources);
        assert!(
            !has_enabled_cdrom(),
            "only a disabled cdrom entry is not an enabled cdrom source"
        );
        fs::write(
            &sources,
            "deb cdrom:[Debian GNU/Linux 12.5.0 _Bookworm_]/ bookworm main\n",
        )
        .unwrap();
        assert!(has_enabled_cdrom());
        let _ = fs::remove_dir_all(&temp);
    }

    /// 让 `write_atomic` 确定性地失败：同目录放置同名临时文件（`create_new` 冲突）。
    fn write_blocker(temp: &Path) {
        let stale = temp
            .join("etc/apt")
            .join(format!(".lkit-mirror-{}.tmp", std::process::id()));
        fs::write(&stale, "stale temp file").unwrap();
    }

    #[test]
    fn fallback_rolls_back_when_cdrom_disable_write_fails() {
        let temp = temp_root("apt-fallback-cdrom-fail");
        let content =
            "deb cdrom:[Debian GNU/Linux 12.5.0 _Bookworm_]/ bookworm contrib main non-free\n";
        let sources = sources_file(&temp, content);
        write_blocker(&temp);
        let _guard = paths_guard(&temp, &sources);
        let host = debian_host();
        assert!(apply(&host.family, &host.codename, MirrorName::Tuna, false, true).is_err());
        assert!(
            !temp.join("var/lib/lkit/mirror-backup/debian").exists(),
            "a failed fallback must roll back and drop the backup"
        );
        assert_eq!(
            fs::read_to_string(&sources).unwrap(),
            content,
            "the cdrom source must be left untouched"
        );
        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn fallback_rolls_back_when_cdrom_conversion_write_fails() {
        let temp = temp_root("apt-fallback-cdrom-convert-fail");
        let content =
            "deb cdrom:[Debian GNU/Linux 12.5.0 _Bookworm_]/ bookworm contrib main non-free\n";
        let sources = sources_file(&temp, content);
        write_blocker(&temp);
        let _guard = paths_guard(&temp, &sources);
        let host = debian_host();
        assert!(apply(&host.family, &host.codename, MirrorName::Tuna, false, false).is_err());
        assert!(
            !temp.join("var/lib/lkit/mirror-backup/debian").exists(),
            "a failed fallback must roll back and drop the backup"
        );
        assert_eq!(
            fs::read_to_string(&sources).unwrap(),
            content,
            "the cdrom source must be left untouched"
        );
        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn fallback_rolls_back_when_synth_write_fails() {
        let temp = temp_root("apt-fallback-synth-fail");
        let content = "deb http://repo.internal.example.com/debian bookworm main\n";
        let sources = sources_file(&temp, content);
        write_blocker(&temp);
        let _guard = paths_guard(&temp, &sources);
        let host = debian_host();
        assert!(apply(&host.family, &host.codename, MirrorName::Tuna, false, true).is_err());
        assert!(
            !temp.join("var/lib/lkit/mirror-backup/debian").exists(),
            "a failed fallback must roll back and drop the backup"
        );
        assert_eq!(
            fs::read_to_string(&sources).unwrap(),
            content,
            "the custom source must be left untouched"
        );
        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn managed_files_follows_symlinks_in_sources_list_d() {
        let temp = temp_root("apt-symlink");
        let sources = temp.join("etc/apt/sources.list");
        fs::create_dir_all(sources.parent().unwrap()).unwrap();
        fs::write(&sources, "").unwrap();
        let list_d = temp.join("etc/apt/sources.list.d");
        fs::create_dir_all(&list_d).unwrap();
        let real = temp.join("etc/apt/real.list");
        fs::write(&real, "deb http://deb.debian.org/debian bookworm main\n").unwrap();
        std::os::unix::fs::symlink(&real, list_d.join("link.list")).unwrap();
        let _guard = paths_guard(&temp, &sources);
        let files = managed_files().unwrap();
        assert!(
            files.contains(&list_d.join("link.list")),
            "symlinked .list files must be managed like apt reads them: {files:?}"
        );
        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn check_format_is_clean_when_no_source_files() {
        let temp = temp_root("apt-check-no-files");
        let sources = temp.join("etc/apt/sources.list");
        let _guard = paths_guard(&temp, &sources);
        assert!(
            check_format().unwrap().is_empty(),
            "no source files is not a format problem"
        );
        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn repeat_apply_same_mirror_keeps_previous_backup() {
        let temp = temp_root("apt-repeat-apply");
        let content = "deb http://repo.internal.example.com/debian bookworm main\n";
        let sources = sources_file(&temp, content);
        let _guard = paths_guard(&temp, &sources);
        let host = debian_host();
        // 第一次：兜底追加镜像条目，保留原始自定义源的备份。
        let first = apply(&host.family, &host.codename, MirrorName::Tuna, false, true).unwrap();
        assert_eq!(first.fallback, Some(Fallback::SourceAdded));
        assert!(first.backup_path.is_some());
        // 第二次：已是目标状态，no-op 不得删掉上一轮的备份。
        let second = apply(&host.family, &host.codename, MirrorName::Tuna, false, true).unwrap();
        assert_eq!(second.changed_files, 0);
        assert!(second.backup_path.is_none());
        assert!(
            temp.join("var/lib/lkit/mirror-backup/debian").is_dir(),
            "a no-op must keep the previous backup"
        );
        // restore 仍能取回最原始的源。
        restore(&host.family).unwrap();
        assert_eq!(fs::read_to_string(&sources).unwrap(), content);
        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn apply_on_empty_sources_file_synthesizes_entries() {
        let temp = temp_root("apt-empty-file");
        let sources = sources_file(&temp, "");
        let _guard = paths_guard(&temp, &sources);
        let host = debian_host();
        let report = apply(&host.family, &host.codename, MirrorName::Tuna, false, true).unwrap();
        assert_eq!(report.fallback, Some(Fallback::SourceAdded));
        let rewritten = fs::read_to_string(&sources).unwrap();
        assert!(
            rewritten.contains(
                "deb https://mirrors.tuna.tsinghua.edu.cn/debian bookworm main contrib non-free"
            ),
            "empty sources.list must get synthesized entries: {rewritten}"
        );
        restore(&host.family).unwrap();
        assert_eq!(fs::read_to_string(&sources).unwrap(), "");
        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn apply_on_comments_only_sources_file_synthesizes_entries() {
        let temp = temp_root("apt-comments-only");
        let content = "# nothing but comments\n# deb http://example.invalid/debian bookworm main\n";
        let sources = sources_file(&temp, content);
        let _guard = paths_guard(&temp, &sources);
        let host = debian_host();
        let report = apply(&host.family, &host.codename, MirrorName::Tuna, false, true).unwrap();
        assert_eq!(report.fallback, Some(Fallback::SourceAdded));
        let rewritten = fs::read_to_string(&sources).unwrap();
        assert!(rewritten.contains("mirrors.tuna.tsinghua.edu.cn/debian"));
        assert!(
            rewritten.starts_with(content),
            "original comments must be kept"
        );
        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn apply_without_any_source_files_creates_mirror_list() {
        let temp = temp_root("apt-no-files-at-all");
        // sources.list 与 sources.list.d 都不存在。
        let sources = temp.join("etc/apt/sources.list");
        let _guard = paths_guard(&temp, &sources);
        let host = debian_host();
        let report = apply(&host.family, &host.codename, MirrorName::Tuna, false, true).unwrap();
        assert_eq!(report.fallback, Some(Fallback::SourceAdded));
        let created = temp.join("etc/apt/sources.list.d/lkit-mirror.list");
        assert!(created.is_file());
        let content = fs::read_to_string(&created).unwrap();
        assert!(content.contains("https://mirrors.tuna.tsinghua.edu.cn/debian bookworm main"));

        // restore 后占位写回（空文件，apt 无影响）。
        restore(&host.family).unwrap();
        assert_eq!(fs::read_to_string(&created).unwrap(), "");
        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn apply_creates_lkit_mirror_list_when_sources_list_is_missing() {
        let temp = temp_root("apt-no-sources-list");
        let list_d = temp.join("etc/apt/sources.list.d");
        fs::create_dir_all(&list_d).unwrap();
        fs::write(list_d.join("debian.sources"), "").unwrap();
        let sources = temp.join("etc/apt/sources.list");
        let _guard = paths_guard(&temp, &sources);
        let host = debian_host();
        let report = apply(&host.family, &host.codename, MirrorName::Aliyun, true, true).unwrap();
        assert_eq!(report.fallback, Some(Fallback::SourceAdded));
        let created = list_d.join("lkit-mirror.list");
        assert!(created.is_file());
        let content = fs::read_to_string(&created).unwrap();
        assert!(content.contains("https://mirrors.aliyun.com/debian bookworm main"));
        assert!(
            content.contains("https://mirrors.aliyun.com/debian-security bookworm-security main")
        );

        // restore 后占位写回（空文件，apt 无影响）。
        restore(&host.family).unwrap();
        assert_eq!(fs::read_to_string(&created).unwrap(), "");
        let _ = fs::remove_dir_all(&temp);
    }
}
