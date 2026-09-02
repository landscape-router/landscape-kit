use std::path::{Path, PathBuf};
use std::sync::RwLock;

/// lkit 地盘固定位置。测试用 `test_territory()`(进程内静态覆盖)或环境变量
/// `LKIT_TERRITORY`(子进程,如 e2e fixture)覆盖。
pub(crate) const LKIT_TERRITORY: &str = "/root/.lkit";
/// landscape 安装根目录的缺省位置(仅 install/migrate 选择安装根时使用)。
pub(crate) const DEFAULT_LANDSCAPE_ROOT: &str = "/root/.lkit/landscape";

/// 进程内地盘覆盖(仅 `test_territory()` 写入)。读写都持锁:单元测试会在任意
/// 线程运行时改写覆盖值,而控制台后台 worker、daemon 线程会并发读取地盘;
/// 用静态覆盖取代旧实现里的环境变量改写,避免 `set_var` 与并发读取之间的
/// 数据竞争(读取方可能得到撕裂或过期的值)。
static TERRITORY_OVERRIDE: RwLock<Option<&'static Path>> = RwLock::new(None);

/// 优先级:进程内静态覆盖 > 环境变量 `LKIT_TERRITORY`(仅测试/工具钩子,
/// 文档不公开;进程内不再改写,只反映外部启动时注入的值) > `/root/.lkit`。
pub(crate) fn lkit_territory() -> &'static Path {
    let override_guard = TERRITORY_OVERRIDE
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(path) = *override_guard {
        return path;
    }
    drop(override_guard);
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

/// 测试辅助:全局互斥串行化(覆盖生效期间其它持有者排队)+ 写入进程内静态
/// 覆盖,Drop 时清除覆盖。
#[cfg(test)]
pub(crate) struct TerritoryOverride {
    _lock: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl Drop for TerritoryOverride {
    fn drop(&mut self) {
        // 先清除覆盖再释放互斥:与 `lkit_territory()` 的读取之间由 RwLock
        // 串行化,读取方不会观察到中间态。
        let mut override_guard = TERRITORY_OVERRIDE
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *override_guard = None;
    }
}

/// 测试辅助:把 lkit 地盘指向临时目录,返回 RAII 守卫(生命周期须覆盖整个测试)。
/// 任何测试都不得写真实 `/root/.lkit`。
#[cfg(test)]
pub(crate) fn test_territory(path: &Path) -> TerritoryOverride {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let lock = LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let leaked: &'static Path = Box::leak(path.to_path_buf().into_boxed_path());
    let mut override_guard = TERRITORY_OVERRIDE
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *override_guard = Some(leaked);
    TerritoryOverride { _lock: lock }
}
