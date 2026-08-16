#[cfg(feature = "test-support")]
#[cfg(feature = "test-support")]
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Args;

use crate::deployment::runtime::InstallRuntime;
use crate::deployment::{lock, plan, state, transaction};
use crate::workflows::uninstall::{UninstallArgs, UninstallOptions, UninstallOutcome};

#[derive(Debug, Args)]
pub struct Uninstall {
    /// 非交互模式确认卸载(卸载计划、数据损失与 none 模式外部实例已停)
    #[arg(long)]
    pub yes: bool,
    /// 保护备份无法创建时继续,不产生可移植的当前配置快照
    #[arg(long)]
    pub allow_no_backup: bool,
    /// 只卸载服务与程序,保留 landscape 根的 data/
    #[arg(long)]
    pub keep_data: bool,
    /// 控制台已确认卸载计划(内部参数,交互模式也跳过 tty 确认)
    #[arg(long, hide = true)]
    pub console_confirmed: bool,
    #[cfg(feature = "test-support")]
    #[arg(long, value_name = "PATH", hide = true)]
    pub test_runtime: Option<PathBuf>,
}

pub async fn run(args: &Uninstall) -> ExitCode {
    let runtime = match resolve_runtime(args) {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("uninstall: {error}");
            return exit_code(&error);
        }
    };
    if !runtime.allow_non_root && unsafe { libc::geteuid() } != 0 {
        eprintln!(
            "uninstall: {}",
            crate::tr!(crate::keys::MANAGE_MUST_RUN_AS_ROOT)
        );
        return ExitCode::FAILURE;
    }
    let normalized = match state::discover_landscape_root() {
        Ok(Some(root)) => root,
        Ok(None) => {
            eprintln!(
                "uninstall: {}",
                crate::tr!(crate::keys::UNINSTALL_REQUIRES_EXISTING_INSTALLATION)
            );
            return ExitCode::from(2);
        }
        Err(error) => {
            eprintln!("uninstall: {error}");
            return exit_code(&error);
        }
    };
    let _lock = match lock::acquire_install_lock() {
        Ok(lock) => lock,
        Err(error) => {
            eprintln!("uninstall: {error}");
            return exit_code(&error);
        }
    };
    let health = match runtime.health_options() {
        Ok(health) => health,
        Err(error) => {
            eprintln!("uninstall: {error}");
            return exit_code(&error);
        }
    };
    let unfinished = match transaction::find_unfinished(&normalized) {
        Ok(transaction) => transaction,
        Err(error) => {
            eprintln!("uninstall: {error}");
            return exit_code(&error);
        }
    };
    if let Some(transaction) = unfinished {
        if let Err(error) = transaction::recover_interrupted(
            &normalized,
            &transaction,
            runtime.service_manager.as_ref(),
            &health,
        )
        .await
        {
            eprintln!("uninstall: {error}");
            return exit_code(&error);
        }
    }
    let Some(state) = (match state::load_state(&normalized) {
        Ok(state) => state,
        Err(error) => {
            eprintln!("uninstall: {error}");
            return exit_code(&error);
        }
    }) else {
        // 中断的卸载事务已被前向完成:卸载目标已经达到,视为成功。
        if let Ok(Some(_)) =
            transaction::find_committed_operation(&normalized, transaction::Operation::Uninstall)
        {
            println!(
                "uninstall: {}",
                crate::tr!(crate::keys::UNINSTALL_ALREADY_COMPLETED)
            );
            return ExitCode::SUCCESS;
        }
        eprintln!(
            "uninstall: {}",
            crate::tr!(crate::keys::UNINSTALL_REQUIRES_EXISTING_INSTALLATION)
        );
        return ExitCode::from(2);
    };
    let data_dir = normalized.canonical.join("data");
    let options = UninstallOptions {
        export_base_url: runtime.export_base_url.clone(),
        token: &(|| {
            crate::backup::export::read_api_token(
                &data_dir.join("landscape_api_token"),
                runtime.managed_uid,
            )
        }),
        confirm: &|prompt| crate::interaction::interactive::confirm(prompt),
        health: &health,
    };
    let args = UninstallArgs {
        yes: args.yes,
        allow_no_backup: args.allow_no_backup,
        keep_data: args.keep_data,
        console_confirmed: args.console_confirmed,
    };
    match crate::workflows::uninstall::uninstall_installation(
        &normalized,
        &state,
        runtime.service_manager.as_ref(),
        &args,
        &options,
    )
    .await
    {
        Ok(UninstallOutcome::Committed { version, backup_id }) => {
            println!(
                "uninstall: {}",
                crate::tr!(
                    crate::keys::UNINSTALL_COMMITTED,
                    version = version,
                    backup_id = backup_id.as_deref().unwrap_or("-")
                )
            );
            println!(
                "uninstall: {}",
                crate::tr!(crate::keys::UNINSTALL_RETAINED_NOTE)
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("uninstall: {error}");
            exit_code(&error)
        }
    }
}

fn exit_code(error: &plan::InstallError) -> ExitCode {
    match error {
        plan::InstallError::ParameterUsage(_) | plan::InstallError::UnsupportedPlatform(_) => {
            ExitCode::from(2)
        }
        _ => ExitCode::FAILURE,
    }
}

#[cfg(feature = "test-support")]
fn resolve_runtime(args: &Uninstall) -> Result<InstallRuntime, plan::InstallError> {
    if let Some(path) = args.test_runtime.as_deref() {
        return InstallRuntime::from_test_file(path);
    }
    Ok(InstallRuntime::production())
}

#[cfg(not(feature = "test-support"))]
fn resolve_runtime(_args: &Uninstall) -> Result<InstallRuntime, plan::InstallError> {
    Ok(InstallRuntime::production())
}

#[cfg(test)]
mod tests {
    use clap::{Command, FromArgMatches};

    use super::*;

    fn parse(args: &[&str]) -> Result<Uninstall, clap::Error> {
        let command = <Uninstall as Args>::augment_args(Command::new("uninstall"));
        let matches = command.try_get_matches_from(args)?;
        Uninstall::from_arg_matches(&matches)
    }

    #[test]
    fn parses_uninstall_options() {
        let uninstall = parse(&["uninstall", "--yes", "--allow-no-backup", "--keep-data"]).unwrap();
        assert!(uninstall.yes);
        assert!(uninstall.allow_no_backup);
        assert!(uninstall.keep_data);
    }

    #[test]
    fn rejects_removed_flags() {
        assert!(parse(&["uninstall", "--purge-root"]).is_err());
        assert!(parse(&["uninstall", "--install-dir", "/opt/landscape"]).is_err());
    }
}
