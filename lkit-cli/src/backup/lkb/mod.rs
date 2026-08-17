mod read;
mod write;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::plan::InstallError;

pub(crate) use self::read::{
    backup_id_format_ok, create_file_mode, create_secure_dir, extract_lkb, read_backup_metadata,
    read_backup_metadata_streamed, verify_lkb,
};
pub(crate) use self::write::{create_backup, publish_no_replace, validate_remark};

pub(crate) const LKB_MAGIC: &[u8; 4] = b"LKB1";

pub(crate) const LKB_HEADER_LEN: usize = 32;

pub(crate) const LKB_METADATA_CAPACITY: usize = 1024 * 1024;

pub(crate) const LKB_MIN_LEN: u64 = 1024 * 1024 + 1;

const BACKUP_ID_ATTEMPTS: usize = 8;

/// 备份创建过程的进度事件。`total` 是归档条目的文件数（目录不算）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum BackupProgress {
    Exporting,
    Archiving {
        done: u64,
        total: u64,
        current: String,
    },
    Finalizing,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct BackupMetadata {
    pub schema_version: u64,
    pub backup_id: String,
    pub created_at: DateTime<Utc>,
    pub landscape_version: String,
    pub lkit_version: String,
    pub architecture: BackupArchitecture,
    pub hostname: String,
    pub remark: String,
    pub auto: bool,
    pub scope: BackupScope,
    pub contents: BackupContents,
    pub checksum: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum BackupArchitecture {
    X86_64,
    Aarch64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum BackupScope {
    Minimal,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct BackupContents {
    pub binary: bool,
    #[serde(rename = "static")]
    pub static_: bool,
    pub static_archive: bool,
    pub init_config: bool,
    pub geo_cache: bool,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn invalid_backup(reason: String) -> InstallError {
    InstallError::InvalidBackup(reason)
}
