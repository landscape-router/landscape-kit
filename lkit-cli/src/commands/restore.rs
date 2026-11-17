use std::path::PathBuf;
use std::process::ExitCode;

use clap::Args;

use crate::deployment::runtime::InstallRuntime;
#[cfg(feature = "test-support")]
use crate::deployment::{lock, transaction};
use crate::deployment::{plan, state};
use crate::workflows::restore::RestoreArgs;
#[cfg(feature = "test-support")]
use crate::workflows::restore::{RestoreOptions, RestoreOutcome};

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
    #[arg(skip)]
    pub(crate) confirmed: Option<RestoreConfirmation>,
    #[cfg(feature = "test-support")]
    #[arg(long, value_name = "PATH", hide = true)]
    pub test_runtime: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub(crate) struct RestoreConfirmation {
    pub root: String,
    pub request: crate::daemon_operations::RestoreRequest,
}

pub(crate) fn confirmed_backup(
    root: &crate::deployment::root::InstallRoot,
    installed: &state::InstallState,
    source: PathBuf,
    prepared: &crate::workflows::restore::PreparedRestore,
) -> Result<RestoreConfirmation, plan::InstallError> {
    Ok(RestoreConfirmation {
        root: root.canonical.to_string_lossy().into_owned(),
        request: crate::daemon_operations::RestoreRequest {
            from_version: installed.active_version.clone(),
            state_identity: crate::daemon_operations::installation_identity(installed)?,
            source: std::path::absolute(source).map_err(plan::InstallError::Io)?,
            backup_id: prepared.metadata.backup_id.clone(),
            backup_sha256: prepared.sha256.clone(),
            target_version: prepared.metadata.landscape_version.clone(),
            allow_no_backup: false,
        },
    })
}

pub async fn run(args: &Restore) -> ExitCode {
    if args.console_confirmed && args.confirmed.is_none() {
        eprintln!("restore: missing restore confirmation context");
        return ExitCode::from(2);
    }
    #[cfg(feature = "test-support")]
    if let Some(path) = args.test_runtime.as_ref() {
        match InstallRuntime::test_uses_daemon(path) {
            Ok(false) => return run_inline(args).await,
            Ok(true) => {}
            Err(error) => {
                eprintln!("restore: {error}");
                return exit_code(&error);
            }
        }
    }
    match submit(args).await {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("restore: {error}");
            exit_code(&error)
        }
    }
}

async fn submit(args: &Restore) -> Result<u8, plan::InstallError> {
    let runtime = resolve_runtime(args)?;
    if !runtime.allow_non_root && unsafe { libc::geteuid() } != 0 {
        return Err(plan::InstallError::ParameterUsage(
            "restore requires root".into(),
        ));
    }
    if args.console_confirmed {
        let confirmation = args.confirmed.as_ref().ok_or_else(|| {
            plan::InstallError::ParameterUsage("missing restore confirmation context".into())
        })?;
        return crate::daemon_operations::submit_restore_and_wait(
            confirmation.root.clone(),
            confirmation.request.clone(),
            true,
        )
        .await;
    }
    let root = state::discover_landscape_root()?.ok_or_else(|| {
        plan::InstallError::ParameterUsage(crate::tr!(
            crate::keys::RESTORE_REQUIRES_EXISTING_INSTALLATION
        ))
    })?;
    let installed = state::load_state(&root)?.ok_or_else(|| {
        plan::InstallError::ParameterUsage(crate::tr!(
            crate::keys::RESTORE_REQUIRES_EXISTING_INSTALLATION
        ))
    })?;
    let workflow = RestoreArgs {
        backup_id: args.backup.clone(),
        file_path: args.file.clone(),
        allow_no_backup: args.allow_no_backup,
        yes: args.yes,
        console_confirmed: args.console_confirmed,
    };
    let prepared = crate::workflows::restore::prepare_restore(&root, &installed, &workflow)?;
    crate::workflows::restore::confirm_plan(
        &workflow,
        &installed,
        &prepared.metadata,
        &crate::interaction::interactive::confirm,
    )?;
    let source = match (&args.backup, &args.file) {
        (Some(id), None) => {
            crate::deployment::layout::territory_backups_dir().join(format!("{id}.lkb"))
        }
        (None, Some(path)) => std::path::absolute(path).map_err(plan::InstallError::Io)?,
        _ => {
            return Err(plan::InstallError::ParameterUsage(
                "choose one restore backup".into(),
            ));
        }
    };
    crate::daemon_operations::submit_restore_and_wait(
        root.canonical.to_string_lossy().into_owned(),
        crate::daemon_operations::RestoreRequest {
            from_version: installed.active_version.clone(),
            state_identity: crate::daemon_operations::installation_identity(&installed)?,
            source,
            backup_id: prepared.metadata.backup_id,
            backup_sha256: prepared.sha256,
            target_version: prepared.metadata.landscape_version,
            allow_no_backup: args.allow_no_backup,
        },
        args.console_confirmed,
    )
    .await
}

#[cfg(feature = "test-support")]
async fn run_inline(args: &Restore) -> ExitCode {
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
    if let Some(transaction) = unfinished
        && let Err(error) = transaction::recover_interrupted(
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
        Ok(RestoreOutcome::AwaitingConfirmation) => {
            println!(
                "restore: {}",
                crate::tr!(crate::keys::RESTORE_AWAITING_CONFIRMATION)
            );
            ExitCode::SUCCESS
        }
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
