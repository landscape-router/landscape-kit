use std::path::Path;

use super::paths;

/// 解析 `/etc/os-release` 的 `VERSION_ID` 字段。引号被剥离，无引号值直接使用。
pub(crate) fn host_version_id(path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    content.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("VERSION_ID=")?;
        let value = rest.trim().trim_matches('"');
        (!value.is_empty()).then(|| value.to_string())
    })
}

/// 主版本号：`40` -> `40`，`7.9.2009` -> `7`，`9` -> `9`。
pub(crate) fn major_version(path: &Path) -> Option<String> {
    host_version_id(path).and_then(|version| version.split('.').next().map(String::from))
}

/// Docker 是否已安装：检查常见安装路径下是否存在 `docker` 可执行文件。
pub(crate) fn docker_installed() -> bool {
    paths()
        .docker_bin
        .iter()
        .any(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_os_release(content: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lkit-software-detect-{}-{}",
            std::process::id(),
            rand_suffix()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("os-release");
        std::fs::write(&path, content).unwrap();
        path
    }

    fn rand_suffix() -> u64 {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        COUNTER.fetch_add(1, Ordering::SeqCst)
    }

    #[test]
    fn reads_version_id_and_major_version() {
        let path = write_os_release("ID=\"rocky\"\nVERSION_ID=\"9.3\"\n");
        assert_eq!(host_version_id(&path).as_deref(), Some("9.3"));
        assert_eq!(major_version(&path).as_deref(), Some("9"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn missing_version_id_is_none() {
        let path = write_os_release("ID=arch\n");
        assert_eq!(host_version_id(&path), None);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
