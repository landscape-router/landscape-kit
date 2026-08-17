mod executor;
mod protocol;
mod wait;

use wait::wait_for_result;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use uuid::Uuid;

use crate::commands::Commands;
use crate::commands::network::NetworkAction;
#[cfg(feature = "test-support")]
use crate::deployment::runtime::InstallRuntime;
use crate::interaction::presentation::{InterruptGuard, OPERATIONS_DIR, operation_screen};
use crate::network::config::NetworkPlan;

use self::protocol::{
    CANCEL_FILE_SUFFIX, RemoveFile, WaitOutcome, WorkerRequest, create_private_file,
    create_private_secret_file, string_environment, terminal_path, write_private_json,
};

/// 委托执行者标记:daemon executor 起子进程时注入的命令行参数。子命令以此为
/// 依据内联执行,不再二次委托——否则 daemon 等待子进程、子进程又等待 daemon
/// 认领自己写下的请求文件,形成死锁。
pub(crate) const DAEMON_WORKER_FLAG: &str = "--internal-daemon-worker";

/// 委托失败分类:Usage 属于使用错误(退出码 2),Infrastructure 属于环境故障。
pub(crate) enum DelegateError {
    Usage(String),
    Infrastructure(String),
}

impl DelegateError {
    fn usage(message: impl Into<String>) -> Self {
        Self::Usage(message.into())
    }

    fn infrastructure(message: impl Into<String>) -> Self {
        Self::Infrastructure(message.into())
    }
}

/// 委托命令清单(与 docs/deployment/transactions-and-recovery.md 的
/// 「委托命令清单」保持一致):所有需要改变 init 系统或 Landscape 运行态的
/// 命令都由常驻 daemon 执行;只读与地盘内命令直接执行。
pub(crate) fn delegates(command: &Commands) -> bool {
    match command {
        Commands::Check(_) | Commands::Reconcile(_) | Commands::SetMirror(_) => false,
        Commands::Software(_) => false,
        Commands::Backup(_) => false,
        Commands::Self_(_) | Commands::Daemon(_) => false,
        Commands::Network(args) => {
            matches!(
                args.action,
                // 手工 rollback 与 confirm 都委托:两者都会切换/恢复宿主网络,
                // 发起会话可能在执行中断开,由 daemon 侧完成收尾。
                NetworkAction::Confirm | NetworkAction::Rollback { automatic: false }
            )
        }
        Commands::Install(_)
        | Commands::Migrate(_)
        | Commands::Switch(_)
        | Commands::Update(_)
        | Commands::Repair(_)
        | Commands::Restore(_)
        | Commands::Reinit(_)
        | Commands::Uninstall(_) => true,
    }
}

pub(crate) fn should_delegate(command: &Commands) -> bool {
    if unsafe { libc::geteuid() } != 0 || test_runtime_is_inline(command) {
        return false;
    }
    delegates(command)
}

#[cfg(feature = "test-support")]
fn test_runtime_is_inline(command: &Commands) -> bool {
    let path = match command {
        Commands::Check(_) => return false,
        Commands::SetMirror(_) => None,
        Commands::Software(_) => None,
        Commands::Self_(_) => None,
        Commands::Daemon(_) => None,
        Commands::Install(args) => args.test_runtime.as_deref(),
        Commands::Migrate(args) => args.test_runtime.as_deref(),
        Commands::Switch(args) => args.test_runtime.as_deref(),
        Commands::Update(args) => args.test_runtime.as_deref(),
        Commands::Repair(args) => args.test_runtime.as_deref(),
        Commands::Restore(args) => args.test_runtime.as_deref(),
        Commands::Reinit(args) => args.test_runtime.as_deref(),
        Commands::Uninstall(args) => args.test_runtime.as_deref(),
        Commands::Backup(args) => match &args.action {
            crate::commands::backup::BackupAction::Create(args) => args.test_runtime.as_deref(),
            crate::commands::backup::BackupAction::List(args) => args.test_runtime.as_deref(),
            crate::commands::backup::BackupAction::Show(args) => args.test_runtime.as_deref(),
            crate::commands::backup::BackupAction::Verify(args) => args.test_runtime.as_deref(),
            crate::commands::backup::BackupAction::Delete(_) => None,
        },
        Commands::Reconcile(args) => args.test_runtime.as_deref(),
        Commands::Network(args) => args.test_runtime.as_deref(),
    };
    let Some(path) = path else {
        return false;
    };
    !InstallRuntime::test_uses_daemon(path).unwrap_or(false)
}

#[cfg(not(feature = "test-support"))]
fn test_runtime_is_inline(_command: &Commands) -> bool {
    false
}

/// 全局常驻 daemon 是否运行中(读 lkit 地盘 pidfile,进程存活即运行中)。
pub(crate) fn daemon_is_running() -> bool {
    let pidfile = crate::deployment::layout::territory_pidfile();
    let Ok(content) = std::fs::read_to_string(pidfile) else {
        return false;
    };
    let Ok(pid) = content.trim().parse::<u32>() else {
        return false;
    };
    crate::daemon::process_alive(pid)
}

/// 委托前置条件的阻断原因:None 表示可委托(非 root 内联、或 root 下 daemon
/// 运行且可 spawn worker)。控制台在进入与开始安装前用它在 TUI 内提前提示,
/// 避免用户填写完安装参数、退出控制台委托时才失败。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DelegationBlock {
    /// daemon 未运行(未部署或已退出)。
    DaemonNotRunning,
    /// daemon 在运行,但其可执行文件已被删除/替换,无法 spawn worker 子进程。
    /// 这种情况下委托请求会永远等不到 result.json(daemon 只把 spawn 失败
    /// 写进 journald),必须恢复文件并重启 daemon。
    WorkerSpawnUnavailable,
}

pub(crate) fn delegation_block() -> Option<DelegationBlock> {
    if unsafe { libc::geteuid() != 0 } {
        return None;
    }
    if !daemon_is_running() {
        return Some(DelegationBlock::DaemonNotRunning);
    }
    if !daemon_worker_spawnable() {
        return Some(DelegationBlock::WorkerSpawnUnavailable);
    }
    None
}

/// daemon 是否还能 spawn 自己的 worker 子进程:daemon 以 `current_exe()`
/// 的路径启动 worker(见 `executor::execute_request_inner`),若该可执行文件
/// 已被删除/替换(路径不可用),spawn 报 ENOENT,前端却毫不知情地永远等待
/// result.json。委托前检查此条件,阻止受影响的活动。
pub(crate) fn daemon_worker_spawnable() -> bool {
    let Some(pid) = daemon_pid() else {
        return false;
    };
    let Ok(exe) = std::fs::read_link(format!("/proc/{pid}/exe")) else {
        return false;
    };
    worker_executable_available(&exe)
}

/// 从地盘 pidfile 读取常驻 daemon 的 pid。
fn daemon_pid() -> Option<u32> {
    let content = std::fs::read_to_string(crate::deployment::layout::territory_pidfile()).ok()?;
    content.trim().parse::<u32>().ok()
}

/// daemon 的 `current_exe` 路径是否仍可用于 spawn:目标文件存在且可执行。
/// Linux 在文件被 unlink 后 `readlink /proc/<pid>/exe` 会追加 " (deleted)"
/// 后缀;若路径上已有替换完成的新文件(常见于 `install -m`/rename 覆盖),
/// 按新文件判断,否则视为不可用。与 euid 无关,便于单元测试。
fn worker_executable_available(exe: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let raw = exe.to_string_lossy();
    let target = raw.strip_suffix(" (deleted)").unwrap_or(&raw);
    let path = Path::new(target);
    std::fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn delegate(
    interrupt: &InterruptGuard,
    mut args: Vec<String>,
    interactive_password: Option<String>,
    network_plan: Option<NetworkPlan>,
    full_screen: bool,
) -> Result<ExitCode, DelegateError> {
    if !daemon_is_running() {
        return Err(DelegateError::usage(
            "the lkit daemon is not running; deploy it with `lkit self install`",
        ));
    }
    if !daemon_worker_spawnable() {
        return Err(DelegateError::usage(
            "the lkit daemon cannot spawn worker commands: its executable was deleted or replaced; restore the executable and restart the daemon",
        ));
    }
    let operation = operation_screen(&args);
    let operation_id = Uuid::now_v7().to_string();
    let directory = PathBuf::from(OPERATIONS_DIR);
    std::fs::create_dir_all(&directory).map_err(|error| {
        DelegateError::infrastructure(format!("create {}: {error}", directory.display()))
    })?;
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).map_err(
        |error| DelegateError::infrastructure(format!("secure {}: {error}", directory.display())),
    )?;

    let request_path = directory.join(format!("{operation_id}.request.json"));
    let result_path = directory.join(format!("{operation_id}.result.json"));
    let stdout_path = directory.join(format!("{operation_id}.stdout.log"));
    let stderr_path = directory.join(format!("{operation_id}.stderr.log"));
    let presentation_path = directory.join(format!("{operation_id}.presentation.jsonl"));
    let cancel_path = directory.join(format!("{operation_id}{CANCEL_FILE_SUFFIX}"));
    let credential_path = directory.join(format!("{operation_id}.credential"));
    let network_plan_path = directory.join(format!("{operation_id}.network.json"));
    let mut environment = string_environment().map_err(DelegateError::infrastructure)?;
    environment.retain(|(key, _)| key != crate::i18n::LANGUAGE_ENV);
    environment.push((
        crate::i18n::LANGUAGE_ENV.to_string(),
        crate::i18n::current().code().to_string(),
    ));
    let working_directory = std::env::current_dir()
        .map_err(|error| DelegateError::infrastructure(error.to_string()))?;
    let terminal = terminal_path();
    let has_credential = interactive_password.is_some();
    if let Some(password) = interactive_password {
        create_private_secret_file(&credential_path, password.as_bytes()).map_err(|error| {
            cleanup_files(&[&credential_path, &network_plan_path]);
            DelegateError::infrastructure(error)
        })?;
        args.extend([
            "--password-file".into(),
            credential_path.display().to_string(),
        ]);
    }
    let has_network_plan = network_plan.is_some();
    if let Some(network_plan) = network_plan {
        if let Err(error) = write_private_json(&network_plan_path, &network_plan) {
            cleanup_files(&[&credential_path, &network_plan_path]);
            return Err(DelegateError::infrastructure(error));
        }
        args.extend([
            "--network-plan-file".into(),
            network_plan_path.display().to_string(),
        ]);
    }
    let request = WorkerRequest {
        schema_version: 2,
        args,
        environment,
        working_directory,
        result_path: result_path.clone(),
        stdout_path: stdout_path.clone(),
        stderr_path: stderr_path.clone(),
        cancel_path: cancel_path.clone(),
        terminal,
        presentation_path: presentation_path.clone(),
        credential_path: has_credential.then(|| credential_path.clone()),
        network_plan_path: has_network_plan.then(|| network_plan_path.clone()),
    };
    if let Err(error) = write_private_json(&request_path, &request) {
        cleanup_files(&[&credential_path, &network_plan_path]);
        return Err(DelegateError::infrastructure(error));
    }
    if let Err(error) = create_private_file(&presentation_path) {
        cleanup_files(&[&request_path, &credential_path, &network_plan_path]);
        return Err(DelegateError::infrastructure(error));
    }

    if interrupt.requested() {
        let _ = std::fs::write(&cancel_path, b"");
        cleanup_files(&[
            &request_path,
            &result_path,
            &stdout_path,
            &stderr_path,
            &presentation_path,
            &cancel_path,
            &credential_path,
            &network_plan_path,
        ]);
        if full_screen {
            crate::interaction::presentation::show_cancelled_screen(interrupt)
                .map_err(DelegateError::infrastructure)?;
        }
        return Ok(ExitCode::from(130));
    }

    let result = wait_for_result(
        &result_path,
        &stdout_path,
        &stderr_path,
        &presentation_path,
        &cancel_path,
        interrupt,
        full_screen,
        operation,
    );
    if matches!(result, Ok(WaitOutcome::Interrupted)) {
        let _ = std::fs::write(&cancel_path, b"");
        cleanup_files(&[
            &request_path,
            &result_path,
            &stdout_path,
            &stderr_path,
            &presentation_path,
            &cancel_path,
            &credential_path,
            &network_plan_path,
        ]);
        if full_screen {
            crate::interaction::presentation::show_cancelled_screen(interrupt)
                .map_err(DelegateError::infrastructure)?;
        }
        return Ok(ExitCode::from(130));
    }
    cleanup_files(&[
        &request_path,
        &result_path,
        &stdout_path,
        &stderr_path,
        &presentation_path,
        &cancel_path,
        &credential_path,
        &network_plan_path,
    ]);
    let _ = RemoveFile::new(&cancel_path);
    match result {
        Ok(WaitOutcome::Completed(code)) => Ok(code),
        // 结果页确认了待确认的网络接管:全屏页已退出,把 `network confirm`
        // 委托给 daemon 执行(与手工 `lkit network confirm` 同一路径)。
        // 确认会切换 Landscape 托管网络,发起会话可能因此断开——委托后
        // 即使前端进程消失,daemon 也会独立完成提交,事务不会停在
        // 半提交状态;前端存活时照常经请求/结果文件回收输出与退出码。
        Ok(WaitOutcome::ConfirmTakeover) => {
            Box::pin(delegate(
                interrupt,
                vec!["network".into(), "confirm".into()],
                None,
                None,
                false,
            ))
            .await
        }
        Ok(WaitOutcome::Interrupted) => unreachable!("interrupted outcome handled above"),
        Err(error) => Err(DelegateError::infrastructure(format!(
            "the lkit daemon did not finish the operation: {error}"
        ))),
    }
}

fn cleanup_files(paths: &[&Path]) {
    for path in paths {
        let _ = std::fs::remove_file(path);
    }
}

pub(crate) use self::executor::execute_request;
pub(crate) use self::protocol::{read_network_plan, string_args};

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;
    use crate::cli::Cli;

    fn delegates_for(args: &[&str]) -> bool {
        let mut cli_args = vec!["lkit"];
        cli_args.extend_from_slice(args);
        let cli = Cli::try_parse_from(cli_args).unwrap();
        delegates(cli.command.as_ref().unwrap())
    }

    #[test]
    fn delegates_state_changing_commands() {
        for args in [
            &["install", "--version", "1.2.3"][..],
            &["migrate", "--from", "/etc/landscape"][..],
            &["switch", "--version", "2.0.0"][..],
            &["update"][..],
            &["repair", "binary"][..],
            &["restore", "--backup", "x"][..],
            &["reinit"][..],
            &["uninstall", "--yes"][..],
            &["network", "rollback"][..],
            &["network", "confirm"][..],
        ] {
            assert!(delegates_for(args), "expected delegation for {args:?}");
        }
    }

    #[test]
    fn does_not_delegate_read_only_commands() {
        for args in [
            &["check"][..],
            &["reconcile"][..],
            &["set-mirror", "tuna"][..],
            &["software", "list"][..],
            &["backup", "create"][..],
            &["self", "install"][..],
            &["daemon"][..],
            &["network", "status"][..],
        ] {
            assert!(!delegates_for(args), "unexpected delegation for {args:?}");
        }
    }

    #[test]
    fn daemon_is_running_follows_the_territory_pidfile() {
        let territory =
            std::env::temp_dir().join(format!("lkit-daemon-running-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&territory);
        std::fs::create_dir_all(territory.join("run")).unwrap();
        let _guard = crate::deployment::layout::test_territory(&territory);
        let pidfile = crate::deployment::layout::territory_pidfile();

        assert!(
            !daemon_is_running(),
            "missing pidfile must mean not running"
        );
        std::fs::write(&pidfile, "99999999\n").unwrap();
        assert!(!daemon_is_running(), "a dead pid must mean not running");
        std::fs::write(&pidfile, format!("{}\n", std::process::id())).unwrap();
        assert!(daemon_is_running(), "a live pid must mean running");
        std::fs::write(&pidfile, "not a pid").unwrap();
        assert!(
            !daemon_is_running(),
            "an unparsable pidfile must mean not running"
        );

        drop(_guard);
        let _ = std::fs::remove_dir_all(&territory);
    }

    #[test]
    fn daemon_worker_spawnable_follows_the_pidfile() {
        let territory =
            std::env::temp_dir().join(format!("lkit-daemon-spawn-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&territory);
        std::fs::create_dir_all(territory.join("run")).unwrap();
        let _guard = crate::deployment::layout::test_territory(&territory);
        let pidfile = crate::deployment::layout::territory_pidfile();

        std::fs::write(&pidfile, "99999999\n").unwrap();
        assert!(
            !daemon_worker_spawnable(),
            "a dead daemon pid must not be spawnable"
        );
        std::fs::write(&pidfile, format!("{}\n", std::process::id())).unwrap();
        assert!(
            daemon_worker_spawnable(),
            "the test process executable must be spawnable"
        );
        std::fs::write(&pidfile, "not a pid").unwrap();
        assert!(
            !daemon_worker_spawnable(),
            "an unparsable pidfile must not be spawnable"
        );

        drop(_guard);
        let _ = std::fs::remove_dir_all(&territory);
    }

    #[test]
    fn worker_executable_available_checks_the_deleted_suffix() {
        use std::os::unix::fs::PermissionsExt;
        let temp = std::env::temp_dir().join(format!("lkit-spawn-exe-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();
        let path = temp.join("lkit");
        std::fs::write(&path, b"#!/bin/sh\nexit 0").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert!(
            worker_executable_available(&path),
            "an existing executable must be spawnable"
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(
            !worker_executable_available(&path),
            "a non-executable file must not be spawnable"
        );

        // 文件被 unlink 后 readlink /proc/<pid>/exe 追加 " (deleted)" 后缀;
        // 路径上已有替换完成的新文件时按新文件判断,否则视为不可用。
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            worker_executable_available(&temp.join("lkit (deleted)")),
            "a replaced executable must be spawnable through the new file"
        );
        assert!(
            !worker_executable_available(&temp.join("gone (deleted)")),
            "a missing replaced file must not be spawnable"
        );
        assert!(
            !worker_executable_available(&temp.join("absent")),
            "a missing file must not be spawnable"
        );

        let _ = std::fs::remove_dir_all(&temp);
    }
}
