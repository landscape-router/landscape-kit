use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde::{Deserialize, Serialize};

use crate::interaction::presentation::OPERATIONS_DIR;
use crate::network::config::NetworkPlan;

#[derive(Debug, Deserialize, Serialize)]
pub(super) struct WorkerRequest {
    pub(super) schema_version: u64,
    pub(super) args: Vec<String>,
    pub(super) environment: Vec<(String, String)>,
    pub(super) working_directory: PathBuf,
    pub(super) result_path: PathBuf,
    pub(super) unit_path: PathBuf,
    pub(super) systemctl: PathBuf,
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
}

pub(crate) fn string_args() -> Result<Vec<String>, String> {
    std::env::args_os()
        .skip(1)
        .map(|value| {
            value.into_string().map_err(|_| {
                "command arguments must be valid UTF-8 for systemd delegation".to_string()
            })
        })
        .collect()
}

pub(super) fn string_environment() -> Result<Vec<(String, String)>, String> {
    std::env::vars_os()
        .map(|(key, value)| {
            let key = key.into_string().map_err(|_| {
                "environment names must be valid UTF-8 for systemd delegation".to_string()
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
