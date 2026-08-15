use std::path::PathBuf;
use std::process::ExitCode;

use clap::Args;

use super::manage::{InstallRequest, RequestMode};

#[derive(Debug, Args)]
pub struct Switch {
    #[arg(long, value_name = "VERSION")]
    pub version: String,
    #[arg(long, num_args = 0..=1, value_name = "BASE_URL")]
    pub repository: Option<Option<String>>,
    #[arg(long, value_name = "PATH")]
    pub install_dir: Option<PathBuf>,
    #[arg(long)]
    pub accept_service_change: bool,
    /// Allow switching while the managed service is stopped; no .lkb backup is
    /// created in this case and automatic rollback cannot restore previous data
    #[arg(long)]
    pub allow_no_backup: bool,
    #[cfg(feature = "test-support")]
    #[arg(long, value_name = "PATH", hide = true)]
    pub test_runtime: Option<PathBuf>,
}

pub async fn run(args: &Switch) -> ExitCode {
    super::manage::run_request(&InstallRequest {
        mode: RequestMode::Switch,
        version: Some(args.version.clone()),
        repository: super::manage::repository_override(&args.repository),
        install_dir: args.install_dir.clone(),
        admin_user: None,
        password_file: None,
        interactive_password: None,
        repair_static: false,
        repair_binary: false,
        allow_no_backup: args.allow_no_backup,
        accept_service_change: args.accept_service_change,
        force: false,
        takeover_network: false,
        network_plan: None,
        console_confirmed: false,
        #[cfg(feature = "test-support")]
        test_runtime: args.test_runtime.clone(),
    })
    .await
}
