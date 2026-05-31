//! `lkit backup` command dispatch — maps subcommands to handlers.

mod create;
mod delete;
mod list;
mod rebuild;
mod restore;

use lkit_app::AppState;

use crate::cli::BackupCommands;

/// Dispatch a parsed BackupCommands variant to the appropriate handler.
pub(crate) async fn dispatch(cmd: BackupCommands, state: &AppState) -> anyhow::Result<()> {
    match cmd {
        BackupCommands::Create { remark } => create::run(remark, state).await,
        BackupCommands::List { json } => list::run(json, state).await,
        BackupCommands::Restore { id_or_path } => restore::run(&id_or_path, state).await,
        BackupCommands::Rebuild { id, target } => rebuild::run(&id, target.as_path(), state).await,
        BackupCommands::Delete { id } => delete::run(&id, state).await,
        BackupCommands::DoRestore { id, recovery_dir } => {
            restore::run_do_restore(&id, &recovery_dir, state).await
        }
    }
}
