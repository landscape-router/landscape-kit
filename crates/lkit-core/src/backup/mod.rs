use serde::{Deserialize, Serialize};

/// Magic bytes identifying a Landscape Kit backup file.
pub const MAGIC: &[u8; 4] = b"LKB1";

/// Current .lkb format version.
pub const HEADER_VERSION: u16 = 1;

/// Total size of the fixed header in bytes.
pub const HEADER_SIZE: usize = 32;

/// Reserved region: byte range [10..16].
pub const RESERVED1_SIZE: usize = 6;

/// Reserved region: byte range [16..32].
pub const RESERVED2_SIZE: usize = 16;

/// Size of the entire metadata region (1 MiB).
pub const META_REGION_SIZE: u64 = 1024 * 1024;

/// Maximum allowed metadata JSON length (1 MiB - 32 byte header).
pub const MAX_JSON_LEN: u32 = (META_REGION_SIZE as u32) - (HEADER_SIZE as u32);

/// Number of automatic backups to retain.
pub const AUTO_BACKUP_LIMIT: usize = 5;

/// A single backup entry as discovered from a .lkb file on disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupEntry {
    /// Unique backup identifier.
    pub backup_id: String,
    /// RFC 3339 creation timestamp.
    pub created_at: String,
    /// Landscape version at backup time.
    pub landscape_version: String,
    /// lkit version that created this backup.
    pub lkit_version: String,
    /// Hostname where the backup was created.
    pub hostname: String,
    /// User-supplied remark (None for auto backups).
    pub remark: Option<String>,
    /// Whether this backup was created automatically.
    pub auto: bool,
    /// Backup scope.
    pub scope: BackupScope,
    /// SHA256 checksum of the tar.gz data.
    pub checksum: String,
    /// File name on disk.
    pub filename: String,
    /// File size in bytes.
    pub file_size: u64,
    /// Filesystem path to the .lkb file.
    #[serde(skip)]
    pub path: std::path::PathBuf,
}

/// Metadata stored inside a .lkb file's JSON header.
///
/// Mirrors BackupEntry but excludes filename/path (those are filesystem concerns).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupMetadata {
    pub backup_id: String,
    pub created_at: String,
    pub landscape_version: String,
    pub lkit_version: String,
    pub hostname: String,
    pub remark: Option<String>,
    pub auto: bool,
    pub scope: BackupScope,
    pub checksum: String,
}

/// Backup content scope.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BackupScope {
    /// Binary + static/ + API-exported landscape_init.toml
    Minimal,
    /// Entire LANDSCAPE_HOME directory
    Full,
}

impl std::fmt::Display for BackupScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Minimal => write!(f, "minimal"),
            Self::Full => write!(f, "full"),
        }
    }
}

/// lkit version, populated at compile time.
pub const LKIT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_max_json_len() {
        assert_eq!(MAX_JSON_LEN, 1_048_544); // 1MiB - 32
    }

    #[test]
    fn test_magic_bytes() {
        assert_eq!(MAGIC, b"LKB1");
    }

    #[test]
    fn test_backup_metadata_serde_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
        let meta = BackupMetadata {
            backup_id: "20260601-143022-a1b2c3d4".into(),
            created_at: "2026-06-01T14:30:22Z".into(),
            landscape_version: "0.19.2".into(),
            lkit_version: "0.3.0".into(),
            hostname: "node-01".into(),
            remark: None,
            auto: false,
            scope: BackupScope::Minimal,
            checksum: "sha256:ab12cd34ef...".into(),
        };
        let json = serde_json::to_string(&meta)?;
        let decoded: BackupMetadata = serde_json::from_str(&json)?;
        assert_eq!(decoded.backup_id, "20260601-143022-a1b2c3d4");
        assert_eq!(decoded.scope, BackupScope::Minimal);
        Ok(())
    }

    #[test]
    fn test_backup_scope_serde_values() -> Result<(), Box<dyn std::error::Error>> {
        let minimal: BackupScope = serde_json::from_str("\"minimal\"")?;
        assert_eq!(minimal, BackupScope::Minimal);
        let full: BackupScope = serde_json::from_str("\"full\"")?;
        assert_eq!(full, BackupScope::Full);
        Ok(())
    }
}
