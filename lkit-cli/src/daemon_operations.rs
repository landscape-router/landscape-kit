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
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Kind {
    RepairService,
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
    if record.schema_version != 1 || record.id != id {
        return Err(error("operation identity or schema mismatch"));
    }
    if let Some(id) = &record.transaction_id {
        validate_id(id)?;
    }
    Ok(record)
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
fn submit(root: String) -> Result<Record, InstallError> {
    let _guard = queue_lock()?;
    let record = Record {
        schema_version: 1,
        id: uuid::Uuid::now_v7().to_string(),
        kind: Kind::RepairService,
        install_root: root,
        status: Status::Queued,
        transaction_id: None,
        message: None,
    };
    save(&record)?;
    Ok(record)
}
pub(crate) fn cancel(id: &str) -> Result<Record, InstallError> {
    let _guard = queue_lock()?;
    let mut record = read(id)?;
    if record.status != Status::Queued {
        return Err(error("only queued operations can be cancelled"));
    }
    record.status = Status::Cancelled;
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
    save(&record)?;
    Ok(Some(record))
}
fn associate(record: &mut Record, tx: &transaction::TransactionFile) -> Result<(), InstallError> {
    if record.install_root != tx.canonical_install_root
        || tx.operation != transaction::Operation::RepairService
    {
        return Err(error("transaction does not belong to this operation"));
    }
    let _guard = queue_lock()?;
    record.transaction_id = Some(tx.transaction_id.clone());
    save(record)
}

/// Reconcile durable evidence, never replay the workflow after a crash.
fn reconcile(record: &mut Record, result: Option<Result<(), String>>) -> Result<(), InstallError> {
    let evidence = (|| -> Result<Option<transaction::Phase>, InstallError> {
        let Some(id) = &record.transaction_id else {
            return Ok(None);
        };
        validate_id(id)?;
        let bytes =
            match std::fs::read(layout::territory_transactions_dir().join(format!("{id}.json"))) {
                Ok(bytes) => bytes,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(InstallError::Io(e)),
            };
        let tx: transaction::TransactionFile = serde_json::from_slice(&bytes)
            .map_err(|e| error(format!("unreadable transaction: {e}")))?;
        transaction::validate_transaction(&tx)?;
        if tx.transaction_id != *id
            || tx.canonical_install_root != record.install_root
            || tx.operation != transaction::Operation::RepairService
        {
            return Err(error("transaction identity mismatch"));
        }
        Ok(Some(tx.phase))
    })();
    use transaction::Phase;
    record.status = match evidence {
        Ok(Some(Phase::Committed)) => Status::Succeeded,
        Ok(Some(Phase::RolledBack | Phase::Failed)) => Status::Failed,
        Ok(Some(_)) => Status::Recovering,
        Ok(None) => match result.as_ref() {
            Some(Ok(())) if record.transaction_id.is_none() => Status::Succeeded,
            Some(Err(_)) => Status::Failed,
            _ => Status::Interrupted,
        },
        Err(e) => {
            record.message = Some(e.to_string());
            Status::Recovering
        }
    };
    if let Some(Err(message)) = result {
        record.message = Some(message);
    }
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
        crate::workflows::repair_service::run_locked(runtime, &mut |tx| associate(record, tx))
    })
}
fn execute_next(
    mut execute: impl FnMut(&mut Record) -> Result<(), InstallError>,
) -> Result<bool, InstallError> {
    let Some(mut record) = claim()? else {
        return Ok(false);
    };
    let result = execute(&mut record).map_err(|e| e.to_string());
    reconcile(&mut record, Some(result))?;
    Ok(true)
}
pub(crate) async fn submit_and_wait(runtime: &InstallRuntime) -> Result<(), InstallError> {
    if !runtime.allow_non_root && unsafe { libc::geteuid() } != 0 {
        return Err(error("service repair requires root"));
    }
    crate::daemon::require_capability("operations-v1.pid")?;
    let root =
        state::discover_landscape_root()?.ok_or_else(|| error("no Landscape installation"))?;
    let record = submit(root.canonical.to_string_lossy().into_owned())?;
    eprintln!(
        "lkit: operation {} (query with `lkit operation status {}`)",
        record.id, record.id
    );
    let mut unavailable_since = None;
    loop {
        let current = read(&record.id)?;
        if current.status.terminal() {
            return if current.status == Status::Succeeded {
                Ok(())
            } else {
                Err(error(format!(
                    "operation {}: {:?}: {}",
                    current.id,
                    current.status,
                    current.message.unwrap_or_default()
                )))
            };
        }
        if crate::daemon::require_capability("operations-v1.pid").is_err() {
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
