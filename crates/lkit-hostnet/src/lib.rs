//! 宿主网络适配:把选中的网络接口从宿主网络管理器中摘除,并在回滚/卸载时恢复。
//!
//! 本 crate 只操作宿主网络配置文件,不调用 systemd、`ip` 或直接操作接口；配置入口、
//! backup 目录与校验工具路径全部由调用方注入。设计见 `docs/network/hostnet.md`。

pub mod adapter;
pub mod error;
pub mod ifupdown;
pub mod model;

pub use adapter::HostNetworkAdapter;
pub use error::HostNetError;
pub use model::{
    EditOutcome, EditPlan, FileEdit, FileMetadata, FileSet, FileSources, Manifest, ManifestFile,
    ToolPaths, UnmanageOutcome, Validation,
};
