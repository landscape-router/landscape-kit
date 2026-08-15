mod cleanup;
mod file;
pub(crate) mod recovery;
pub(crate) mod validation;

use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::plan::InstallError;
use super::root::InstallRoot;
use super::systemd::Systemd;

pub(crate) use self::cleanup::{
    cleanup_failed_first_install, cleanup_uncommitted_network_install,
    restore_uncommitted_network_systemd,
};
use self::file::{append_log, load_transaction_file, write_transaction};

pub(crate) const TRANSACTION_SCHEMA_VERSION: u64 = 4;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Operation {
    Install,
    Repair,
    Switch,
    Restore,
    ServiceMigration,
    Uninstall,
    Reinit,
    Migrate,
}

impl Operation {
    pub(crate) fn key(&self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Repair => "repair",
            Self::Switch => "switch",
            Self::Restore => "restore",
            Self::ServiceMigration => "service_migration",
            Self::Uninstall => "uninstall",
            Self::Reinit => "reinit",
            Self::Migrate => "migrate",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Phase {
    Preparing,
    Prepared,
    Stopping,
    Activating,
    Verifying,
    AwaitingNetworkConfirmation,
    Finalizing,
    RollingBack,
    Committed,
    RolledBack,
    Failed,
}

impl Phase {
    pub(crate) fn key(&self) -> &'static str {
        match self {
            Self::Preparing => "preparing",
            Self::Prepared => "prepared",
            Self::Stopping => "stopping",
            Self::Activating => "activating",
            Self::Verifying => "verifying",
            Self::AwaitingNetworkConfirmation => "awaiting_network_confirmation",
            Self::Finalizing => "finalizing",
            Self::RollingBack => "rolling_back",
            Self::Committed => "committed",
            Self::RolledBack => "rolled_back",
            Self::Failed => "failed",
        }
    }

    pub(crate) fn is_terminal(self) -> bool {
        matches!(self, Self::Committed | Self::RolledBack | Self::Failed)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum TransactionServiceManager {
    Systemd,
    None,
}

impl TransactionServiceManager {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Systemd => "systemd",
            Self::None => "none",
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct BackupRef {
    pub backup_id: String,
    pub path: String,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct StaticBackupRef {
    pub path: String,
    pub target: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct SystemdBefore {
    pub registration: Registration,
    pub enabled: bool,
    pub active: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct HostServiceBefore {
    pub unit: String,
    pub installed: bool,
    pub active: bool,
    pub enable_state: String,
}

/// migrate 事务记录的旧部署 systemd unit 事务前状态。
/// 旧 unit 原件位于 `/etc/systemd/system` 时,`stop` 后会把 unit 文件移入事务目录
/// (`file_moved: true`),因为 `mask` 会在该目录创建同名符号链接,与受管
/// `landscape-router.service` 的注册冲突;位于 `/usr/lib` 或 `/run` 时走
/// `stop + disable + mask`。
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct LegacyUnitBefore {
    pub unit: String,
    pub installed: bool,
    pub active: bool,
    pub enable_state: String,
    #[serde(default)]
    pub file_moved: bool,
    /// 旧 unit 原件路径(`file_moved` 时非空)。
    #[serde(default)]
    pub file_path: Option<String>,
    /// 事务目录内相对备份路径(`file_moved` 时非空)。
    #[serde(default)]
    pub file_backup: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct NetworkTakeoverTransaction {
    pub plan: crate::network::config::NetworkPlan,
    pub host_services: Vec<HostServiceBefore>,
    pub confirmation_deadline: DateTime<Utc>,
    pub rollback_service: String,
    pub rollback_timer: String,
    pub boot_rollback_service: String,
    pub recovery_binary: String,
    pub pending_state: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct Registration {
    pub kind: RegistrationKind,
    pub target: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum RegistrationKind {
    Missing,
    Symlink,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct TransactionFile {
    pub schema_version: u64,
    pub transaction_id: String,
    pub operation: Operation,
    pub phase: Phase,
    pub install_root: String,
    pub canonical_install_root: String,
    pub from_version: Option<String>,
    pub target_version: Option<String>,
    pub from_service_manager: Option<TransactionServiceManager>,
    pub target_service_manager: Option<TransactionServiceManager>,
    pub previous_current: Option<String>,
    pub target_release: Option<String>,
    pub backup: Option<BackupRef>,
    /// restore 事务记录的用户选择的目标 `.lkb`。其他 operation 必须为 null。
    #[serde(default)]
    pub restore_backup: Option<BackupRef>,
    /// 停止服务且用户显式 `--allow-no-backup` 时记录 true:
    /// 本事务没有 `.lkb` 配置快照,失败回滚不能恢复之前的数据。
    #[serde(default)]
    pub no_backup: bool,
    pub static_backup: Option<StaticBackupRef>,
    pub systemd_before: Option<SystemdBefore>,
    pub resolv_conf_backup: Option<String>,
    #[serde(default)]
    pub network_takeover: Option<NetworkTakeoverTransaction>,
    /// migrate 事务记录被接管的旧部署 systemd unit 事务前状态。
    /// 旧实例为前台进程时保持 None。
    #[serde(default)]
    pub legacy_unit: Option<LegacyUnitBefore>,
    pub log_path: String,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl TransactionFile {
    pub(crate) fn new_install(
        root: &InstallRoot,
        version: &semver::Version,
    ) -> Result<Self, InstallError> {
        let transaction_id = Uuid::now_v7().to_string();
        let now = Utc::now();
        let transaction = Self {
            schema_version: TRANSACTION_SCHEMA_VERSION,
            transaction_id: transaction_id.clone(),
            operation: Operation::Install,
            phase: Phase::Preparing,
            install_root: root.install_root.display().to_string(),
            canonical_install_root: root.canonical.display().to_string(),
            from_version: None,
            target_version: Some(version.to_string()),
            from_service_manager: None,
            target_service_manager: None,
            previous_current: None,
            target_release: Some(format!("releases/{version}")),
            backup: None,
            restore_backup: None,
            no_backup: false,
            static_backup: None,
            systemd_before: None,
            resolv_conf_backup: None,
            network_takeover: None,
            legacy_unit: None,
            log_path: format!("logs/{transaction_id}.log"),
            started_at: now,
            updated_at: now,
        };
        validate_transaction(&transaction)?;
        Ok(transaction)
    }

    pub(crate) fn new_switch(
        root: &InstallRoot,
        from_version: &semver::Version,
        target_version: &semver::Version,
    ) -> Result<Self, InstallError> {
        let transaction_id = Uuid::now_v7().to_string();
        let now = Utc::now();
        let transaction = Self {
            schema_version: TRANSACTION_SCHEMA_VERSION,
            transaction_id: transaction_id.clone(),
            operation: Operation::Switch,
            phase: Phase::Preparing,
            install_root: root.install_root.display().to_string(),
            canonical_install_root: root.canonical.display().to_string(),
            from_version: Some(from_version.to_string()),
            target_version: Some(target_version.to_string()),
            from_service_manager: None,
            target_service_manager: None,
            previous_current: Some(format!("releases/{from_version}")),
            target_release: Some(format!("releases/{target_version}")),
            backup: None,
            restore_backup: None,
            no_backup: false,
            static_backup: None,
            systemd_before: None,
            resolv_conf_backup: None,
            network_takeover: None,
            legacy_unit: None,
            log_path: format!("logs/{transaction_id}.log"),
            started_at: now,
            updated_at: now,
        };
        validate_transaction(&transaction)?;
        Ok(transaction)
    }

    /// 从 `.lkb` 恢复事务:目标版本由备份 metadata 决定,可以同版本、较低或较高,
    /// 不经过仓库下载。`restore_backup` 在事务创建后由调用方记录用户选择的目标备份。
    pub(crate) fn new_restore(
        root: &InstallRoot,
        from_version: &semver::Version,
        target_version: &semver::Version,
    ) -> Result<Self, InstallError> {
        let transaction_id = Uuid::now_v7().to_string();
        let now = Utc::now();
        let transaction = Self {
            schema_version: TRANSACTION_SCHEMA_VERSION,
            transaction_id: transaction_id.clone(),
            operation: Operation::Restore,
            phase: Phase::Preparing,
            install_root: root.install_root.display().to_string(),
            canonical_install_root: root.canonical.display().to_string(),
            from_version: Some(from_version.to_string()),
            target_version: Some(target_version.to_string()),
            from_service_manager: None,
            target_service_manager: None,
            previous_current: Some(format!("releases/{from_version}")),
            target_release: Some(format!("releases/{target_version}")),
            backup: None,
            restore_backup: None,
            no_backup: false,
            static_backup: None,
            systemd_before: None,
            resolv_conf_backup: None,
            network_takeover: None,
            legacy_unit: None,
            log_path: format!("logs/{transaction_id}.log"),
            started_at: now,
            updated_at: now,
        };
        validate_transaction(&transaction)?;
        Ok(transaction)
    }

    /// 后端修复事务:同版本重装可信后端,创建 `.lkb` 备份。
    pub(crate) fn new_repair_binary(
        root: &InstallRoot,
        version: &semver::Version,
    ) -> Result<Self, InstallError> {
        Self::new_repair(root, Some(version.to_string()))
    }

    /// 纯静态页面修复事务:不改变版本关系,只备份并替换 `static/`。
    pub(crate) fn new_repair_static(root: &InstallRoot) -> Result<Self, InstallError> {
        Self::new_repair(root, None)
    }

    /// 无 systemd 环境 pending→complete 初始化观测 repair:不备份、不改变版本资产。
    pub(crate) fn new_observation_repair(root: &InstallRoot) -> Result<Self, InstallError> {
        Self::new_repair(root, None)
    }

    fn new_repair(root: &InstallRoot, version: Option<String>) -> Result<Self, InstallError> {
        let transaction_id = Uuid::now_v7().to_string();
        let now = Utc::now();
        let transaction = Self {
            schema_version: TRANSACTION_SCHEMA_VERSION,
            transaction_id: transaction_id.clone(),
            operation: Operation::Repair,
            phase: Phase::Preparing,
            install_root: root.install_root.display().to_string(),
            canonical_install_root: root.canonical.display().to_string(),
            from_version: version.clone(),
            target_version: version,
            from_service_manager: None,
            target_service_manager: None,
            previous_current: None,
            target_release: None,
            backup: None,
            restore_backup: None,
            no_backup: false,
            static_backup: None,
            systemd_before: None,
            resolv_conf_backup: None,
            network_takeover: None,
            legacy_unit: None,
            log_path: format!("logs/{transaction_id}.log"),
            started_at: now,
            updated_at: now,
        };
        validate_transaction(&transaction)?;
        Ok(transaction)
    }

    /// service manager 迁移事务。`systemd_before` 是事务开始前的受管状态,
    /// 在创建事务前完成捕获。
    pub(crate) fn new_service_migration(
        root: &InstallRoot,
        from: TransactionServiceManager,
        to: TransactionServiceManager,
        systemd_before: SystemdBefore,
    ) -> Result<Self, InstallError> {
        let transaction_id = Uuid::now_v7().to_string();
        let now = Utc::now();
        let transaction = Self {
            schema_version: TRANSACTION_SCHEMA_VERSION,
            transaction_id: transaction_id.clone(),
            operation: Operation::ServiceMigration,
            phase: Phase::Preparing,
            install_root: root.install_root.display().to_string(),
            canonical_install_root: root.canonical.display().to_string(),
            from_version: None,
            target_version: None,
            from_service_manager: Some(from),
            target_service_manager: Some(to),
            previous_current: None,
            target_release: None,
            backup: None,
            restore_backup: None,
            no_backup: false,
            static_backup: None,
            systemd_before: Some(systemd_before),
            resolv_conf_backup: None,
            network_takeover: None,
            legacy_unit: None,
            log_path: format!("logs/{transaction_id}.log"),
            started_at: now,
            updated_at: now,
        };
        validate_transaction(&transaction)?;
        Ok(transaction)
    }

    /// 迁移事务:从非 lkit 手工部署接管到全新安装根。`target_version` 是被迁移
    /// 部署导出的版本(备份不升级),`backup` 是迁移 `.lkb`,在创建后由调用方记录;
    /// `legacy_unit` 在停止旧 systemd unit 前由调用方记录。
    pub(crate) fn new_migrate(
        root: &InstallRoot,
        version: &semver::Version,
    ) -> Result<Self, InstallError> {
        let transaction_id = Uuid::now_v7().to_string();
        let now = Utc::now();
        let transaction = Self {
            schema_version: TRANSACTION_SCHEMA_VERSION,
            transaction_id: transaction_id.clone(),
            operation: Operation::Migrate,
            phase: Phase::Preparing,
            install_root: root.install_root.display().to_string(),
            canonical_install_root: root.canonical.display().to_string(),
            from_version: None,
            target_version: Some(version.to_string()),
            from_service_manager: None,
            target_service_manager: None,
            previous_current: None,
            target_release: Some(format!("releases/{version}")),
            backup: None,
            restore_backup: None,
            no_backup: false,
            static_backup: None,
            systemd_before: None,
            resolv_conf_backup: None,
            network_takeover: None,
            legacy_unit: None,
            log_path: format!("logs/{transaction_id}.log"),
            started_at: now,
            updated_at: now,
        };
        validate_transaction(&transaction)?;
        Ok(transaction)
    }

    /// 卸载事务:记录当前已提交版本关系。`backup` 在保护 `.lkb` 创建成功后由调用方
    /// 记录;`--allow-no-backup` 时保持 null 且 `no_backup: true`。
    pub(crate) fn new_uninstall(
        root: &InstallRoot,
        version: &semver::Version,
    ) -> Result<Self, InstallError> {
        let transaction_id = Uuid::now_v7().to_string();
        let now = Utc::now();
        let transaction = Self {
            schema_version: TRANSACTION_SCHEMA_VERSION,
            transaction_id: transaction_id.clone(),
            operation: Operation::Uninstall,
            phase: Phase::Preparing,
            install_root: root.install_root.display().to_string(),
            canonical_install_root: root.canonical.display().to_string(),
            from_version: Some(version.to_string()),
            target_version: None,
            from_service_manager: None,
            target_service_manager: None,
            previous_current: Some(format!("releases/{version}")),
            target_release: None,
            backup: None,
            restore_backup: None,
            no_backup: false,
            static_backup: None,
            systemd_before: None,
            resolv_conf_backup: None,
            network_takeover: None,
            legacy_unit: None,
            log_path: format!("logs/{transaction_id}.log"),
            started_at: now,
            updated_at: now,
        };
        validate_transaction(&transaction)?;
        Ok(transaction)
    }

    /// reinit 事务:同版本配置重建,不改变版本关系。`backup` 在保护 `.lkb` 创建成功后
    /// 由调用方记录;`--allow-no-backup` 时保持 null 且 `no_backup: true`。
    /// `network_takeover` 在健康检查通过、arm 恢复机制后由调用方记录。
    pub(crate) fn new_reinit(
        root: &InstallRoot,
        version: &semver::Version,
    ) -> Result<Self, InstallError> {
        let transaction_id = Uuid::now_v7().to_string();
        let now = Utc::now();
        let transaction = Self {
            schema_version: TRANSACTION_SCHEMA_VERSION,
            transaction_id: transaction_id.clone(),
            operation: Operation::Reinit,
            phase: Phase::Preparing,
            install_root: root.install_root.display().to_string(),
            canonical_install_root: root.canonical.display().to_string(),
            from_version: Some(version.to_string()),
            target_version: None,
            from_service_manager: None,
            target_service_manager: None,
            previous_current: None,
            target_release: None,
            backup: None,
            restore_backup: None,
            no_backup: false,
            static_backup: None,
            systemd_before: None,
            resolv_conf_backup: None,
            network_takeover: None,
            legacy_unit: None,
            log_path: format!("logs/{transaction_id}.log"),
            started_at: now,
            updated_at: now,
        };
        validate_transaction(&transaction)?;
        Ok(transaction)
    }
}

pub(crate) fn begin(root: &InstallRoot, transaction: &TransactionFile) -> Result<(), InstallError> {
    validate_transaction(transaction)?;
    if find_unfinished(root)?.is_some() {
        return Err(InstallError::BlockedByTransaction(
            "another unfinished transaction already exists".into(),
        ));
    }
    std::fs::create_dir_all(root.canonical.join("logs")).map_err(InstallError::Io)?;
    let log_path = root.canonical.join(&transaction.log_path);
    let mut log = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&log_path)
        .map_err(|_| {
            InstallError::Io(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("transaction log {} already exists", log_path.display()),
            ))
        })?;
    writeln!(log, "phase: {}", transaction.phase.key()).map_err(InstallError::Io)?;
    log.sync_all().map_err(InstallError::Io)?;
    write_transaction(root, transaction)
}

pub(crate) fn mark_phase(
    root: &InstallRoot,
    transaction: &TransactionFile,
    phase: Phase,
) -> Result<(), InstallError> {
    append_log(root, transaction, &format!("phase: {}", phase.key()))?;
    let mut updated = transaction.clone();
    updated.phase = phase;
    updated.updated_at = Utc::now();
    write_transaction(root, &updated)
}

pub(crate) fn persist(
    root: &InstallRoot,
    transaction: &TransactionFile,
) -> Result<(), InstallError> {
    validate_transaction(transaction)?;
    write_transaction(root, transaction)
}

pub(crate) fn find_unfinished(root: &InstallRoot) -> Result<Option<TransactionFile>, InstallError> {
    let dir = root.canonical.join("transactions");
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(InstallError::Io(error)),
    };
    for entry in entries {
        let entry = entry.map_err(InstallError::Io)?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let transaction = load_transaction_file(root, &path)?;
        if !transaction.phase.is_terminal() {
            return Ok(Some(transaction));
        }
    }
    Ok(None)
}

/// 查找指定 operation 的已提交事务(用于识别已被中断恢复完成的卸载)。
pub(crate) fn find_committed_operation(
    root: &InstallRoot,
    operation: Operation,
) -> Result<Option<TransactionFile>, InstallError> {
    let dir = root.canonical.join("transactions");
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(InstallError::Io(error)),
    };
    for entry in entries {
        let entry = entry.map_err(InstallError::Io)?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let transaction = load_transaction_file(root, &path)?;
        if transaction.operation == operation && transaction.phase == Phase::Committed {
            return Ok(Some(transaction));
        }
    }
    Ok(None)
}

pub(crate) async fn recover_interrupted<P: super::health::DocsProbe>(
    root: &InstallRoot,
    transaction: &TransactionFile,
    systemd: &Systemd,
    health: &super::health::HealthOptions<P>,
) -> Result<(), InstallError> {
    recovery::recover_interrupted(root, transaction, systemd, health).await
}

pub(crate) fn validate_transaction(transaction: &TransactionFile) -> Result<(), InstallError> {
    validation::validate_transaction(transaction)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("lkit-tx-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn new_root(path: &std::path::Path) -> InstallRoot {
        InstallRoot {
            install_root: path.to_path_buf(),
            canonical: path.to_path_buf(),
        }
    }

    fn install_transaction(root: &InstallRoot) -> TransactionFile {
        TransactionFile::new_install(root, &semver::Version::new(1, 2, 3)).unwrap()
    }

    #[test]
    fn creates_valid_install_transaction() {
        let temp = temp_root("valid");
        let root = new_root(&temp);
        let transaction = install_transaction(&root);
        assert_eq!(transaction.operation, Operation::Install);
        assert_eq!(transaction.phase, Phase::Preparing);
        assert_eq!(transaction.schema_version, 4);
        assert_eq!(
            transaction.target_release.as_deref(),
            Some("releases/1.2.3")
        );
        assert!(validate_transaction(&transaction).is_ok());
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn begins_and_commits_transaction() {
        let temp = temp_root("lifecycle");
        let root = new_root(&temp);
        let transaction = install_transaction(&root);
        begin(&root, &transaction).unwrap();
        assert!(temp.join(&transaction.log_path).is_file());
        assert!(
            temp.join("transactions")
                .join(format!("{}.json", transaction.transaction_id))
                .is_file()
        );
        assert!(find_unfinished(&root).unwrap().is_some());
        mark_phase(&root, &transaction, Phase::Prepared).unwrap();
        mark_phase(&root, &transaction, Phase::Activating).unwrap();
        mark_phase(&root, &transaction, Phase::Committed).unwrap();
        assert!(find_unfinished(&root).unwrap().is_none());
        let log = std::fs::read_to_string(temp.join(&transaction.log_path)).unwrap();
        assert!(log.contains("phase: committed"));
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn marks_failed_and_ignores_on_detection() {
        let temp = temp_root("failed");
        let root = new_root(&temp);
        let transaction = install_transaction(&root);
        begin(&root, &transaction).unwrap();
        mark_phase(&root, &transaction, Phase::Failed).unwrap();
        assert!(find_unfinished(&root).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn rejects_second_unfinished_transaction() {
        let temp = temp_root("second");
        let root = new_root(&temp);
        let first = install_transaction(&root);
        begin(&root, &first).unwrap();
        let second = install_transaction(&root);
        assert!(matches!(
            begin(&root, &second),
            Err(InstallError::BlockedByTransaction(_))
        ));
        let _ = std::fs::remove_dir_all(&temp);
    }
}
