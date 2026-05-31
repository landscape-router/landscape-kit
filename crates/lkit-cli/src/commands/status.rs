//! `lkit status` command handler.

use lkit_app::AppState;
use lkit_app::status::StatusUseCase;

use crate::cli::StatusArgs;
use crate::messages::msg;

/// Run the status command: query systemd and Landscape API, print report.
pub(crate) async fn run(args: StatusArgs, state: &AppState) -> anyhow::Result<()> {
    let uc = StatusUseCase::new(state.client.clone(), state.service_manager.clone());
    let report = uc.execute().await?;

    if args.json {
        let output = serde_json::json!({
            "service": report.service,
            "landscape_version": report.landscape_version,
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
        if report.landscape_version.is_none() {
            std::process::exit(4);
        }
        return Ok(());
    }

    let systemd_msg = if report.service.active {
        msg("status.systemd.active")
    } else {
        msg("status.systemd.inactive")
    };
    eprintln!("{}", systemd_msg);

    if let Some(ref version) = report.landscape_version {
        eprintln!("{}: {}", msg("status.api.ok"), version);
    } else {
        eprintln!("{}", msg("status.api.unreachable"));
        std::process::exit(4);
    }

    Ok(())
}
