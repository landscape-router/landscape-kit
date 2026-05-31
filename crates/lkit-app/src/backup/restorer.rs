//! Backup extraction and health check utilities.

use std::collections::HashMap;
use std::io::Read;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::Duration;

use lkit_core::LandscapePaths;

use crate::AppError;

/// Extract a tar.gz backup archive into `staging_dir`.
///
/// Returns a map of entry names (relative paths inside the archive) to their
/// absolute paths on disk within `staging_dir`.
pub fn extract_package(
    backup_path: &Path,
    staging_dir: &Path,
) -> Result<HashMap<String, PathBuf>, AppError> {
    use flate2::read::GzDecoder;

    let file = std::fs::File::open(backup_path)?;
    let decoder = GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);

    std::fs::create_dir_all(staging_dir)?;

    let mut result = HashMap::new();
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_path_buf();
        let path_str = path.to_string_lossy().to_string();
        let target = staging_dir.join(&path);

        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }

        entry.unpack(&target)?;

        // Preserve executable permission for binary.
        if path_str == "landscape-webserver" {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755))?;
        }

        result.insert(path_str, target);
    }

    Ok(result)
}

/// Read and parse `metadata.json` from a backup archive without full extraction.
pub fn read_metadata(backup_path: &Path) -> Result<crate::backup::BackupMetadata, AppError> {
    use flate2::read::GzDecoder;

    let file = std::fs::File::open(backup_path)?;
    let decoder = GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_string_lossy().to_string();
        if path == "metadata.json" {
            let mut content = String::new();
            entry.read_to_string(&mut content)?;
            let meta: crate::backup::BackupMetadata = serde_json::from_str(&content)
                .map_err(|e| AppError::Backup(format!("invalid metadata.json: {e}")))?;
            return Ok(meta);
        }
    }

    Err(AppError::Backup("metadata.json not found in archive".into()))
}

/// Perform a TCP-level health check by connecting to the configured listen port.
///
/// Reads `web.https_port` and `web.address` from `landscape.toml`.
/// Defaults to `127.0.0.1:443` if the config file cannot be parsed.
/// Retries up to 5 times with a 2-second interval between attempts.
pub fn health_check(landscape_paths: &LandscapePaths) -> Result<(), AppError> {
    let addr = parse_listen_addr(landscape_paths);

    let max_retries = 5;
    let timeout = Duration::from_secs(2);
    let interval = Duration::from_secs(2);

    for attempt in 1..=max_retries {
        match TcpStream::connect_timeout(&addr, timeout) {
            Ok(_) => return Ok(()),
            Err(e) => {
                if attempt < max_retries {
                    std::thread::sleep(interval);
                } else {
                    return Err(AppError::HealthCheckFailed(format!(
                        "cannot connect to {addr} after {max_retries} attempts: {e}"
                    )));
                }
            }
        }
    }

    Err(AppError::HealthCheckFailed("health check reached unreachable code".into()))
}

/// Parse listen address from landscape.toml, falling back to `127.0.0.1:6443`.
///
/// Supports two config formats:
/// - Runtime format: `[web].https_port`
/// - Backup/export format: `[config.web].https_port`
fn parse_listen_addr(paths: &LandscapePaths) -> std::net::SocketAddr {
    let default: std::net::SocketAddr = ([127, 0, 0, 1], 6443).into();
    let content = match std::fs::read_to_string(&paths.landscape_config) {
        Ok(c) => c,
        Err(_) => return default,
    };
    let value: toml::Value = match content.parse() {
        Ok(v) => v,
        Err(_) => return default,
    };
    // Try `[web]` (runtime format) then `[config.web]` (backup/export format).
    let web = value.get("web").or_else(|| value.get("config").and_then(|c| c.get("web")));
    let web = match web {
        Some(w) => w,
        None => return default,
    };
    let ip = web.get("address").and_then(|v| v.as_str()).unwrap_or("127.0.0.1");
    let port: u16 =
        web.get("https_port").and_then(|v| v.as_integer()).map(|p| p as u16).unwrap_or(6443);

    format!("{ip}:{port}")
        .to_socket_addrs()
        .ok()
        .and_then(|mut addrs| addrs.next())
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use tempfile::TempDir;

    #[test]
    fn health_check_succeeds_when_port_open() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();

        let dir = TempDir::new().unwrap();
        // Write a landscape.toml with a web.https_port matching our listener.
        let config = format!(
            r#"
[web]
address = "127.0.0.1"
https_port = {}
"#,
            port
        );
        std::fs::write(dir.path().join("landscape.toml"), &config).unwrap();
        let paths = LandscapePaths::new(dir.path().to_path_buf());

        // Start accepting connections in background.
        let _handle = thread::spawn(move || {
            let _ = listener.accept();
        });

        let result = health_check(&paths);
        assert!(result.is_ok());
    }

    #[test]
    fn health_check_fails_when_port_closed() {
        // Use a port that's very unlikely to be open.
        let dir = TempDir::new().unwrap();
        let config = r#"
[web]
address = "127.0.0.1"
https_port = 1
"#;
        std::fs::write(dir.path().join("landscape.toml"), &config).unwrap();
        let paths = LandscapePaths::new(dir.path().to_path_buf());

        let result = health_check(&paths);
        assert!(result.is_err());
    }

    #[test]
    fn parse_listen_addr_default_when_no_config() {
        let dir = TempDir::new().unwrap();
        let paths = LandscapePaths::new(dir.path().to_path_buf());
        // No landscape.toml exists; should use default.
        let addr = parse_listen_addr(&paths);
        assert_eq!(addr.port(), 6443);
    }

    #[test]
    fn read_metadata_fails_on_invalid_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("invalid.tar.gz");
        // Write garbage.
        std::fs::write(&path, "not-a-tar-gz").unwrap();
        let result = read_metadata(&path);
        assert!(result.is_err());
    }

    #[test]
    fn extract_package_roundtrip() {
        let staging = TempDir::new().unwrap();
        // Create real files and use build_package to create the archive.
        std::fs::write(staging.path().join("hello.txt"), "hello").unwrap();
        std::fs::write(staging.path().join("landscape-webserver"), "binary-content").unwrap();

        let archive_dir = TempDir::new().unwrap();
        let archive_path = archive_dir.path().join("test.tar.gz");
        crate::backup::builder::build_package(staging.path(), &archive_path).unwrap();

        // Now extract to a fresh directory.
        let staging2 = TempDir::new().unwrap();
        let result = extract_package(&archive_path, staging2.path()).unwrap();
        assert!(result.contains_key("hello.txt"));
        assert!(result.contains_key("landscape-webserver"));

        // Check binary has 0755.
        use std::os::unix::fs::PermissionsExt;
        let bin_path = staging2.path().join("landscape-webserver");
        let meta = std::fs::metadata(&bin_path).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o755);
    }
}
