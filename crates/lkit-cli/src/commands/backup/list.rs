//! `lkit backup list` — list all available backups.

use comfy_table::{Cell, Row, Table};
use lkit_app::AppState;
use lkit_app::backup::BackupUseCase;

/// Run `lkit backup list`.
pub(crate) async fn run(json: bool, state: &AppState) -> anyhow::Result<()> {
    let usecase = BackupUseCase::new(
        state.client.clone(),
        state.service_manager.clone(),
        state.landscape_paths.clone(),
        state.manager_paths.clone(),
    );
    let entries = usecase.list().await?;

    if json {
        let output = serde_json::to_string_pretty(&entries)?;
        println!("{output}");
        return Ok(());
    }

    if entries.is_empty() {
        println!("(no backups)");
        return Ok(());
    }

    let mut table = Table::new();
    table.set_header(vec!["ID", "Created", "Version", "Type", "Remark"]);

    for e in &entries {
        let auto_label = if e.metadata.auto { "auto" } else { "manual" };
        table.add_row(Row::from(vec![
            Cell::new(&e.id),
            Cell::new(&e.metadata.created_at),
            Cell::new(&e.metadata.landscape_version),
            Cell::new(auto_label),
            Cell::new(e.metadata.remark.as_deref().unwrap_or("")),
        ]));
    }

    println!("{table}");
    Ok(())
}
