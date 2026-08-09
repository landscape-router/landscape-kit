//! ifupdown(`/etc/network/interfaces`) 适配器:把选中接口的 `iface` 块改写为
//! `manual` 并从 `auto`/`allow-*` 中删除;原始文件逐字备份,回滚/卸载时按
//! manifest 恢复。语义严格遵循 ifupdown(5),见 `parse` 模块。

mod backup;
mod collect;
mod edit;
mod parse;
mod validate;

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
