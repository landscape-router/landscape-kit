use std::path::{Path, PathBuf};

/// lkit 地盘固定位置。测试用环境变量 `LKIT_TERRITORY` 覆盖。
pub(crate) const LKIT_TERRITORY: &str = "/root/.lkit";
/// landscape 安装根目录的缺省位置(仅 install/migrate 选择安装根时使用)。
pub(crate) const DEFAULT_LANDSCAPE_ROOT: &str = "/root/.lkit/landscape";

/// 环境变量 LKIT_TERRITORY 存在时用之(仅测试/工具钩子,文档不公开),否则 `/root/.lkit`。
pub(crate) fn lkit_territory() -> &'static Path {
    if let Ok(value) = std::env::var("LKIT_TERRITORY")
        && !value.is_empty()
    {
        return Path::new(Box::leak(value.into_boxed_str()));
    }
    Path::new(LKIT_TERRITORY)
}

pub(crate) fn territory_state_path() -> PathBuf {
    lkit_territory().join("state").join("install-state.json")
}

pub(crate) fn territory_transactions_dir() -> PathBuf {
    lkit_territory().join("transactions")
}

pub(crate) fn territory_backups_dir() -> PathBuf {
    lkit_territory().join("backups")
}

pub(crate) fn territory_logs_dir() -> PathBuf {
    lkit_territory().join("logs")
}

pub(crate) fn territory_run_dir() -> PathBuf {
    lkit_territory().join("run")
}

pub(crate) fn territory_install_lock() -> PathBuf {
    territory_run_dir().join("install.lock")
}

pub(crate) fn territory_pidfile() -> PathBuf {
    territory_run_dir().join("lkit.pid")
}

pub(crate) fn territory_config_file() -> PathBuf {
    lkit_territory().join("config.toml")
}

/// 解析事务/状态/备份记录中的 lkit 地盘相对路径(`backups/…`、`transactions/…`、
/// `logs/…`)为地盘绝对路径。其余前缀按地盘根下的相对路径解析。
pub(crate) fn territory_relative(relative: &str) -> PathBuf {
    let relative = Path::new(relative);
    if let Ok(rest) = relative.strip_prefix("backups") {
        territory_backups_dir().join(rest)
    } else if let Ok(rest) = relative.strip_prefix("transactions") {
        territory_transactions_dir().join(rest)
    } else if let Ok(rest) = relative.strip_prefix("logs") {
        territory_logs_dir().join(rest)
    } else {
        lkit_territory().join(relative)
    }
}

/// 测试辅助:全局互斥串行化 + 设置 `LKIT_TERRITORY`,Drop 时恢复原值。
#[cfg(test)]
pub(crate) struct TerritoryOverride {
    _lock: std::sync::MutexGuard<'static, ()>,
    previous: Option<std::ffi::OsString>,
}

#[cfg(test)]
impl Drop for TerritoryOverride {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => unsafe { std::env::set_var("LKIT_TERRITORY", value) },
            None => unsafe { std::env::remove_var("LKIT_TERRITORY") },
        }
    }
}

/// 测试辅助:把 lkit 地盘指向临时目录,返回 RAII 守卫(生命周期须覆盖整个测试)。
/// 任何测试都不得写真实 `/root/.lkit`。
#[cfg(test)]
pub(crate) fn test_territory(path: &Path) -> TerritoryOverride {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let lock = LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous = std::env::var_os("LKIT_TERRITORY");
    unsafe { std::env::set_var("LKIT_TERRITORY", path) };
    TerritoryOverride {
        _lock: lock,
        previous,
    }
}
