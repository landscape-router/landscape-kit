//! Binary discovery via `/proc/*/exe` with fallback to the configured home directory.

use std::path::PathBuf;

use lkit_core::LandscapePaths;

use crate::AppError;

/// Discover the running landscape-webserver binary path.
///
/// Strategy:
/// 1. Traverse `/proc/[0-9]*/exe`, resolve each symlink, and match
///    filenames containing `landscape-webserver`. Returns the first match.
/// 2. If no running process matches, falls back to
///    `{landscape_paths.home}/landscape-webserver`.
/// 3. If the fallback file does not exist, returns `AppError::NotFound`.
pub fn discover_binary(landscape_paths: &LandscapePaths) -> Result<PathBuf, AppError> {
    if let Some(path) = discover_via_proc() {
        return Ok(path);
    }
    let fallback = landscape_paths.home.join("landscape-webserver");
    if fallback.exists() {
        return Ok(fallback);
    }
    Err(AppError::NotFound("Landscape binary not found — is the service running?".into()))
}

/// Walk /proc/*/exe looking for landscape-webserver.
fn discover_via_proc() -> Option<PathBuf> {
    let proc = PathBuf::from("/proc");
    let dir = std::fs::read_dir(&proc).ok()?;
    for entry in dir.filter_map(|e| e.ok()) {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if !name_str.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let exe = entry.path().join("exe");
        let target = std::fs::read_link(&exe).ok()?;
        if target
            .file_name()
            .and_then(|f| f.to_str())
            .is_some_and(|f| f.contains("landscape-webserver"))
        {
            return Some(target);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use tempfile::TempDir;

    fn fake_landscape_paths(home: &std::path::Path) -> LandscapePaths {
        LandscapePaths::new(home.to_path_buf())
    }

    #[test]
    fn discover_via_proc_finds_binary() {
        let dir = TempDir::new().unwrap();
        let proc = dir.path().join("proc");
        let pid_dir = proc.join("1234");
        std::fs::create_dir_all(&pid_dir).unwrap();
        let fake_bin = dir.path().join("landscape-webserver");
        std::fs::write(&fake_bin, "fake-binary").unwrap();
        symlink(&fake_bin, pid_dir.join("exe")).unwrap();
        let paths = fake_landscape_paths(dir.path());
        let result = discover_binary(&paths).unwrap();
        assert_eq!(result, fake_bin);
    }

    #[test]
    fn fallback_uses_landscape_home() {
        let dir = TempDir::new().unwrap();
        let bin = dir.path().join("landscape-webserver");
        std::fs::write(&bin, "fake-binary").unwrap();
        let paths = fake_landscape_paths(dir.path());
        let result = discover_binary(&paths).unwrap();
        assert_eq!(result, bin);
    }

    #[test]
    fn fallback_fails_when_missing() {
        let dir = TempDir::new().unwrap();
        let paths = fake_landscape_paths(dir.path());
        let result = discover_binary(&paths);
        assert!(result.is_err());
    }
}
