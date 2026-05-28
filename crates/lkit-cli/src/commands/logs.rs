//! `lkit logs` command handler.

use lkit_app::AppState;
use lkit_app::logs::LogsUseCase;

use crate::cli::LogsArgs;

/// Run the logs command: read and display recent Landscape log lines.
pub(crate) async fn run(args: LogsArgs, state: &AppState) -> anyhow::Result<()> {
    let uc = LogsUseCase::new(state.log_reader.clone());
    let lines = uc.recent(args.lines).await?;

    for line in &lines {
        eprintln!("{}", line);
    }

    Ok(())
}
