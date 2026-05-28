//! `lkit diagnose` command handler.

use lkit_app::diagnose::DiagnoseUseCase;
use lkit_app::AppState;
use lkit_core::DiagnosticResult;

use crate::cli::DiagnoseArgs;
use crate::messages::msg;

pub async fn run(args: DiagnoseArgs, state: &AppState) -> anyhow::Result<()> {
    let uc = DiagnoseUseCase::new(
        state.client.clone(),
        state.service_manager.clone(),
        state.landscape_paths.clone(),
    );
    let result = uc.execute().await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        for check in &result.checks {
            let status = if check.passed {
                msg("diagnose.pass")
            } else {
                msg("diagnose.fail")
            };
            eprintln!("{} {} — {}", status, check.name, check.message);
        }
    }

    if let Some(code) = diagnose_exit_code(&result) {
        std::process::exit(code);
    }

    Ok(())
}

/// Determine exit code from diagnostic results per spec §6.4:
/// - Only API failed → 4
/// - Other failures → 1
/// - All passed → 0 (returns None)
fn diagnose_exit_code(result: &DiagnosticResult) -> Option<i32> {
    if result.all_passed() {
        return None;
    }
    let api_failed = result
        .checks
        .iter()
        .any(|c| c.name == "api" && !c.passed);
    let other_failed = result
        .checks
        .iter()
        .any(|c| c.name != "api" && !c.passed);
    if api_failed && !other_failed {
        Some(4)
    } else {
        Some(1)
    }
}
