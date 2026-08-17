//! lkit 常驻服务。
//!
//! 常驻服务承担两件事:
//! 1. **委托命令执行**:生产命令由 CLI 写入
//!    [`OPERATIONS_DIR`](OPERATIONS_DIR) 的
//!    root-only 请求文件,daemon 周期扫描并执行,CLI 轮询结果。daemon 由 init
//!    系统启动,天然脱离用户会话,SSH 断开不会中止进行中的事务;
//! 2. **周期中断恢复**:CLI 进程因 SSH 断开、崩溃等原因消失后,遗留的未完成
//!    事务由 daemon 自动接管并执行与 CLI 相同的恢复语义
//!    ([`crate::deployment::transaction::recover_interrupted`])。
//!
//! daemon 全局唯一,固定读取 lkit 地盘(`/root/.lkit/`):pidfile 写入
//! [`layout::territory_pidfile`](crate::deployment::layout::territory_pidfile),
//! 恢复目标从地盘的状态与事务发现 landscape 根,不再绑定任何安装根。
//! 并发安全依赖安装锁:CLI 命令在整个操作期间持有
//! `<territory>/run/install.lock`,daemon 每个周期以非阻塞方式尝试获取,
//! 获取失败说明有活动命令,跳过本周期。网络接管待确认阶段保持人工
//! `lkit network confirm|rollback`,daemon 不代替用户确认。

use std::path::Path;
#[cfg(feature = "test-support")]
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};

use clap::Args;

use crate::deployment::plan::InstallError;
use crate::deployment::runtime::InstallRuntime;
use crate::deployment::{layout, lock, state, transaction};
use crate::interaction::presentation::OPERATIONS_DIR;

/// 每个恢复周期之间的基础间隔。
pub(crate) const DAEMON_CYCLE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

#[derive(Debug, Args)]
pub struct Daemon {
    #[cfg(feature = "test-support")]
    #[arg(long, value_name = "PATH", hide = true)]
    pub test_runtime: Option<PathBuf>,
}

static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

extern "C" fn handle_termination(_signal: libc::c_int) {
    SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
}

/// 运行 lkit 常驻服务直到收到 SIGTERM/SIGINT。
pub(crate) async fn run(args: &Daemon) -> ExitCode {
    run_with_runtime(resolve_runtime(args)).await
}

pub(crate) async fn run_with_runtime(runtime: InstallRuntime) -> ExitCode {
    match run_inner(runtime).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("lkit daemon: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run_inner(runtime: InstallRuntime) -> Result<(), InstallError> {
    let pidfile = layout::territory_pidfile();
    std::fs::create_dir_all(
        pidfile
            .parent()
            .expect("territory pidfile has a parent directory"),
    )
    .map_err(InstallError::Io)?;
    write_pidfile(&pidfile)?;

    unsafe {
        let handler: extern "C" fn(libc::c_int) = handle_termination;
        libc::signal(libc::SIGTERM, handler as libc::sighandler_t);
        libc::signal(libc::SIGINT, handler as libc::sighandler_t);
    }
    loop {
        execute_pending_requests();
        recovery_cycle(&runtime).await;
        // 分片睡眠,保证 SIGTERM 及时生效。
        let slices = DAEMON_CYCLE_INTERVAL.as_millis().div_ceil(200).max(1) as u64;
        let mut shutdown = false;
        for _ in 0..slices {
            if SHUTDOWN_REQUESTED.load(Ordering::SeqCst) {
                shutdown = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        if shutdown {
            break;
        }
    }
    let _ = std::fs::remove_file(&pidfile);
    Ok(())
}

/// 扫描并执行委托请求。请求文件由 CLI 写入
/// `OPERATIONS_DIR/<id>.request.json`;daemon 逐次认领执行,期间阻塞周期循环,
/// 完成后立即检查下一个请求(恢复周期让位于执行)。
fn execute_pending_requests() {
    let operations = match std::fs::read_dir(OPERATIONS_DIR) {
        Ok(operations) => operations,
        Err(_) => return,
    };
    let mut requests: Vec<std::path::PathBuf> = operations
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().ends_with(".request.json"))
        })
        .collect();
    requests.sort();
    for request in requests {
        if request.parent() != Some(std::path::Path::new(OPERATIONS_DIR)) {
            continue;
        }
        let exit_code = crate::daemon_worker::execute_request(&request);
        if exit_code != 0 {
            eprintln!("lkit daemon: delegated operation exited with {exit_code}");
        }
    }
}

/// 单个恢复周期:以非阻塞方式获取安装锁,存在未完成事务时执行
/// `recover_interrupted`(与 CLI 相同的恢复语义)。
async fn recovery_cycle(runtime: &InstallRuntime) {
    let lock = match lock::acquire_install_lock() {
        Ok(lock) => lock,
        Err(_) => return,
    };
    // 恢复目标从 lkit 地盘的状态与事务发现 landscape 根:状态记录已提交安装,
    // 中断的首次安装还没有状态,从未完成事务记录的根发现。
    let Some(install_root) = discover_recovery_root() else {
        return;
    };
    let unfinished = match transaction::find_unfinished(&install_root) {
        Ok(unfinished) => unfinished,
        Err(_) => return,
    };
    let Some(unfinished) = unfinished else { return };
    if matches!(
        unfinished.phase,
        transaction::Phase::AwaitingNetworkConfirmation
            | transaction::Phase::Finalizing
            | transaction::Phase::RollingBack
    ) && unfinished.network_takeover.is_some()
    {
        // 网络接管待确认/回滚由 `lkit network confirm|rollback` 人工处理。
        return;
    }
    let health = match runtime.health_options() {
        Ok(health) => health,
        Err(_) => return,
    };
    match transaction::recover_interrupted(
        &install_root,
        &unfinished,
        runtime.service_manager.as_ref(),
        &health,
    )
    .await
    {
        Ok(()) => println!(
            "lkit daemon: recovered interrupted {} transaction {}",
            unfinished.operation.key(),
            unfinished.transaction_id
        ),
        Err(error) => eprintln!(
            "lkit daemon: recovering interrupted {} transaction {} failed: {error}",
            unfinished.operation.key(),
            unfinished.transaction_id
        ),
    }
    drop(lock);
}

/// 从 lkit 地盘的状态与事务发现 landscape 恢复目标。状态文件缺失时
/// (中断的首次安装),从未完成事务记录的 `canonical_install_root` 发现。
fn discover_recovery_root() -> Option<crate::deployment::root::InstallRoot> {
    if let Ok(Some(root)) = state::discover_landscape_root() {
        return Some(root);
    }
    let dir = layout::territory_transactions_dir();
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(content) = std::fs::read(&path) else {
            continue;
        };
        let Ok(tx) = serde_json::from_slice::<transaction::TransactionFile>(&content) else {
            continue;
        };
        if tx.phase.is_terminal() {
            continue;
        }
        let Ok(root) =
            crate::deployment::root::normalize_install_root(Path::new(&tx.canonical_install_root))
        else {
            continue;
        };
        return Some(root);
    }
    None
}

fn write_pidfile(pidfile: &Path) -> Result<(), InstallError> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    std::fs::create_dir_all(
        pidfile
            .parent()
            .expect("territory pidfile has a parent directory"),
    )
    .map_err(InstallError::Io)?;
    if let Ok(existing) = std::fs::read_to_string(pidfile)
        && let Ok(pid) = existing.trim().parse::<u32>()
        && process_alive(pid)
    {
        return Err(InstallError::ProcessConflict(format!(
            "another lkit daemon is already running with pid {pid}"
        )));
    }
    let tmp = pidfile.with_extension("tmp");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(InstallError::Io)?;
    writeln!(file, "{}", std::process::id()).map_err(InstallError::Io)?;
    file.sync_all().map_err(InstallError::Io)?;
    std::fs::rename(&tmp, pidfile).map_err(InstallError::Io)?;
    Ok(())
}

pub(crate) fn process_alive(pid: u32) -> bool {
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn resolve_runtime(_args: &Daemon) -> InstallRuntime {
    #[cfg(feature = "test-support")]
    if let Some(path) = _args.test_runtime.as_deref()
        && let Ok(runtime) = InstallRuntime::from_test_file(path)
    {
        return runtime;
    }
    InstallRuntime::production()
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::MetadataExt;

    use super::*;

    #[test]
    fn cycle_interval_is_sliceable() {
        assert!(DAEMON_CYCLE_INTERVAL.as_millis() >= 200);
    }

    fn territory(name: &str) -> (layout::TerritoryOverride, std::path::PathBuf) {
        let temp =
            std::env::temp_dir().join(format!("lkit-daemon-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();
        let guard = layout::test_territory(&temp);
        (guard, temp)
    }

    #[test]
    fn pidfile_is_written_into_the_territory_with_0600() {
        let (guard, temp) = territory("pidfile");
        let pidfile = layout::territory_pidfile();
        assert_eq!(
            pidfile,
            temp.join("run/lkit.pid"),
            "pidfile must be <territory>/run/lkit.pid"
        );
        write_pidfile(&pidfile).unwrap();
        let metadata = std::fs::metadata(&pidfile).unwrap();
        assert_eq!(metadata.mode() & 0o077, 0, "pidfile must be root-only");
        assert_eq!(
            std::fs::read_to_string(&pidfile).unwrap().trim(),
            std::process::id().to_string()
        );
        drop(guard);
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn refuses_to_start_while_a_live_instance_exists() {
        let (guard, temp) = territory("conflict");
        let pidfile = layout::territory_pidfile();
        write_pidfile(&pidfile).unwrap();
        assert!(matches!(
            write_pidfile(&pidfile),
            Err(InstallError::ProcessConflict(_))
        ));
        drop(guard);
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn overwrites_a_stale_pidfile() {
        let (guard, temp) = territory("stale");
        let pidfile = layout::territory_pidfile();
        std::fs::create_dir_all(pidfile.parent().unwrap()).unwrap();
        std::fs::write(&pidfile, "99999999\n").unwrap();
        write_pidfile(&pidfile).unwrap();
        assert_eq!(
            std::fs::read_to_string(&pidfile).unwrap().trim(),
            std::process::id().to_string()
        );
        drop(guard);
        let _ = std::fs::remove_dir_all(&temp);
    }
}
