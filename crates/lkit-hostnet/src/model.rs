use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub const MANIFEST_SCHEMA_VERSION: u64 = 1;

/// 宿主网络配置文件的入口:ifupdown 主文件 `/etc/network/interfaces`。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileSources {
    pub interfaces: PathBuf,
}

impl FileSources {
    pub fn new(interfaces: PathBuf) -> Self {
        Self { interfaces }
    }
}

/// 由 `collect` 得到的完整文件集合:主文件 + `source` 展开的文件,主文件排第一,
/// 其余按路径排序,已按 canonical path 去重。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileSet {
    pub interfaces: PathBuf,
    pub files: Vec<PathBuf>,
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileEdit {
    pub path: PathBuf,
    pub original_content: Vec<u8>,
    pub content: String,
    pub metadata: FileMetadata,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditOutcome {
    pub edited: Vec<PathBuf>,
}

/// 备份清单:`backup` 相对备份根目录,`original` 为被备份文件的绝对路径。
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Manifest {
    pub schema_version: u64,
    pub files: Vec<ManifestFile>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManifestFile {
    pub original: PathBuf,
    pub backup: PathBuf,
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
}
