//! NetworkManager 适配器:在 conf.d 目录写入(或改写)`lkit-unmanage.conf`
//! drop-in,以 `[device] unmanaged-devices` 把选中接口声明为不受 NM 管理;
//! 未选接口的既有配置不触碰。恢复删除 drop-in(新建)或逐字还原(改写)。
//!
//! 运行时套用(`nmcli general reload` 使 drop-in 生效、设备转入 unmanaged)
//! 与恢复后的 reload 由调用方执行,本 crate 只做文件与清单。

mod collect;
mod edit;

use std::path::Path;

use crate::adapter::HostNetworkAdapter;
use crate::error::HostNetError;
use crate::model::{EditOutcome, EditPlan, FileSet, FileSources, Manifest, ToolPaths, Validation};

/// lkit 拥有的 drop-in 文件名;宿主同名文件视为外部冲突,计划阶段拒绝。
pub const UNMANAGE_CONF: &str = "lkit-unmanage.conf";

/// NetworkManager 适配器,无状态,方法线程安全。
pub struct NmAdapter;

impl NmAdapter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for NmAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl NmAdapter {
    /// 反查当前 drop-in 声明的 unmanaged 接口集合;drop-in 缺失或不可读时返回
    /// 空集。drop-in 是 NM 摘除现场的真值,reinit 的同集校验以此为准。
    pub fn unmanaged_interfaces(sources: &FileSources) -> Vec<String> {
        let Some(conf_d) = &sources.nm_conf_d else {
            return Vec::new();
        };
        let Ok(content) = std::fs::read_to_string(conf_d.join(UNMANAGE_CONF)) else {
            return Vec::new();
        };
        let mut names = Vec::new();
        for line in content.lines() {
            let Some(value) = line.trim().strip_prefix("unmanaged-devices=") else {
                continue;
            };
            for entry in value.split(';') {
                if let Some(name) = entry.trim().strip_prefix("interface-name:") {
                    let name = name.trim();
                    if !name.is_empty() {
                        names.push(name.to_string());
                    }
                }
            }
        }
        names.sort();
        names.dedup();
        names
    }
}

impl HostNetworkAdapter for NmAdapter {
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
        crate::ifupdown::apply_edits(plan)
    }

    fn backup(&self, plan: &EditPlan, dest: &Path) -> Result<Manifest, HostNetError> {
        crate::ifupdown::plan_backup(plan, dest)
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

    /// NM 的 conf.d drop-in 没有 dry-run 工具;文件级正确性由计划/原子写保证,
    /// 运行时效果(unmanaged 状态)由调用方 reload 后自查。
    fn validate(
        &self,
        _file_set: &FileSet,
        _tools: &ToolPaths,
    ) -> Result<Validation, HostNetError> {
        Ok(Validation::Unavailable)
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn nm_fixture(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lkit-hostnet-nm-{}-{name}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("conf.d")).unwrap();
        dir
    }

    #[test]
    fn unmanage_writes_drop_in_and_restore_deletes_it() {
        let dir = nm_fixture("drop-in");
        let conf_d = dir.join("conf.d");
        let sources = FileSources {
            nm_conf_d: Some(conf_d.clone()),
            ..Default::default()
        };

        let outcome = NmAdapter::new()
            .execute_unmanage(
                &sources,
                &["ens3".to_string(), "ens4".to_string()],
                &dir.join("backup"),
                &ToolPaths::default(),
            )
            .unwrap();
        assert!(outcome.manifest.is_some());

        let content = std::fs::read_to_string(conf_d.join(UNMANAGE_CONF)).unwrap();
        assert!(content.contains("[device]"));
        assert!(content.contains("interface-name:ens3;"));
        assert!(content.contains("interface-name:ens4"));
        // 备份记录为"新建",没有逐字副本。
        let manifest = outcome.manifest.unwrap();
        assert!(manifest.files.is_empty());
        assert_eq!(manifest.created.len(), 1);
        assert_eq!(manifest.created[0].path, conf_d.join(UNMANAGE_CONF));

        NmAdapter::new().restore(&manifest).unwrap();
        assert!(!conf_d.join(UNMANAGE_CONF).exists());
        // conf.d 目录与其他文件保留。
        assert!(conf_d.is_dir());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unmanaged_interfaces_reads_the_live_drop_in() {
        let dir = nm_fixture("reverse");
        let conf_d = dir.join("conf.d");
        let sources = FileSources {
            nm_conf_d: Some(conf_d.clone()),
            ..Default::default()
        };
        assert!(
            NmAdapter::unmanaged_interfaces(&sources).is_empty(),
            "no drop-in means no standing unmanaged set"
        );

        NmAdapter::new()
            .execute_unmanage(
                &sources,
                &["ens4".to_string(), "ens3".to_string()],
                &dir.join("backup"),
                &ToolPaths::default(),
            )
            .unwrap();
        assert_eq!(
            NmAdapter::unmanaged_interfaces(&sources),
            vec!["ens3".to_string(), "ens4".to_string()]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_conf_d_or_empty_selection_is_a_noop() {
        let dir = nm_fixture("noop");
        // 未提供 conf.d 入口:NM 不在宿主上。
        let sources = FileSources::new(dir.join("interfaces"));
        let outcome = NmAdapter::new()
            .execute_unmanage(
                &sources,
                &["ens3".to_string()],
                &dir.join("backup"),
                &ToolPaths::default(),
            )
            .unwrap();
        assert!(outcome.manifest.is_none());
        assert!(!dir.join("backup").exists());

        // conf.d 存在但没有选中接口:同样是 no-op。
        let sources = FileSources {
            nm_conf_d: Some(dir.join("conf.d")),
            ..Default::default()
        };
        let outcome = NmAdapter::new()
            .execute_unmanage(&sources, &[], &dir.join("backup2"), &ToolPaths::default())
            .unwrap();
        assert!(outcome.manifest.is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 已有 drop-in(上一次摘除遗留,或宿主同名文件)时按改写处理:逐字备份、
    /// 原子覆盖,恢复逐字还原。
    #[test]
    fn existing_drop_in_is_rewritten_and_byte_restored() {
        let dir = nm_fixture("existing");
        let conf_d = dir.join("conf.d");
        let drop_in = conf_d.join(UNMANAGE_CONF);
        std::fs::write(
            &drop_in,
            "# foreign content\n[device]\nunmanaged-devices=interface-name:ens9\n",
        )
        .unwrap();
        let sources = FileSources {
            nm_conf_d: Some(conf_d.clone()),
            ..Default::default()
        };

        let outcome = NmAdapter::new()
            .execute_unmanage(
                &sources,
                &["ens3".to_string()],
                &dir.join("backup"),
                &ToolPaths::default(),
            )
            .unwrap();
        let manifest = outcome.manifest.expect("drop-in rewritten");
        assert!(manifest.created.is_empty());
        assert_eq!(manifest.files.len(), 1);
        let content = std::fs::read_to_string(&drop_in).unwrap();
        assert!(content.contains("interface-name:ens3"));
        assert!(!content.contains("ens9"));

        NmAdapter::new().restore(&manifest).unwrap();
        assert_eq!(
            std::fs::read_to_string(&drop_in).unwrap(),
            "# foreign content\n[device]\nunmanaged-devices=interface-name:ens9\n"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// guarded 恢复:外部改写过的 drop-in 保留并上报,仍是本次结果的删除。
    #[test]
    fn guarded_restore_preserves_external_drop_in_edits() {
        let dir = nm_fixture("guarded");
        let conf_d = dir.join("conf.d");
        let sources = FileSources {
            nm_conf_d: Some(conf_d.clone()),
            ..Default::default()
        };
        let adapter = NmAdapter::new();
        let file_set = adapter.collect(&sources).unwrap();
        let plan = adapter
            .plan_unmanage(&file_set, &["ens3".to_string()])
            .unwrap();
        let manifest = adapter.backup(&plan, &dir.join("backup")).unwrap();
        adapter.apply(&plan).unwrap();

        // 外部改写后,事务失败的 guarded 恢复不得覆盖。
        std::fs::write(
            conf_d.join(UNMANAGE_CONF),
            "[device]\nunmanaged-devices=interface-name:other0\n",
        )
        .unwrap();
        let error = adapter.restore_if_unchanged(&manifest, &plan).unwrap_err();
        assert!(matches!(error, HostNetError::ConcurrentModification { .. }));
        assert!(conf_d.join(UNMANAGE_CONF).is_file());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// drop-in 权限与 conf.d 既有文件对齐时保持私有;默认 0644 root:root。
    #[test]
    fn drop_in_uses_deterministic_metadata() {
        let dir = nm_fixture("metadata");
        let conf_d = dir.join("conf.d");
        let sources = FileSources {
            nm_conf_d: Some(conf_d.clone()),
            ..Default::default()
        };
        let adapter = NmAdapter::new();
        let outcome = adapter
            .execute_unmanage(
                &sources,
                &["ens3".to_string()],
                &dir.join("backup"),
                &ToolPaths::default(),
            )
            .unwrap();
        let manifest = outcome.manifest.unwrap();
        let mode = std::fs::metadata(conf_d.join(UNMANAGE_CONF))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o644);
        assert_eq!(manifest.created[0].metadata.mode, 0o644);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
