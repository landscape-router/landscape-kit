//! Binary discovery for landscape-webserver.

use std::path::{Path, PathBuf};

use crate::AppError;

/// Discover the landscape-webserver binary path.
///
/// 1. Scan /proc/[0-9]*/exe for a process whose basename == "landscape-webserver".
/// 2. Fallback: {LANDSCAPE_HOME}/landscape-webserver.
/// 3. Not found: Err(AppError::Backup("landscape-webserver binary not found")).
pub fn discover_binary(landscape_home: &Path) -> Result<PathBuf, AppError> {
    if let Some(path) = scan_proc_for_binary("landscape-webserver") {
        return Ok(path);
    }

    let fallback = landscape_home.join("landscape-webserver");
    if fallback.is_file() {
        return Ok(fallback);
    }

    Err(AppError::Backup(
        "landscape-webserver binary not found".into(),
    ))
}

/// Walk /proc/[0-9]*/exe looking for a binary whose filename matches `name`.
fn scan_proc_for_binary(name: &str) -> Option<PathBuf> {
    let proc_dir = std::fs::read_dir("/proc").ok()?;

    for entry in proc_dir.flatten() {
        let file_name = entry.file_name();
        let dir_name = file_name.to_str()?;

        if !dir_name.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }

        let exe_path = entry.path().join("exe");
        if let Ok(target) = std::fs::read_link(&exe_path)
            && let Some(base) = target.file_name().and_then(|n| n.to_str())
            && base == name
        {
            return Some(target);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discover_binary_fallback_when_proc_empty() {
        let home = std::env::temp_dir();
        let result = discover_binary(&home);
        assert!(result.is_err());
        match result {
            Err(AppError::Backup(msg)) => {
                assert!(msg.contains("not found"));
            }
            _ => panic!("expected Backup error"),
        }
    }

    #[test]
    fn test_scan_proc_for_non_existent_binary() {
        assert!(scan_proc_for_binary("nonexistent-binary-xyz").is_none());
    }
}
