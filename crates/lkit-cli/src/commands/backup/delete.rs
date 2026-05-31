//! `lkit backup delete` — delete a backup archive.

use std::collections::HashMap;

use lkit_app::AppState;
use lkit_app::backup::BackupUseCase;

use crate::messages::CliMessages;

/// Run `lkit backup delete <id>`.
///
/// Manual backups require user confirmation; automatic backups are deleted
/// without confirmation (the upgrade flow handles its own prompts).
pub(crate) async fn run(id: &str, state: &AppState) -> anyhow::Result<()> {
    let usecase = BackupUseCase::new(
        state.client.clone(),
        state.service_manager.clone(),
        state.landscape_paths.clone(),
        state.manager_paths.clone(),
    );
    let entry = usecase.resolve(id).await?;

    if !entry.metadata.auto {
        let mut params = HashMap::new();
        params.insert("id", entry.id.as_str());
        let prompt = CliMessages::format("backup.confirm_delete", &params);
        eprint!("{} [y/N] ", prompt);
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if input.trim().to_lowercase() != "y" {
            eprintln!("已取消");
            return Ok(());
        }
    }

    usecase.delete(&entry).await?;

    let mut params = HashMap::new();
    params.insert("id", entry.id.as_str());
    println!("{}", CliMessages::format("backup.deleted", &params));
    Ok(())
}
