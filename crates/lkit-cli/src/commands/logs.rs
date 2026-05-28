//! `lkit logs` command handler.

use lkit_app::AppState;
use lkit_app::logs::LogsUseCase;

use crate::cli::LogsArgs;

pub async fn run(args: LogsArgs, state: &AppState) -> anyhow::Result<()> {
    let uc = LogsUseCase::new(state.log_reader.clone());
    let lines = uc.recent(args.lines).await?;

    for line in &lines {
        eprintln!("{}", line);
    }

    Ok(())
}
