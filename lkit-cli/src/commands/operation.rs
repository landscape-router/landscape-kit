use clap::{Args, Subcommand};
use std::process::ExitCode;

#[derive(Debug, Args)]
pub struct OperationCommand {
    #[command(subcommand)]
    action: Action,
}
#[derive(Debug, Subcommand)]
enum Action {
    Status { id: String },
    List,
    Cancel { id: String },
}

pub fn run(args: &OperationCommand) -> ExitCode {
    use crate::daemon_operations as operations;
    if crate::cli::debug_enabled()
        && let Action::Status { id } = &args.action
        && let Ok(record) = operations::read(id)
    {
        eprintln!("{}", operations::timeline::debug_snapshot(&record));
        match operations::timeline::read_events(id) {
            Ok(entries) => {
                for entry in entries {
                    eprintln!("[debug] {}", entry.display());
                }
            }
            Err(_) => eprintln!("[debug] operation timeline unavailable"),
        }
    }
    let result = match &args.action {
        Action::Status { id } => operations::read(id).and_then(|r| {
            serde_json::to_string_pretty(&r)
                .map_err(crate::deployment::plan::InstallError::StateWrite)
        }),
        Action::List => operations::list().and_then(|r| {
            serde_json::to_string_pretty(&r)
                .map_err(crate::deployment::plan::InstallError::StateWrite)
        }),
        Action::Cancel { id } => operations::cancel(id).and_then(|r| {
            serde_json::to_string_pretty(&r)
                .map_err(crate::deployment::plan::InstallError::StateWrite)
        }),
    };
    match result {
        Ok(json) => {
            println!("{json}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("lkit operation: {e}");
            ExitCode::FAILURE
        }
    }
}
