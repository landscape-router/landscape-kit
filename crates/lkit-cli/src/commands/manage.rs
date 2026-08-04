use std::path::PathBuf;
use std::process::ExitCode;

use clap::ValueEnum;

use crate::deployment::runtime::InstallRuntime;
use crate::deployment::{lock, plan, root, state, transaction};
use crate::interaction::credentials::{self, Credentials};
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

#[derive(Debug)]
pub(crate) struct InstallRequest {
    pub(crate) mode: RequestMode,
    /// Target version: `<version>` or `latest`
    pub(crate) version: Option<String>,

    /// Repository source: bare flag uses the default HTTP mirror, a value uses the given protocol v1 HTTP repository
    pub(crate) repository: Option<Option<String>>,

    /// Full install root directory
    pub(crate) install_dir: Option<PathBuf>,

    /// First-install admin username, defaults to `admin`
    pub(crate) admin_user: Option<String>,

    /// First-install password read from a restricted file
    pub(crate) password_file: Option<PathBuf>,

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

    /// Prompt the user to manually clean the existing directory before a clean install
    pub(crate) force: bool,

    /// Interactively configure Landscape as the host network owner.
    pub(crate) takeover_network: bool,

    #[cfg(feature = "test-support")]
    pub(crate) test_runtime: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ServiceManagerArg {
    Systemd,
    None,
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
                "install: an installation already exists; use switch, repair, reconcile, or service-manager"
            );
            ExitCode::from(2)
        }
        (_, plan::StatePresence::FirstInstall) => {
            eprintln!("install: this command requires an existing installation");
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
            "install: --force cannot be combined with --repair-static, --repair-binary, or any --accept-* flag"
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
        "install: install root is {}",
        normalized.canonical.display()
    );
    eprintln!("install: --force does not delete, move, overwrite, or quarantine any file");
    eprintln!(
        "install: the install root may contain databases, credentials, certificates, backups, and user files"
    );
    eprintln!(
        "install: manually inspect and delete the entire install root, then re-run `lkit install` without --force"
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
    let provider = match provider_for(plan.provider.kind, &plan.provider.location) {
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
    let network_plan = if args.takeover_network {
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
                eprintln!("install: network takeover requires an interactive terminal: {error}");
                return exit_code(&error);
            }
        };
        match crate::network::discovery::prompt_plan(
            &interfaces,
            &routes,
            std::env::var("SSH_CONNECTION").ok().as_deref(),
            &mut tty,
        ) {
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
            if outcome.pending_network_address.is_some() {
                println!(
                    "install: activated {} and is awaiting network confirmation",
                    outcome.release.version
                );
            } else {
                println!(
                    "install: committed first install of {}",
                    outcome.release.version
                );
            }
            match outcome.manager {
                pipeline::ServiceManager::Systemd => {
                    println!(
                        "install: systemd unit landscape-router.service is registered, enabled, and running"
                    );
                    if let Some(address) = outcome.pending_network_address {
                        println!("install: network takeover is awaiting confirmation");
                        println!("install: reconnect to {address} and run `lkit network confirm`");
                    } else {
                        println!("install: management interface https://127.0.0.1:6443");
                    }
                }
                pipeline::ServiceManager::None => {
                    println!(
                        "install: initialization is pending; start the service manually with:"
                    );
                    println!("{}", pipeline::reference_command(&plan.root));
                }
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
    let password = match &args.password_file {
        Some(path) => credentials::read_password_file(path, managed_uid)?,
        None => match crate::interaction::interactive::read_password("Enter admin password") {
            Ok(password) => password,
            Err(plan::InstallError::NonInteractive(reason)) => {
                return Err(plan::InstallError::ParameterUsage(format!(
                    "--password-file is required in non-interactive mode: {reason}; install lkit first and run `sudo lkit install ...` directly from a terminal, or provide a root-owned 0400/0600 password file"
                )));
            }
            Err(error) => return Err(error),
        },
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
        if matches!(
            transaction.phase,
            transaction::Phase::AwaitingNetworkConfirmation | transaction::Phase::Finalizing
        ) {
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
            password_file: args.password_file.is_some(),
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
    let repository = match &args.repository {
        None => plan::RepositoryChoice::Github,
        Some(None) => plan::RepositoryChoice::Mirror,
        Some(Some(url)) => plan::RepositoryChoice::Http(url.clone()),
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
            "install: warning: found an old manual deployment at /root/.landscape-router; v1 does not migrate it and rejects deployments that could overwrite it or conflict on the fixed ports; a dedicated migration flow will be provided in the future"
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
            "must run as root (uid 0)".into(),
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
