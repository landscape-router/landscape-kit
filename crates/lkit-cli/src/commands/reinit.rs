use std::path::PathBuf;
use std::process::ExitCode;

use clap::Args;

use crate::deployment::runtime::InstallRuntime;
use crate::deployment::{lock, plan, root, state, transaction};
use crate::interaction::credentials::{self, Credentials};
use crate::network::config::NetworkPlan;
use crate::workflows::reinit::{ReinitArgs, ReinitOptions, ReinitOutcome};

#[derive(Args)]
pub struct Reinit {
    /// Full install root directory
    #[arg(long, value_name = "PATH")]
    pub install_dir: Option<PathBuf>,
    /// New admin username, defaults to `admin`
    #[arg(long, value_name = "NAME")]
    pub admin_user: Option<String>,
    /// New admin password read from a restricted file
    #[arg(long, value_name = "PATH")]
    pub password_file: Option<PathBuf>,
    /// Password captured by the interactive console. Never populated by CLI parsing.
    #[arg(skip)]
    pub(crate) interactive_password: Option<String>,
    /// Allow reinit without a protection `.lkb` when the running instance
    /// cannot export its configuration
    #[arg(long)]
    pub allow_no_backup: bool,
    /// Confirm the destructive plan in non-interactive mode
    #[arg(long)]
    pub yes: bool,
    /// The interactive console already confirmed the plan; skip every /dev/tty prompt
    #[arg(long, hide = true)]
    pub console_confirmed: bool,
    /// Network plan captured by the full-screen console. Never populated by CLI parsing.
    #[arg(skip)]
    pub(crate) network_plan: Option<NetworkPlan>,
    /// Root-only network plan file created for an internal daemon worker.
    #[arg(long, value_name = "PATH", hide = true)]
    pub(crate) network_plan_file: Option<PathBuf>,
    #[cfg(feature = "test-support")]
    #[arg(long, value_name = "PATH", hide = true)]
    pub test_runtime: Option<PathBuf>,
}

impl std::fmt::Debug for Reinit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Reinit")
            .field("install_dir", &self.install_dir)
            .field("admin_user", &self.admin_user)
            .field(
                "password_file",
                &self.password_file.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "interactive_password",
                &self.interactive_password.as_ref().map(|_| "[REDACTED]"),
            )
            .field("allow_no_backup", &self.allow_no_backup)
            .field("yes", &self.yes)
            .finish_non_exhaustive()
    }
}

pub async fn run(args: &Reinit) -> ExitCode {
    let runtime = match resolve_runtime(args) {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("reinit: {error}");
            return exit_code(&error);
        }
    };
    if !runtime.allow_non_root && unsafe { libc::geteuid() } != 0 {
        eprintln!(
            "reinit: {}",
            crate::tr!(crate::keys::MANAGE_MUST_RUN_AS_ROOT)
        );
        return ExitCode::FAILURE;
    }
    let install_root = match plan::select_install_root(
        args.install_dir.as_deref(),
        std::env::var("LKIT_INSTALL_DIR").ok().as_deref(),
    ) {
        Ok(install_root) => install_root,
        Err(error) => {
            eprintln!("reinit: {error}");
            return exit_code(&error);
        }
    };
    let normalized = match root::normalize_install_root(&install_root) {
        Ok(normalized) => normalized,
        Err(error) => {
            eprintln!("reinit: {error}");
            return exit_code(&error);
        }
    };
    let _lock = match lock::acquire_install_lock(&normalized) {
        Ok(lock) => lock,
        Err(error) => {
            eprintln!("reinit: {error}");
            return exit_code(&error);
        }
    };
    let health = match runtime.health_options() {
        Ok(health) => health,
        Err(error) => {
            eprintln!("reinit: {error}");
            return exit_code(&error);
        }
    };
    let unfinished = match transaction::find_unfinished(&normalized) {
        Ok(transaction) => transaction,
        Err(error) => {
            eprintln!("reinit: {error}");
            return exit_code(&error);
        }
    };
    if let Some(transaction) = unfinished {
        if transaction.network_takeover.is_some()
            && matches!(
                transaction.phase,
                transaction::Phase::AwaitingNetworkConfirmation
                    | transaction::Phase::Finalizing
                    | transaction::Phase::RollingBack
            )
        {
            eprintln!(
                "reinit: {}",
                crate::tr!(
                    crate::keys::REINIT_BLOCKED_BY_PENDING_TAKEOVER,
                    transaction_id = transaction.transaction_id,
                    phase = transaction.phase.key()
                )
            );
            return ExitCode::from(1);
        }
        if let Err(error) = transaction::recover_interrupted(
            &normalized,
            &transaction,
            runtime.service_manager.as_ref(),
            &health,
        )
        .await
        {
            eprintln!("reinit: {error}");
            return exit_code(&error);
        }
    }
    let Some(state) = (match state::load_state(&normalized) {
        Ok(state) => state,
        Err(error) => {
            eprintln!("reinit: {error}");
            return exit_code(&error);
        }
    }) else {
        eprintln!(
            "reinit: {}",
            crate::tr!(crate::keys::REINIT_REQUIRES_EXISTING_INSTALLATION)
        );
        return ExitCode::from(2);
    };
    if state.service.manager != crate::deployment::state::StateServiceManager::Systemd {
        eprintln!(
            "reinit: {}",
            crate::tr!(crate::keys::REINIT_REQUIRES_SYSTEMD)
        );
        return ExitCode::from(2);
    }
    if !crate::workflows::uninstall::host_network_services_masked(runtime.service_manager.as_ref())
    {
        eprintln!(
            "reinit: {}",
            crate::tr!(crate::keys::REINIT_REQUIRES_NETWORK_TAKEOVER)
        );
        return ExitCode::from(2);
    }
    if let Err(error) = crate::network::takeover::preflight(&runtime) {
        eprintln!("reinit: {error}");
        return exit_code(&error);
    }
    let credentials = match resolve_credentials(args, runtime.managed_uid) {
        Ok(credentials) => credentials,
        Err(error) => {
            eprintln!("reinit: {error}");
            return exit_code(&error);
        }
    };
    let network_plan = match (&args.network_plan, &args.network_plan_file) {
        (Some(plan), None) => plan.clone(),
        (None, Some(path)) => match crate::daemon_worker::read_network_plan(path) {
            Ok(plan) => plan,
            Err(error) => {
                eprintln!("reinit: {error}");
                return ExitCode::from(2);
            }
        },
        (None, None) => match collect_network_plan(&runtime) {
            Ok(plan) => plan,
            Err(error) => {
                eprintln!("reinit: {error}");
                return exit_code(&error);
            }
        },
        (Some(_), Some(_)) => {
            eprintln!("reinit: internal network plans cannot be combined");
            return ExitCode::from(2);
        }
    };
    let data_dir = normalized.canonical.join("data");
    let options = ReinitOptions {
        export_base_url: runtime.export_base_url.clone(),
        token: &(|| {
            crate::backup::export::read_api_token(
                &data_dir.join("landscape_api_token"),
                runtime.managed_uid,
            )
        }),
        confirm: &|prompt| crate::interaction::interactive::confirm(prompt),
        health: &health,
    };
    let args = ReinitArgs {
        allow_no_backup: args.allow_no_backup,
        yes: args.yes,
        console_confirmed: args.console_confirmed,
    };
    match crate::workflows::reinit::reinit_installation(
        &normalized,
        &state,
        runtime.service_manager.as_ref(),
        &credentials,
        &network_plan,
        &args,
        &options,
        &runtime,
    )
    .await
    {
        Ok(ReinitOutcome::Committed {
            version,
            backup_id,
            pending_network_address,
        }) => {
            println!(
                "reinit: {}",
                crate::tr!(
                    crate::keys::REINIT_ACTIVATED_AWAITING_CONFIRMATION,
                    version = version
                )
            );
            match backup_id {
                Some(backup_id) => println!(
                    "reinit: {}",
                    crate::tr!(
                        crate::keys::REINIT_PROTECTION_BACKUP_ID,
                        backup_id = backup_id
                    )
                ),
                None => println!(
                    "reinit: {}",
                    crate::tr!(crate::keys::REINIT_NO_PROTECTION_BACKUP)
                ),
            }
            match pending_network_address {
                Some(address) => {
                    println!(
                        "reinit: {}",
                        crate::tr!(crate::keys::REINIT_AWAITING_CONFIRMATION, address = address)
                    );
                }
                None => {
                    println!(
                        "reinit: {}",
                        crate::tr!(crate::keys::REINIT_AWAITING_CONFIRMATION_DHCP)
                    );
                }
            }
            let minutes = runtime.network_confirm_timeout.as_secs().div_ceil(60);
            println!(
                "reinit: {}",
                crate::tr!(
                    crate::keys::REINIT_CONFIRM_BEFORE_ROLLBACK,
                    minutes = minutes
                )
            );
            ExitCode::SUCCESS
        }
        Ok(ReinitOutcome::RolledBack { version }) => {
            eprintln!(
                "reinit: {}",
                crate::tr!(crate::keys::REINIT_FAILED_ROLLED_BACK, version = version)
            );
            ExitCode::from(5)
        }
        Ok(ReinitOutcome::RollbackFailed { version, reason }) => {
            eprintln!(
                "reinit: {}",
                crate::tr!(
                    crate::keys::REINIT_FAILED_ROLLBACK_FAILED,
                    version = version,
                    reason = reason
                )
            );
            ExitCode::from(6)
        }
        Err(error) => {
            eprintln!("reinit: {error}");
            exit_code(&error)
        }
    }
}

fn resolve_credentials(args: &Reinit, managed_uid: u32) -> Result<Credentials, plan::InstallError> {
    let admin_user = args
        .admin_user
        .clone()
        .unwrap_or_else(|| "admin".to_string());
    let password = match (&args.interactive_password, &args.password_file) {
        (Some(password), None) => password.clone(),
        (None, Some(path)) => credentials::read_password_file(path, managed_uid)?,
        (None, None) => {
            match crate::interaction::interactive::read_password(&crate::tr!(
                crate::keys::REINIT_ENTER_ADMIN_PASSWORD
            )) {
                Ok(password) => password,
                Err(plan::InstallError::NonInteractive(reason)) => {
                    return Err(plan::InstallError::ParameterUsage(format!(
                        "--password-file is required in non-interactive mode: {reason}"
                    )));
                }
                Err(error) => return Err(error),
            }
        }
        (Some(_), Some(_)) => {
            return Err(plan::InstallError::ParameterUsage(
                "interactive password and --password-file cannot be combined".into(),
            ));
        }
    };
    credentials::validate_password(&password)?;
    Ok(Credentials {
        admin_user,
        password,
    })
}

fn collect_network_plan(runtime: &InstallRuntime) -> Result<NetworkPlan, plan::InstallError> {
    let (interfaces, routes) =
        crate::network::discovery::discover(&runtime.sys_class_net, &runtime.ip_command)?;
    let mut tty = crate::interaction::interactive::Tty::open().map_err(|error| {
        if matches!(error, plan::InstallError::NonInteractive(_)) {
            plan::InstallError::ParameterUsage(format!(
                "reinit requires an interactive terminal for the WAN/LAN plan: {error}"
            ))
        } else {
            error
        }
    })?;
    crate::network::discovery::prompt_plan(&interfaces, &routes, &mut tty)
}

fn exit_code(error: &plan::InstallError) -> ExitCode {
    match error {
        plan::InstallError::ParameterUsage(_) | plan::InstallError::UnsupportedPlatform(_) => {
            ExitCode::from(2)
        }
        _ => ExitCode::FAILURE,
    }
}

#[cfg(feature = "test-support")]
fn resolve_runtime(args: &Reinit) -> Result<InstallRuntime, plan::InstallError> {
    if let Some(path) = args.test_runtime.as_deref() {
        return InstallRuntime::from_test_file(path);
    }
    Ok(InstallRuntime::production())
}

#[cfg(not(feature = "test-support"))]
fn resolve_runtime(_args: &Reinit) -> Result<InstallRuntime, plan::InstallError> {
    Ok(InstallRuntime::production())
}

#[cfg(test)]
mod tests {
    use clap::{Command, FromArgMatches};

    use super::*;

    fn parse(args: &[&str]) -> Result<Reinit, clap::Error> {
        let command = <Reinit as Args>::augment_args(Command::new("reinit"));
        let matches = command.try_get_matches_from(args)?;
        Reinit::from_arg_matches(&matches)
    }

    #[test]
    fn parses_reinit_options() {
        let reinit = parse(&[
            "reinit",
            "--allow-no-backup",
            "--yes",
            "--admin-user",
            "router",
        ])
        .unwrap();
        assert!(reinit.allow_no_backup);
        assert!(reinit.yes);
        assert_eq!(reinit.admin_user.as_deref(), Some("router"));
    }

    #[test]
    fn rejects_non_reinit_workflow_flags() {
        assert!(parse(&["reinit", "--takeover-network"]).is_err());
        assert!(parse(&["reinit", "--version", "0.19.2"]).is_err());
    }
}
