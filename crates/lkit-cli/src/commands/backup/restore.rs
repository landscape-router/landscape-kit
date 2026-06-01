//! `lkit backup restore` — restore a backup with foreground + detached phases.

use std::io::IsTerminal;
#[cfg(unix)]
use std::os::unix::process::CommandExt;

use lkit_app::AppState;
use lkit_app::backup::BackupUseCase;

use crate::cli::BackupRestoreArgs;
use crate::messages::CliMessages;

use super::format_size;

pub(crate) async fn run(args: BackupRestoreArgs, state: &AppState) -> anyhow::Result<()> {
    let use_case = BackupUseCase::new(
        state.client.clone(),
        state.service_manager.clone(),
        state.landscape_paths.clone(),
        state.manager_paths.clone(),
    );

    let entry = use_case.resolve(&args.id_or_path)?;

    // Print summary table
    let mut table = comfy_table::Table::new();
    table.add_row(["Backup ID", &entry.backup_id]);
    table.add_row(["Version", &entry.landscape_version]);
    table.add_row(["Created", &entry.created_at]);
    table.add_row(["Host", &entry.hostname]);
    table.add_row(["Scope", &entry.scope.to_string()]);
    table.add_row(["Size", &format_size(entry.file_size)]);
    println!("{table}");

    // Interactive confirmation if TTY
    if std::io::stdin().is_terminal() {
        use dialoguer::Confirm;
        if !Confirm::new().with_prompt("确认恢复？").default(false).interact()? {
            return Ok(());
        }
    }

    // Foreground phase
    let status_file = use_case.restore_foreground(&entry).await?;

    let mut params = std::collections::HashMap::new();
    params.insert("status_file", status_file.to_str().unwrap_or("?"));
    eprintln!("{}", CliMessages::format("backup.restore_ready", &params));

    // Detach using process_group(0) — child calls setsid() at startup
    use std::process::{Command, Stdio};
    let exe = std::env::current_exe()?;
    let child = Command::new(exe)
        .arg("do-restore")
        .arg(&entry.backup_id)
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    drop(child);

    // Exit immediately — the detached child handles the rest.
    // Spec S3.3 foreground step 10: exit 0
    std::process::exit(0);
}
