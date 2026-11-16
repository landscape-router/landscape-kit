use super::*;
use crate::daemon_operations::{Record, Stage};
use crate::workflows::operation_control as control;

pub(crate) async fn run_operation(
    runtime: &InstallRuntime,
    record: &Record,
) -> Result<u8, InstallError> {
    let args = record
        .reinit
        .as_ref()
        .ok_or_else(|| InstallError::ParameterUsage("missing reinit parameters".into()))?;
    let root = crate::deployment::root::normalize_install_root(std::path::Path::new(
        &record.install_root,
    ))?;
    let installed = state::load_state(&root)?
        .ok_or_else(|| InstallError::Preflight("missing installation".into()))?;
    if installed.active_version != args.from_version
        || crate::daemon_operations::installation_identity(&installed)? != args.state_identity
    {
        return Err(InstallError::Preflight(
            "installation changed since reinit confirmation".into(),
        ));
    }
    let manager = runtime.service_manager.as_ref();
    if installed.service.manager != manager.kind()
        || !matches!(
            manager.probe(),
            crate::service::manager::Availability::Available { .. }
        )
    {
        return Err(InstallError::Preflight(
            "matching service backend unavailable".into(),
        ));
    }
    let origin = root.canonical.join("service/landscape-router.service");
    if !matches!(manager.query_registration(ManagedService::LandscapeRouter)?, crate::service::manager::SystemRegistration::Symlink { target } if target == origin)
    {
        return Err(InstallError::Preflight(
            "service registration is not owned by this installation".into(),
        ));
    }
    manager.validate_definition(
        ManagedService::LandscapeRouter,
        &std::fs::read_to_string(origin).map_err(InstallError::Io)?,
        &root.canonical,
    )?;
    if !crate::workflows::uninstall::host_network_services_masked(manager) {
        return Err(InstallError::Preflight(
            "reinit requires a network takeover installation".into(),
        ));
    }
    crate::network::takeover::preflight(runtime)?;
    crate::network::takeover::verify_interfaces(&args.network, runtime)?;
    let credentials = Credentials {
        admin_user: args.admin_user.clone(),
        password: crate::daemon_operations::reinit::take_password(record, runtime.managed_uid)?,
    };
    control::phase(Stage::Preparing)?;
    let health = runtime.health_options()?;
    let options = ReinitOptions {
        export_base_url: runtime.export_base_url.clone(),
        token: &|| {
            crate::backup::export::read_api_token(
                &root.canonical.join("data/landscape_api_token"),
                runtime.managed_uid,
            )
        },
        confirm: &|_| {
            Err(InstallError::Preflight(
                "daemon cannot read terminal input".into(),
            ))
        },
        health: &health,
    };
    let plan = ReinitArgs {
        allow_no_backup: args.allow_no_backup,
        yes: true,
        console_confirmed: true,
    };
    match reinit_installation(
        &root,
        &installed,
        manager,
        &credentials,
        &args.network,
        &plan,
        &options,
        runtime,
    )
    .await?
    {
        ReinitOutcome::Committed {
            version,
            backup_id,
            pending_network_address,
        } => {
            control::message(&crate::tr!(
                crate::keys::REINIT_NATIVE_AWAITING,
                version = version,
                address = pending_network_address
                    .map(|address| address.to_string())
                    .unwrap_or_else(|| "-".into()),
                backup = backup_id.as_deref().unwrap_or("-")
            ))?;
            Ok(0)
        }
        ReinitOutcome::RolledBack { .. } => Ok(5),
        ReinitOutcome::RollbackFailed { reason, .. } => {
            control::message(&reason)?;
            Ok(6)
        }
    }
}
