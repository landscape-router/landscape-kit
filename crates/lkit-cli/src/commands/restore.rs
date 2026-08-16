use std::path::PathBuf;
use std::process::ExitCode;

use clap::Args;

use crate::deployment::runtime::InstallRuntime;
use crate::deployment::{lock, plan, state, transaction};
use crate::workflows::restore::{RestoreArgs, RestoreOptions, RestoreOutcome};

#[derive(Debug, Args)]
pub struct Restore {
    /// lkit 地盘 `backups/` 下的备份 ID
    #[arg(long, value_name = "ID", conflicts_with = "file")]
    pub backup: Option<String>,
    /// 外部复制的 `.lkb` 文件路径
    #[arg(long, value_name = "PATH", conflicts_with = "backup")]
    pub file: Option<PathBuf>,
    /// 保护备份无法创建时继续,不产生可移植的当前配置快照
    #[arg(long)]
    pub allow_no_backup: bool,
    /// 非交互模式确认恢复
    #[arg(long)]
    pub yes: bool,
    /// 控制台已确认恢复计划（内部参数，交互模式也跳过 tty 确认）
    #[arg(long, hide = true)]
    pub console_confirmed: bool,
    #[cfg(feature = "test-support")]
    #[arg(long, value_name = "PATH", hide = true)]
    pub test_runtime: Option<PathBuf>,
}

pub async fn run(args: &Restore) -> ExitCode {
    let runtime = match resolve_runtime(args) {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("restore: {error}");
            return exit_code(&error);
        }
    };
    if !runtime.allow_non_root && unsafe { libc::geteuid() } != 0 {
        eprintln!(
            "restore: {}",
            crate::tr!(crate::keys::MANAGE_MUST_RUN_AS_ROOT)
        );
        return ExitCode::FAILURE;
    }
    let normalized = match state::discover_landscape_root() {
        Ok(Some(root)) => root,
        Ok(None) => {
            eprintln!(
                "restore: {}",
                crate::tr!(crate::keys::RESTORE_REQUIRES_EXISTING_INSTALLATION)
            );
            return ExitCode::from(2);
        }
        Err(error) => {
            eprintln!("restore: {error}");
            return exit_code(&error);
        }
    };
    let _lock = match lock::acquire_install_lock() {
        Ok(lock) => lock,
        Err(error) => {
            eprintln!("restore: {error}");
            return exit_code(&error);
        }
    };
    let health = match runtime.health_options() {
        Ok(health) => health,
        Err(error) => {
            eprintln!("restore: {error}");
            return exit_code(&error);
        }
    };
    let unfinished = match transaction::find_unfinished(&normalized) {
        Ok(transaction) => transaction,
        Err(error) => {
            eprintln!("restore: {error}");
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
            eprintln!("restore: {error}");
            return exit_code(&error);
        }
    }
    let Some(state) = (match state::load_state(&normalized) {
        Ok(state) => state,
        Err(error) => {
            eprintln!("restore: {error}");
            return exit_code(&error);
        }
    }) else {
        // 状态在锁后消失(如已完成的卸载事务):按未安装处理。
        eprintln!(
            "restore: {}",
            crate::tr!(crate::keys::RESTORE_REQUIRES_EXISTING_INSTALLATION)
        );
        return ExitCode::from(2);
    };
    let data_dir = normalized.canonical.join("data");
    let options = RestoreOptions {
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
    let args = RestoreArgs {
        backup_id: args.backup.clone(),
        file_path: args.file.clone(),
        allow_no_backup: args.allow_no_backup,
        yes: args.yes,
        console_confirmed: args.console_confirmed,
    };
    match crate::workflows::restore::restore_version(
        &normalized,
        &state,
        runtime.service_manager.as_ref(),
        &args,
        &options,
    )
    .await
    {
        Ok(RestoreOutcome::Committed { version, backup_id }) => {
            println!(
                "restore: {}",
                crate::tr!(
                    crate::keys::RESTORE_COMMITTED,
                    version = version,
                    backup_id = backup_id
                )
            );
            ExitCode::SUCCESS
        }
        Ok(RestoreOutcome::RolledBack { version }) => {
            eprintln!(
                "restore: {}",
                crate::tr!(crate::keys::RESTORE_FAILED_ROLLED_BACK, version = version)
            );
            ExitCode::from(5)
        }
        Ok(RestoreOutcome::RollbackFailed { version, reason }) => {
            eprintln!(
                "restore: {}",
                crate::tr!(
                    crate::keys::RESTORE_FAILED_ROLLBACK_FAILED,
                    version = version,
                    reason = reason
                )
            );
            ExitCode::from(6)
        }
        Err(error) => {
            eprintln!("restore: {error}");
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
fn resolve_runtime(args: &Restore) -> Result<InstallRuntime, plan::InstallError> {
    if let Some(path) = args.test_runtime.as_deref() {
        return InstallRuntime::from_test_file(path);
    }
    Ok(InstallRuntime::production())
}

#[cfg(not(feature = "test-support"))]
fn resolve_runtime(_args: &Restore) -> Result<InstallRuntime, plan::InstallError> {
    Ok(InstallRuntime::production())
}
