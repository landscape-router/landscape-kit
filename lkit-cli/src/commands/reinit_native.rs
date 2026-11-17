use super::*;
use crate::daemon_operations::{ReinitRequest, installation_identity};

#[derive(Clone, Debug)]
pub(crate) struct ReinitConfirmation {
    pub root: String,
    pub from_version: String,
    pub state_identity: String,
}
pub(crate) fn capture_confirmation() -> Result<ReinitConfirmation, plan::InstallError> {
    let root = state::discover_landscape_root()?
        .ok_or_else(|| plan::InstallError::ParameterUsage("no installation".into()))?;
    let installed = state::load_state(&root)?
        .ok_or_else(|| plan::InstallError::ParameterUsage("missing installation state".into()))?;
    Ok(ReinitConfirmation {
        root: root.canonical.to_string_lossy().into_owned(),
        from_version: installed.active_version.clone(),
        state_identity: installation_identity(&installed)?,
    })
}
pub(super) async fn run(args: &Reinit) -> ExitCode {
    match submit(args).await {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("reinit: {e}");
            exit_code(&e)
        }
    }
}
async fn submit(args: &Reinit) -> Result<u8, plan::InstallError> {
    if args.console_confirmed && args.confirmed.is_none() {
        return Err(plan::InstallError::ParameterUsage(
            "reinit requires a bound console confirmation; internal plan files are unsupported"
                .into(),
        ));
    }
    let runtime = resolve_runtime(args)?;
    if !runtime.allow_non_root && unsafe { libc::geteuid() } != 0 {
        return Err(plan::InstallError::UnsupportedPlatform(
            "reinit requires root".into(),
        ));
    }
    crate::daemon::require_capability("reinit-v1.pid")?;
    let context = match &args.confirmed {
        Some(context) => context.clone(),
        None => capture_confirmation()?,
    };
    let root =
        crate::deployment::root::normalize_install_root(std::path::Path::new(&context.root))?;
    let installed = state::load_state(&root)?
        .ok_or_else(|| plan::InstallError::ParameterUsage("missing installation".into()))?;
    if installation_identity(&installed)? != context.state_identity {
        return Err(plan::InstallError::ParameterUsage(
            "installation changed since confirmation".into(),
        ));
    }
    if transaction::find_unfinished(&root)?.is_some() {
        return Err(plan::InstallError::ParameterUsage(
            "unfinished transaction; inspect lkit network status".into(),
        ));
    }
    crate::network::takeover::require_existing_takeover(
        &root,
        &installed,
        runtime.service_manager.as_ref(),
    )?;
    crate::network::takeover::preflight(&runtime)?;
    let credentials = resolve_credentials(args, runtime.managed_uid)?;
    let network = match &args.network_plan {
        Some(plan) => plan.clone(),
        None => collect_network_plan(&runtime)?,
    };
    network.validate()?;
    crate::workflows::reinit::confirm_reinit(
        &crate::interaction::interactive::confirm,
        &ReinitArgs {
            allow_no_backup: args.allow_no_backup,
            yes: args.yes,
            console_confirmed: args.console_confirmed,
        },
        &installed,
        &network,
    )?;
    crate::daemon_operations::submit_reinit_and_wait(
        context.root,
        ReinitRequest {
            from_version: context.from_version,
            state_identity: context.state_identity,
            admin_user: credentials.admin_user,
            network,
            allow_no_backup: args.allow_no_backup,
        },
        &credentials.password,
        args.console_confirmed,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn bare_console_confirmation_is_rejected_before_accessing_installation() {
        let args = Reinit {
            admin_user: None,
            password_file: None,
            interactive_password: None,
            allow_no_backup: false,
            yes: true,
            console_confirmed: true,
            network_plan: None,
            confirmed: None,
            #[cfg(feature = "test-support")]
            test_runtime: None,
        };
        assert!(matches!(
            submit(&args).await,
            Err(plan::InstallError::ParameterUsage(_))
        ));
    }
}
