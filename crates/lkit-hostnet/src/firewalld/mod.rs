//! firewalld 适配器:把选中接口从 zone 定义(`/etc/firewalld/zones/*.xml`)的
//! `<interface name="..."/>` 行中移除,服务保持运行、继续管理未选接口。只删除
//! 能整行匹配的自闭合单属性形态;选中接口出现在其他形态(多属性、跨行、注释
//! 内)时保守拒绝,不做猜测。其余内容逐字节保留,原文件逐字备份。
//!
//! 运行时摘除(`firewall-cmd --remove-interface` 或 reload)与恢复后的重新
//! 绑定由调用方执行,本 crate 只做文件与清单。

use std::path::{Path, PathBuf};

use crate::adapter::HostNetworkAdapter;
use crate::error::HostNetError;
use crate::ifupdown::{capture_metadata, plan_backup};
use crate::model::{
    EditOutcome, EditPlan, FileEdit, FileSet, FileSources, Manifest, ToolPaths, Validation,
};

/// firewalld 适配器,无状态,方法线程安全。
pub struct FirewalldAdapter;

impl FirewalldAdapter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for FirewalldAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl FirewalldAdapter {
    /// 反查摘除的接口集合:manifest 快照中原本以可整行删除形态出现的名字,
    /// 减去现场文件仍包含的名字。快照缺失或现场文件被删除时按现场不含处理。
    pub fn unmanaged_interfaces(manifest: &Manifest) -> Vec<String> {
        let mut removed: Vec<String> = Vec::new();
        for file in &manifest.files {
            let Ok(original) = std::fs::read_to_string(&file.backup) else {
                continue;
            };
            let live = std::fs::read_to_string(&file.original).unwrap_or_default();
            for name in original.lines().filter_map(interface_line_name) {
                let still_present = live
                    .lines()
                    .any(|line| interface_line_name(line) == Some(name));
                if !still_present && !removed.iter().any(|standing| standing == name) {
                    removed.push(name.to_string());
                }
            }
        }
        removed.sort();
        removed
    }
}

impl HostNetworkAdapter for FirewalldAdapter {
    fn collect(&self, sources: &FileSources) -> Result<FileSet, HostNetError> {
        let Some(zones) = &sources.firewalld_zones else {
            return Ok(FileSet::default());
        };
        if !zones.is_absolute() {
            return Err(HostNetError::PathSafety {
                path: zones.clone(),
                reason: "firewalld zones directory must be absolute".into(),
            });
        }
        match std::fs::symlink_metadata(zones) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(HostNetError::PathSafety {
                    path: zones.clone(),
                    reason: "refusing to operate through a symbolic zones directory".into(),
                });
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(FileSet::default());
            }
            Err(source) => {
                return Err(HostNetError::UnreadableFile {
                    path: zones.clone(),
                    source,
                });
            }
        }
        let mut files = Vec::new();
        let entries = std::fs::read_dir(zones).map_err(|source| HostNetError::UnreadableFile {
            path: zones.clone(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| HostNetError::UnreadableFile {
                path: zones.clone(),
                source,
            })?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("xml") {
                continue;
            }
            match std::fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(HostNetError::PathSafety {
                        path,
                        reason: "zone file must be a regular file".into(),
                    });
                }
                Ok(_) => files.push(path),
                Err(source) => {
                    return Err(HostNetError::UnreadableFile { path, source });
                }
            }
        }
        files.sort();
        Ok(FileSet {
            interfaces: PathBuf::new(),
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
            // split('\n') 会保留结尾换行产生的空元素,join 即可逐字节还原,
            // 删除整行只是去掉对应元素。
            let lines: Vec<&str> = content.split('\n').collect();
            let mut kept: Vec<String> = Vec::with_capacity(lines.len());
            let mut removed_any = false;
            for line in lines {
                if selected
                    .iter()
                    .any(|name| is_removable_interface_line(line, name))
                {
                    removed_any = true;
                    continue;
                }
                kept.push(line.to_string());
            }
            let rewritten = kept.join("\n");
            // 删除整行形态后,选中接口仍以其他形态(多属性、跨行)出现在任何
            // `<interface>` 元素中:继续会留下半删除状态,保守拒绝整个摘除。
            if let Some(name) = selected
                .iter()
                .find(|name| mentions_interface_element(&rewritten, name))
            {
                return Err(HostNetError::UnsupportedSyntax {
                    path: path.clone(),
                    line: 0,
                    reason: format!(
                        "interface {name} appears in a non-removable <interface> element"
                    ),
                });
            }
            if !removed_any {
                continue;
            }
            let original = std::fs::read(path).map_err(|source| HostNetError::UnreadableFile {
                path: path.clone(),
                source,
            })?;
            edits.push(FileEdit {
                path: path.clone(),
                original_content: original,
                content: rewritten,
                metadata: capture_metadata(path)?,
                created: false,
                removed: false,
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

    /// firewalld 没有 conf 级 dry-run;文件正确性由保守解析与原子写保证,
    /// 运行时 zone 归属由调用方 reload/查询后自查。
    fn validate(
        &self,
        _file_set: &FileSet,
        _tools: &ToolPaths,
    ) -> Result<Validation, HostNetError> {
        Ok(Validation::Unavailable)
    }
}

/// 整行自闭合单属性形态:`<interface name="X"/>`,允许前后空白与 `/>` 前空白。
fn is_removable_interface_line(line: &str, name: &str) -> bool {
    interface_line_name(line) == Some(name)
}

/// 提取可整行删除形态(`<interface name="X"/>`,单属性、自闭合、两种引号)
/// 中的接口名;其他形态返回 None。
fn interface_line_name(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    let tag = trimmed.strip_prefix("<interface")?.strip_suffix("/>")?;
    let tag = tag.trim_end();
    ['"', '\''].iter().find_map(|&quote| {
        let open = format!(" name={quote}");
        let close = quote.to_string();
        tag.strip_prefix(&open)
            .and_then(|value| value.strip_suffix(close.as_str()))
            .filter(|value| !value.is_empty())
    })
}

/// 内容中是否存在引用该接口的 `<interface` 元素(用于拒绝无法整行删除的形态)。
fn mentions_interface_element(content: &str, name: &str) -> bool {
    let mut rest = content;
    while let Some(start) = rest.find("<interface") {
        let after = &rest[start..];
        let end = after.find('>').unwrap_or(after.len());
        let tag = &after[..end];
        if tag.contains(&format!("name=\"{name}\"")) || tag.contains(&format!("name='{name}'")) {
            return true;
        }
        rest = &rest[start + "<interface".len()..];
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zones_fixture(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lkit-hostnet-firewalld-{}-{name}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("zones")).unwrap();
        dir
    }

    const PUBLIC: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<zone>\n  <short>Public</short>\n  <interface name=\"ens3\"/>\n  <interface name=\"ens9\"/>\n  <service name=\"ssh\"/>\n</zone>\n";

    fn sources(dir: &Path) -> FileSources {
        FileSources {
            firewalld_zones: Some(dir.join("zones")),
            ..Default::default()
        }
    }

    #[test]
    fn unmanage_removes_selected_interface_lines_and_restores_verbatim() {
        let dir = zones_fixture("remove");
        let zones = dir.join("zones");
        std::fs::write(zones.join("public.xml"), PUBLIC).unwrap();
        std::fs::write(
            zones.join("internal.xml"),
            "<zone>\n  <interface name='ens4'/>\n</zone>\n",
        )
        .unwrap();

        let adapter = FirewalldAdapter::new();
        let outcome = adapter
            .execute_unmanage(
                &sources(&dir),
                &["ens3".to_string(), "ens4".to_string()],
                &dir.join("backup"),
                &ToolPaths::default(),
            )
            .unwrap();
        let manifest = outcome.manifest.expect("zones were edited");

        let public = std::fs::read_to_string(zones.join("public.xml")).unwrap();
        assert!(!public.contains("ens3"));
        assert!(public.contains("<interface name=\"ens9\"/>"));
        assert!(public.contains("<service name=\"ssh\"/>"));
        let internal = std::fs::read_to_string(zones.join("internal.xml")).unwrap();
        assert!(!internal.contains("ens4"));
        // 单引号形态同样整行删除。
        assert_eq!(internal, "<zone>\n</zone>\n");
        assert_eq!(manifest.files.len(), 2);
        assert!(manifest.created.is_empty());

        adapter.restore(&manifest).unwrap();
        assert_eq!(
            std::fs::read_to_string(zones.join("public.xml")).unwrap(),
            PUBLIC
        );
        assert_eq!(
            std::fs::read_to_string(zones.join("internal.xml")).unwrap(),
            "<zone>\n  <interface name='ens4'/>\n</zone>\n"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn zones_without_selected_interfaces_are_untouched() {
        let dir = zones_fixture("untouched");
        let zones = dir.join("zones");
        std::fs::write(zones.join("public.xml"), PUBLIC).unwrap();

        let adapter = FirewalldAdapter::new();
        let outcome = adapter
            .execute_unmanage(
                &sources(&dir),
                &["ens40".to_string()],
                &dir.join("backup"),
                &ToolPaths::default(),
            )
            .unwrap();
        assert!(outcome.manifest.is_none());
        assert!(!dir.join("backup").exists());
        assert_eq!(
            std::fs::read_to_string(zones.join("public.xml")).unwrap(),
            PUBLIC
        );

        // 未提供 zones 入口(firewalld 不在宿主上)同样是 no-op。
        let outcome = adapter
            .execute_unmanage(
                &FileSources::new(dir.join("interfaces")),
                &["ens3".to_string()],
                &dir.join("backup2"),
                &ToolPaths::default(),
            )
            .unwrap();
        assert!(outcome.manifest.is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 选中接口出现在多属性或跨行的 `<interface>` 元素中:保守拒绝,不改任何文件。
    #[test]
    fn non_line_shaped_interface_elements_are_rejected() {
        let dir = zones_fixture("reject");
        let zones = dir.join("zones");
        std::fs::write(
            zones.join("public.xml"),
            "<zone>\n  <interface name=\"ens3\" zone=\"public\"/>\n</zone>\n",
        )
        .unwrap();
        let adapter = FirewalldAdapter::new();
        let error = adapter
            .execute_unmanage(
                &sources(&dir),
                &["ens3".to_string()],
                &dir.join("backup"),
                &ToolPaths::default(),
            )
            .unwrap_err();
        assert!(matches!(error, HostNetError::UnsupportedSyntax { .. }));
        assert_eq!(
            std::fs::read_to_string(zones.join("public.xml")).unwrap(),
            "<zone>\n  <interface name=\"ens3\" zone=\"public\"/>\n</zone>\n"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 反查摘除集合:快照中原有的整行接口名减去现场仍存在的;其他 zone 文件
    /// 与未摘除名字不参与。
    #[test]
    fn unmanaged_interfaces_diffs_snapshots_against_live_zones() {
        let dir = zones_fixture("reverse");
        let zones = dir.join("zones");
        std::fs::write(
            zones.join("public.xml"),
            "<?xml version=\"1.0\"?>\n<zone>\n  <interface name=\"ens3\"/>\n  <interface name=\"ens9\"/>\n</zone>\n",
        )
        .unwrap();
        std::fs::write(
            zones.join("internal.xml"),
            "<zone>\n  <interface name='ens4'/>\n</zone>\n",
        )
        .unwrap();
        let sources = sources(&dir);
        let outcome = FirewalldAdapter::new()
            .execute_unmanage(
                &sources,
                &["ens3".to_string(), "ens4".to_string()],
                &dir.join("backup"),
                &ToolPaths::default(),
            )
            .unwrap();
        let manifest = outcome.manifest.expect("zone edits applied");

        assert_eq!(
            FirewalldAdapter::unmanaged_interfaces(&manifest),
            vec!["ens3".to_string(), "ens4".to_string()],
            "removed names come from every zone snapshot"
        );

        // 现场恢复一行(如人工补回 ens3)后,反查集合随之缩小;未摘除的名字
        // (ens9)必须仍留在现场,否则按漂移处理。
        std::fs::write(
            zones.join("public.xml"),
            "<?xml version=\"1.0\"?>\n<zone>\n  <interface name=\"ens3\"/>\n  <interface name=\"ens9\"/>\n</zone>\n",
        )
        .unwrap();
        assert_eq!(
            FirewalldAdapter::unmanaged_interfaces(&manifest),
            vec!["ens4".to_string()]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 混合形态(可删除的整行 + 不可删除的多属性)出现在不同文件时,计划阶段
    /// 整体拒绝:任何文件都不被修改。
    #[test]
    fn mixed_shapes_across_files_reject_without_touching_any_file() {
        let dir = zones_fixture("partial");
        let zones = dir.join("zones");
        std::fs::write(
            zones.join("a.xml"),
            "<zone>\n  <interface name=\"ens3\"/>\n</zone>\n",
        )
        .unwrap();
        std::fs::write(
            zones.join("b.xml"),
            "<zone>\n  <interface name=\"ens3\" extra=\"1\"/>\n</zone>\n",
        )
        .unwrap();
        let adapter = FirewalldAdapter::new();
        let error = adapter
            .execute_unmanage(
                &sources(&dir),
                &["ens3".to_string()],
                &dir.join("backup"),
                &ToolPaths::default(),
            )
            .unwrap_err();
        assert!(matches!(error, HostNetError::UnsupportedSyntax { .. }));
        assert_eq!(
            std::fs::read_to_string(zones.join("a.xml")).unwrap(),
            "<zone>\n  <interface name=\"ens3\"/>\n</zone>\n"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
