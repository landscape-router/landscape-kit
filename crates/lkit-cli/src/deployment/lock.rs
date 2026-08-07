use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;

use super::plan::InstallError;
use super::root::InstallRoot;

const KNOWN_TOP_LEVEL: [&str; 10] = [
    "releases",
    "data",
    "state",
    "transactions",
    "backups",
    "run",
    "service",
    "logs",
    "current",
    "config.toml",
];

pub(crate) struct InstallLock {
    file: File,
}

pub(crate) fn acquire_install_lock(root: &InstallRoot) -> Result<InstallLock, InstallError> {
    if !root.canonical.exists() {
        std::fs::create_dir_all(&root.canonical).map_err(InstallError::Io)?;
    } else {
        for entry in std::fs::read_dir(&root.canonical).map_err(InstallError::Io)? {
            let name = entry.map_err(InstallError::Io)?.file_name();
            let known = KNOWN_TOP_LEVEL
                .iter()
                .any(|known| name.as_encoded_bytes() == known.as_bytes());
            if !known {
                return Err(InstallError::DangerousDirectory(format!(
                    "{} contains unknown content; refuse to create files for locking",
                    root.canonical.display()
                )));
            }
        }
    }
    let run_dir = root.canonical.join("run");
    std::fs::create_dir_all(&run_dir).map_err(InstallError::Io)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(run_dir.join("install.lock"))
        .map_err(InstallError::Io)?;
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result != 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
            return Err(InstallError::LockBusy);
        }
        return Err(InstallError::Io(error));
    }
    Ok(InstallLock { file })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("lkit-lock-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn new_root(path: &std::path::Path) -> InstallRoot {
        InstallRoot {
            install_root: path.to_path_buf(),
            canonical: path.to_path_buf(),
        }
    }

    #[test]
    fn acquires_lock_on_missing_root() {
        let temp = temp_root("missing");
        let target = temp.join("nested").join("root");
        let root = new_root(&target);
        let lock = acquire_install_lock(&root).unwrap();
        assert!(target.join("run/install.lock").is_file());
        drop(lock);
        assert!(acquire_install_lock(&root).is_ok());
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn rejects_second_concurrent_lock() {
        let temp = temp_root("busy");
        let root = new_root(&temp);
        let first = acquire_install_lock(&root).unwrap();
        assert!(matches!(
            acquire_install_lock(&root),
            Err(InstallError::LockBusy)
        ));
        drop(first);
        assert!(acquire_install_lock(&root).is_ok());
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn rejects_unknown_content() {
        let temp = temp_root("unknown");
        std::fs::write(temp.join("random.txt"), b"x").unwrap();
        let root = new_root(&temp);
        assert!(matches!(
            acquire_install_lock(&root),
            Err(InstallError::DangerousDirectory(_))
        ));
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn allows_empty_and_known_layout() {
        let temp = temp_root("empty");
        let root = new_root(&temp);
        assert!(acquire_install_lock(&root).is_ok());
        std::fs::create_dir_all(temp.join("data")).unwrap();
        assert!(acquire_install_lock(&root).is_ok());
        std::fs::write(temp.join("config.toml"), b"schema_version = 1\n").unwrap();
        assert!(acquire_install_lock(&root).is_ok());
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn ignores_transient_files_inside_run_dir() {
        let temp = temp_root("run-tmp");
        let root = new_root(&temp);
        assert!(acquire_install_lock(&root).is_ok());
        std::fs::write(temp.join("run/.current.tmp"), b"").unwrap();
        assert!(acquire_install_lock(&root).is_ok());
        let _ = std::fs::remove_dir_all(&temp);
    }
}
