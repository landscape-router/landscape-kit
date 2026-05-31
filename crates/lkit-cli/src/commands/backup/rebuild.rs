//! `lkit backup rebuild` — extract a backup to a target directory without service interaction.

use std::collections::HashMap;
use std::path::Path;

use lkit_app::AppState;
use lkit_app::backup::BackupUseCase;

use crate::messages::CliMessages;

/// Run `lkit backup rebuild <id> --target <path>`.
pub(crate) async fn run(id: &str, target: &Path, state: &AppState) -> anyhow::Result<()> {
    let usecase = BackupUseCase::new(
        state.client.clone(),
        state.service_manager.clone(),
        state.landscape_paths.clone(),
        state.manager_paths.clone(),
    );
    let entry = usecase.resolve(id).await?;
    usecase.rebuild(&entry, target).await?;

    let path_str = target.to_string_lossy();
    let mut params = HashMap::new();
    params.insert("path", path_str.as_ref());
    println!("{}", CliMessages::format("backup.rebuilt", &params));
    Ok(())
}
