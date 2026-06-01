//! `lkit backup create` — create a new backup.

use std::collections::HashMap;

use lkit_app::AppState;

use crate::cli::BackupCreateArgs;
use crate::messages::CliMessages;

pub(crate) async fn run(args: BackupCreateArgs, state: &AppState) -> anyhow::Result<()> {
    use lkit_app::backup::BackupUseCase;

    let use_case = BackupUseCase::from_state(state);

    let entry = use_case.create(args.remark, false, args.all).await?;

    let mut params = HashMap::new();
    params.insert("id", entry.backup_id.as_str());
    eprintln!("{}", CliMessages::format("backup.created", &params));

    Ok(())
}
