//! systemd-networkd 适配器:把 `[Match]` 只引用选中接口的 `.network` 文件整体
//! 移出(备份持有逐字副本,恢复按 manifest 重建)。文件同时引用选中与未选
//! 接口、或以 glob 形式引用选中接口时保守拒绝——无法归因完整匹配集就不猜。
//! `[Match]` 不含 `Name=` 的文件(按 MAC/Driver 等匹配)不按名字归因,原样
//! 跳过。`.netdev` 定义虚拟设备,选中接口均为物理接口,不参与。
//!
//! 运行时套用(`networkctl reload` 让 networkd 丢弃已移除的配置)与恢复后的
//! reload 由调用方执行,本 crate 只做文件与清单。

use std::path::{Path, PathBuf};

use crate::adapter::HostNetworkAdapter;
use crate::error::HostNetError;
use crate::ifupdown::{capture_metadata, plan_backup};
use crate::model::{
    EditOutcome, EditPlan, FileEdit, FileSet, FileSources, Manifest, ToolPaths, Validation,
};

/// systemd-networkd 适配器,无状态,方法线程安全。
pub struct NetworkdAdapter;

impl NetworkdAdapter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for NetworkdAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl NetworkdAdapter {
    /// 反查摘除的接口集合:manifest 中 original 路径已消失的条目,其备份快照
    /// `[Match] Name=` 的精确名字即被移出的接口;文件被重新创建则不再计入。
    pub fn unmanaged_interfaces(manifest: &Manifest) -> Vec<String> {
        let mut removed: Vec<String> = Vec::new();
        for file in &manifest.files {
            if file.original.exists() {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&file.backup) else {
                continue;
            };
            for name in match_names(&content) {
                if !name.contains(GLOB_META_CHARS) && !removed.iter().any(|r| r == &name) {
                    removed.push(name);
                }
            }
        }
        removed.sort();
        removed
    }
}

impl HostNetworkAdapter for NetworkdAdapter {
    fn collect(&self, sources: &FileSources) -> Result<FileSet, HostNetError> {
        let Some(dir) = &sources.networkd_dir else {
            return Ok(FileSet::default());
        };
        let metadata = match std::fs::symlink_metadata(dir) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(FileSet::default());
            }
            Err(source) => {
                return Err(HostNetError::UnreadableFile {
                    path: dir.clone(),
                    source,
                });
            }
        };
        if metadata.file_type().is_symlink() {
            return Err(HostNetError::PathSafety {
                path: dir.clone(),
                reason: "networkd directory is a symlink".into(),
            });
        }
        let mut files = Vec::new();
        for entry in sorted_dir_entries(dir)? {
            let path = dir.join(&entry);
            let metadata = std::fs::symlink_metadata(&path).map_err(|source| {
                HostNetError::UnreadableFile {
                    path: path.clone(),
                    source,
                }
            })?;
            if metadata.file_type().is_symlink() {
                return Err(HostNetError::PathSafety {
                    path,
                    reason: "networkd config is a symlink".into(),
                });
            }
            if metadata.is_file() && entry.ends_with(".network") {
                files.push(path);
            }
        }
        Ok(FileSet {
            interfaces: sources.interfaces.clone(),
            files,
            conf_d: None,
        })
    }

    fn plan_unmanage(
        &self,
        file_set: &FileSet,
        selected: &[String],
    ) -> Result<EditPlan, HostNetError> {
        if selected.is_empty() {
            return Ok(EditPlan { edits: Vec::new() });
        }
        let mut edits = Vec::new();
        for path in &file_set.files {
            let content =
                std::fs::read_to_string(path).map_err(|source| HostNetError::UnreadableFile {
                    path: path.clone(),
                    source,
                })?;
            let names = match_names(&content);
            let mut selected_refs = Vec::new();
            let mut unselected_refs = Vec::new();
            for name in &names {
                if name.chars().any(|c| GLOB_META_CHARS.contains(&c)) {
                    // glob 无法归因完整匹配集,可能同时匹配未选接口。
                    if let Some(hit) = selected
                        .iter()
                        .find(|candidate| glob_matches(name, candidate))
                    {
                        return Err(HostNetError::UnsupportedSyntax {
                            path: path.clone(),
                            line: 0,
                            reason: format!(
                                "Name={name} uses a glob that matches selected interface {hit}"
                            ),
                        });
                    }
                    continue;
                }
                if selected.iter().any(|candidate| candidate == name) {
                    selected_refs.push(name.clone());
                } else {
                    unselected_refs.push(name.clone());
                }
            }
            if selected_refs.is_empty() {
                continue;
            }
            if !unselected_refs.is_empty() {
                return Err(HostNetError::UnsupportedSyntax {
                    path: path.clone(),
                    line: 0,
                    reason: format!(
                        "matches selected [{}] and unselected [{}] interfaces in one file",
                        selected_refs.join(", "),
                        unselected_refs.join(", ")
                    ),
                });
            }
            let original = std::fs::read(path).map_err(|source| HostNetError::UnreadableFile {
                path: path.clone(),
                source,
            })?;
            edits.push(FileEdit {
                path: path.clone(),
                original_content: original,
                content: String::new(),
                metadata: capture_metadata(path)?,
                created: false,
                removed: true,
            });
        }
        Ok(EditPlan { edits })
    }

    fn apply(&self, plan: &EditPlan) -> Result<EditOutcome, HostNetError> {
        crate::ifupdown::apply_edits(plan)
    }

    fn backup(&self, plan: &EditPlan, dest: &Path) -> Result<Manifest, HostNetError> {
        plan_backup(plan, dest)
    }

    fn restore(&self, manifest: &Manifest) -> Result<(), HostNetError> {
        crate::ifupdown::manifest_restore(manifest)
    }

    fn restore_if_unchanged(
        &self,
        manifest: &Manifest,
        plan: &EditPlan,
    ) -> Result<(), HostNetError> {
        crate::ifupdown::manifest_restore_if_unchanged(manifest, plan)
    }

    /// networkd 没有离线配置校验工具;文件级正确性由计划/原子删除保证,
    /// 运行时效果由调用方 reload 后自查。
    fn validate(
        &self,
        _file_set: &FileSet,
        _tools: &ToolPaths,
    ) -> Result<Validation, HostNetError> {
        Ok(Validation::Unavailable)
    }
}

/// fnmatch 风格的 glob 元字符。
const GLOB_META_CHARS: [char; 3] = ['*', '?', '['];

fn sorted_dir_entries(dir: &Path) -> Result<Vec<String>, HostNetError> {
    let mut entries: Vec<String> = std::fs::read_dir(dir)
        .map_err(|source| HostNetError::UnreadableFile {
            path: dir.to_path_buf(),
            source,
        })?
        .map(map_file_name)
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort();
    Ok(entries)
}

/// 提取 `[Match]` 段全部 `Name=` 的空格分隔精确名(glob 名原样返回,由调用方
/// 判定)。段外与注释不参与;`Name=` 行尾续行符无法安全展开,含引用该文件的
/// 选中接口时由调用方以 UnsupportedSyntax 拒绝。
fn match_names(content: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_match = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_match = trimmed.eq_ignore_ascii_case("[Match]");
            continue;
        }
        if !in_match {
            continue;
        }
        if trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }
        if let Some(value) = trimmed.strip_prefix("Name=") {
            for token in value.split_whitespace() {
                if !token.is_empty() && !names.iter().any(|name| name == token) {
                    names.push(token.to_string());
                }
            }
        }
    }
    names
}

fn map_file_name(entry: std::io::Result<std::fs::DirEntry>) -> Result<String, HostNetError> {
    entry
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .map_err(|source| HostNetError::UnreadableFile {
            path: PathBuf::from("."),
            source,
        })
}

fn glob_matches(pattern: &str, candidate: &str) -> bool {
    glob::Pattern::new(pattern).is_ok_and(|pattern| pattern.matches(candidate))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn networkd_fixture(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lkit-hostnet-networkd-{}-{name}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("network")).unwrap();
        dir
    }

    fn sources(dir: &Path) -> FileSources {
        FileSources {
            networkd_dir: Some(dir.join("network")),
            ..Default::default()
        }
    }

    #[test]
    fn unmanage_removes_attributable_files_and_restores_them() {
        let dir = networkd_fixture("remove");
        let network = dir.join("network");
        let wan = network.join("10-wan.network");
        let wan_original = "[Match]\nName=ens3\n\n[Network]\nDHCP=yes\n";
        std::fs::write(&wan, wan_original).unwrap();
        let lan = network.join("20-lan.network");
        std::fs::write(
            &lan,
            "[Match]\nName=ens4 ens3\n\n[Network]\nAddress=192.168.10.1/24\n",
        )
        .unwrap();
        // 未引用选中接口的文件与 .netdev 不参与。
        std::fs::write(
            network.join("30-other.network"),
            "[Match]\nName=ens9\n\n[Network]\nDHCP=yes\n",
        )
        .unwrap();
        std::fs::write(
            network.join("40-br.netdev"),
            "[NetDev]\nName=br_lan\nKind=bridge\n",
        )
        .unwrap();

        let selected = ["ens3".to_string(), "ens4".to_string()];
        let outcome = NetworkdAdapter::new()
            .execute_unmanage(
                &sources(&dir),
                &selected,
                &dir.join("backup"),
                &ToolPaths::default(),
            )
            .unwrap();
        let manifest = outcome.manifest.expect("files removed");
        assert!(!wan.exists(), "the WAN .network file must be removed");
        assert!(!lan.exists(), "the LAN .network file must be removed");
        assert!(network.join("30-other.network").is_file());
        assert!(network.join("40-br.netdev").is_file());

        NetworkdAdapter::new().restore(&manifest).unwrap();
        assert_eq!(std::fs::read_to_string(&wan).unwrap(), wan_original);
        assert_eq!(
            std::fs::read_to_string(&lan).unwrap(),
            "[Match]\nName=ens4 ens3\n\n[Network]\nAddress=192.168.10.1/24\n"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 同一文件同时引用选中与未选接口:拒绝,现场不动。
    #[test]
    fn mixed_selected_and_unselected_names_are_rejected() {
        let dir = networkd_fixture("mixed");
        let network = dir.join("network");
        let file = network.join("10-mixed.network");
        std::fs::write(
            &file,
            "[Match]\nName=ens3\nName=ens9\n\n[Network]\nDHCP=yes\n",
        )
        .unwrap();

        let error = NetworkdAdapter::new()
            .execute_unmanage(
                &sources(&dir),
                &["ens3".to_string()],
                &dir.join("backup"),
                &ToolPaths::default(),
            )
            .unwrap_err();
        assert!(matches!(error, HostNetError::UnsupportedSyntax { .. }));
        assert!(file.is_file(), "the rejected file must be untouched");
        assert!(!dir.join("backup").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// glob 引用选中接口:无法归因完整匹配集,拒绝。
    #[test]
    fn glob_referencing_selected_interface_is_rejected() {
        let dir = networkd_fixture("glob");
        let network = dir.join("network");
        let file = network.join("10-glob.network");
        std::fs::write(&file, "[Match]\nName=en*\n\n[Network]\nDHCP=yes\n").unwrap();

        let error = NetworkdAdapter::new()
            .execute_unmanage(
                &sources(&dir),
                &["ens3".to_string()],
                &dir.join("backup"),
                &ToolPaths::default(),
            )
            .unwrap_err();
        assert!(
            matches!(error, HostNetError::UnsupportedSyntax { ref reason, .. } if reason.contains("glob")),
            "unexpected error: {error:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 无 Name= 的文件不按名字归因,原样跳过;目录缺失是 no-op。
    #[test]
    fn files_without_name_match_and_missing_dir_are_skipped() {
        let dir = networkd_fixture("skip");
        let network = dir.join("network");
        let mac = network.join("10-mac.network");
        std::fs::write(
            &mac,
            "[Match]\nMACAddress=52:54:00:12:34:01\n\n[Network]\nDHCP=yes\n",
        )
        .unwrap();

        let outcome = NetworkdAdapter::new()
            .execute_unmanage(
                &sources(&dir),
                &["ens3".to_string()],
                &dir.join("backup"),
                &ToolPaths::default(),
            )
            .unwrap();
        assert!(outcome.manifest.is_none());
        assert!(mac.is_file());

        let missing = FileSources {
            networkd_dir: Some(dir.join("missing")),
            ..Default::default()
        };
        let outcome = NetworkdAdapter::new()
            .execute_unmanage(
                &missing,
                &["ens3".to_string()],
                &dir.join("backup2"),
                &ToolPaths::default(),
            )
            .unwrap();
        assert!(outcome.manifest.is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 反查:被移出文件的精确 Name 集即摘除现场;被人工重建的文件不再计入。
    #[test]
    fn unmanaged_interfaces_reads_removed_files() {
        let dir = networkd_fixture("reverse");
        let network = dir.join("network");
        std::fs::write(
            network.join("10-wan.network"),
            "[Match]\nName=ens3\n\n[Network]\nDHCP=yes\n",
        )
        .unwrap();
        let sources = sources(&dir);
        let outcome = NetworkdAdapter::new()
            .execute_unmanage(
                &sources,
                &["ens3".to_string()],
                &dir.join("backup"),
                &ToolPaths::default(),
            )
            .unwrap();
        let manifest = outcome.manifest.unwrap();
        assert_eq!(
            NetworkdAdapter::unmanaged_interfaces(&manifest),
            vec!["ens3".to_string()]
        );

        std::fs::write(
            network.join("10-wan.network"),
            "[Match]\nName=ens3\n\n[Network]\nDHCP=yes\n",
        )
        .unwrap();
        assert!(
            NetworkdAdapter::unmanaged_interfaces(&manifest).is_empty(),
            "a recreated file is no longer standing"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// guarded 恢复:删除后被外部以其他内容重建的文件保留并上报。
    #[test]
    fn guarded_restore_preserves_external_recreation() {
        let dir = networkd_fixture("guarded");
        let network = dir.join("network");
        let file = network.join("10-wan.network");
        std::fs::write(&file, "[Match]\nName=ens3\n\n[Network]\nDHCP=yes\n").unwrap();
        let sources = sources(&dir);
        let adapter = NetworkdAdapter::new();
        let file_set = adapter.collect(&sources).unwrap();
        let plan = adapter
            .plan_unmanage(&file_set, &["ens3".to_string()])
            .unwrap();
        let manifest = adapter.backup(&plan, &dir.join("backup")).unwrap();
        adapter.apply(&plan).unwrap();

        std::fs::write(
            &file,
            "[Match]\nName=ens3\n\n[Network]\nAddress=198.51.100.20/24\n",
        )
        .unwrap();
        let error = adapter.restore_if_unchanged(&manifest, &plan).unwrap_err();
        assert!(matches!(error, HostNetError::ConcurrentModification { .. }));
        assert!(
            file.is_file(),
            "the externally recreated file must be preserved"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
