#[cfg(feature = "test-support")]
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

pub async fn run(args: &Switch, internal_worker: bool) -> ExitCode {
    if internal_worker {
        eprintln!("lkit: switch requires the structured daemon protocol; submit again");
        return ExitCode::FAILURE;
    }
    match native_runtime(args) {
        Ok(Some(runtime)) => {
            return match submit(args, &runtime).await {
                Ok(code) => ExitCode::from(code),
                Err(error) => {
                    eprintln!("lkit switch: {error}");
                    super::manage::exit_code(&error)
                }
            };
        }
        Ok(None) => {}
        Err(error) => {
            eprintln!("lkit switch: {error}");
            return super::manage::exit_code(&error);
        }
    }

    super::manage::run_request(&InstallRequest {
        mode: RequestMode::Switch,
        version: Some(args.version.clone()),
        repository: super::manage::repository_override(&args.repository),
        install_dir: None,
        admin_user: None,
        password_file: None,
        interactive_password: None,
        repair_static: false,
        repair_official: false,
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

fn native_runtime(
    _args: &Switch,
) -> Result<Option<crate::deployment::runtime::InstallRuntime>, crate::deployment::plan::InstallError>
{
    #[cfg(feature = "test-support")]
    if let Some(path) = &_args.test_runtime {
        if !crate::deployment::runtime::InstallRuntime::test_uses_daemon(path)? {
            return Ok(None);
        }
        return crate::deployment::runtime::InstallRuntime::from_test_file(path).map(Some);
    }
    Ok(Some(
        crate::deployment::runtime::InstallRuntime::production(),
    ))
}
async fn submit(
    args: &Switch,
    runtime: &crate::deployment::runtime::InstallRuntime,
) -> Result<u8, crate::deployment::plan::InstallError> {
    use crate::deployment::{plan::InstallError, state};
    crate::deployment::plan::TargetVersion::parse(&args.version)?;
    let installed = state::read_state()?
        .ok_or_else(|| InstallError::ParameterUsage("no Landscape installation".into()))?;
    crate::daemon_operations::submit_switch_and_wait(
        runtime,
        crate::daemon_operations::SwitchRequest {
            state_identity: crate::daemon_operations::installation_identity(&installed)?,
            from_version: installed.active_version,
            target: args.version.clone(),
            repository: super::manage::repository_override(&args.repository),
            accept_service_change: args.accept_service_change,
            allow_no_backup: args.allow_no_backup,
        },
    )
    .await
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn stale_worker_is_rejected_without_resubmission() {
        let args = Switch {
            version: "2.0.0".into(),
            repository: None,
            accept_service_change: false,
            allow_no_backup: false,
            #[cfg(feature = "test-support")]
            test_runtime: None,
        };
        assert_eq!(run(&args, true).await, ExitCode::FAILURE);
    }
}
