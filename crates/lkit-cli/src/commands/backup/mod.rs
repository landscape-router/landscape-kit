//! `lkit backup` — backup management commands.

pub(crate) mod create;
pub(crate) mod delete;
pub(crate) mod extract;
pub(crate) mod list;
pub(crate) mod restore;

use std::io::IsTerminal;

use clap::Parser;
use lkit_app::AppState;

use crate::cli::{BackupAction, BackupCmd};
use crate::messages::msg;

/// Format bytes as human-readable size string.
pub(crate) fn format_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.1} GB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    } else if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

/// Dispatch backup subcommands.
pub(crate) async fn dispatch(cmd: BackupCmd, state: &AppState) -> anyhow::Result<()> {
    let Some(action) = cmd.action else {
        if std::io::stdin().is_terminal() {
            return interactive_menu(state).await;
        }
        crate::cli::Cli::try_parse_from(["lkit", "backup", "--help"])?;
        return Ok(());
    };

    match action {
        BackupAction::Create(args) => create::run(args, state).await,
        BackupAction::List(args) => list::run(args.json, state).await,
        BackupAction::Restore(args) => restore::run(args, state).await,
        BackupAction::Extract(args) => extract::run(args, state).await,
        BackupAction::Delete(args) => delete::run(args, state).await,
    }
}

/// Handle the hidden _do_restore command.
pub(crate) async fn run_do_restore(backup_id: &str, state: &AppState) -> anyhow::Result<()> {
    let _ = nix::unistd::setsid();

    let use_case = lkit_app::backup::BackupUseCase::from_state(state);

    use_case.restore_detached(backup_id).await?;
    Ok(())
}

/// Interactive backup sub-menu for TTY launcher.
async fn interactive_menu(state: &AppState) -> anyhow::Result<()> {
    use dialoguer::Select;

    let items = vec![
        msg("backup.menu.list"),
        msg("backup.menu.create"),
        msg("backup.menu.restore"),
        msg("backup.menu.extract"),
        msg("backup.menu.delete"),
        msg("menu.exit"),
    ];

    loop {
        let selection = Select::new()
            .with_prompt(msg("backup.menu.title"))
            .items(&items)
            .default(0)
            .interact()?;

        match selection {
            0 => list::run(false, state).await?,
            1 => {
                // TODO: interactive remark + full backup prompt
                create::run(crate::cli::BackupCreateArgs { remark: None, all: false }, state)
                    .await?;
            }
            2 => {
                eprintln!("TODO: interactive restore selection");
            }
            3 => {
                eprintln!("TODO: interactive extract selection");
            }
            4 => {
                eprintln!("TODO: interactive delete selection");
            }
            _ => break,
        }
    }
    Ok(())
}
