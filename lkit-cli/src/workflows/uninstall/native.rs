use super::*;
use crate::daemon_operations::Record;
use crate::deployment::runtime::InstallRuntime;

pub(crate) async fn run_operation(
    runtime: &InstallRuntime,
    record: &Record,
) -> Result<u8, InstallError> {
    let request = record
        .uninstall
        .as_ref()
        .ok_or_else(|| InstallError::ParameterUsage("missing uninstall parameters".into()))?;
    let root =
        super::super::root::normalize_install_root(std::path::Path::new(&record.install_root))?;
    let installed = super::super::state::load_state(&root)?
        .ok_or_else(|| InstallError::Preflight("missing installation".into()))?;
    if installed.active_version != request.from_version
        || crate::daemon_operations::installation_identity(&installed)? != request.state_identity
    {
        return Err(InstallError::Preflight(
            "installation state changed since uninstall confirmation".into(),
        ));
    }
    let manager = runtime.service_manager.as_ref();
    if installed.service.manager != manager.kind()
        || !matches!(
            manager.probe(),
            super::super::manager::Availability::Available { .. }
        )
    {
        return Err(InstallError::Preflight(
            "matching service backend is unavailable".into(),
        ));
    }
    let origin = root.canonical.join("service/landscape-router.service");
    if !matches!(manager.query_registration(ManagedService::LandscapeRouter)?, super::super::manager::SystemRegistration::Symlink { target } if target == origin)
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
    let health = runtime.health_options()?;
    let options = UninstallOptions {
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
    let args = UninstallArgs {
        yes: true,
        console_confirmed: true,
        allow_no_backup: request.allow_no_backup,
        keep_data: request.keep_data,
    };
    let UninstallOutcome::Committed { version, backup_id } =
        uninstall_installation(&root, &installed, manager, &args, &options).await?;
    control::message(&format!(
        "{}\n{}",
        crate::tr!(
            crate::keys::UNINSTALL_COMMITTED,
            version = version,
            backup_id = backup_id.as_deref().unwrap_or("-")
        ),
        crate::tr!(crate::keys::UNINSTALL_RETAINED_NOTE)
    ))?;
    Ok(0)
}
