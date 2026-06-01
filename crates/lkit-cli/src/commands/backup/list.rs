//! `lkit backup list` — list all backups.

use lkit_app::AppState;

use super::format_size;

pub(crate) async fn run(json: bool, state: &AppState) -> anyhow::Result<()> {
    use lkit_app::backup::BackupUseCase;

    let use_case = BackupUseCase::new(
        state.client.clone(),
        state.service_manager.clone(),
        state.landscape_paths.clone(),
        state.manager_paths.clone(),
    );

    let entries = use_case.list()?;

    if json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }

    if entries.is_empty() {
        println!("(no backups found)");
        return Ok(());
    }

    let mut table = comfy_table::Table::new();
    table.set_header(["Filename", "Size", "Version", "Time", "Remark", "Scope", "Status"]);

    for e in &entries {
        let size = format_size(e.file_size);
        let status = if e.backup_id == "corrupted" { "corrupted" } else { "ok" };
        table.add_row([
            &e.filename,
            &size,
            &e.landscape_version,
            &e.created_at,
            e.remark.as_deref().unwrap_or("-"),
            &e.scope.to_string(),
            status,
        ]);
    }

    println!("{table}");
    Ok(())
}
