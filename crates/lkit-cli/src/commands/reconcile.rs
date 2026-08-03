use std::path::PathBuf;
use std::process::ExitCode;

use clap::Args;

use super::manage::{InstallRequest, RequestMode};

#[derive(Debug, Args)]
pub struct Reconcile {
    #[arg(long, num_args = 0..=1, value_name = "BASE_URL")]
    pub repository: Option<Option<String>>,
    #[arg(long, value_name = "PATH")]
    pub install_dir: Option<PathBuf>,
    #[arg(long)]
    pub accept_service_change: bool,
    #[cfg(feature = "test-support")]
    #[arg(long, value_name = "PATH", hide = true)]
    pub test_runtime: Option<PathBuf>,
}

pub async fn run(args: &Reconcile) -> ExitCode {
    super::manage::run_request(&InstallRequest {
        mode: RequestMode::Reconcile,
        version: None,
        repository: args.repository.clone(),
        install_dir: args.install_dir.clone(),
        admin_user: None,
        password_file: None,
        service_manager: None,
        repair_static: false,
        repair_binary: false,
        allow_no_backup: false,
        accept_service_change: args.accept_service_change,
        force: false,
        #[cfg(feature = "test-support")]
        test_runtime: args.test_runtime.clone(),
    })
    .await
}
