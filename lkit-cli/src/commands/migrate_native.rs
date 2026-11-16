use super::*;
async fn submit(args: &Migrate) -> Result<u8, plan::InstallError> {
    if state::read_state()?.is_some() {
        return Err(plan::InstallError::ParameterUsage(
            "migration requires an uninstalled Landscape".into(),
        ));
    }
    if args.resume_transaction.is_some() || args.console_confirmed {
        return Err(plan::InstallError::ParameterUsage(
            "migration requires a freshly confirmed source; internal resume flags are unsupported"
                .into(),
        ));
    }
    let runtime = resolve_runtime(args)?;
    if !runtime.allow_non_root && unsafe { libc::geteuid() } != 0 {
        return Err(plan::InstallError::UnsupportedPlatform(
            "migration requires root".into(),
        ));
    }
    crate::daemon::require_capability("migrate-v1.pid")?;
    let selected = plan::select_install_root(
        args.install_dir.as_deref(),
        std::env::var("LKIT_INSTALL_DIR").ok().as_deref(),
    )?;
    let root = root::normalize_install_root(&selected)?;
    if transaction::find_unfinished(&root)?.is_some() {
        return Err(plan::InstallError::ParameterUsage(
            "wait for daemon transaction recovery before migrating".into(),
        ));
    }
    let workflow = MigrateArgs {
        config_dir: args.from.clone(),
        yes: args.yes,
        console_confirmed: false,
        repository: super::super::manage::repository_override(&args.repository),
        resume_transaction: None,
    };
    let request = crate::workflows::migrate::native::inspect(&runtime, &workflow).await?;
    crate::workflows::migrate::native::validate_root(&root, &request.source.source)?;
    let health = runtime.health_options()?;
    let options = MigrateOptions {
        export_base_url: runtime.export_base_url.clone(),
        managed_uid: runtime.managed_uid,
        confirm: &|prompt| crate::interaction::interactive::confirm(prompt),
        health: &health,
        probe_ports: &health.ports,
        interrupted: &|| false,
    };
    eprintln!(
        "migrate: source {} -> {}; old service: {}",
        request.source.source.display(),
        root.canonical.display(),
        request
            .source
            .unit
            .as_deref()
            .unwrap_or("foreground process; manual stop required")
    );
    crate::workflows::migrate::confirm_migrate(
        &options,
        &workflow,
        &request.source.source,
        &semver::Version::parse(&request.version)
            .map_err(|e| plan::InstallError::ParameterUsage(e.to_string()))?,
        runtime.service_manager.kind(),
    )?;
    crate::daemon_operations::submit_migrate_and_wait(
        root.canonical.to_string_lossy().into_owned(),
        request,
    )
    .await
}
pub(super) async fn run(args: &Migrate) -> ExitCode {
    match submit(args).await {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("migrate: {error}");
            exit_code(&error)
        }
    }
}
