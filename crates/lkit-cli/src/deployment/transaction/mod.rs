pub(crate) mod recovery;
pub(crate) mod validation;

use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::plan::InstallError;
use super::root::InstallRoot;
use super::systemd::Systemd;

pub(crate) const TRANSACTION_SCHEMA_VERSION: u64 = 3;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Operation {
    Install,
    Repair,
    Switch,
    ServiceMigration,
}

impl Operation {
    pub(crate) fn key(&self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Repair => "repair",
            Self::Switch => "switch",
            Self::ServiceMigration => "service_migration",
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct HostServiceBefore {
    pub unit: String,
    pub installed: bool,
    pub active: bool,
    pub enable_state: String,
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
    /// 停止服务且用户显式 `--allow-no-backup` 时记录 true:
    /// 本事务没有 `.lkb` 配置快照,失败回滚不能恢复之前的数据。
    #[serde(default)]
    pub no_backup: bool,
    pub static_backup: Option<StaticBackupRef>,
    pub systemd_before: Option<SystemdBefore>,
    pub resolv_conf_backup: Option<String>,
    #[serde(default)]
    pub network_takeover: Option<NetworkTakeoverTransaction>,
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
            no_backup: false,
            static_backup: None,
            systemd_before: None,
            resolv_conf_backup: None,
            network_takeover: None,
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
            no_backup: false,
            static_backup: None,
            systemd_before: None,
            resolv_conf_backup: None,
            network_takeover: None,
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
            no_backup: false,
            static_backup: None,
            systemd_before: None,
            resolv_conf_backup: None,
            network_takeover: None,
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
            no_backup: false,
            static_backup: None,
            systemd_before: Some(systemd_before),
            resolv_conf_backup: None,
            network_takeover: None,
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

pub(crate) async fn recover_interrupted<P: super::health::DocsProbe>(
    root: &InstallRoot,
    transaction: &TransactionFile,
    systemd: &Systemd,
    health: &super::health::HealthOptions<P>,
) -> Result<(), InstallError> {
    recovery::recover_interrupted(root, transaction, systemd, health).await
}

pub(crate) fn cleanup_failed_first_install(
    root: &InstallRoot,
    transaction: &TransactionFile,
    systemd: &Systemd,
) -> Result<(), InstallError> {
    if let Some(before) = &transaction.systemd_before {
        let unit_origin = root.canonical.join("service/landscape-router.service");
        super::systemd::restore_systemd_before(systemd, before, &unit_origin)?;
        if let Some(backup_path) = &transaction.resolv_conf_backup {
            let backup_dir = root.canonical.join(backup_path);
            super::resolv::restore(&systemd.resolv_conf, &backup_dir)?;
        }
    }
    if let Some(target_release) = transaction.target_release.as_deref() {
        let _ = std::fs::remove_dir_all(root.canonical.join(target_release));
    }
    if let Some(target_version) = transaction.target_version.as_deref() {
        let _ = std::fs::remove_dir_all(
            root.canonical
                .join("releases")
                .join(format!(".install-{target_version}.tmp")),
        );
    }
    let _ = std::fs::remove_file(root.canonical.join("run/.current.tmp"));
    if let Some(target_release) = transaction.target_release.as_deref()
        && let Ok(target) = std::fs::read_link(root.canonical.join("current"))
        && target == Path::new(target_release)
    {
        let _ = std::fs::remove_file(root.canonical.join("current"));
    }
    let _ = std::fs::remove_file(root.canonical.join("data/landscape_init.toml"));
    let _ = std::fs::remove_file(root.canonical.join("state/install-state.json"));
    Ok(())
}

/// Strict cleanup for an uncommitted network takeover install.
///
/// Unlike ordinary first-install failure cleanup, this path may remove the
/// entire Landscape data directory. It is only valid while the install has no
/// previous version, backup, or committed state.
pub(crate) fn cleanup_uncommitted_network_install(
    root: &InstallRoot,
    transaction: &TransactionFile,
) -> Result<(), InstallError> {
    validate_network_takeover_rollback(root, transaction)?;
    let current_present = validate_current_for_target(root, transaction.target_release.as_deref())?;

    if current_present {
        std::fs::remove_file(root.canonical.join("current")).map_err(InstallError::Io)?;
    }
    if let Some(target_release) = transaction.target_release.as_deref() {
        remove_path_if_present(&root.canonical.join(target_release))?;
    }
    if let Some(target_version) = transaction.target_version.as_deref() {
        remove_path_if_present(
            &root
                .canonical
                .join("releases")
                .join(format!(".install-{target_version}.tmp")),
        )?;
    }
    remove_path_if_present(&root.canonical.join("run/.current.tmp"))?;
    remove_path_if_present(&root.canonical.join("state/install-state.json"))?;
    if let Some(network) = &transaction.network_takeover {
        remove_path_if_present(&root.canonical.join(&network.pending_state))?;
    }
    remove_path_if_present(&root.canonical.join("data"))?;
    Ok(())
}

pub(crate) fn restore_uncommitted_network_systemd(
    root: &InstallRoot,
    transaction: &TransactionFile,
    systemd: &Systemd,
) -> Result<(), InstallError> {
    validate_network_takeover_rollback(root, transaction)?;
    if let Some(before) = &transaction.systemd_before {
        let unit_origin = root.canonical.join("service/landscape-router.service");
        super::systemd::restore_systemd_before(systemd, before, &unit_origin)?;
        if let Some(backup_path) = &transaction.resolv_conf_backup {
            let backup_dir = root.canonical.join(backup_path);
            super::resolv::restore(&systemd.resolv_conf, &backup_dir)?;
        }
    }
    Ok(())
}

fn validate_network_takeover_rollback(
    root: &InstallRoot,
    transaction: &TransactionFile,
) -> Result<(), InstallError> {
    if transaction.operation != Operation::Install
        || transaction.network_takeover.is_none()
        || !matches!(
            transaction.phase,
            Phase::AwaitingNetworkConfirmation | Phase::Finalizing | Phase::RollingBack
        )
    {
        return Err(InstallError::BlockedByTransaction(format!(
            "transaction {} is not an uncommitted network takeover install",
            transaction.transaction_id
        )));
    }
    if transaction.from_version.is_some()
        || transaction.previous_current.is_some()
        || transaction.backup.is_some()
        || super::state::load_state(root)?.is_some()
    {
        return Err(InstallError::CorruptedTransaction(
            "network takeover rollback would affect an already committed installation".into(),
        ));
    }
    let data = root.canonical.join("data");
    if let Ok(metadata) = std::fs::symlink_metadata(&data) {
        if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
            return Err(InstallError::DangerousDirectory(format!(
                "{} is not a real data directory",
                data.display()
            )));
        }
    }
    Ok(())
}

fn validate_current_for_target(
    root: &InstallRoot,
    target_release: Option<&str>,
) -> Result<bool, InstallError> {
    let current = root.canonical.join("current");
    let metadata = match std::fs::symlink_metadata(&current) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(InstallError::Io(error)),
    };
    if !metadata.file_type().is_symlink() {
        return Err(InstallError::CorruptedTransaction(
            "current is not a symbolic link during network rollback".into(),
        ));
    }
    let target = std::fs::read_link(&current).map_err(InstallError::Io)?;
    if Some(target.as_path()) != target_release.map(Path::new) {
        return Err(InstallError::CorruptedTransaction(format!(
            "current points to {} instead of the network takeover target",
            target.display()
        )));
    }
    Ok(true)
}

fn remove_path_if_present(path: &Path) -> Result<(), InstallError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(InstallError::Io(error)),
    };
    if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
        std::fs::remove_dir_all(path).map_err(InstallError::Io)
    } else {
        std::fs::remove_file(path).map_err(InstallError::Io)
    }
}

pub(crate) fn validate_transaction(transaction: &TransactionFile) -> Result<(), InstallError> {
    validation::validate_transaction(transaction)
}

fn write_transaction(
    root: &InstallRoot,
    transaction: &TransactionFile,
) -> Result<(), InstallError> {
    validate_transaction(transaction)?;
    let dir = root.canonical.join("transactions");
    std::fs::create_dir_all(&dir).map_err(InstallError::Io)?;
    let bytes = serde_json::to_vec_pretty(transaction).map_err(InstallError::StateWrite)?;
    let path = dir.join(format!("{}.json", transaction.transaction_id));
    let tmp = dir.join(format!(".transaction.{}.tmp", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(InstallError::Io)?;
    file.write_all(&bytes).map_err(InstallError::Io)?;
    file.sync_all().map_err(InstallError::Io)?;
    std::fs::rename(&tmp, &path).map_err(InstallError::Io)?;
    Ok(())
}

fn load_transaction_file(root: &InstallRoot, path: &Path) -> Result<TransactionFile, InstallError> {
    let bytes = std::fs::read(path).map_err(InstallError::Io)?;
    let transaction: TransactionFile = serde_json::from_slice(&bytes).map_err(|error| {
        InstallError::CorruptedTransaction(format!(
            "{} is not a valid transaction: {error}",
            path.display()
        ))
    })?;
    validate_transaction(&transaction)?;
    if Path::new(&transaction.canonical_install_root) != root.canonical {
        return Err(InstallError::CorruptedTransaction(format!(
            "{} records canonical_install_root {} which does not match the real install root {}",
            path.display(),
            transaction.canonical_install_root,
            root.canonical.display()
        )));
    }
    Ok(transaction)
}

fn append_log(
    root: &InstallRoot,
    transaction: &TransactionFile,
    line: &str,
) -> Result<(), InstallError> {
    let log_path = root.canonical.join(&transaction.log_path);
    let mut log = OpenOptions::new()
        .append(true)
        .mode(0o600)
        .open(&log_path)
        .map_err(InstallError::Io)?;
    writeln!(log, "{line}").map_err(InstallError::Io)?;
    log.sync_all().map_err(InstallError::Io)?;
    Ok(())
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
    struct FakeDocs;

    impl super::super::health::DocsProbe for FakeDocs {
        async fn docs_ok(&self) -> bool {
            true
        }
    }

    fn test_health() -> super::super::health::HealthOptions<FakeDocs> {
        super::super::health::HealthOptions {
            docs: FakeDocs,
            ports: Vec::new(),
            startup_timeout: std::time::Duration::from_secs(5),
            stable_duration: std::time::Duration::from_millis(100),
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
        assert_eq!(transaction.schema_version, 3);
        assert_eq!(
            transaction.target_release.as_deref(),
            Some("releases/1.2.3")
        );
        assert!(validate_transaction(&transaction).is_ok());
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn accepts_v1_transactions_and_names_stopping_phase() {
        let temp = temp_root("schema-compatibility");
        let root = new_root(&temp);
        let mut transaction = install_transaction(&root);
        transaction.schema_version = 1;
        assert!(validate_transaction(&transaction).is_ok());
        assert_eq!(Phase::Stopping.key(), "stopping");
        transaction.phase = Phase::Stopping;
        assert!(validate_transaction(&transaction).is_err());
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

    #[test]
    fn rejects_corrupted_transactions() {
        let temp = temp_root("corrupt");
        let root = new_root(&temp);
        std::fs::create_dir_all(temp.join("transactions")).unwrap();
        std::fs::write(temp.join("transactions/bad.json"), b"not json").unwrap();
        assert!(matches!(
            find_unfinished(&root),
            Err(InstallError::CorruptedTransaction(_))
        ));

        let mut transaction = install_transaction(&root);
        transaction.schema_version = 4;
        assert!(validate_transaction(&transaction).is_err());

        let mut transaction = install_transaction(&root);
        transaction.log_path = "../escape.log".into();
        assert!(validate_transaction(&transaction).is_err());

        let mut transaction = install_transaction(&root);
        transaction.target_release = Some("../escape".into());
        assert!(validate_transaction(&transaction).is_err());
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn validates_operation_specific_rules() {
        let temp = temp_root("ops");
        let root = new_root(&temp);
        let mut transaction = install_transaction(&root);

        transaction.operation = Operation::Switch;
        transaction.from_version = Some("1.1.0".into());
        transaction.previous_current = Some("releases/1.1.0".into());
        assert!(validate_transaction(&transaction).is_ok());
        transaction.phase = Phase::Prepared;
        assert!(validate_transaction(&transaction).is_err());
        transaction.phase = Phase::Failed;
        assert!(validate_transaction(&transaction).is_ok());

        transaction.backup = Some(BackupRef {
            backup_id: "b".into(),
            path: "backups/b.lkb".into(),
            sha256: "a".repeat(64),
        });
        assert!(validate_transaction(&transaction).is_ok());

        transaction.operation = Operation::ServiceMigration;
        transaction.backup = None;
        transaction.from_version = None;
        transaction.previous_current = None;
        transaction.target_version = None;
        transaction.target_release = None;
        assert!(validate_transaction(&transaction).is_err());
        transaction.from_service_manager = Some(TransactionServiceManager::Systemd);
        transaction.target_service_manager = Some(TransactionServiceManager::None);
        assert!(validate_transaction(&transaction).is_err());
        transaction.systemd_before = Some(SystemdBefore {
            registration: Registration {
                kind: RegistrationKind::Missing,
                target: None,
            },
            enabled: false,
            active: false,
        });
        assert!(validate_transaction(&transaction).is_ok());
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn recovers_interrupted_install() {
        let temp = temp_root("recover");
        let root = new_root(&temp);
        let transaction = install_transaction(&root);
        begin(&root, &transaction).unwrap();
        mark_phase(&root, &transaction, Phase::Activating).unwrap();

        std::fs::create_dir_all(temp.join("releases/1.2.3/static")).unwrap();
        std::os::unix::fs::symlink("releases/1.2.3", temp.join("current")).unwrap();
        std::fs::create_dir_all(temp.join("data")).unwrap();
        std::fs::write(
            temp.join("data/landscape_init.toml"),
            b"version = \"1.2.3\"",
        )
        .unwrap();
        std::fs::create_dir_all(temp.join("state")).unwrap();
        std::fs::write(temp.join("state/install-state.json"), b"{}").unwrap();

        let tx = find_unfinished(&root).unwrap().unwrap();
        let health = test_health();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            recover_interrupted(&root, &tx, &Systemd::host(), &health)
                .await
                .unwrap()
        });
        assert!(!temp.join("releases/1.2.3").exists());
        assert!(!temp.join("current").exists());
        assert!(!temp.join("data/landscape_init.toml").exists());
        assert!(!temp.join("state/install-state.json").exists());
        assert!(find_unfinished(&root).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn keeps_completed_install_on_recovery() {
        let temp = temp_root("keep");
        let root = new_root(&temp);
        let transaction = install_transaction(&root);
        begin(&root, &transaction).unwrap();
        mark_phase(&root, &transaction, Phase::Activating).unwrap();

        std::fs::create_dir_all(temp.join("releases/1.2.3")).unwrap();
        std::os::unix::fs::symlink("releases/1.2.3", temp.join("current")).unwrap();
        std::fs::create_dir_all(temp.join("state")).unwrap();
        let state = serde_json::json!({
            "schema_version": 1,
            "layout_version": 1,
            "install_root": temp.display().to_string(),
            "canonical_install_root": std::fs::canonicalize(&temp).unwrap().display().to_string(),
            "active_version": "1.2.3",
            "repository": {"kind": "http", "location": "https://example.com/"},
            "assets": {
                "webserver": {"architecture": "x86_64", "sha256": "a".repeat(64), "size": 1},
                "static_archive": {"sha256": "b".repeat(64), "size": 1}
            },
            "initialization": {"status": "pending", "lock_present": false, "initialized_at": null},
            "service": {"manager": "none", "registered": false, "enabled": false, "verified": false, "definition_path": null, "definition_sha256": null},
            "last_transaction_id": null,
            "committed_at": null
        });
        std::fs::write(temp.join("state/install-state.json"), state.to_string()).unwrap();

        let tx = find_unfinished(&root).unwrap().unwrap();
        let health = test_health();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            recover_interrupted(&root, &tx, &Systemd::host(), &health)
                .await
                .unwrap()
        });
        assert!(temp.join("releases/1.2.3").exists());
        assert!(temp.join("current").exists());
        assert!(temp.join("state/install-state.json").exists());
        let tx = load_finished(&root, &temp);
        assert_eq!(tx.phase, Phase::Committed);
        assert!(find_unfinished(&root).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&temp);
    }

    fn load_finished(root: &InstallRoot, temp: &std::path::Path) -> TransactionFile {
        let entries: Vec<_> = std::fs::read_dir(temp.join("transactions"))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(entries.len(), 1);
        load_transaction_file(root, &entries[0].path()).unwrap()
    }

    #[test]
    fn recovers_failed_switch_before_backup_creation() {
        let temp = temp_root("switch-recover");
        let root = new_root(&temp);
        let transaction = TransactionFile::new_switch(
            &root,
            &semver::Version::new(1, 1, 0),
            &semver::Version::new(1, 2, 3),
        )
        .unwrap();
        begin(&root, &transaction).unwrap();
        mark_phase(&root, &transaction, Phase::Failed).unwrap();
        assert!(find_unfinished(&root).unwrap().is_none());

        let transaction = TransactionFile::new_switch(
            &root,
            &semver::Version::new(1, 1, 0),
            &semver::Version::new(1, 3, 0),
        )
        .unwrap();
        begin(&root, &transaction).unwrap();
        std::fs::create_dir_all(temp.join("releases/1.3.0/static")).unwrap();

        let tx = find_unfinished(&root).unwrap().unwrap();
        let health = test_health();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            recover_interrupted(&root, &tx, &Systemd::host(), &health)
                .await
                .unwrap()
        });
        assert!(!temp.join("releases/1.3.0").exists());
        assert!(find_unfinished(&root).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn recovers_repair_transaction_in_preparing() {
        let temp = temp_root("repair-recover");
        let root = new_root(&temp);
        let tx = TransactionFile::new_repair_binary(&root, &semver::Version::new(1, 1, 0)).unwrap();
        begin(&root, &tx).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            recover_interrupted(&root, &tx, &Systemd::host(), &test_health())
                .await
                .unwrap()
        });
        assert!(find_unfinished(&root).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn recovers_service_migration_transaction_in_preparing() {
        let temp = temp_root("migration-recover");
        let root = new_root(&temp);
        let before = SystemdBefore {
            registration: Registration {
                kind: RegistrationKind::Missing,
                target: None,
            },
            enabled: false,
            active: false,
        };
        let tx = TransactionFile::new_service_migration(
            &root,
            TransactionServiceManager::None,
            TransactionServiceManager::Systemd,
            before,
        )
        .unwrap();
        begin(&root, &tx).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            recover_interrupted(&root, &tx, &Systemd::host(), &test_health())
                .await
                .unwrap()
        });
        assert!(find_unfinished(&root).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&temp);
    }
}
