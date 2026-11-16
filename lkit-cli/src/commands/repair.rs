#[cfg(feature = "test-support")]
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Args;

use super::manage::{InstallRequest, RequestMode};
use clap::ValueEnum;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum RepairTarget {
    Static,
    Binary,
    Service,
}

#[derive(Debug, Args)]
pub struct Repair {
    #[arg(value_enum)]
    pub target: RepairTarget,
    #[arg(long, num_args = 0..=1, value_name = "BASE_URL")]
    pub repository: Option<Option<String>>,
    /// Restore the official frontend pages unconditionally, ignoring any configured custom frontend source
    #[arg(long)]
    pub official: bool,
    #[cfg(feature = "test-support")]
    #[arg(long, value_name = "PATH", hide = true)]
    pub test_runtime: Option<PathBuf>,
}

pub async fn run(args: &Repair) -> ExitCode {
    if args.target == RepairTarget::Service {
        if args.repository.is_some() || args.official {
            eprintln!("lkit: repair service does not accept repository or official options");
            return ExitCode::from(2);
        }
        let runtime = {
            #[cfg(feature = "test-support")]
            if let Some(path) = &args.test_runtime {
                match crate::deployment::runtime::InstallRuntime::from_test_file(path) {
                    Ok(runtime) => {
                        if !crate::deployment::runtime::InstallRuntime::test_uses_daemon(path)
                            .unwrap_or(false)
                        {
                            return service_result(crate::workflows::repair_service::run(&runtime));
                        }
                        return service_result(
                            crate::daemon_operations::submit_and_wait(&runtime).await,
                        );
                    }
                    Err(error) => {
                        eprintln!("lkit: {error}");
                        return ExitCode::FAILURE;
                    }
                }
            }
            crate::deployment::runtime::InstallRuntime::production()
        };
        return service_result(crate::daemon_operations::submit_and_wait(&runtime).await);
    }
    let binary = args.target == RepairTarget::Binary;
    if binary && args.official {
        eprintln!("lkit: --official is only valid for `lkit repair static`");
        return ExitCode::from(2);
    }
    super::manage::run_request(&InstallRequest {
        mode: if binary {
            RequestMode::RepairBinary
        } else {
            RequestMode::RepairStatic
        },
        version: None,
        repository: super::manage::repository_override(&args.repository),
        install_dir: None,
        admin_user: None,
        password_file: None,
        interactive_password: None,
        repair_static: !binary,
        repair_binary: binary,
        repair_official: !binary && args.official,
        allow_no_backup: false,
        accept_service_change: false,
        force: false,
        takeover_network: false,
        network_plan: None,
        console_confirmed: false,
        #[cfg(feature = "test-support")]
        test_runtime: args.test_runtime.clone(),
    })
    .await
}

fn service_result(result: Result<(), crate::deployment::plan::InstallError>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("lkit repair service: {error}");
            ExitCode::FAILURE
        }
    }
}
