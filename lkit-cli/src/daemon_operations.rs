//! Durable, typed operations executed by the daemon without an lkit child.
mod install;
mod migrate;
pub(crate) use install::{InstallRequest, submit_install_and_wait};
pub(crate) use migrate::{MigrateRequest, SourceIdentity, submit_migrate_and_wait};
pub(crate) mod reinit;
use crate::deployment::{
    layout, lock, plan::InstallError, runtime::InstallRuntime, state, transaction,
};
pub(crate) use reinit::{ReinitRequest, submit_reinit_and_wait};
use serde::{Deserialize, Serialize};
use std::os::{
    fd::AsRawFd,
    unix::fs::{OpenOptionsExt, PermissionsExt},
};
use std::path::PathBuf;

fn error(message: impl Into<String>) -> InstallError {
    InstallError::Preflight(message.into())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Status {
    Queued,
    Running,
    Recovering,
    AwaitingConfirmation,
    Succeeded,
    Failed,
    Interrupted,
    Cancelled,
}
impl Status {
    pub(crate) fn terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Interrupted | Self::Cancelled
        )
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Kind {
    #[serde(rename = "repair_service")]
    Service,
    #[serde(rename = "repair_static")]
    Static,
    #[serde(rename = "repair_binary")]
    Binary,
    Switch,
    Restore,
    Uninstall,
    NetworkConfirm,
    NetworkRollback,
    Reinit,
    Install,
    Migrate,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Stage {
    #[default]
    Checking,
    Downloading,
    Preparing,
    Stopping,
    WaitingForSourceStop,
    Activating,
    Verifying,
    RollingBack,
    Finished,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AssetRequest {
    pub version: String,
    pub state_identity: String,
    pub repository: Option<crate::deployment::plan::RepositoryChoice>,
    pub official: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SwitchRequest {
    pub from_version: String,
    pub state_identity: String,
    pub target: String,
    pub repository: Option<crate::deployment::plan::RepositoryChoice>,
    pub accept_service_change: bool,
    pub allow_no_backup: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RestoreRequest {
    pub from_version: String,
    pub state_identity: String,
    pub source: PathBuf,
    pub backup_id: String,
    pub backup_sha256: String,
    pub target_version: String,
    pub allow_no_backup: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UninstallRequest {
    pub from_version: String,
    pub state_identity: String,
    pub allow_no_backup: bool,
    pub keep_data: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Record {
    pub schema_version: u32,
    pub id: String,
    pub kind: Kind,
    pub install_root: String,
    pub status: Status,
    pub transaction_id: Option<String>,
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assets: Option<AssetRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub switch: Option<SwitchRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore: Option<RestoreRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uninstall: Option<UninstallRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_transaction: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reinit: Option<ReinitRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install: Option<InstallRequest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub migrate: Option<MigrateRequest>,
    #[serde(default)]
    pub resolved_version: Option<String>,
    #[serde(default)]
    pub stage: Stage,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub cancel_requested: bool,
    #[serde(default)]
    pub cancel_cleaned: bool,
    #[serde(default)]
    pub cancellable: bool,
    #[serde(default)]
    pub downloaded_bytes: u64,
    #[serde(default)]
    pub total_bytes: Option<u64>,
    #[serde(default)]
    pub exit_code: Option<u8>,
}

pub(crate) fn installation_identity(
    installed: &state::InstallState,
) -> Result<String, InstallError> {
    Ok(crate::release::artifacts::hash_str(
        &serde_json::to_string(installed).map_err(InstallError::StateWrite)?,
    ))
}

fn directory() -> PathBuf {
    layout::territory_run_dir().join("operations")
}
fn validate_id(id: &str) -> Result<(), InstallError> {
    if uuid::Uuid::parse_str(id).is_ok_and(|uuid| uuid.to_string() == id) {
        Ok(())
    } else {
        Err(error("invalid operation or transaction ID"))
    }
}
fn path(id: &str) -> Result<PathBuf, InstallError> {
    validate_id(id)?;
    Ok(directory().join(format!("{id}.json")))
}
fn queue_lock() -> Result<std::fs::File, InstallError> {
    let dir = directory();
    std::fs::create_dir_all(&dir).map_err(InstallError::Io)?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
        .map_err(InstallError::Io)?;
    std::fs::File::open(layout::territory_run_dir())
        .and_then(|parent| parent.sync_all())
        .map_err(InstallError::Io)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(dir.join("queue.lock"))
        .map_err(InstallError::Io)?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(InstallError::Io(std::io::Error::last_os_error()));
    }
    Ok(file)
}
fn save(record: &Record) -> Result<(), InstallError> {
    crate::workflows::repair_service::atomic_write(
        &path(&record.id)?,
        &serde_json::to_vec_pretty(record).map_err(InstallError::StateWrite)?,
        0o600,
    )
}
pub(crate) fn read(id: &str) -> Result<Record, InstallError> {
    let bytes = std::fs::read(path(id)?).map_err(InstallError::Io)?;
    let record: Record = serde_json::from_slice(&bytes)
        .map_err(|e| error(format!("invalid operation record: {e}")))?;
    if !(1..=9).contains(&record.schema_version) || record.id != id {
        return Err(error("operation identity or schema mismatch"));
    }
    if let Some(id) = &record.transaction_id {
        validate_id(id)?;
    }
    if matches!(record.kind, Kind::Static | Kind::Binary) && record.schema_version != 2 {
        return Err(error("asset repair requires operation schema v2"));
    }
    if record.kind == Kind::Migrate {
        migrate::validate_record(&record)?;
        return Ok(record);
    }
    if record.migrate.is_some() {
        return Err(error("unexpected migration payload"));
    }
    if record.kind == Kind::Install {
        install::validate_record(&record)?;
        return Ok(record);
    }
    if record.install.is_some() {
        return Err(error("unexpected install payload"));
    }
    if record.kind == Kind::Reinit {
        reinit::validate_record(&record)?;
        return Ok(record);
    }
    if record.reinit.is_some() {
        return Err(error("unexpected reinit payload"));
    }
    if is_network(record.kind) {
        if record.schema_version != 6
            || record.assets.is_some()
            || record.switch.is_some()
            || record.restore.is_some()
            || record.uninstall.is_some()
            || record.resolved_version.is_some()
        {
            return Err(error("invalid network operation schema"));
        }
        validate_id(
            record
                .network_transaction
                .as_deref()
                .ok_or_else(|| error("missing network transaction"))?,
        )?;
        if record.transaction_id.is_some() && record.transaction_id != record.network_transaction {
            return Err(error("network transaction identity mismatch"));
        }
        return Ok(record);
    }
    if record.network_transaction.is_some() {
        return Err(error("unexpected network payload"));
    }
    if record.kind == Kind::Uninstall {
        if record.schema_version != 5
            || record.assets.is_some()
            || record.switch.is_some()
            || record.restore.is_some()
            || record.resolved_version.is_some()
        {
            return Err(error("invalid uninstall operation schema"));
        }
        validate_uninstall(
            record
                .uninstall
                .as_ref()
                .ok_or_else(|| error("missing uninstall parameters"))?,
        )?;
        return Ok(record);
    }
    if record.uninstall.is_some() {
        return Err(error("unexpected uninstall payload"));
    }
    if record.kind == Kind::Restore {
        if record.schema_version != 4
            || record.assets.is_some()
            || record.switch.is_some()
            || record.resolved_version.is_some()
        {
            return Err(error("invalid restore operation schema"));
        }
        validate_restore(
            record
                .restore
                .as_ref()
                .ok_or_else(|| error("missing restore parameters"))?,
        )?;
        return Ok(record);
    }
    if record.restore.is_some() {
        return Err(error("unexpected restore payload"));
    }
    if record.kind == Kind::Switch {
        if record.schema_version != 3 || record.assets.is_some() {
            return Err(error("invalid switch operation schema"));
        }
        validate_switch(
            record
                .switch
                .as_ref()
                .ok_or_else(|| error("missing switch parameters"))?,
        )?;
        if let Some(version) = &record.resolved_version {
            crate::workflows::install::parse_stable_version(version)
                .map_err(|e| InstallError::ParameterUsage(e.to_string()))?;
        }
    } else {
        if record.switch.is_some() || record.resolved_version.is_some() {
            return Err(error("unexpected switch payload"));
        }
        validate_request(record.kind, record.assets.as_ref())?;
    }
    Ok(record)
}
fn validate_request(kind: Kind, assets: Option<&AssetRequest>) -> Result<(), InstallError> {
    match (kind, assets) {
        (Kind::Service, None) => Ok(()),
        (Kind::Static | Kind::Binary, Some(args)) => {
            if args.state_identity.len() != 64
                || !args.state_identity.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err(error("invalid installation identity"));
            }
            semver::Version::parse(&args.version)
                .map_err(|_| error("invalid requested version"))?;
            if kind == Kind::Binary && args.official {
                return Err(error("binary repair does not accept official"));
            }
            if let Some(repository) = &args.repository {
                repository.clone().resolve()?;
            }
            Ok(())
        }
        _ => Err(error(
            "repair request payload does not match operation type",
        )),
    }
}
pub(crate) fn list() -> Result<Vec<Record>, InstallError> {
    let entries = match std::fs::read_dir(directory()) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(InstallError::Io(e)),
    };
    let mut records = vec![];
    for entry in entries {
        let path = entry.map_err(InstallError::Io)?.path();
        if path.extension().is_some_and(|ext| ext == "json") {
            records.push(read(
                path.file_stem()
                    .unwrap()
                    .to_str()
                    .ok_or_else(|| error("invalid operation filename"))?,
            )?);
        }
    }
    records.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(records)
}
#[cfg(test)]
fn submit(root: String) -> Result<Record, InstallError> {
    submit_request(root, Kind::Service, None)
}
fn submit_request(
    root: String,
    kind: Kind,
    assets: Option<AssetRequest>,
) -> Result<Record, InstallError> {
    submit_operation(root, kind, assets, None)
}
fn validate_switch(args: &SwitchRequest) -> Result<(), InstallError> {
    crate::deployment::plan::TargetVersion::parse(&args.target)?;
    validate_request(
        Kind::Binary,
        Some(&AssetRequest {
            version: args.from_version.clone(),
            state_identity: args.state_identity.clone(),
            repository: args.repository.clone(),
            official: false,
        }),
    )
}
fn submit_operation(
    root: String,
    kind: Kind,
    assets: Option<AssetRequest>,
    switch: Option<SwitchRequest>,
) -> Result<Record, InstallError> {
    if kind == Kind::Switch {
        validate_switch(
            switch
                .as_ref()
                .ok_or_else(|| error("missing switch parameters"))?,
        )?;
        if assets.is_some() {
            return Err(error("unexpected repair parameters"));
        }
    } else {
        if switch.is_some() {
            return Err(error("unexpected switch parameters"));
        }
        validate_request(kind, assets.as_ref())?;
    }
    let _guard = queue_lock()?;
    let record = Record {
        schema_version: if kind == Kind::Switch { 3 } else { 2 },
        id: uuid::Uuid::now_v7().to_string(),
        kind,
        install_root: root,
        status: Status::Queued,
        transaction_id: None,
        message: None,
        assets,
        switch,
        restore: None,
        uninstall: None,
        network_transaction: None,
        reinit: None,
        install: None,
        migrate: None,
        resolved_version: None,
        stage: Stage::Checking,
        revision: 1,
        cancel_requested: false,
        cancel_cleaned: false,
        cancellable: true,
        downloaded_bytes: 0,
        total_bytes: None,
        exit_code: None,
    };
    save(&record)?;
    Ok(record)
}
pub(crate) fn cancel(id: &str) -> Result<Record, InstallError> {
    let _guard = queue_lock()?;
    let mut record = read(id)?;
    match record.status {
        Status::Cancelled => return Ok(record),
        Status::Queued => {
            reinit::remove_input(&record)?;
            record.status = Status::Cancelled;
            record.stage = Stage::Finished;
            record.cancel_cleaned = true;
            record.exit_code = Some(130);
            record.cancellable = false;
        }
        Status::Running
            if record.kind != Kind::Service
                && record.stage == Stage::Downloading
                && record.cancellable => {}
        _ => {
            return Err(error(
                "operation can only be cancelled while queued or downloading",
            ));
        }
    }
    record.cancel_requested = true;
    record.revision += 1;
    save(&record)?;
    Ok(record)
}
fn claim() -> Result<Option<Record>, InstallError> {
    claim_kind(false)
}
fn claim_kind(network: bool) -> Result<Option<Record>, InstallError> {
    let _guard = queue_lock()?;
    let records = list()?;
    if records.iter().any(|r| {
        matches!(
            r.status,
            Status::Running | Status::Recovering | Status::AwaitingConfirmation
        ) && (!network || is_network(r.kind))
    }) {
        return Ok(None);
    }
    let Some(mut record) = records
        .into_iter()
        .find(|r| r.status == Status::Queued && is_network(r.kind) == network)
    else {
        return Ok(None);
    };
    record.status = Status::Running;
    record.cancellable = false;
    record.revision += 1;
    save(&record)?;
    Ok(Some(record))
}
fn matches_transaction(record: &Record, tx: &transaction::TransactionFile) -> bool {
    record.install_root == tx.canonical_install_root
        && match record.kind {
            Kind::Migrate => {
                tx.operation == transaction::Operation::Migrate
                    && tx.schema_version == 13
                    && record
                        .migrate
                        .as_ref()
                        .is_some_and(|args| tx.target_version.as_deref() == Some(&args.version))
            }
            Kind::Install => {
                tx.operation == transaction::Operation::Install
                    && tx.schema_version == 12
                    && tx.target_version == record.resolved_version
            }
            Kind::Reinit => {
                tx.operation == transaction::Operation::Reinit
                    && tx.schema_version == 11
                    && record
                        .reinit
                        .as_ref()
                        .is_some_and(|args| tx.from_version.as_deref() == Some(&args.from_version))
            }
            Kind::NetworkConfirm | Kind::NetworkRollback => {
                record.network_transaction.as_deref() == Some(&tx.transaction_id)
                    && tx.network_takeover.is_some()
                    && matches!(
                        tx.operation,
                        transaction::Operation::Install | transaction::Operation::Reinit
                    )
            }
            Kind::Switch => {
                tx.operation == transaction::Operation::Switch
                    && tx.schema_version == 8
                    && tx.from_version.as_deref()
                        == record
                            .switch
                            .as_ref()
                            .map(|args| args.from_version.as_str())
                    && tx.target_version == record.resolved_version
            }
            Kind::Uninstall => {
                tx.operation == transaction::Operation::Uninstall
                    && tx.schema_version == 10
                    && record.uninstall.as_ref().is_some_and(|args| {
                        tx.from_version.as_deref() == Some(&args.from_version)
                            && tx.uninstall_keep_data == args.keep_data
                    })
            }
            Kind::Restore => {
                tx.operation == transaction::Operation::Restore
                    && tx.schema_version == 9
                    && record.restore.as_ref().is_some_and(|args| {
                        tx.from_version.as_deref() == Some(&args.from_version)
                            && tx.target_version.as_deref() == Some(&args.target_version)
                    })
            }
            Kind::Service => tx.operation == transaction::Operation::RepairService,
            Kind::Static => {
                tx.operation == transaction::Operation::Repair
                    && tx.schema_version == 7
                    && tx.target_version.is_none()
            }
            Kind::Binary => {
                tx.operation == transaction::Operation::Repair
                    && tx.schema_version == 7
                    && tx.target_version.as_deref()
                        == record.assets.as_ref().map(|a| a.version.as_str())
            }
        }
}
fn associate(record: &mut Record, tx: &transaction::TransactionFile) -> Result<(), InstallError> {
    associate_id(&record.id, tx)?;
    *record = read(&record.id)?;
    Ok(())
}
pub(crate) fn associate_id(
    id: &str,
    tx: &transaction::TransactionFile,
) -> Result<(), InstallError> {
    let _guard = queue_lock()?;
    let mut record = read(id)?;
    if record.status != Status::Running
        || record.transaction_id.is_some()
        || !matches_transaction(&record, tx)
    {
        return Err(error(
            "transaction does not belong to this running operation",
        ));
    }
    record.transaction_id = Some(tx.transaction_id.clone());
    record.revision += 1;
    save(&record)
}
pub(crate) fn set_stage(id: &str, stage: Stage) -> Result<(), InstallError> {
    let _guard = queue_lock()?;
    let mut record = read(id)?;
    if record.status != Status::Running {
        return Err(error("operation is not running"));
    }
    if record.cancel_requested {
        return Err(error("download cancellation requested"));
    }
    record.stage = stage;
    record.cancellable = stage == Stage::Downloading && record.kind != Kind::Service;
    record.revision += 1;
    save(&record)
}
pub(crate) fn set_progress(id: &str, position: u64, total: u64) -> Result<(), InstallError> {
    let _guard = queue_lock()?;
    let mut record = read(id)?;
    if record.status != Status::Running || record.stage != Stage::Downloading {
        return Ok(());
    }
    record.downloaded_bytes = position;
    record.total_bytes = (total != 0).then_some(total);
    record.revision += 1;
    save(&record)
}
fn read_transaction(record: &Record) -> Result<Option<transaction::TransactionFile>, InstallError> {
    let Some(id) = &record.transaction_id else {
        return Ok(None);
    };
    validate_id(id)?;
    let bytes = match std::fs::read(layout::territory_transactions_dir().join(format!("{id}.json")))
    {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(InstallError::Io(e)),
    };
    let tx: transaction::TransactionFile = serde_json::from_slice(&bytes)
        .map_err(|e| error(format!("unreadable transaction: {e}")))?;
    transaction::validate_transaction(&tx)?;
    if tx.transaction_id != *id || !matches_transaction(record, &tx) {
        return Err(error("transaction identity mismatch"));
    }
    Ok(Some(tx))
}
fn cleanup_cancel(
    record: &mut Record,
    tx: &transaction::TransactionFile,
) -> Result<(), InstallError> {
    if record.stage != Stage::Downloading
        || !matches!(
            tx.phase,
            transaction::Phase::Preparing | transaction::Phase::Failed
        )
    {
        return Err(error(
            "cannot prove cancellation preceded installation changes",
        ));
    }
    let root = crate::deployment::root::normalize_install_root(std::path::Path::new(
        &record.install_root,
    ))?;
    if matches!(record.kind, Kind::Switch | Kind::Install) {
        crate::workflows::switch::cleanup_preparing(&root, tx)?;
    } else {
        crate::workflows::repair::cleanup_preparing(&root, tx)?;
    }
    record.cancel_cleaned = true;
    Ok(())
}

/// Reconcile durable evidence, never replay the workflow after a crash.
fn reconcile(record: &mut Record, result: Option<Result<(), String>>) -> Result<(), InstallError> {
    reinit::remove_input(record)?;
    let exit_code = record.exit_code;
    {
        let _guard = queue_lock()?;
        *record = read(&record.id)?;
        record.cancellable = false;
        record.revision += 1;
        save(record)?;
    }
    record.exit_code = exit_code.or(record.exit_code);
    let evidence = read_transaction(record).and_then(|tx| {
        if record.cancel_requested
            && !record.cancel_cleaned
            && let Some(tx) = &tx
        {
            cleanup_cancel(record, tx)?;
        }
        Ok(tx.map(|tx| tx.phase))
    });
    use transaction::Phase;
    let rolled_back = matches!(evidence, Ok(Some(Phase::RolledBack)));
    record.status = match evidence {
        Ok(_) if record.cancel_cleaned => Status::Cancelled,
        Ok(Some(Phase::AwaitingNetworkConfirmation))
            if matches!(record.kind, Kind::Reinit | Kind::Install) =>
        {
            Status::AwaitingConfirmation
        }
        Ok(Some(Phase::AwaitingNetworkConfirmation)) if is_network(record.kind) => {
            if result.is_some() {
                Status::Failed
            } else {
                Status::Interrupted
            }
        }
        Ok(Some(Phase::Committed)) if record.kind == Kind::NetworkRollback => Status::Failed,
        Ok(Some(Phase::RolledBack)) if record.kind == Kind::NetworkRollback => Status::Succeeded,
        Ok(Some(Phase::Committed)) => Status::Succeeded,
        Ok(Some(Phase::RolledBack | Phase::Failed)) => Status::Failed,
        Ok(Some(_)) => Status::Recovering,
        Ok(None) => match result.as_ref() {
            Some(Ok(()))
                if record.transaction_id.is_none()
                    && matches!(record.kind, Kind::Service | Kind::Switch) =>
            {
                Status::Succeeded
            }
            Some(Err(_)) => Status::Failed,
            _ => Status::Interrupted,
        },
        Err(e) => {
            record.message = Some(e.to_string());
            Status::Recovering
        }
    };
    if let Some(Err(message)) = result {
        if record.status == Status::Recovering {
            record.exit_code = Some(6);
        }
        record.message = Some(message);
    }
    if record.status.terminal() {
        record.cancellable = false;
        record.stage = Stage::Finished;
        record.exit_code = Some(match record.status {
            Status::Succeeded => 0,
            Status::Cancelled => 130,
            _ if rolled_back => {
                if matches!(
                    record.kind,
                    Kind::Binary | Kind::Switch | Kind::Restore | Kind::Reinit | Kind::Migrate
                ) {
                    5
                } else {
                    1
                }
            }
            _ => record.exit_code.unwrap_or(1),
        });
    }
    record.revision += 1;
    let _guard = queue_lock()?;
    save(record)
}

/// Returns false when legacy requests may run (including network confirmation).
pub(crate) fn process_one(runtime: &InstallRuntime) -> Result<bool, InstallError> {
    let _install_lock = match lock::acquire_install_lock() {
        Ok(lock) => lock,
        Err(InstallError::LockBusy) => return Ok(false),
        Err(e) => return Err(e),
    };
    reinit::cleanup_orphan_inputs()?;
    for mut record in list()? {
        if matches!(
            record.status,
            Status::Running | Status::Recovering | Status::AwaitingConfirmation
        ) {
            reconcile(&mut record, None)?;
        }
    }
    if let Some(record) = claim_kind(true)? {
        return execute_record(record, |record| execute_network(runtime, record));
    }
    if list()?.iter().any(|r| {
        matches!(
            r.status,
            Status::Running | Status::Recovering | Status::AwaitingConfirmation
        )
    }) {
        return Ok(true);
    }
    let root = match state::discover_landscape_root()? {
        Some(root) => Some(root),
        None => state::discover_landscape_root_from_unfinished_transaction()?,
    };
    if let Some(root) = &root
        && transaction::find_unfinished(root)?.is_some()
    {
        return Ok(false);
    }
    execute_next(|record| {
        if !matches!(record.kind, Kind::Install | Kind::Migrate) {
            let root = root
                .as_ref()
                .ok_or_else(|| error("no Landscape installation"))?;
            if record.install_root != root.canonical.to_string_lossy() {
                return Err(error("installation changed since submission"));
            }
        }
        if record.kind == Kind::Service {
            return crate::workflows::repair_service::run_locked(runtime, &mut |tx| {
                associate(record, tx)
            });
        }
        let _control = crate::workflows::operation_control::enter(&record.id);
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(InstallError::Io)?;
        let code = if record.kind == Kind::Migrate {
            executor.block_on(crate::workflows::migrate::native::run_operation(
                runtime, record,
            ))?
        } else if record.kind == Kind::Install {
            executor.block_on(crate::workflows::install::run_operation(runtime, record))?
        } else if record.kind == Kind::Reinit {
            executor.block_on(crate::workflows::reinit::run_operation(runtime, record))?
        } else if record.kind == Kind::Uninstall {
            executor.block_on(crate::workflows::uninstall::run_operation(runtime, record))?
        } else if record.kind == Kind::Restore {
            executor.block_on(crate::workflows::restore::run_operation(runtime, record))?
        } else if record.kind == Kind::Switch {
            executor.block_on(crate::workflows::switch::run_operation(runtime, record))?
        } else {
            executor.block_on(crate::workflows::repair::run_assets(runtime, record))?
        };
        record.exit_code = Some(code);
        if code == 0 {
            Ok(())
        } else {
            Err(error(read(&record.id)?.message.unwrap_or_else(|| {
                format!("operation finished with exit code {code}")
            })))
        }
    })
}
fn execute_next(
    execute: impl FnMut(&mut Record) -> Result<(), InstallError>,
) -> Result<bool, InstallError> {
    let Some(record) = claim()? else {
        return Ok(false);
    };
    execute_record(record, execute)
}
fn execute_record(
    mut record: Record,
    mut execute: impl FnMut(&mut Record) -> Result<(), InstallError>,
) -> Result<bool, InstallError> {
    let result = execute(&mut record).map_err(|e| {
        record.exit_code = record.exit_code.or(Some(match e {
            InstallError::ParameterUsage(_) | InstallError::UnsupportedPlatform(_) => 2,
            _ => 1,
        }));
        e.to_string()
    });
    #[cfg(feature = "test-support")]
    if result.is_ok()
        && matches!(
            record.kind,
            Kind::Static
                | Kind::Binary
                | Kind::Switch
                | Kind::Restore
                | Kind::Uninstall
                | Kind::NetworkConfirm
                | Kind::NetworkRollback
                | Kind::Install
                | Kind::Migrate
        )
    {
        crate::workflows::operation_control::checkpoint("before-result")?;
    }
    reconcile(&mut record, Some(result))?;
    Ok(true)
}
pub(crate) async fn submit_and_wait(
    runtime: &InstallRuntime,
    kind: Kind,
    repository: Option<crate::deployment::plan::RepositoryChoice>,
    official: bool,
) -> Result<u8, InstallError> {
    if !runtime.allow_non_root && unsafe { libc::geteuid() } != 0 {
        return Err(error("repair requires root"));
    }
    let capability = if kind == Kind::Service {
        "operations-v2.pid"
    } else {
        "asset-repair-v2.pid"
    };
    crate::daemon::require_capability(capability)?;
    let root =
        state::discover_landscape_root()?.ok_or_else(|| error("no Landscape installation"))?;
    let assets = if kind == Kind::Service {
        None
    } else {
        let installed =
            state::load_state(&root)?.ok_or_else(|| error("missing installation state"))?;
        let state_identity = installation_identity(&installed)?;
        Some(AssetRequest {
            version: installed.active_version,
            state_identity,
            repository,
            official,
        })
    };
    let record = submit_request(root.canonical.to_string_lossy().into_owned(), kind, assets)?;
    wait_for_result(record, capability, false).await
}

pub(crate) async fn submit_switch_and_wait(
    runtime: &InstallRuntime,
    args: SwitchRequest,
) -> Result<u8, InstallError> {
    if !runtime.allow_non_root && unsafe { libc::geteuid() } != 0 {
        return Err(error("switch requires root"));
    }
    let capability = "switch-v1.pid";
    crate::daemon::require_capability(capability)?;
    let root =
        state::discover_landscape_root()?.ok_or_else(|| error("no Landscape installation"))?;
    let record = submit_operation(
        root.canonical.to_string_lossy().into_owned(),
        Kind::Switch,
        None,
        Some(args),
    )?;
    wait_for_result(record, capability, false).await
}
fn validate_uninstall(args: &UninstallRequest) -> Result<(), InstallError> {
    if args.state_identity.len() != 64
        || !args.state_identity.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(error("invalid uninstall installation identity"));
    }
    crate::workflows::install::parse_stable_version(&args.from_version)
        .map_err(|e| error(e.to_string()))?;
    Ok(())
}
fn submit_uninstall(root: String, args: UninstallRequest) -> Result<Record, InstallError> {
    validate_uninstall(&args)?;
    let _guard = queue_lock()?;
    let record = Record {
        schema_version: 5,
        id: uuid::Uuid::now_v7().to_string(),
        kind: Kind::Uninstall,
        install_root: root,
        status: Status::Queued,
        transaction_id: None,
        message: None,
        assets: None,
        switch: None,
        restore: None,
        uninstall: Some(args),
        network_transaction: None,
        reinit: None,
        install: None,
        migrate: None,
        resolved_version: None,
        stage: Stage::Checking,
        revision: 1,
        cancel_requested: false,
        cancel_cleaned: false,
        cancellable: true,
        downloaded_bytes: 0,
        total_bytes: None,
        exit_code: None,
    };
    save(&record)?;
    Ok(record)
}
pub(crate) async fn submit_uninstall_and_wait(
    root: String,
    args: UninstallRequest,
) -> Result<u8, InstallError> {
    crate::daemon::require_capability("uninstall-v1.pid")?;
    wait_for_result(submit_uninstall(root, args)?, "uninstall-v1.pid", false).await
}
fn validate_restore(args: &RestoreRequest) -> Result<(), InstallError> {
    for hash in [&args.state_identity, &args.backup_sha256] {
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(error("invalid restore identity"));
        }
    }
    if !args.source.is_absolute() {
        return Err(error("restore source must be absolute"));
    }
    if !crate::backup::lkb::backup_id_format_ok(&args.backup_id) {
        return Err(error("invalid backup ID"));
    }
    for version in [&args.from_version, &args.target_version] {
        crate::workflows::install::parse_stable_version(version)
            .map_err(|e| error(e.to_string()))?;
    }
    Ok(())
}
fn submit_restore(root: String, args: RestoreRequest) -> Result<Record, InstallError> {
    validate_restore(&args)?;
    let _guard = queue_lock()?;
    let record = Record {
        schema_version: 4,
        id: uuid::Uuid::now_v7().to_string(),
        kind: Kind::Restore,
        install_root: root,
        status: Status::Queued,
        transaction_id: None,
        message: None,
        assets: None,
        switch: None,
        restore: Some(args),
        uninstall: None,
        network_transaction: None,
        reinit: None,
        install: None,
        migrate: None,
        resolved_version: None,
        stage: Stage::Checking,
        revision: 1,
        cancel_requested: false,
        cancel_cleaned: false,
        cancellable: true,
        downloaded_bytes: 0,
        total_bytes: None,
        exit_code: None,
    };
    save(&record)?;
    Ok(record)
}
pub(crate) async fn submit_restore_and_wait(
    root: String,
    args: RestoreRequest,
    full_screen: bool,
) -> Result<u8, InstallError> {
    crate::daemon::require_capability("restore-v1.pid")?;
    let record = submit_restore(root, args)?;
    wait_for_result(record, "restore-v1.pid", full_screen).await
}
pub(crate) async fn submit_confirmed_switch_and_wait(
    runtime: &InstallRuntime,
    root: String,
    args: SwitchRequest,
    full_screen: bool,
) -> Result<u8, InstallError> {
    if !runtime.allow_non_root && unsafe { libc::geteuid() } != 0 {
        return Err(error("update requires root"));
    }
    crate::daemon::require_capability("switch-v1.pid")?;
    let record = submit_operation(root, Kind::Switch, None, Some(args))?;
    wait_for_result(record, "switch-v1.pid", full_screen).await
}
pub(crate) fn set_message(id: &str, message: &str) -> Result<(), InstallError> {
    let _guard = queue_lock()?;
    let mut record = read(id)?;
    if record.status != Status::Running {
        return Err(error("operation is not running"));
    }
    record.message = Some(message.into());
    record.revision += 1;
    save(&record)
}
pub(crate) fn resolve_switch_version(id: &str, version: &str) -> Result<(), InstallError> {
    crate::workflows::install::parse_stable_version(version)
        .map_err(|e| InstallError::ParameterUsage(e.to_string()))?;
    let _guard = queue_lock()?;
    let mut record = read(id)?;
    if !matches!(record.kind, Kind::Switch | Kind::Install)
        || record.status != Status::Running
        || record.resolved_version.is_some()
    {
        return Err(error("cannot resolve switch target"));
    }
    record.resolved_version = Some(version.into());
    record.revision += 1;
    save(&record)
}
async fn wait_for_result(
    record: Record,
    capability: &str,
    full_screen: bool,
) -> Result<u8, InstallError> {
    use crate::interaction::presentation::{
        OperationPresentation, PresentationAction, operation_screen,
    };
    eprintln!(
        "lkit: operation {} (query with `lkit operation status {}`)",
        record.id, record.id
    );
    let command = if record.kind == Kind::Install {
        "install"
    } else if record.kind == Kind::Reinit {
        "reinit"
    } else if record.kind == Kind::Restore {
        "restore"
    } else {
        "update"
    };
    let mut presentation = full_screen
        .then(|| OperationPresentation::new(true, operation_screen(command)))
        .filter(|screen| screen.is_full_screen());
    if let Some(screen) = &mut presentation {
        screen.capture_log(&format!("Operation {}", record.id));
    }
    let mut last_revision = None;
    let mut unavailable_since = None;
    let mut displayed = None;
    let mut displayed_message = None;
    loop {
        let current = read(&record.id)?;
        if let Some(screen) = &mut presentation {
            if last_revision != Some(current.revision) {
                screen.observe_operation(&current);
                last_revision = Some(current.revision);
            }
            match screen.poll_action().map_err(error)? {
                Some(PresentationAction::Stop) => match cancel(&record.id) {
                    Ok(_) => screen.cancel_requested(),
                    Err(e) => {
                        screen.capture_log(&e.to_string());
                        screen.ignore_stop();
                    }
                },
                Some(PresentationAction::Close) => {
                    screen.finish();
                    eprintln!(
                        "lkit: detached; query operation {} for its result",
                        record.id
                    );
                    return Ok(130);
                }
                _ => {}
            }
        }
        if presentation.is_none() && displayed_message != current.message {
            if let Some(message) = &current.message {
                eprintln!("lkit: {message}");
            }
            displayed_message = current.message.clone();
        }
        let progress = (
            current.status,
            current.stage,
            current.downloaded_bytes,
            current.total_bytes,
        );
        if presentation.is_none() && displayed.as_ref() != Some(&progress) {
            eprintln!(
                "lkit: {:?} {:?} {} / {:?} bytes",
                current.status, current.stage, current.downloaded_bytes, current.total_bytes
            );
            displayed = Some(progress);
        }
        if current.status.terminal()
            || current.status == Status::AwaitingConfirmation
            || (current.status == Status::Recovering && current.exit_code == Some(6))
        {
            let code = current.exit_code.unwrap_or(
                if matches!(
                    current.status,
                    Status::Succeeded | Status::AwaitingConfirmation
                ) {
                    0
                } else {
                    1
                },
            );
            if let Some(screen) = &mut presentation {
                screen.capture_log(&format!("Operation {}: {:?}", record.id, current.status));
                screen.show_result(code, current.status == Status::AwaitingConfirmation);
                while screen.is_full_screen() {
                    match screen.poll_action().map_err(error)? {
                        Some(PresentationAction::Close) => break,
                        Some(PresentationAction::ConfirmTakeover) => {
                            screen.finish();
                            let tx = current
                                .transaction_id
                                .clone()
                                .ok_or_else(|| error("missing network transaction"))?;
                            return Box::pin(submit_network_and_wait(
                                current.install_root.clone(),
                                Kind::NetworkConfirm,
                                tx,
                            ))
                            .await;
                        }
                        _ => {}
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
                screen.finish();
            }
            if let Some(message) = current.message {
                if current.kind == Kind::Uninstall && code == 0 {
                    println!("uninstall: {message}");
                } else {
                    eprintln!("lkit: {message}");
                }
            }
            return Ok(code);
        }

        if crate::daemon::require_capability(capability).is_err() {
            let since = unavailable_since.get_or_insert_with(std::time::Instant::now);
            if since.elapsed() >= std::time::Duration::from_secs(30) {
                return Err(error(format!(
                    "daemon unavailable; operation {} remains recorded; query its status after daemon recovery",
                    record.id
                )));
            }
        } else {
            unavailable_since = None;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

#[cfg(test)]
mod tests;

fn is_network(kind: Kind) -> bool {
    matches!(kind, Kind::NetworkConfirm | Kind::NetworkRollback)
}
fn submit_network(
    root: String,
    kind: Kind,
    transaction_id: String,
) -> Result<Record, InstallError> {
    if !is_network(kind) {
        return Err(error("invalid network operation kind"));
    }
    validate_id(&transaction_id)?;
    let _guard = queue_lock()?;
    let record = Record {
        schema_version: 6,
        id: uuid::Uuid::now_v7().to_string(),
        kind,
        install_root: root,
        status: Status::Queued,
        transaction_id: None,
        message: None,
        assets: None,
        switch: None,
        restore: None,
        uninstall: None,
        network_transaction: Some(transaction_id),
        reinit: None,
        install: None,
        migrate: None,
        resolved_version: None,
        stage: Stage::Checking,
        revision: 1,
        cancel_requested: false,
        cancel_cleaned: false,
        cancellable: true,
        downloaded_bytes: 0,
        total_bytes: None,
        exit_code: None,
    };
    save(&record)?;
    Ok(record)
}
pub(crate) async fn submit_network_and_wait(
    root: String,
    kind: Kind,
    transaction_id: String,
) -> Result<u8, InstallError> {
    crate::daemon::require_capability("network-operations-v1.pid")?;
    wait_for_result(
        submit_network(root, kind, transaction_id)?,
        "network-operations-v1.pid",
        false,
    )
    .await
}
fn execute_network(runtime: &InstallRuntime, record: &mut Record) -> Result<(), InstallError> {
    let root = crate::deployment::root::normalize_install_root(std::path::Path::new(
        &record.install_root,
    ))?;
    let tx = transaction::find_unfinished(&root)?
        .ok_or_else(|| InstallError::ParameterUsage("no pending network transaction".into()))?;
    if !matches_transaction(record, &tx) {
        return Err(InstallError::ParameterUsage(
            "network transaction changed since submission".into(),
        ));
    }
    // A rejected confirmation must not turn into a recovering operation that
    // waits forever for another session to decide the takeover's outcome.
    crate::network::takeover::validate_action(&tx, record.kind == Kind::NetworkConfirm)?;
    associate(record, &tx)?;
    set_stage(
        &record.id,
        if record.kind == Kind::NetworkConfirm {
            Stage::Verifying
        } else {
            Stage::RollingBack
        },
    )?;
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(InstallError::Io)?;
    let result = if record.kind == Kind::NetworkConfirm {
        executor.block_on(crate::network::takeover::confirm(&root, runtime))
    } else {
        executor.block_on(crate::network::takeover::rollback(&root, runtime, false))
    };
    if result.is_err() {
        record.exit_code = Some(6);
    }
    result
}
