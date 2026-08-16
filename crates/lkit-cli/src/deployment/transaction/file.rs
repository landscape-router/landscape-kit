use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use super::super::layout;
use super::super::plan::InstallError;
use super::super::root::InstallRoot;
use super::TransactionFile;
use super::validate_transaction;

pub(super) fn write_transaction(
    _root: &InstallRoot,
    transaction: &TransactionFile,
) -> Result<(), InstallError> {
    validate_transaction(transaction)?;
    let dir = layout::territory_transactions_dir();
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

pub(super) fn load_transaction_file(
    root: &InstallRoot,
    path: &Path,
) -> Result<TransactionFile, InstallError> {
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

pub(super) fn append_log(
    _root: &InstallRoot,
    transaction: &TransactionFile,
    line: &str,
) -> Result<(), InstallError> {
    let log_path = layout::territory_relative(&transaction.log_path);
    let mut log = OpenOptions::new()
        .append(true)
        .mode(0o600)
        .open(&log_path)
        .map_err(InstallError::Io)?;
    writeln!(log, "{line}").map_err(InstallError::Io)?;
    log.sync_all().map_err(InstallError::Io)?;
    Ok(())
}
