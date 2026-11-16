use super::*;
use crate::deployment::{plan, root, runtime::InstallRuntime, state, transaction};

pub(super) async fn run(args: &Install) -> ExitCode {
    match submit(args).await {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("install: {e}");
            super::super::manage::exit_code(&e)
        }
    }
}
async fn submit(args: &Install) -> Result<u8, plan::InstallError> {
    if state::read_state()?.is_some() {
        return Err(plan::InstallError::ParameterUsage(
            "Landscape is already installed; uninstall it before installing again".into(),
        ));
    }
    if args.network_plan.is_some() && !args.takeover_network {
        return Err(plan::InstallError::ParameterUsage(
            "install requires a structured takeover plan; internal plan files are unsupported"
                .into(),
        ));
    }
    let runtime = runtime(args)?;
    if !runtime.allow_non_root && unsafe { libc::geteuid() } != 0 {
        return Err(plan::InstallError::UnsupportedPlatform(
            "install requires root".into(),
        ));
    }
    let selected = plan::select_install_root(
        args.install_dir.as_deref(),
        std::env::var("LKIT_INSTALL_DIR").ok().as_deref(),
    )?;
    let root = root::normalize_install_root(&selected)?;
    crate::workflows::install::native::validate_root(&root)?;
    if transaction::find_unfinished(&root)?.is_some() {
        return Err(plan::InstallError::ParameterUsage(
            "unfinished transaction; inspect lkit network status or wait for daemon recovery"
                .into(),
        ));
    }
    let target = plan::TargetVersion::parse(args.version.as_deref().unwrap_or("latest"))?;
    let repository = super::super::manage::repository_override(&args.repository)
        .map(Ok)
        .unwrap_or_else(crate::deployment::config::resolve_default_choice)?;
    repository.clone().resolve()?;
    crate::daemon::require_capability("install-v1.pid")?;
    crate::workflows::install::require_manager(runtime.service_manager.as_ref())?;
    if args.takeover_network {
        crate::network::takeover::preflight(&runtime)?;
    }
    crate::service::preflight::run_preflight(
        &root.canonical,
        None,
        false,
        &runtime,
        args.takeover_network,
    )?;
    let credentials =
        super::super::manage::resolve_credentials(&args.request(None), runtime.managed_uid)?;
    plan::validate_admin_user(&credentials.admin_user)?;
    let network = if let Some(plan) = &args.network_plan {
        Some(plan.clone())
    } else if args.takeover_network {
        let (interfaces, routes) =
            crate::network::discovery::discover(&runtime.sys_class_net, &runtime.ip_command)?;
        let mut tty = crate::interaction::interactive::Tty::open()?;
        Some(crate::network::discovery::prompt_plan(
            &interfaces,
            &routes,
            &mut tty,
        )?)
    } else {
        None
    };
    crate::daemon_operations::submit_install_and_wait(
        root.canonical.to_string_lossy().into_owned(),
        crate::daemon_operations::InstallRequest {
            target: target.to_string(),
            repository,
            admin_user: credentials.admin_user,
            network,
        },
        &credentials.password,
        args.from_console,
    )
    .await
}
fn runtime(_args: &Install) -> Result<InstallRuntime, plan::InstallError> {
    #[cfg(feature = "test-support")]
    if let Some(path) = &_args.test_runtime {
        return InstallRuntime::from_test_file(path);
    }
    Ok(InstallRuntime::production())
}
