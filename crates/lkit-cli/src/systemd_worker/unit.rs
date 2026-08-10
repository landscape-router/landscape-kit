use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::WORKER_COMMAND;

pub(super) fn render_unit(
    executable: &Path,
    request_path: &Path,
    stdout_path: &Path,
    stderr_path: &Path,
) -> String {
    format!(
        "[Unit]\nDescription=Landscape Kit operation\n\n[Service]\nType=exec\nExecStart={} {} {}\nKillMode=control-group\nTimeoutStartSec=infinity\nStandardInput=null\nStandardOutput=append:{}\nStandardError=append:{}\n",
        unit_quote(&executable.display().to_string()),
        WORKER_COMMAND,
        unit_quote(&request_path.display().to_string()),
        unit_escape(&stdout_path.display().to_string()),
        unit_escape(&stderr_path.display().to_string()),
    )
}

fn unit_quote(value: &str) -> String {
    format!("\"{}\"", unit_escape(value).replace('"', "\\\""))
}

fn unit_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('%', "%%")
}

pub(super) fn terminal_path() -> Option<PathBuf> {
    let path = std::fs::read_link("/proc/self/fd/0").ok()?;
    (path.starts_with("/dev/") && !path.as_os_str().is_empty()).then_some(path)
}

pub(super) fn write_unit(path: &Path, content: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("unit path {} has no parent", path.display()))?;
    if !parent.is_dir() {
        return Err(format!(
            "systemd runtime unit directory {} is missing",
            parent.display()
        ));
    }
    let temporary = path.with_extension("service.tmp");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o644)
        .open(&temporary)
        .map_err(|error| format!("create temporary worker unit: {error}"))?;
    file.write_all(content.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("write worker unit: {error}"))?;
    std::fs::rename(&temporary, path)
        .map_err(|error| format!("install worker unit {}: {error}", path.display()))
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
    use std::os::unix::fs::MetadataExt;
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
