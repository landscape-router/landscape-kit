use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;

use super::layout;
use super::plan::InstallError;

/// lkit 地盘顶层的已知条目:lkit 元数据目录/文件与缺省 landscape 根。
const KNOWN_TOP_LEVEL: [&str; 7] = [
    "config.toml",
    "state",
    "transactions",
    "backups",
    "logs",
    "run",
    "landscape",
];

pub(crate) struct InstallLock {
    file: File,
}

/// 获取 lkit 地盘安装锁。创建地盘与 `run/`;顶层检查:已知名或任意目录
/// (跟随软链)放行,未知文件 → 危险目录。锁文件为 `<territory>/run/install.lock`。
pub(crate) fn acquire_install_lock() -> Result<InstallLock, InstallError> {
    let territory = layout::lkit_territory();
    if !territory.exists() {
        std::fs::create_dir_all(territory).map_err(InstallError::Io)?;
    } else {
        for entry in std::fs::read_dir(territory).map_err(InstallError::Io)? {
            let entry = entry.map_err(InstallError::Io)?;
            let name = entry.file_name();
            let known = KNOWN_TOP_LEVEL
                .iter()
                .any(|known| name.as_encoded_bytes() == known.as_bytes());
            if known {
                continue;
            }
            let metadata = entry.metadata().map_err(|_| {
                InstallError::DangerousDirectory(format!(
                    "{} contains unknown content; refuse to create files for locking",
                    territory.display()
                ))
            })?;
            if !metadata.is_dir() {
                return Err(InstallError::DangerousDirectory(format!(
                    "{} contains unknown content; refuse to create files for locking",
                    territory.display()
                )));
            }
        }
    }
    let run_dir = layout::territory_run_dir();
    std::fs::create_dir_all(&run_dir).map_err(InstallError::Io)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(layout::territory_install_lock())
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
    use crate::deployment::layout;

    /// 建立隔离测试现场:返回 (守卫, 地盘)。
    fn setup(name: &str) -> (layout::TerritoryOverride, std::path::PathBuf) {
        let temp =
            std::env::temp_dir().join(format!("lkit-lock-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();
        let territory = temp.join("territory");
        let guard = layout::test_territory(&territory);
        (guard, territory)
    }

    #[test]
    fn acquires_lock_on_missing_territory_dir() {
        let (_guard, territory) = setup("missing-dir");
        assert!(
            !territory.exists(),
            "the territory directory must be missing"
        );
        let lock = acquire_install_lock().unwrap();
        assert!(territory.join("run/install.lock").is_file());
        drop(lock);
        assert!(acquire_install_lock().is_ok());
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    #[test]
    fn rejects_second_concurrent_lock() {
        let (_guard, territory) = setup("busy");
        let first = acquire_install_lock().unwrap();
        assert!(matches!(
            acquire_install_lock(),
            Err(InstallError::LockBusy)
        ));
        drop(first);
        assert!(acquire_install_lock().is_ok());
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    #[test]
    fn rejects_unknown_files_in_the_territory() {
        let (_guard, territory) = setup("unknown");
        std::fs::create_dir_all(&territory).unwrap();
        std::fs::write(territory.join("random.txt"), b"x").unwrap();
        assert!(matches!(
            acquire_install_lock(),
            Err(InstallError::DangerousDirectory(_))
        ));
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    #[test]
    fn allows_empty_and_known_layout() {
        let (_guard, territory) = setup("empty");
        assert!(acquire_install_lock().is_ok());
        std::fs::create_dir_all(territory.join("state")).unwrap();
        assert!(acquire_install_lock().is_ok());
        std::fs::write(territory.join("config.toml"), b"schema_version = 1\n").unwrap();
        assert!(acquire_install_lock().is_ok());
        std::fs::create_dir_all(territory.join("landscape")).unwrap();
        assert!(acquire_install_lock().is_ok());
        std::fs::create_dir_all(territory.join("arbitrary-dir")).unwrap();
        assert!(acquire_install_lock().is_ok());
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    #[test]
    fn ignores_transient_files_inside_run_dir() {
        let (_guard, territory) = setup("run-tmp");
        assert!(acquire_install_lock().is_ok());
        std::fs::write(territory.join("run/.current.tmp"), b"").unwrap();
        assert!(acquire_install_lock().is_ok());
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }
}
