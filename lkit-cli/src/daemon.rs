//! lkit 常驻服务。
//!
//! 常驻服务承担三件事:
//! 1. **操作执行**:领取持久化结构化请求并直接调用业务工作流。安装锁保证串行，
//!    前端断开不影响执行；操作记录保存结果并供 CLI/TUI 观察。
//! 2. **周期中断恢复**:CLI 进程因 SSH 断开、崩溃等原因消失后,遗留的未完成
//!    事务由 daemon 自动接管并执行与 CLI 相同的恢复语义
//!    ([`crate::deployment::transaction::recover_interrupted`]);
//! 3. **恒常托管 flare 服务端**:Linux 上 daemon 启动即托管 Landscape Terrain
//!    (L2 防失联通道)服务端,新装默认公开发现;
//!    每周期对比配置指纹,变更时重启 flare 任务拾取新配置。网络接管失败等
//!    IP 路径不可用时,操作员仍可经 L2 通道连接(见 [`reload_flare`])。
//!    网卡 DOWN 时 L2 通道无法收发帧,故 daemon 启动、托管 flare 之前会尽力
//!    拉起所有物理以太网卡(见 [`crate::network::ifup`]),失败只记录不阻断。
//!
//! daemon 全局唯一,固定读取 lkit 地盘(`/root/.lkit/`):pidfile 写入
//! [`layout::territory_pidfile`](crate::deployment::layout::territory_pidfile),
//! 恢复目标从地盘的状态与事务发现 landscape 根,不再绑定任何安装根。
//! 并发安全依赖安装锁:CLI 命令在整个操作期间持有
//! `<territory>/run/install.lock`,daemon 发现待处理工作时以非阻塞方式尝试获取,
//! 获取失败说明有活动命令,跳过本周期。网络接管由 daemon 监测期限与主机重启,
//! 自动回滚未确认事务并继续已接受的提交;用户仍需显式确认。

use std::path::Path;
#[cfg(feature = "test-support")]
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};

use clap::Args;

use crate::deployment::plan::InstallError;
use crate::deployment::runtime::InstallRuntime;
use crate::deployment::{layout, lock, state, transaction};

/// 每个恢复周期之间的基础间隔。
pub(crate) const DAEMON_CYCLE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

#[derive(Debug, Args)]
pub struct Daemon {
    #[arg(long, hide = true)]
    pub supervise: bool,
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
    if args.supervise {
        return crate::daemon_supervisor::run(args).await;
    }
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
    let _instance_lock =
        crate::daemon_supervisor::instance_lock("daemon.lock").map_err(InstallError::Io)?;
    // A published PID may be stopped immediately by the supervisor. Install
    // handlers before publication, including the best-effort startup window.
    unsafe {
        let handler: extern "C" fn(libc::c_int) = handle_termination;
        libc::signal(libc::SIGTERM, handler as libc::sighandler_t);
        libc::signal(libc::SIGINT, handler as libc::sighandler_t);
    }
    let runtime = std::sync::Arc::new(runtime);
    let pidfile = layout::territory_pidfile();
    std::fs::create_dir_all(
        pidfile
            .parent()
            .expect("territory pidfile has a parent directory"),
    )
    .map_err(InstallError::Io)?;
    write_pidfile(&pidfile)?;
    let recovery_capability = layout::territory_run_dir().join("network-recovery-v1.pid");
    write_pidfile(&recovery_capability)?;
    let service_capability = layout::territory_run_dir().join("service-repair-v1.pid");
    write_pidfile(&service_capability)?;
    let operations_capability = layout::territory_run_dir().join("operations-v1.pid");
    write_pidfile(&operations_capability)?;
    let operations_v3 = layout::territory_run_dir().join("operations-v3.pid");
    write_pidfile(&operations_v3)?;
    let operations_v2 = layout::territory_run_dir().join("operations-v2.pid");
    write_pidfile(&operations_v2)?;
    let migrate_capability = layout::territory_run_dir().join("migrate-v1.pid");
    write_pidfile(&migrate_capability)?;
    let install_capability = layout::territory_run_dir().join("install-v1.pid");
    write_pidfile(&install_capability)?;
    let reinit_capability = layout::territory_run_dir().join("reinit-v1.pid");
    write_pidfile(&reinit_capability)?;
    let network_operations = layout::territory_run_dir().join("network-operations-v1.pid");
    write_pidfile(&network_operations)?;
    let uninstall_capability = layout::territory_run_dir().join("uninstall-v1.pid");
    write_pidfile(&uninstall_capability)?;
    let restore_capability = layout::territory_run_dir().join("restore-v2.pid");
    write_pidfile(&restore_capability)?;
    let switch_capability = layout::territory_run_dir().join("switch-v1.pid");
    write_pidfile(&switch_capability)?;
    let assets_capability = layout::territory_run_dir().join("asset-repair-v2.pid");
    write_pidfile(&assets_capability)?;
    #[cfg(feature = "test-support")]
    crate::workflows::operation_control::checkpoint("daemon-starting")?;

    // Best effort only: bpffs preparation must not disable the L2 recovery
    // channel or prevent Landscape's normal service startup.
    if runtime.prepare_bpffs {
        crate::deployment::bpffs::prepare_best_effort();
    }

    // flare 的 L2 防失联通道依赖链路层收发帧:网卡 DOWN 时即使 flare 服务端
    // 在运行也无法交换帧。托管 flare 之前尽力拉起所有物理以太网卡(失败只
    // 记录日志,不阻断 daemon 启动)。
    #[cfg(target_os = "linux")]
    crate::network::ifup::bring_up_physical_interfaces(&runtime.sys_class_net, &runtime.ip_command);

    // daemon 恒常托管 Landscape Terrain 服务端(L2 防失联通道)。daemon 退出时
    // drop shutdown 发送端,task 收到关闭通知后优雅退出。
    let mut flare = initial_flare();

    let mut operation_task: Option<tokio::task::JoinHandle<()>> = None;
    loop {
        if SHUTDOWN_REQUESTED.load(Ordering::SeqCst) {
            break;
        }
        recovery_cycle(&runtime).await;
        if !SHUTDOWN_REQUESTED.load(Ordering::SeqCst)
            && operation_task
                .as_ref()
                .is_none_or(|task| task.is_finished())
        {
            if let Some(task) = operation_task.take() {
                let _ = task.await;
            }
            let runtime = runtime.clone();
            operation_task =
                Some(tokio::task::spawn_blocking(
                    move || match crate::daemon_operations::process_one(&runtime) {
                        Ok(true) => {}
                        Ok(false) => {}
                        Err(error) => {
                            eprintln!("lkit daemon: operation scheduling failed: {error}")
                        }
                    },
                ));
        }
        flare = reload_flare(flare);
        // 分片睡眠,保证 SIGTERM 及时生效。
        let slices = DAEMON_CYCLE_INTERVAL.as_millis().div_ceil(200).max(1) as u64;
        let mut shutdown = false;
        for _ in 0..slices {
            if SHUTDOWN_REQUESTED.load(Ordering::SeqCst) {
                shutdown = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        if shutdown {
            break;
        }
    }
    if let Some(task) = operation_task {
        let _ = task.await;
    }
    drop(flare);
    let _ = std::fs::remove_file(&migrate_capability);
    let _ = std::fs::remove_file(&install_capability);
    let _ = std::fs::remove_file(&reinit_capability);
    let _ = std::fs::remove_file(&network_operations);
    let _ = std::fs::remove_file(&uninstall_capability);
    let _ = std::fs::remove_file(&restore_capability);
    let _ = std::fs::remove_file(&switch_capability);
    let _ = std::fs::remove_file(&assets_capability);
    let _ = std::fs::remove_file(&operations_v3);
    let _ = std::fs::remove_file(&operations_v2);
    let _ = std::fs::remove_file(&operations_capability);
    let _ = std::fs::remove_file(&service_capability);
    let _ = std::fs::remove_file(&recovery_capability);
    let _ = std::fs::remove_file(&pidfile);
    Ok(())
}

/// 运行中的 flare 服务端任务:退出时 drop `shutdown` 发送端即可触发优雅关闭。
struct FlareTask {
    shutdown: tokio::sync::oneshot::Sender<()>,
    handle: tokio::task::JoinHandle<()>,
    /// 启动时生效的 `[flare]` 段(缺省字段已按 serde 默认值补齐),用于检测
    /// 配置变更:变更时重启任务拾取新配置。
    section: crate::deployment::config::FlareSection,
}

/// Linux daemon owns the Flare task and its discovery policy.
#[cfg(target_os = "linux")]
fn initial_flare() -> Option<FlareTask> {
    match effective_flare_config() {
        Ok(section) => spawn_flare_from(&section).map(|(shutdown, handle)| FlareTask {
            shutdown,
            handle,
            section,
        }),
        Err(error) => {
            eprintln!("lkit daemon: cannot start flare server: {error}");
            None
        }
    }
}

/// 非 Linux 平台不提供 flare 服务。
#[cfg(not(target_os = "linux"))]
fn initial_flare() -> Option<FlareTask> {
    None
}

/// 每个周期重估 flare 服务:配置变更时重启任务拾取新配置,任务意外退出(如链路
/// 暂不可用)时自动重新拉起。配置被删除、损坏或 psk 被清空时保持现役任务,
/// 绝不因配置问题切断恢复通道。
#[cfg(target_os = "linux")]
fn reload_flare(running: Option<FlareTask>) -> Option<FlareTask> {
    reload_flare_with(running, spawn_flare_from)
}

#[cfg(target_os = "linux")]
fn reload_flare_with(
    running: Option<FlareTask>,
    mut spawn: impl FnMut(
        &crate::deployment::config::FlareSection,
    ) -> Option<(
        tokio::sync::oneshot::Sender<()>,
        tokio::task::JoinHandle<()>,
    )>,
) -> Option<FlareTask> {
    let Some(running) = running else {
        let section = match effective_flare_config() {
            Ok(section) => section,
            Err(error) => {
                eprintln!("lkit daemon: cannot start flare server; will retry: {error}");
                return None;
            }
        };
        return spawn(&section).map(|(shutdown, handle)| FlareTask {
            shutdown,
            handle,
            section,
        });
    };
    // Read corrections before retrying a failed task. Invalid/cleared configuration
    // retains the last usable section, including while that task is being retried.
    let current = crate::deployment::config::load_flare().filter(|s| {
        s.public_discovery == Some(true) || s.psk.as_deref().is_some_and(|p| !p.is_empty())
    });
    if !running.handle.is_finished()
        && !current
            .as_ref()
            .is_some_and(|section| flare_needs_restart(&running.section, section))
    {
        return Some(running);
    }
    let section = current.unwrap_or(running.section);
    drop(running.shutdown);
    spawn(&section).map(|(shutdown, handle)| FlareTask {
        shutdown,
        handle,
        section,
    })
}

/// 非 Linux 平台不提供 flare 服务。
#[cfg(not(target_os = "linux"))]
fn reload_flare(running: Option<FlareTask>) -> Option<FlareTask> {
    running
}

/// 配置变更是否需要重启 flare 任务:psk 被清空或段缺失时不重启(保持现役,
/// 不切断恢复通道),其余字段变化且 psk 非空时重启拾取新配置。
#[cfg(target_os = "linux")]
fn flare_needs_restart(
    running: &crate::deployment::config::FlareSection,
    current: &crate::deployment::config::FlareSection,
) -> bool {
    (current.public_discovery == Some(true) || current.psk.is_some()) && current != running
}

/// Initialize public discovery on fresh installs and preserve privacy on v5 upgrades.
#[cfg(target_os = "linux")]
fn effective_flare_config() -> Result<crate::deployment::config::FlareSection, InstallError> {
    use crate::deployment::config::{read_flare, update_flare};
    let section = match read_flare()? {
        // A PSK without a discovery field already selects private mode. Returning
        // the saved section also keeps reload comparisons stable across cycles.
        Some(s) if s.public_discovery.is_some() || s.psk.is_some() => s,
        _ => update_flare(|s| {
            s.public_discovery.get_or_insert(s.psk.is_none());
        })?,
    };
    if section.public_discovery != Some(true) && section.psk.as_deref().is_none_or(str::is_empty) {
        return Err(InstallError::CorruptedState(
            "private discovery needs a discovery PSK".into(),
        ));
    }
    Ok(section)
}

/// 按给定的 `[flare]` 段启动 flare 服务端 task,返回 shutdown 发送端与 task
/// 句柄。非 Linux 平台返回 `None`(不启动)。
#[cfg(target_os = "linux")]
fn spawn_flare_from(
    section: &crate::deployment::config::FlareSection,
) -> Option<(
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
)> {
    let psk = if section.public_discovery == Some(true) {
        None
    } else {
        Some(section.psk.clone()?)
    };
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut args = crate::commands::flare::ServeArgs {
        psk,
        device_name: section.device_name.clone(),
        mac: None,
        dev: section.devices.clone().unwrap_or_else(|| "any".to_string()),
        ethertype: section.ethertype,
        forward_ports: section.forward_ports.clone(),
        token: section.token.clone(),
    };
    if let Some(mac) = section.mac.as_deref() {
        args.mac = landscape_terrain_proto::cli::parse_mac(mac).ok();
    }
    let handle = landscape_terrain_proto::concurrency::spawn_task(
        crate::task_names::DAEMON_FLARE,
        async move {
            if let Err(error) = crate::commands::flare::run_serve(&args, Some(rx)).await {
                eprintln!("lkit daemon: flare server failed: {error}");
            }
        },
    );
    Some((tx, handle))
}

/// 非 Linux 平台不提供 flare 服务。
#[cfg(not(target_os = "linux"))]
fn spawn_flare_from(
    _section: &crate::deployment::config::FlareSection,
) -> Option<(
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
)> {
    None
}

/// 单个恢复周期:以非阻塞方式获取安装锁,存在未完成事务时执行
/// `recover_interrupted`(与 CLI 相同的恢复语义)。
async fn recovery_cycle(runtime: &InstallRuntime) {
    if pending_recovery().is_none() {
        return;
    }
    let lock = match lock::acquire_install_lock() {
        Ok(lock) => lock,
        Err(_) => return,
    };
    // The CLI may have committed or replaced the transaction since the poll.
    let Some((install_root, unfinished)) = pending_recovery() else {
        return;
    };
    if (unfinished.operation == transaction::Operation::Reinit
        || matches!(
            unfinished.phase,
            transaction::Phase::AwaitingNetworkConfirmation
                | transaction::Phase::Finalizing
                | transaction::Phase::RollingBack
        ))
        && unfinished.confirmation_window().is_some()
    {
        if let Err(error) =
            crate::network::recovery::recover(&install_root, &unfinished, runtime).await
        {
            eprintln!("lkit daemon: network recovery failed; will retry: {error}");
        }
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

fn pending_recovery() -> Option<(
    crate::deployment::root::InstallRoot,
    transaction::TransactionFile,
)> {
    let root = discover_recovery_root()?;
    let transaction = transaction::find_unfinished(&root).ok()??;
    Some((root, transaction))
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
    // run_inner holds daemon.lock before publishing any PID. A leftover PID
    // may belong to an unrelated process after a reboot or PID reuse.
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

pub(crate) fn is_running() -> bool {
    instance_lock_held()
        && std::fs::read_to_string(layout::territory_pidfile())
            .ok()
            .and_then(|text| text.trim().parse::<u32>().ok())
            .is_some_and(process_alive)
}

fn instance_lock_held() -> bool {
    use std::os::fd::AsRawFd;
    let Ok(file) = std::fs::File::open(layout::territory_run_dir().join("daemon.lock")) else {
        return false;
    };
    // A separate open file description must conflict with the daemon's lock.
    // Successfully taking the shared lock means no daemon owns the exclusive one.
    (unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) }) != 0
        && std::io::Error::last_os_error().kind() == std::io::ErrorKind::WouldBlock
}

pub(crate) fn process_alive(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
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

/// Capabilities identify the live daemon, not its init supervisor.
pub(crate) fn require_capability(name: &str) -> Result<(), InstallError> {
    let pid = match std::fs::read_to_string(layout::territory_pidfile()) {
        Ok(pid) => pid,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(InstallError::Preflight(
                "lkit daemon is not running; run `lkit self install` before this command".into(),
            ));
        }
        Err(error) => return Err(InstallError::Io(error)),
    };
    let number = pid.trim().parse::<i32>().unwrap_or(0);
    let capability =
        std::fs::read_to_string(layout::territory_run_dir().join(name)).unwrap_or_default();
    if number <= 0
        || !instance_lock_held()
        || !process_alive(number as u32)
        || capability.trim() != pid.trim()
    {
        return Err(InstallError::Preflight("running daemon lacks required recovery capability; upgrade lkit and run `lkit self install`".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::MetadataExt;

    use super::*;

    #[test]
    fn missing_daemon_pid_explains_the_required_bootstrap() {
        let (_guard, temp) = territory("missing-capability-pid");
        let error = require_capability("install-v1.pid").unwrap_err();
        assert!(matches!(error, InstallError::Preflight(_)));
        let message = error.to_string();
        assert!(message.contains("not running"));
        assert!(message.contains("lkit self install"));
        assert!(!layout::territory_pidfile().exists());
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn capability_requires_the_live_daemon_pid_and_matching_marker() {
        let (_guard, temp) = territory("capabilities");
        std::fs::create_dir_all(layout::territory_run_dir()).unwrap();
        std::fs::write(layout::territory_pidfile(), std::process::id().to_string()).unwrap();
        assert!(require_capability("service-repair-v1.pid").is_err());
        let marker = layout::territory_run_dir().join("service-repair-v1.pid");
        std::fs::write(&marker, "0").unwrap();
        assert!(require_capability("service-repair-v1.pid").is_err());
        std::fs::write(&marker, std::process::id().to_string()).unwrap();
        assert!(require_capability("service-repair-v1.pid").is_err());
        assert!(!is_running());
        let instance = crate::daemon_supervisor::instance_lock("daemon.lock").unwrap();
        assert!(is_running());
        assert!(require_capability("service-repair-v1.pid").is_ok());
        drop(instance);
        assert!(!is_running());
        assert!(require_capability("service-repair-v1.pid").is_err());
        std::fs::write(layout::territory_pidfile(), "0").unwrap();
        assert!(require_capability("service-repair-v1.pid").is_err());
        std::fs::remove_dir_all(temp).unwrap();
    }

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

    #[cfg(feature = "test-support")]
    #[tokio::test]
    async fn idle_polls_leave_the_install_lock_free_and_still_clean_orphan_inputs() {
        let (_guard, temp) = territory("idle-poll");
        let run_dir = layout::territory_run_dir();
        std::fs::create_dir_all(&run_dir).unwrap();
        let config = run_dir.join("test-runtime.json");
        let runtime = serde_json::json!({
            "schema_version": 1, "allow_non_root": true, "managed_uid": 0,
            "os_release_path": temp.join("os-release"),
            "systemd": {
                "systemctl": temp.join("systemctl"), "system_unit_dir": temp.join("units"),
                "run_systemd_dir": run_dir, "pid1_is_systemd": false,
                "resolv_conf": temp.join("resolv.conf")
            },
            "health": {
                "base_url": "https://127.0.0.1:1", "dns_tcp_port": 53, "dns_udp_port": 53,
                "http_port": 80, "https_port": 443, "startup_timeout_ms": 1000, "stable_duration_ms": 1000
            },
            "export_base_url": "https://127.0.0.1:1"
        });
        std::fs::write(&config, runtime.to_string()).unwrap();
        let runtime = InstallRuntime::from_test_file(&config).unwrap();
        recovery_cycle(&runtime).await;
        assert!(!crate::daemon_operations::process_one(&runtime).unwrap());
        assert!(!layout::territory_install_lock().exists());

        let operations = run_dir.join("operations");
        std::fs::create_dir_all(&operations).unwrap();
        let orphan = operations.join(format!("{}.password", uuid::Uuid::now_v7()));
        std::fs::write(&orphan, b"fixture").unwrap();
        let lock = lock::acquire_install_lock().unwrap();
        assert!(!crate::daemon_operations::process_one(&runtime).unwrap());
        assert!(orphan.exists());
        drop(lock);
        assert!(!crate::daemon_operations::process_one(&runtime).unwrap());
        assert!(!orphan.exists());
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn instance_lock_rejects_duplicates_and_allows_replacing_reused_pids() {
        let (_guard, temp) = territory("conflict");
        let instance = crate::daemon_supervisor::instance_lock("daemon.lock").unwrap();
        assert!(crate::daemon_supervisor::instance_lock("daemon.lock").is_err());
        let pidfile = layout::territory_pidfile();
        // This process is alive but the file may be a leftover from another boot.
        std::fs::write(&pidfile, std::process::id().to_string()).unwrap();
        write_pidfile(&pidfile).unwrap();
        drop(instance);
        assert!(!is_running());
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn invalid_pids_never_address_process_groups() {
        for pid in [0, i32::MAX as u32 + 1, u32::MAX] {
            assert!(!process_alive(pid));
        }
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

    #[cfg(target_os = "linux")]
    #[test]
    fn new_configuration_is_public_and_has_no_generated_secret() {
        let (_guard, temp) = territory("flare-public");
        let section = effective_flare_config().unwrap();
        assert_eq!(section.public_discovery, Some(true));
        assert!(section.psk.is_none());
        assert_eq!(crate::deployment::config::load_flare(), Some(section));
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn reuses_an_existing_psk_instead_of_regenerating() {
        use crate::deployment::config::{FlareSection, default_flare_section, save_flare};

        let (guard, temp) = territory("flare-reuse");
        save_flare(&FlareSection {
            psk: Some("an-existing-recovery-secret".into()),
            ..default_flare_section()
        })
        .unwrap();
        let path = layout::territory_config_file();
        let before = std::fs::read(&path).unwrap();
        let section = effective_flare_config().unwrap();
        assert_eq!(
            section.psk.as_deref(),
            Some("an-existing-recovery-secret"),
            "a configured psk must be reused"
        );
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(crate::deployment::config::load_flare(), Some(section));
        drop(guard);
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[cfg(target_os = "linux")]
    fn fake_flare_task() -> (
        tokio::sync::oneshot::Sender<()>,
        tokio::task::JoinHandle<()>,
    ) {
        let (shutdown, receiver) = tokio::sync::oneshot::channel();
        (
            shutdown,
            tokio::spawn(async move {
                let _ = receiver.await;
            }),
        )
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn failed_flare_restarts_with_corrected_configuration() {
        let (_guard, temp) = territory("flare-corrected");
        let old = crate::deployment::config::update_flare(|section| {
            section.psk = Some("recovery-secret".into());
            section.devices = Some("missing-device".into());
        })
        .unwrap();
        let (shutdown, _) = tokio::sync::oneshot::channel();
        let mut failed = FlareTask {
            shutdown,
            handle: tokio::spawn(async {}),
            section: old,
        };
        (&mut failed.handle).await.unwrap();
        crate::deployment::config::update_flare(|section| section.devices = Some("eth0".into()))
            .unwrap();
        let restarted = reload_flare_with(Some(failed), |config| {
            assert_eq!(config.devices.as_deref(), Some("eth0"));
            Some(fake_flare_task())
        })
        .unwrap();
        assert_eq!(restarted.section.devices.as_deref(), Some("eth0"));
        drop(restarted.shutdown);
        restarted.handle.await.unwrap();
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn absent_flare_retries_after_configuration_becomes_writable() {
        let (_guard, temp) = territory("flare-retry-initial");
        let config = layout::territory_config_file();
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        // A directory at the config path makes initial persistence fail without
        // invoking any real service or depending on root permission semantics.
        std::fs::create_dir(&config).unwrap();
        let missing = reload_flare_with(None, |_| {
            panic!("unpersisted credentials must not start a task")
        });
        assert!(missing.is_none());
        std::fs::remove_dir(&config).unwrap();
        let running = reload_flare_with(missing, |_| Some(fake_flare_task())).unwrap();
        assert_eq!(
            crate::deployment::config::load_flare().unwrap(),
            running.section
        );
        drop(running.shutdown);
        running.handle.await.unwrap();
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn invalid_flare_configuration_preserves_the_live_task() {
        let (_guard, temp) = territory("flare-keep-live");
        let running = reload_flare_with(None, |_| Some(fake_flare_task())).unwrap();
        let task_id = running.handle.id();
        std::fs::write(layout::territory_config_file(), "invalid toml [[[").unwrap();
        let kept = reload_flare_with(Some(running), |_| {
            panic!("live recovery channel must be retained")
        })
        .unwrap();
        assert_eq!(kept.handle.id(), task_id);
        drop(kept.shutdown);
        kept.handle.await.unwrap();
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn missing_private_discovery_secret_fails_closed() {
        let (_guard, temp) = territory("flare-missing-key");
        crate::deployment::config::update_flare(|s| s.public_discovery = Some(false)).unwrap();
        assert!(effective_flare_config().is_err());
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn flare_section_changes_are_detected_for_reload() {
        use crate::deployment::config::{FlareSection, default_flare_section};

        let first = FlareSection {
            psk: Some("first-recovery-secret".into()),
            ..default_flare_section()
        };
        // 配置未变:不重启。
        assert!(!flare_needs_restart(&first, &first));
        // psk 变更:重启。
        let second = FlareSection {
            psk: Some("second-recovery-secret".into()),
            ..first.clone()
        };
        assert!(flare_needs_restart(&first, &second));
        // psk 被清空:保持现役,不切断恢复通道。
        let cleared = FlareSection {
            psk: None,
            ..first.clone()
        };
        assert!(!flare_needs_restart(&first, &cleared));
        // 其它字段(如监听设备)变化且 psk 非空:重启。
        let devices = FlareSection {
            psk: Some("first-recovery-secret".into()),
            devices: Some("eth1".into()),
            ..first.clone()
        };
        assert!(flare_needs_restart(&first, &devices));
    }
}
