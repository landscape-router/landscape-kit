//! `lkit backup extract` — extract backup to a target directory.

use std::collections::HashMap;

use lkit_app::AppState;
use lkit_app::backup::BackupUseCase;

use crate::cli::BackupExtractArgs;
use crate::messages::CliMessages;

pub(crate) async fn run(args: BackupExtractArgs, state: &AppState) -> anyhow::Result<()> {
    let use_case = BackupUseCase::new(
        state.client.clone(),
        state.service_manager.clone(),
        state.landscape_paths.clone(),
        state.manager_paths.clone(),
    );

    let entry = use_case.resolve(&args.id_or_path)?;
    use_case.extract(&entry, &args.target, args.force)?;

    let mut params = HashMap::new();
    params.insert("path", args.target.to_str().unwrap_or("?"));
    eprintln!("{}", CliMessages::format("backup.extracted", &params));

    Ok(())
}
