use std::path::PathBuf;
use std::process::ExitCode;

use clap::ValueEnum;

use crate::deployment::runtime::InstallRuntime;
use crate::deployment::{lock, plan, root, state, transaction};
use crate::interaction::credentials::{self, Credentials};
use crate::network::config::NetworkPlan;
use crate::release::repository::provider_for;
use crate::workflows::install as pipeline;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestMode {
    Install,
    Switch,
    RepairStatic,
    RepairBinary,
    Reconcile,
    ServiceManager,
}

pub(crate) struct InstallRequest {
    pub(crate) mode: RequestMode,
    /// Target version: `<version>` or `latest`
    pub(crate) version: Option<String>,

    /// Explicit repository source. None keeps the command's default source.
    pub(crate) repository: Option<plan::RepositoryChoice>,

    /// Full install root directory
    pub(crate) install_dir: Option<PathBuf>,

    /// First-install admin username, defaults to `admin`
    pub(crate) admin_user: Option<String>,

    /// First-install password read from a restricted file
    pub(crate) password_file: Option<PathBuf>,

    /// First-install password captured by the interactive console.
    pub(crate) interactive_password: Option<String>,

    /// Service manager: `systemd` or `none`
    pub(crate) service_manager: Option<ServiceManagerArg>,

    /// Restore official static pages from the target release
    pub(crate) repair_static: bool,

    /// Authorize repairing a same-version backend with a mismatched checksum
    pub(crate) repair_binary: bool,

    /// Authorize switching while the managed service is stopped without a
    /// configuration snapshot; no `.lkb` backup is created in this case
    pub(crate) allow_no_backup: bool,

    /// Non-interactively accept a modified managed systemd unit
    pub(crate) accept_service_change: bool,

    /// The interactive console already confirmed the operation; skip every
    /// /dev/tty prompt (delegated workers cannot read TUI keyboard input)
    pub(crate) console_confirmed: bool,

    /// Prompt the user to manually clean the existing directory before a clean install
    pub(crate) force: bool,

    /// Interactively configure Landscape as the host network owner.
    pub(crate) takeover_network: bool,

    /// Network plan captured by the full-screen console.
    pub(crate) network_plan: Option<NetworkPlan>,

    #[cfg(feature = "test-support")]
    pub(crate) test_runtime: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ServiceManagerArg {
    Systemd,
    None,
}

pub(crate) fn repository_override(
    value: &Option<Option<String>>,
) -> Option<plan::RepositoryChoice> {
    match value {
        None => None,
        Some(None) => Some(plan::RepositoryChoice::Mirror),
        Some(Some(value)) if value == "github" => Some(plan::RepositoryChoice::Github(
            crate::release::repository::github::DEFAULT_REPOSITORY.into(),
        )),
        Some(Some(url)) => Some(plan::RepositoryChoice::Http(url.clone())),
    }
}

pub(crate) async fn run_request(args: &InstallRequest) -> ExitCode {
    let runtime = match resolve_runtime(args) {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("install: {error}");
            return exit_code(&error);
        }
    };
    if args.force {
        return run_force(args);
    }
    let (plan, _lock) = match execute(args, &runtime).await {
        Ok(result) => result,
        Err(error) => {
            eprintln!("install: {error}");
            return exit_code(&error);
        }
    };
    match (args.mode, plan.state) {
        (RequestMode::Install, plan::StatePresence::FirstInstall) => {
            run_first_install(args, &plan, &runtime).await
        }
        (RequestMode::Install, plan::StatePresence::Installed) => {
            eprintln!(
                "install: {}",
                crate::tr!(crate::keys::MANAGE_INSTALLATION_ALREADY_EXISTS)
            );
            ExitCode::from(2)
        }
        (_, plan::StatePresence::FirstInstall) => {
            eprintln!(
                "install: {}",
                crate::tr!(crate::keys::MANAGE_COMMAND_REQUIRES_EXISTING_INSTALLATION)
            );
            ExitCode::from(2)
        }
        (_, plan::StatePresence::Installed) => super::existing::run(args, &plan, &runtime).await,
    }
}

/// 按错误类型映射退出码:`ParameterUsage` 属于参数或参数组合错误,返回 `2`;
/// 其余普通失败返回 `1`。
pub(super) fn exit_code(error: &plan::InstallError) -> ExitCode {
    match error {
        plan::InstallError::ParameterUsage(_) => ExitCode::from(2),
        _ => ExitCode::FAILURE,
    }
}

/// v1 `--force` 语义:不删除、不移动、不覆盖、不隔离任何文件。
/// 显示规范化安装根目录,警告目录可能包含数据库、凭据、证书、备份和用户文件,
/// 提示用户自行检查并删除整个安装根目录,不输出可直接复制执行的 `rm -rf`,
/// 返回退出码 `1`。用户清理完成后重新执行不带 `--force` 的安装。
fn run_force(args: &InstallRequest) -> ExitCode {
    if args.repair_static || args.repair_binary || args.accept_service_change {
        eprintln!(
            "install: {}",
            crate::tr!(crate::keys::MANAGE_FORCE_CANNOT_BE_COMBINED)
        );
        return ExitCode::from(2);
    }
    let install_root = match plan::select_install_root(
        args.install_dir.as_deref(),
        std::env::var("LKIT_INSTALL_DIR").ok().as_deref(),
    ) {
        Ok(install_root) => install_root,
        Err(error) => {
            eprintln!("install: {error}");
            return exit_code(&error);
        }
    };
    let normalized = match root::normalize_install_root(&install_root) {
        Ok(normalized) => normalized,
        Err(error) => {
            eprintln!("install: {error}");
            return exit_code(&error);
        }
    };
    eprintln!(
        "install: {} {}",
        crate::tr!(crate::keys::MANAGE_INSTALL_ROOT_IS),
        normalized.canonical.display()
    );
    eprintln!(
        "install: {}",
        crate::tr!(crate::keys::MANAGE_FORCE_DOES_NOT_MODIFY)
    );
    eprintln!(
        "install: {}",
        crate::tr!(crate::keys::MANAGE_INSTALL_ROOT_MAY_CONTAIN)
    );
    eprintln!(
        "install: {}",
        crate::tr!(crate::keys::MANAGE_MANUALLY_DELETE_INSTALL_ROOT)
    );
    ExitCode::FAILURE
}

async fn run_first_install(
    args: &InstallRequest,
    plan: &plan::Plan,
    runtime: &InstallRuntime,
) -> ExitCode {
    let credentials = match resolve_credentials(args, runtime.managed_uid) {
        Ok(credentials) => credentials,
        Err(error) => {
            eprintln!("install: {error}");
            return exit_code(&error);
        }
    };
    let spec = match plan.provider.as_ref() {
        Some(spec) => spec,
        None => {
            let error = plan::InstallError::CorruptedState(
                "first install plan is missing a repository source".into(),
            );
            eprintln!("install: {error}");
            return exit_code(&error);
        }
    };
    let provider = match provider_for(spec.kind, &spec.location) {
        Ok(provider) => provider,
        Err(error) => {
            let error = plan::InstallError::from(error);
            eprintln!("install: {error}");
            return exit_code(&error);
        }
    };
    let manager_choice = match args.service_manager {
        Some(ServiceManagerArg::Systemd) => pipeline::ManagerChoice::Systemd,
        Some(ServiceManagerArg::None) => pipeline::ManagerChoice::None,
        None => pipeline::ManagerChoice::Auto,
    };
    let network_plan = if let Some(network_plan) = args.network_plan.clone() {
        Some(network_plan)
    } else if args.takeover_network {
        if let Err(error) =
            crate::network::discovery::ensure_management_bridge_absent(&runtime.sys_class_net)
        {
            eprintln!("install: {error}");
            return exit_code(&error);
        }
        let (interfaces, routes) = match crate::network::discovery::discover(
            &runtime.sys_class_net,
            &runtime.ip_command,
        ) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("install: {error}");
                return exit_code(&error);
            }
        };
        let mut tty = match crate::interaction::interactive::Tty::open() {
            Ok(tty) => tty,
            Err(error) => {
                eprintln!(
                    "install: {}",
                    crate::tr!(
                        crate::keys::MANAGE_TAKEOVER_REQUIRES_INTERACTIVE_TERMINAL,
                        error = error
                    )
                );
                return exit_code(&error);
            }
        };
        match crate::network::discovery::prompt_plan(&interfaces, &routes, &mut tty) {
            Ok(plan) => Some(plan),
            Err(error) => {
                eprintln!("install: {error}");
                return exit_code(&error);
            }
        }
    } else {
        None
    };
    let health_options = match runtime.health_options() {
        Ok(options) => options,
        Err(error) => {
            eprintln!("install: {error}");
            return exit_code(&error);
        }
    };
    let result = if let Some(network) = network_plan.as_ref() {
        pipeline::first_install_with_network(
            &plan.root,
            &provider,
            &plan.target,
            &credentials,
            manager_choice,
            &runtime.systemd,
            &health_options,
            network,
            runtime,
        )
        .await
    } else {
        pipeline::first_install(
            &plan.root,
            &provider,
            &plan.target,
            &credentials,
            manager_choice,
            &runtime.systemd,
            &health_options,
        )
        .await
    };
    match result {
        Ok(outcome) => {
            if outcome.pending_network_confirmation {
                println!(
                    "install: {}",
                    crate::tr!(
                        crate::keys::MANAGE_ACTIVATED_AWAITING_NETWORK_CONFIRMATION,
                        version = outcome.release.version
                    )
                );
            } else {
                println!(
                    "install: {}",
                    crate::tr!(
                        crate::keys::MANAGE_COMMITTED_FIRST_INSTALL,
                        version = outcome.release.version
                    )
                );
            }
            match outcome.manager {
                pipeline::ServiceManager::Systemd => {
                    println!(
                        "install: {}",
                        crate::tr!(crate::keys::MANAGE_SYSTEMD_UNIT_REGISTERED)
                    );
                    if let Some(address) = outcome.pending_network_address {
                        println!(
                            "install: {}",
                            crate::tr!(crate::keys::MANAGE_TAKEOVER_AWAITING_CONFIRMATION)
                        );
                        println!(
                            "install: {}",
                            crate::tr!(
                                crate::keys::MANAGE_RECONNECT_AND_RUN_CONFIRM,
                                address = address
                            )
                        );
                    } else if outcome.pending_network_confirmation {
                        println!(
                            "install: {}",
                            crate::tr!(crate::keys::MANAGE_TAKEOVER_AWAITING_CONFIRMATION_DHCP)
                        );
                    } else {
                        println!(
                            "install: {}",
                            crate::tr!(crate::keys::MANAGE_MANAGEMENT_INTERFACE)
                        );
                    }
                }
                pipeline::ServiceManager::None => {
                    println!(
                        "install: {}",
                        crate::tr!(crate::keys::MANAGE_INITIALIZATION_PENDING)
                    );
                    println!("{}", pipeline::reference_command(&plan.root));
                }
            }
            if outcome.pending_network_confirmation {
                let minutes = runtime.network_confirm_timeout.as_secs().div_ceil(60);
                println!(
                    "install: {}",
                    crate::tr!(
                        crate::keys::MANAGE_CONFIRM_BEFORE_ROLLBACK,
                        minutes = minutes
                    )
                );
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("install: {error}");
            exit_code(&error)
        }
    }
}

fn resolve_credentials(
    args: &InstallRequest,
    managed_uid: u32,
) -> Result<Credentials, plan::InstallError> {
    let admin_user = args
        .admin_user
        .clone()
        .unwrap_or_else(|| "admin".to_string());
    let password = match (&args.interactive_password, &args.password_file) {
        (Some(password), None) => password.clone(),
        (None, Some(path)) => credentials::read_password_file(path, managed_uid)?,
        (None, None) => {
            match crate::interaction::interactive::read_password(&crate::tr!(
                crate::keys::MANAGE_ENTER_ADMIN_PASSWORD
            )) {
                Ok(password) => password,
                Err(plan::InstallError::NonInteractive(reason)) => {
                    return Err(plan::InstallError::ParameterUsage(format!(
                        "--password-file is required in non-interactive mode: {reason}; install lkit first and run `sudo lkit install ...` directly from a terminal, or provide a root-owned 0400/0600 password file"
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

async fn execute(
    args: &InstallRequest,
    runtime: &InstallRuntime,
) -> Result<(plan::Plan, lock::InstallLock), plan::InstallError> {
    check_environment(runtime)?;
    if args.takeover_network {
        if args.mode != RequestMode::Install {
            return Err(plan::InstallError::ParameterUsage(
                "--takeover-network is only valid for first install".into(),
            ));
        }
        if args.service_manager == Some(ServiceManagerArg::None) {
            return Err(plan::InstallError::ParameterUsage(
                "--takeover-network requires --service-manager systemd".into(),
            ));
        }
        crate::network::takeover::preflight(runtime)?;
    }
    let target = match &args.version {
        Some(value) => plan::TargetVersion::parse(value)?,
        None => plan::TargetVersion::Latest,
    };
    if let Some(user) = &args.admin_user {
        plan::validate_admin_user(user)?;
    }
    let install_root = plan::select_install_root(
        args.install_dir.as_deref(),
        std::env::var("LKIT_INSTALL_DIR").ok().as_deref(),
    )?;
    let normalized = root::normalize_install_root(&install_root)?;
    let lock = lock::acquire_install_lock(&normalized)?;
    if let Some(transaction) = transaction::find_unfinished(&normalized)? {
        let health = runtime.health_options()?;
        if transaction.network_takeover.is_some()
            && matches!(
                transaction.phase,
                transaction::Phase::AwaitingNetworkConfirmation
                    | transaction::Phase::Finalizing
                    | transaction::Phase::RollingBack
            )
        {
            return Err(plan::InstallError::BlockedByTransaction(format!(
                "network takeover {} is {}; use `lkit network status`, `lkit network confirm`, or `lkit network rollback`",
                transaction.transaction_id,
                transaction.phase.key()
            )));
        }
        transaction::recover_interrupted(&normalized, &transaction, &runtime.systemd, &health)
            .await?;
    }
    let loaded = state::load_state(&normalized)?;
    let presence = if loaded.is_some() {
        plan::StatePresence::Installed
    } else {
        plan::StatePresence::FirstInstall
    };
    plan::validate_applicability(
        presence,
        &normalized.canonical,
        plan::UsageFlags {
            admin_user: args.admin_user.is_some(),
            password_file: args.password_file.is_some() || args.interactive_password.is_some(),
            repair_static: args.repair_static,
            repair_binary: args.repair_binary,
            accept_service_change: args.accept_service_change,
            force: args.force,
        },
    )?;
    check_host_conflicts(
        presence,
        &normalized.canonical,
        &loaded,
        runtime,
        args.repair_binary,
        args.takeover_network,
    )?;
    pipeline::run_preflight(
        &normalized.canonical,
        loaded.as_ref(),
        args.repair_binary,
        runtime,
        args.takeover_network,
    )?;
    let repository = match (&args.repository, presence) {
        (Some(choice), _) => Some(choice.clone()),
        (None, plan::StatePresence::FirstInstall) if args.mode == RequestMode::Install => Some(
            crate::deployment::config::resolve_default_choice(&normalized)?,
        ),
        (None, plan::StatePresence::FirstInstall) | (None, plan::StatePresence::Installed) => None,
    };
    let plan = plan::build_plan(normalized, target, repository, presence)?;
    Ok((plan, lock))
}

/// 检测旧手工部署与固定端口上的冲突进程。
/// 已安装环境:受管进程放行,无法确认的占用者阻断;
/// 首次安装:任何固定端口占用者都阻断。
/// `allow_sha_drift` 用于 `--repair-binary`,允许执行文件摘要漂移的受管进程。
fn check_host_conflicts(
    presence: plan::StatePresence,
    canonical: &std::path::Path,
    loaded: &Option<state::InstallState>,
    runtime: &InstallRuntime,
    allow_sha_drift: bool,
    network_takeover: bool,
) -> Result<(), plan::InstallError> {
    let old = std::path::Path::new("/root/.landscape-router");
    if old.exists() {
        eprintln!(
            "install: {}",
            crate::tr!(crate::keys::MANAGE_OLD_MANUAL_DEPLOYMENT_WARNING)
        );
    }
    let ports: Vec<(crate::service::process::Protocol, u16)> = runtime
        .health_ports
        .iter()
        .filter(|check| !network_takeover || check.port != 53)
        .map(|check| (check.protocol, check.port))
        .collect();
    match (presence, loaded) {
        (plan::StatePresence::Installed, Some(state)) => {
            if allow_sha_drift {
                crate::service::process::check_conflicts_relaxed(canonical, state, &ports)?;
            } else {
                crate::service::process::check_conflicts(canonical, state, &ports)?;
            }
        }
        _ => {
            let pids = crate::service::process::pids_for_ports(&ports);
            if !pids.is_empty() {
                return Err(plan::InstallError::ProcessConflict(format!(
                    "the fixed ports are occupied by processes {pids:?} that cannot be confirmed as part of a managed install"
                )));
            }
        }
    }
    Ok(())
}

fn check_environment(runtime: &InstallRuntime) -> Result<(), plan::InstallError> {
    if !runtime.allow_non_root && unsafe { libc::geteuid() } != 0 {
        return Err(plan::InstallError::UnsupportedPlatform(
            crate::tr!(crate::keys::MANAGE_MUST_RUN_AS_ROOT).into(),
        ));
    }
    Ok(())
}

fn resolve_runtime(_args: &InstallRequest) -> Result<InstallRuntime, plan::InstallError> {
    #[cfg(feature = "test-support")]
    if let Some(path) = _args.test_runtime.as_deref() {
        return InstallRuntime::from_test_file(path);
    }
    Ok(InstallRuntime::production())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request_with_password(password: Option<&str>) -> InstallRequest {
        InstallRequest {
            mode: RequestMode::Install,
            version: None,
            repository: None,
            install_dir: None,
            admin_user: Some("admin".into()),
            password_file: None,
            interactive_password: password.map(str::to_string),
            service_manager: None,
            repair_static: false,
            repair_binary: false,
            allow_no_backup: false,
            accept_service_change: false,
            force: false,
            takeover_network: false,
            network_plan: None,
            console_confirmed: false,
            #[cfg(feature = "test-support")]
            test_runtime: None,
        }
    }

    #[test]
    fn resolves_console_password_without_opening_a_tty() {
        let request = request_with_password(Some("Secret123"));
        let credentials = resolve_credentials(&request, unsafe { libc::geteuid() }).unwrap();
        assert_eq!(credentials.admin_user, "admin");
        assert_eq!(credentials.password, "Secret123");
    }

    #[test]
    fn validates_console_password_complexity() {
        let request = request_with_password(Some("lowercase1"));
        assert!(matches!(
            resolve_credentials(&request, unsafe { libc::geteuid() }),
            Err(plan::InstallError::InvalidPassword(_))
        ));
    }

    #[test]
    fn maps_cli_repository_overrides_to_domain_choices() {
        assert_eq!(repository_override(&None), None);
        assert_eq!(
            repository_override(&Some(None)),
            Some(plan::RepositoryChoice::Mirror)
        );
        assert_eq!(
            repository_override(&Some(Some("github".into()))),
            Some(plan::RepositoryChoice::Github(
                crate::release::repository::github::DEFAULT_REPOSITORY.into()
            ))
        );
        assert_eq!(
            repository_override(&Some(Some("https://example.com/releases/".into()))),
            Some(plan::RepositoryChoice::Http(
                "https://example.com/releases/".into()
            ))
        );
    }
}
