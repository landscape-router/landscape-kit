//! `lkit service` command handler.

use lkit_app::AppState;
use lkit_app::service::ServiceUseCase;

use crate::cli::{ServiceAction, ServiceArgs};
use crate::messages::msg;

pub async fn run(args: ServiceArgs, state: &AppState) -> anyhow::Result<()> {
    let uc = ServiceUseCase::new(state.service_manager.clone());

    match args.action {
        ServiceAction::Start => {
            uc.start().await?;
            eprintln!("{}", msg("service.started"));
        }
        ServiceAction::Stop => {
            uc.stop().await?;
            eprintln!("{}", msg("service.stopped"));
        }
        ServiceAction::Restart => {
            uc.restart().await?;
            eprintln!("{}", msg("service.restarted"));
        }
    }

    Ok(())
}
