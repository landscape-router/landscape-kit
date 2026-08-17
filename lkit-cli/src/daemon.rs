//! lkit 常驻服务。
//!
//! 常驻服务承担三件事:
//! 1. **委托命令执行**:生产命令由 CLI 写入
//!    [`OPERATIONS_DIR`](OPERATIONS_DIR) 的
//!    root-only 请求文件,daemon 周期扫描并执行,CLI 轮询结果。daemon 由 init
//!    系统启动,天然脱离用户会话,SSH 断开不会中止进行中的事务;
//! 2. **周期中断恢复**:CLI 进程因 SSH 断开、崩溃等原因消失后,遗留的未完成
//!    事务由 daemon 自动接管并执行与 CLI 相同的恢复语义
//!    ([`crate::deployment::transaction::recover_interrupted`]);
//! 3. **恒常托管 flare 服务端**:Linux 上 daemon 启动即托管 Landscape Terrain
//!    (L2 防失联通道)服务端,`[flare]` 段缺失或无 psk 时生成随机 psk 并持久化;
//!    每周期对比配置指纹,变更时重启 flare 任务拾取新配置。网络接管失败等
//!    IP 路径不可用时,操作员仍可经 L2 通道连接(见 [`reload_flare`])。
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

    // daemon 恒常托管 Landscape Terrain 服务端(L2 防失联通道)。daemon 退出时
    // drop shutdown 发送端,task 收到关闭通知后优雅退出。
    let mut flare = initial_flare();

    unsafe {
        let handler: extern "C" fn(libc::c_int) = handle_termination;
        libc::signal(libc::SIGTERM, handler as libc::sighandler_t);
        libc::signal(libc::SIGINT, handler as libc::sighandler_t);
    }
    loop {
        execute_pending_requests();
        recovery_cycle(&runtime).await;
        flare = reload_flare(flare);
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
    drop(flare);
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

/// 恒常启动 flare 服务端:Linux 上 daemon 启动即托管,`[flare]` 段缺失或无 psk
/// 时生成随机 psk 并持久化,启动时打印一次分发提示(后续安装会覆盖它)。
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
    let running = running?;
    if running.handle.is_finished() {
        // 服务端已退出(正常关闭或启动失败):用同一配置重新拉起。
        running.handle.abort();
        drop(running.shutdown);
        let (shutdown, handle) = spawn_flare_from(&running.section)?;
        return Some(FlareTask {
            shutdown,
            handle,
            section: running.section,
        });
    }
    let Some(current) = crate::deployment::config::load_flare() else {
        // 配置被删除或损坏:保持现役任务运行。
        return Some(running);
    };
    if !flare_needs_restart(&running.section, &current) {
        return Some(running);
    }
    drop(running.shutdown);
    let (shutdown, handle) = spawn_flare_from(&current)?;
    Some(FlareTask {
        shutdown,
        handle,
        section: current,
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
    current.psk.is_some() && current != running
}

/// 计算当前生效的 flare 配置:有 psk 直接使用,段缺失或无 psk 时生成随机 psk
/// 并持久化到 `config.toml` 的 `[flare]` 段(缺省字段由 serde 默认值补齐)。
/// 生成时打印一次 psk,提示分发给恢复操作员。
#[cfg(target_os = "linux")]
fn effective_flare_config() -> Result<crate::deployment::config::FlareSection, InstallError> {
    use crate::deployment::config::{default_flare_section, generate_psk, load_flare, save_flare};

    let mut section = match load_flare() {
        Some(section) => section,
        None => default_flare_section(),
    };
    if section.psk.is_none() {
        let psk = generate_psk();
        section.psk = Some(psk.clone());
        save_flare(&section)?;
        println!(
            "lkit daemon: generated flare recovery psk (written to {}); distribute it to operators, a later `lkit install` or `lkit flare setup` replaces it: {psk}",
            layout::territory_config_file().display()
        );
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
    let psk = section.psk.as_deref()?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    let mut args = crate::commands::flare::ServeArgs {
        psk: Some(psk.to_string()),
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
    let handle = tokio::spawn(async move {
        if let Err(error) = crate::commands::flare::run_serve(&args, Some(rx)).await {
            eprintln!("lkit daemon: flare server failed: {error}");
        }
    });
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

    #[cfg(target_os = "linux")]
    #[test]
    fn generates_and_persists_a_psk_when_the_flare_section_is_missing() {
        use crate::deployment::config::FLARE_PSK_MIN_LENGTH;
        use std::os::unix::fs::MetadataExt;

        let (guard, temp) = territory("flare-generate");
        let section = effective_flare_config().unwrap();
        assert!(section.psk.is_some());
        let psk = section.psk.as_deref().unwrap();
        assert!(psk.len() >= FLARE_PSK_MIN_LENGTH);
        let reloaded = crate::deployment::config::load_flare().unwrap();
        assert_eq!(
            reloaded.psk.as_deref(),
            Some(psk),
            "the psk must be persisted"
        );
        let config = layout::territory_config_file();
        let metadata = std::fs::metadata(&config).unwrap();
        assert_eq!(
            metadata.mode() & 0o077,
            0,
            "config with a psk must be root-only"
        );
        drop(guard);
        let _ = std::fs::remove_dir_all(&temp);
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
        let section = effective_flare_config().unwrap();
        assert_eq!(
            section.psk.as_deref(),
            Some("an-existing-recovery-secret"),
            "a configured psk must be reused"
        );
        drop(guard);
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn regenerates_a_psk_when_the_configured_one_was_removed() {
        use crate::deployment::config::{FlareSection, default_flare_section, save_flare};

        let (guard, temp) = territory("flare-regenerate");
        save_flare(&FlareSection {
            psk: Some("the-first-secret".into()),
            ..default_flare_section()
        })
        .unwrap();
        save_flare(&FlareSection {
            psk: None,
            ..default_flare_section()
        })
        .unwrap();
        let section = effective_flare_config().unwrap();
        let psk = section.psk.as_deref().unwrap();
        assert_ne!(psk, "the-first-secret");
        assert_eq!(
            crate::deployment::config::load_flare()
                .unwrap()
                .psk
                .as_deref(),
            Some(psk),
            "the regenerated psk must be persisted"
        );
        drop(guard);
        let _ = std::fs::remove_dir_all(&temp);
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
