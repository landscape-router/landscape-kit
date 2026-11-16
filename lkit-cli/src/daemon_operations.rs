//! Durable, typed operations executed by the daemon without an lkit child.
use crate::deployment::{
    layout, lock, plan::InstallError, runtime::InstallRuntime, state, transaction,
};
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
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Stage {
    #[default]
    Checking,
    Downloading,
    Preparing,
    Stopping,
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
pub(crate) struct Record {
    pub schema_version: u32,
    pub id: String,
    pub kind: Kind,
    pub install_root: String,
    pub status: Status,
    pub transaction_id: Option<String>,
    pub message: Option<String>,
    #[serde(default)]
    pub assets: Option<AssetRequest>,
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
    if !(1..=2).contains(&record.schema_version) || record.id != id {
        return Err(error("operation identity or schema mismatch"));
    }
    if let Some(id) = &record.transaction_id {
        validate_id(id)?;
    }
    if record.kind != Kind::Service && record.schema_version != 2 {
        return Err(error("asset repair requires operation schema v2"));
    }
    validate_request(record.kind, record.assets.as_ref())?;
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
    validate_request(kind, assets.as_ref())?;
    let _guard = queue_lock()?;
    let record = Record {
        schema_version: 2,
        id: uuid::Uuid::now_v7().to_string(),
        kind,
        install_root: root,
        status: Status::Queued,
        transaction_id: None,
        message: None,
        assets,
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
    let _guard = queue_lock()?;
    let records = list()?;
    if records
        .iter()
        .any(|r| matches!(r.status, Status::Running | Status::Recovering))
    {
        return Ok(None);
    }
    let Some(mut record) = records.into_iter().find(|r| r.status == Status::Queued) else {
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
    crate::workflows::repair::cleanup_preparing(&root, tx)?;
    record.cancel_cleaned = true;
    Ok(())
}

/// Reconcile durable evidence, never replay the workflow after a crash.
fn reconcile(record: &mut Record, result: Option<Result<(), String>>) -> Result<(), InstallError> {
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
        Ok(Some(Phase::Committed)) => Status::Succeeded,
        Ok(Some(Phase::RolledBack | Phase::Failed)) => Status::Failed,
        Ok(Some(_)) => Status::Recovering,
        Ok(None) => match result.as_ref() {
            Some(Ok(())) if record.transaction_id.is_none() && record.kind == Kind::Service => {
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
                if record.kind == Kind::Binary {
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
    for mut record in list()? {
        if matches!(record.status, Status::Running | Status::Recovering) {
            reconcile(&mut record, None)?;
        }
    }
    if list()?
        .iter()
        .any(|r| matches!(r.status, Status::Running | Status::Recovering))
    {
        return Ok(true);
    }
    let Some(root) = state::discover_landscape_root()? else {
        // Claim and fail missing installations instead of leaving requests queued forever.
        return execute_next(|_| Err(error("no Landscape installation")));
    };
    if transaction::find_unfinished(&root)?.is_some() {
        return Ok(false);
    }
    execute_next(|record| {
        if record.install_root != root.canonical.to_string_lossy() {
            return Err(error("installation changed since submission"));
        }
        if record.kind == Kind::Service {
            return crate::workflows::repair_service::run_locked(runtime, &mut |tx| {
                associate(record, tx)
            });
        }
        let _control = crate::workflows::repair_control::enter(&record.id);
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(InstallError::Io)?;
        let code = executor.block_on(crate::workflows::repair::run_assets(runtime, record))?;
        record.exit_code = Some(code);
        if code == 0 {
            Ok(())
        } else {
            Err(error(format!("repair finished with exit code {code}")))
        }
    })
}
fn execute_next(
    mut execute: impl FnMut(&mut Record) -> Result<(), InstallError>,
) -> Result<bool, InstallError> {
    let Some(mut record) = claim()? else {
        return Ok(false);
    };
    let result = execute(&mut record).map_err(|e| {
        record.exit_code = record.exit_code.or(Some(match e {
            InstallError::ParameterUsage(_) | InstallError::UnsupportedPlatform(_) => 2,
            _ => 1,
        }));
        e.to_string()
    });
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
    eprintln!(
        "lkit: operation {} (query with `lkit operation status {}`)",
        record.id, record.id
    );
    let mut unavailable_since = None;
    let mut displayed = None;
    loop {
        let current = read(&record.id)?;
        let progress = (
            current.status,
            current.stage,
            current.downloaded_bytes,
            current.total_bytes,
        );
        if displayed.as_ref() != Some(&progress) {
            eprintln!(
                "lkit: {:?} {:?} {} / {:?} bytes",
                current.status, current.stage, current.downloaded_bytes, current.total_bytes
            );
            displayed = Some(progress);
        }
        if current.status.terminal()
            || (current.status == Status::Recovering && current.exit_code == Some(6))
        {
            if let Some(message) = current.message {
                eprintln!("lkit: {message}");
            }
            return Ok(current
                .exit_code
                .unwrap_or(if current.status == Status::Succeeded {
                    0
                } else {
                    1
                }));
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
