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
            matches!(args.action, NetworkAction::Rollback { automatic: false })
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

/// 委托前置条件是否未满足:root 下全局常驻 daemon 未运行。控制台在进入与
/// 开始安装前用它在 TUI 内提前提示/阻断,避免用户填写完安装参数、退出
/// 控制台委托时才失败;非 root 内联执行,不要求 daemon,恒为 false。
pub(crate) fn delegation_blocked() -> bool {
    (unsafe { libc::geteuid() == 0 }) && !daemon_is_running()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn delegate(
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
    result
        .map(|outcome| match outcome {
            WaitOutcome::Completed(code) => code,
            WaitOutcome::Interrupted => unreachable!("interrupted outcome handled above"),
        })
        .map_err(|error| {
            DelegateError::infrastructure(format!(
                "the lkit daemon did not finish the operation: {error}"
            ))
        })
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
            &["network", "confirm"][..],
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
}
