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
    if (args.target == RepairTarget::Service && (args.repository.is_some() || args.official))
        || (args.target == RepairTarget::Binary && args.official)
    {
        eprintln!("lkit: invalid options for repair target");
        return ExitCode::from(2);
    }
    let runtime = match resolve_runtime(args) {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("lkit: {error}");
            return ExitCode::FAILURE;
        }
    };
    let inline = match inline_runtime(args) {
        Ok(inline) => inline,
        Err(error) => {
            eprintln!("lkit: {error}");
            return ExitCode::FAILURE;
        }
    };
    if !inline {
        use crate::daemon_operations::Kind;
        let kind = match args.target {
            RepairTarget::Service => Kind::Service,
            RepairTarget::Static => Kind::Static,
            RepairTarget::Binary => Kind::Binary,
        };
        return match crate::daemon_operations::submit_and_wait(
            &runtime,
            kind,
            super::manage::repository_override(&args.repository),
            args.official,
        )
        .await
        {
            Ok(code) => ExitCode::from(code),
            Err(error) => {
                eprintln!("lkit repair: {error}");
                ExitCode::FAILURE
            }
        };
    }
    #[cfg(feature = "test-support")]
    if args.target == RepairTarget::Service {
        return service_result(crate::workflows::repair_service::run(&runtime));
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

#[cfg(feature = "test-support")]
fn service_result(result: Result<(), crate::deployment::plan::InstallError>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("lkit repair service: {error}");
            ExitCode::FAILURE
        }
    }
}

fn resolve_runtime(
    _args: &Repair,
) -> Result<crate::deployment::runtime::InstallRuntime, crate::deployment::plan::InstallError> {
    #[cfg(feature = "test-support")]
    if let Some(path) = &_args.test_runtime {
        return crate::deployment::runtime::InstallRuntime::from_test_file(path);
    }
    Ok(crate::deployment::runtime::InstallRuntime::production())
}
fn inline_runtime(_args: &Repair) -> Result<bool, crate::deployment::plan::InstallError> {
    #[cfg(feature = "test-support")]
    if let Some(path) = &_args.test_runtime {
        return crate::deployment::runtime::InstallRuntime::test_uses_daemon(path)
            .map(|uses| !uses);
    }
    Ok(false)
}
