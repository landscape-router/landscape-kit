use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde::{Deserialize, Serialize};

use crate::interaction::presentation::OPERATIONS_DIR;
use crate::network::config::NetworkPlan;

pub(super) const CANCEL_FILE_SUFFIX: &str = ".cancel";

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct WorkerRequest {
    pub(super) schema_version: u64,
    pub(super) args: Vec<String>,
    pub(super) environment: Vec<(String, String)>,
    pub(super) working_directory: PathBuf,
    pub(super) result_path: PathBuf,
    pub(super) stdout_path: PathBuf,
    pub(super) stderr_path: PathBuf,
    pub(super) cancel_path: PathBuf,
    pub(super) terminal: Option<PathBuf>,
    pub(super) presentation_path: PathBuf,
    #[serde(default)]
    pub(super) credential_path: Option<PathBuf>,
    #[serde(default)]
    pub(super) network_plan_path: Option<PathBuf>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct WorkerResult {
    pub(super) schema_version: u64,
    pub(super) exit_code: i32,
}

pub(super) enum WaitOutcome {
    Completed(ExitCode),
    Interrupted,
    /// 结果页上用户确认了待确认的网络接管:调用方应内联执行 `lkit network confirm`。
    ConfirmTakeover,
}

pub(crate) fn string_args() -> Result<Vec<String>, String> {
    std::env::args_os()
        .skip(1)
        .map(|value| {
            value.into_string().map_err(|_| {
                "command arguments must be valid UTF-8 for daemon delegation".to_string()
            })
        })
        .collect()
}

pub(super) fn string_environment() -> Result<Vec<(String, String)>, String> {
    std::env::vars_os()
        .map(|(key, value)| {
            let key = key.into_string().map_err(|_| {
                "environment names must be valid UTF-8 for daemon delegation".to_string()
            })?;
            let value = value
                .into_string()
                .map_err(|_| format!("environment value for {key} must be valid UTF-8"))?;
            Ok((key, value))
        })
        .collect()
}

pub(super) fn validate_credential_path(path: &Path) -> Result<(), String> {
    if path.parent() != Some(Path::new(OPERATIONS_DIR))
        || !path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().ends_with(".credential"))
    {
        return Err(format!(
            "internal credential path must be under {OPERATIONS_DIR}"
        ));
    }
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("inspect internal credential file: {error}"))?;
    if !metadata.is_file() || metadata.uid() != 0 || metadata.mode() & 0o077 != 0 {
        return Err("internal credential file must be root-only regular file".into());
    }
    Ok(())
}

pub(super) fn validate_network_plan_path(path: &Path) -> Result<(), String> {
    if path.parent() != Some(Path::new(OPERATIONS_DIR))
        || !path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().ends_with(".network.json"))
    {
        return Err(format!(
            "internal network plan path must be under {OPERATIONS_DIR}"
        ));
    }
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("inspect internal network plan file: {error}"))?;
    if !metadata.is_file() || metadata.uid() != 0 || metadata.mode() & 0o077 != 0 {
        return Err("internal network plan must be a root-only regular file".into());
    }
    Ok(())
}

pub(crate) fn read_network_plan(path: &Path) -> Result<NetworkPlan, String> {
    validate_network_plan_path(path)?;
    let content =
        std::fs::read(path).map_err(|error| format!("read internal network plan: {error}"))?;
    let plan: NetworkPlan = serde_json::from_slice(&content)
        .map_err(|error| format!("parse internal network plan: {error}"))?;
    plan.validate().map_err(|error| error.to_string())?;
    Ok(plan)
}

pub(super) struct RemoveFile {
    path: PathBuf,
}

impl RemoveFile {
    pub(super) fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
        }
    }
}

impl Drop for RemoveFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

pub(super) fn validate_request_path(path: &Path) -> Result<(), String> {
    if path.parent() != Some(Path::new(OPERATIONS_DIR)) {
        return Err(format!("worker request must be under {OPERATIONS_DIR}"));
    }
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("inspect worker request {}: {error}", path.display()))?;
    if metadata.uid() != 0 || metadata.mode() & 0o077 != 0 {
        return Err("worker request must be root-owned and inaccessible to group/other".into());
    }
    Ok(())
}

pub(super) fn terminal_path() -> Option<PathBuf> {
    let path = std::fs::read_link("/proc/self/fd/0").ok()?;
    (path.starts_with("/dev/") && !path.as_os_str().is_empty()).then_some(path)
}

pub(super) fn write_private_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let temporary = path.with_extension("tmp");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|error| format!("create {}: {error}", temporary.display()))?;
    serde_json::to_writer_pretty(&mut file, value)
        .map_err(|error| format!("serialize {}: {error}", path.display()))?;
    file.write_all(b"\n")
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("write {}: {error}", path.display()))?;
    std::fs::rename(&temporary, path).map_err(|error| format!("commit {}: {error}", path.display()))
}

pub(super) fn create_private_file(path: &Path) -> Result<(), String> {
    OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map(|_| ())
        .map_err(|error| format!("create {}: {error}", path.display()))
}

pub(super) fn create_private_secret_file(path: &Path, content: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| format!("create internal credential file: {error}"))?;
    if let Err(error) = file.write_all(content).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(format!("write internal credential file: {error}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::protocol::RemoveFile;
    use super::*;
    use uuid::Uuid;

    #[test]
    fn internal_credential_file_is_private_and_removed_by_guard() {
        let path = std::env::temp_dir().join(format!(
            "lkit-worker-credential-{}-{}.credential",
            std::process::id(),
            Uuid::now_v7()
        ));
        create_private_secret_file(&path, b"Secret123").unwrap();
        let metadata = std::fs::metadata(&path).unwrap();
        assert_eq!(metadata.mode() & 0o077, 0);
        assert_eq!(std::fs::read(&path).unwrap(), b"Secret123");
        {
            let _credential = RemoveFile::new(&path);
        }
        assert!(!path.exists());
    }
}
