use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const MANIFEST_SCHEMA_VERSION: u64 = 1;

/// 宿主网络配置文件的入口。ifupdown 适配器读 `interfaces` 主文件;
/// NetworkManager 适配器读 `nm_conf_d` drop-in 目录,firewalld 适配器读
/// `firewalld_zones` zone 目录——各适配器只消费自己的字段,未提供的入口
/// 视为该管理器不在宿主上,摘除为 no-op。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FileSources {
    pub interfaces: PathBuf,
    pub nm_conf_d: Option<PathBuf>,
    pub firewalld_zones: Option<PathBuf>,
}

impl FileSources {
    pub fn new(interfaces: PathBuf) -> Self {
        Self {
            interfaces,
            ..Default::default()
        }
    }
}

/// 由 `collect` 得到的完整文件集合。ifupdown:`files` 为主文件 + `source` 展开
/// 的文件,主文件排第一,其余按路径排序,已按 canonical path 去重;NetworkManager:
/// `conf_d` 为 drop-in 目录(目录存在才有值),`files` 只含已存在的 lkit drop-in。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FileSet {
    pub interfaces: PathBuf,
    pub files: Vec<PathBuf>,
    pub conf_d: Option<PathBuf>,
}

impl FileSet {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// 改写计划:`edits` 为空表示没有需要修改的文件。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditPlan {
    pub edits: Vec<FileEdit>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct FileMetadata {
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
}

/// 单个文件的完整改写结果(含所有未改动行的原样内容和 apply 前快照)。
/// `created = true` 表示目标文件在计划时不不存在、由本次摘除新建,恢复时
/// 整体删除而不是逐字还原。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileEdit {
    pub path: PathBuf,
    pub original_content: Vec<u8>,
    pub content: String,
    pub metadata: FileMetadata,
    pub created: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditOutcome {
    pub edited: Vec<PathBuf>,
}

/// 备份清单:`files` 为被改写文件的逐字备份对,`created` 为摘除新建、恢复时
/// 整体删除的文件(如 NetworkManager 的 conf.d drop-in)。
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Manifest {
    pub schema_version: u64,
    #[serde(default)]
    pub files: Vec<ManifestFile>,
    #[serde(default)]
    pub created: Vec<ManifestCreated>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManifestFile {
    pub original: PathBuf,
    pub backup: PathBuf,
    pub metadata: FileMetadata,
}

/// 摘除新建的文件:路径必须是绝对路径,恢复(幂等)时删除;guarded 恢复仅在
/// 内容仍是本次写入结果时删除,外部改写过的保留并上报。
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManifestCreated {
    pub path: PathBuf,
    pub metadata: FileMetadata,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnmanageOutcome {
    pub manifest: Option<Manifest>,
    pub edited: Vec<PathBuf>,
    pub validation: Validation,
}

/// dry-run 校验结果:`Unavailable` 表示工具缺失或不可执行,不阻断调用方;
/// `Failed` 由调用方决定恢复备份并中止。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Validation {
    Clean,
    Unavailable,
    Failed { exit: Option<i32>, stderr: String },
}

/// 校验工具路径,全部可选;缺失时校验返回 `Validation::Unavailable`。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ToolPaths {
    pub ifup: Option<PathBuf>,
    /// NM/firewalld 适配器的文件级摘除没有 dry-run 工具,运行时套用
    /// (reload/运行时摘除)由调用方执行;这两个路径供后续运行时校验使用。
    pub nmcli: Option<PathBuf>,
    pub firewall_cmd: Option<PathBuf>,
}
