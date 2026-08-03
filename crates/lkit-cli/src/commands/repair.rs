use std::path::PathBuf;
use std::process::ExitCode;

use clap::Args;

use super::manage::{InstallRequest, RequestMode};
use clap::ValueEnum;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum RepairTarget {
    Static,
    Binary,
}

#[derive(Debug, Args)]
pub struct Repair {
    #[arg(value_enum)]
    pub target: RepairTarget,
    #[arg(long, num_args = 0..=1, value_name = "BASE_URL")]
    pub repository: Option<Option<String>>,
    #[arg(long, value_name = "PATH")]
    pub install_dir: Option<PathBuf>,
    #[cfg(feature = "test-support")]
    #[arg(long, value_name = "PATH", hide = true)]
    pub test_runtime: Option<PathBuf>,
}

pub async fn run(args: &Repair) -> ExitCode {
    let binary = args.target == RepairTarget::Binary;
    super::manage::run_request(&InstallRequest {
        mode: if binary {
            RequestMode::RepairBinary
        } else {
            RequestMode::RepairStatic
        },
        version: None,
        repository: args.repository.clone(),
        install_dir: args.install_dir.clone(),
        admin_user: None,
        password_file: None,
        service_manager: None,
        repair_static: !binary,
        repair_binary: binary,
        allow_no_backup: false,
        accept_service_change: false,
        force: false,
        #[cfg(feature = "test-support")]
        test_runtime: args.test_runtime.clone(),
    })
    .await
}
