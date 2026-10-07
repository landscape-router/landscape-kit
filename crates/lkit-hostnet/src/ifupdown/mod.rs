//! ifupdown(`/etc/network/interfaces`) 适配器:把选中接口的 `iface` 块改写为
//! `manual` 并从 `auto`/`allow-*` 中删除;原始文件逐字备份,回滚/卸载时按
//! manifest 恢复。语义严格遵循 ifupdown(5),见 `parse` 模块。

mod backup;
mod collect;
mod edit;
mod parse;
mod validate;

// 备份/恢复按 EditPlan 与 Manifest 工作,与具体适配器无关;nm/firewalld 适配器
// 经这些别名复用同一实现。
pub(crate) use backup::{
    backup as plan_backup, restore as manifest_restore,
    restore_if_unchanged as manifest_restore_if_unchanged,
};
pub(crate) use edit::{apply as apply_edits, capture_metadata};

use std::collections::BTreeMap;
use std::path::Path;

use crate::adapter::HostNetworkAdapter;
use crate::error::HostNetError;
use crate::model::{EditOutcome, EditPlan, FileSet, FileSources, Manifest, ToolPaths, Validation};

/// ifupdown 适配器,无状态,方法线程安全。
pub struct IfupdownAdapter;

impl IfupdownAdapter {
    pub fn new() -> Self {
        Self
    }

    /// 上一次摘除实际移出 ifupdown 管理的接口集合(升序去重):当前文件集合中
    /// 所有 stanza 均为 `manual` 且无 inherits/选项,同时在 manifest 的原始快照
    /// 中并非如此——宿主自己声明的 manual-bare stanza 与摘除后新增的 stanza 都
    /// 不算。调用方用它校验接管现场与新的接口选择是否一致(reinit)。
    pub fn unmanaged_interfaces(
        &self,
        sources: &FileSources,
        manifest: &Manifest,
    ) -> Result<Vec<String>, HostNetError> {
        let current = manual_bare_map(&collect::collect(sources)?)?;
        let snapshots = FileSet {
            interfaces: sources.interfaces.clone(),
            files: manifest
                .files
                .iter()
                .map(|file| file.backup.clone())
                .collect(),
            conf_d: None,
        };
        let original = manual_bare_map(&snapshots)?;
        Ok(current
            .into_iter()
            .filter(|(iface, non_bare)| !non_bare && original.get(iface) == Some(&true))
            .map(|(iface, _)| iface)
            .collect())
    }
}

/// 解析文件集合,得到每个接口是否含非 manual-bare stanza(值 `true` 表示至少
/// 一个 stanza 仍带配置)。
fn manual_bare_map(file_set: &FileSet) -> Result<BTreeMap<String, bool>, HostNetError> {
    let mut map: BTreeMap<String, bool> = BTreeMap::new();
    for path in &file_set.files {
        let content =
            std::fs::read_to_string(path).map_err(|error| HostNetError::UnreadableFile {
                path: path.clone(),
                source: error,
            })?;
        let parsed = parse::parse(path, &content)?;
        for block in &parsed.blocks {
            let bare = block.method == "manual"
                && block.inherits.is_none()
                && block.option_lines.is_empty();
            map.entry(block.iface.clone())
                .and_modify(|non_bare| *non_bare = *non_bare || !bare)
                .or_insert(!bare);
        }
    }
    Ok(map)
}

impl Default for IfupdownAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl HostNetworkAdapter for IfupdownAdapter {
    fn collect(&self, sources: &FileSources) -> Result<FileSet, HostNetError> {
        collect::collect(sources)
    }

    fn plan_unmanage(
        &self,
        file_set: &FileSet,
        selected: &[String],
    ) -> Result<EditPlan, HostNetError> {
        edit::plan_unmanage(file_set, selected)
    }

    fn apply(&self, plan: &EditPlan) -> Result<EditOutcome, HostNetError> {
        edit::apply(plan)
    }

    fn backup(&self, plan: &EditPlan, dest: &Path) -> Result<Manifest, HostNetError> {
        backup::backup(plan, dest)
    }

    fn restore(&self, manifest: &Manifest) -> Result<(), HostNetError> {
        backup::restore(manifest)
    }

    fn restore_if_unchanged(
        &self,
        manifest: &Manifest,
        plan: &EditPlan,
    ) -> Result<(), HostNetError> {
        backup::restore_if_unchanged(manifest, plan)
    }

    fn validate(&self, file_set: &FileSet, tools: &ToolPaths) -> Result<Validation, HostNetError> {
        validate::validate(file_set, tools)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INTERFACES: &str = "\
auto ens3 ens5 mgmt0
iface ens3 inet dhcp

iface ens5 inet static
    address 198.51.100.10/24

iface mgmt0 inet manual
";

    /// 摘除后按 manifest 反查"哪些接口是被摘除的"。
    #[test]
    fn unmanaged_interfaces_reports_only_stripped_stanzas() {
        let dir = std::env::temp_dir().join(format!(
            "lkit-hostnet-unmanaged-{}-{:?}",
            std::process::id(),
            std::time::Instant::now()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let interfaces = dir.join("interfaces");
        std::fs::write(&interfaces, INTERFACES).unwrap();

        let adapter = IfupdownAdapter::new();
        let sources = FileSources::new(interfaces.clone());
        let outcome = adapter
            .execute_unmanage(
                &sources,
                &["ens3".to_string()],
                &dir.join("backup"),
                &ToolPaths::default(),
            )
            .unwrap();
        let manifest = outcome.manifest.expect("ens3 was rewritten");

        // 宿主自己的 manual-bare stanza(mgmt0)与未摘除接口(ens5)都不算。
        assert_eq!(
            adapter.unmanaged_interfaces(&sources, &manifest).unwrap(),
            vec!["ens3".to_string()]
        );

        // 摘除后宿主新增的 manual-bare stanza 不算摘除。
        let rewritten = std::fs::read_to_string(&interfaces).unwrap();
        std::fs::write(
            &interfaces,
            format!("{rewritten}\nauto extra0\niface extra0 inet manual\n"),
        )
        .unwrap();
        assert_eq!(
            adapter.unmanaged_interfaces(&sources, &manifest).unwrap(),
            vec!["ens3".to_string()]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 宿主把已摘除的 stanza 改回带配置的形态:不再视为摘除态。
    #[test]
    fn unmanaged_interfaces_drops_reconfigured_stanzas() {
        let dir = std::env::temp_dir().join(format!(
            "lkit-hostnet-unmanaged-reconf-{}-{:?}",
            std::process::id(),
            std::time::Instant::now()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let interfaces = dir.join("interfaces");
        std::fs::write(&interfaces, INTERFACES).unwrap();

        let adapter = IfupdownAdapter::new();
        let sources = FileSources::new(interfaces.clone());
        let manifest = adapter
            .execute_unmanage(
                &sources,
                &["ens3".to_string()],
                &dir.join("backup"),
                &ToolPaths::default(),
            )
            .unwrap()
            .manifest
            .expect("ens3 was rewritten");

        std::fs::write(
            &interfaces,
            "auto ens3 ens5 mgmt0\niface ens3 inet static\n    address 192.0.2.10/24\n\niface ens5 inet static\n    address 198.51.100.10/24\n\niface mgmt0 inet manual\n",
        )
        .unwrap();
        assert_eq!(
            adapter.unmanaged_interfaces(&sources, &manifest).unwrap(),
            Vec::<String>::new()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
