//! `lkit backup delete` — delete a backup file.

use std::collections::HashMap;

use lkit_app::AppState;
use lkit_app::backup::BackupUseCase;

use crate::cli::BackupDeleteArgs;
use crate::messages::CliMessages;

pub(crate) async fn run(args: BackupDeleteArgs, state: &AppState) -> anyhow::Result<()> {
    let use_case = BackupUseCase::new(
        state.client.clone(),
        state.service_manager.clone(),
        state.landscape_paths.clone(),
        state.manager_paths.clone(),
    );

    let entry = use_case.resolve(&args.id_or_path)?;
    use_case.delete(&entry)?;

    let mut params = HashMap::new();
    params.insert("id", entry.backup_id.as_str());
    eprintln!("{}", CliMessages::format("backup.deleted", &params));
    Ok(())
}
