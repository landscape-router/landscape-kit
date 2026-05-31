//! `lkit backup create` — create a backup of the running Landscape installation.

use std::collections::HashMap;

use lkit_app::AppState;
use lkit_app::backup::BackupUseCase;

use crate::messages::CliMessages;

/// Run `lkit backup create`.
pub(crate) async fn run(remark: Option<String>, state: &AppState) -> anyhow::Result<()> {
    let usecase = BackupUseCase::new(
        state.client.clone(),
        state.service_manager.clone(),
        state.landscape_paths.clone(),
        state.manager_paths.clone(),
    );
    let entry = usecase.create(remark).await?;
    let mut params = HashMap::new();
    params.insert("id", entry.id.as_str());
    println!("{}", CliMessages::format("backup.created", &params));
    Ok(())
}
