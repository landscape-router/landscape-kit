//! Command handlers — each subcommand delegates to its app-layer use case.

mod backup;
mod config;
mod diagnose;
pub(crate) mod install;
mod logs;
mod mirror;
mod rollback;
pub(crate) mod self_cmd;
mod service;
mod status;
mod upgrade;

use lkit_app::AppState;

use crate::cli::Commands;
use crate::messages::msg;

/// Dispatch a parsed CLI command to the appropriate handler.
///
/// Maps AppError to exit codes per spec §5.7.
pub async fn dispatch(cmd: Commands, state: &AppState) -> anyhow::Result<()> {
    let result = match cmd {
        Commands::Status(args) => status::run(args, state).await,
        Commands::Service(args) => service::run(args, state).await,
        Commands::Logs(args) => logs::run(args, state).await,
        Commands::Diagnose(args) => diagnose::run(args, state).await,
        Commands::Install(args) => {
            anyhow::bail!(
                "Install({init_file:?}) reached dispatch — should be handled in main",
                init_file = args.init_file
            )
        }
        Commands::Backup(_) => backup::run().await,
        Commands::Upgrade(_) => upgrade::run().await,
        Commands::Rollback(_) => rollback::run().await,
        Commands::Config(_) => config::run().await,
        Commands::SelfCmd(args) => self_cmd::run(args).await,
        Commands::Mirror(args) => mirror::run(args).await,
    };

    if let Err(ref e) = result
        && let Some(app_err) = e.downcast_ref::<lkit_app::AppError>()
    {
        let (exit_code, suggestion) = match app_err {
            lkit_app::AppError::PermissionDenied(_) => (2, msg("error.suggestion.permission")),
            lkit_app::AppError::NotFound(_) => (3, msg("error.suggestion.not_installed")),
            _ => (1, msg("error.suggestion.generic")),
        };
        eprintln!("Error: {:#}", e);
        eprintln!("Suggestion: {}", suggestion);
        std::process::exit(exit_code);
    }

    result
}
