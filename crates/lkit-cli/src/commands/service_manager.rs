use std::path::PathBuf;
use std::process::ExitCode;

use clap::Args;

use super::manage::ServiceManagerArg;
use super::manage::{InstallRequest, RequestMode};

#[derive(Debug, Args)]
pub struct ServiceManager {
    #[arg(value_enum)]
    pub target: ServiceManagerArg,
    #[arg(long, value_name = "PATH")]
    pub install_dir: Option<PathBuf>,
    #[cfg(feature = "test-support")]
    #[arg(long, value_name = "PATH", hide = true)]
    pub test_runtime: Option<PathBuf>,
}

pub async fn run(args: &ServiceManager) -> ExitCode {
    super::manage::run_request(&InstallRequest {
        mode: RequestMode::ServiceManager,
        version: None,
        repository: None,
        install_dir: args.install_dir.clone(),
        admin_user: None,
        password_file: None,
        service_manager: Some(args.target),
        repair_static: false,
        repair_binary: false,
        allow_no_backup: false,
        accept_service_change: false,
        force: false,
        #[cfg(feature = "test-support")]
        test_runtime: args.test_runtime.clone(),
    })
    .await
}
