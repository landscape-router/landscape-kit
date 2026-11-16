use super::*;
use crate::daemon_operations::{MigrateRequest, Record, SourceIdentity, Stage};
use crate::deployment::runtime::InstallRuntime;
use crate::workflows::operation_control as control;
use std::os::unix::fs::MetadataExt;

fn refused(message: impl Into<String>) -> InstallError {
    InstallError::ParameterUsage(message.into())
}
pub(crate) fn validate_root(root: &InstallRoot, source: &Path) -> Result<(), InstallError> {
    if super::super::state::read_state()?.is_some() {
        return Err(refused("migration requires an uninstalled Landscape"));
    }
    if source.starts_with(&root.canonical) || root.canonical.starts_with(source) {
        return Err(refused("migration source and target must not overlap"));
    }
    for name in ["data", "releases", "service", "current"] {
        match std::fs::symlink_metadata(root.canonical.join(name)) {
            Ok(_) => return Err(refused(format!("migration target contains {name}"))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(InstallError::Io(e)),
        }
    }
    Ok(())
}
fn parse_start(stat: &str) -> Result<(bool, u64), InstallError> {
    let (_, fields) = stat
        .rsplit_once(')')
        .ok_or_else(|| refused("invalid process identity"))?;
    let fields: Vec<_> = fields.split_whitespace().collect();
    let started = fields
        .get(19)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| refused("invalid process start time"))?;
    Ok((
        fields.first().is_some_and(|s| *s != "Z" && *s != "X"),
        started,
    ))
}
fn process_start(pid: u32) -> Result<(bool, u64), InstallError> {
    parse_start(&std::fs::read_to_string(format!("/proc/{pid}/stat")).map_err(InstallError::Io)?)
}
pub(super) fn identity(
    source: &Path,
    instance: &Process,
    systemd: &Systemd,
) -> Result<SourceIdentity, InstallError> {
    let instance =
        process::read_process(instance.pid).ok_or_else(|| refused("source process disappeared"))?;
    let metadata = std::fs::metadata(source).map_err(InstallError::Io)?;
    let (alive, started) = process_start(instance.pid)?;
    if !alive {
        return Err(refused("migration source process has exited"));
    }
    let units = legacy::resolve_legacy_units(systemd, source, &instance)?;
    if units.len() > 1 {
        return Err(refused("multiple services own the migration source"));
    }
    let unit = units.into_iter().next();
    let unit_sha256 = unit
        .as_ref()
        .map(|name| hash_file(Path::new(&systemd::fragment_path(systemd, name)?)).map(|v| v.0))
        .transpose()?;
    Ok(SourceIdentity {
        source: source.to_path_buf(),
        device: metadata.dev(),
        inode: metadata.ino(),
        pid: instance.pid,
        started,
        executable_sha256: instance
            .exe_sha256
            .clone()
            .ok_or_else(|| refused("cannot identify source executable"))?,
        arguments_sha256: crate::release::artifacts::hash_str(
            &serde_json::to_string(&instance.args).map_err(InstallError::StateWrite)?,
        ),
        unit,
        unit_sha256,
    })
}
pub(crate) async fn inspect(
    runtime: &InstallRuntime,
    args: &MigrateArgs,
) -> Result<MigrateRequest, InstallError> {
    let systemd = systemd::downcast(runtime.service_manager.as_ref())?;
    pipeline::require_manager(systemd)?;
    let source = validate_source_dir(&args.config_dir)?;
    let health = runtime.health_options()?;
    let process = identify_running_instance(&source, &health.ports)?;
    let source = identity(&source, &process, systemd)?;
    let token = export::read_api_token(
        &source.source.join("landscape_api_token"),
        runtime.managed_uid,
    )?;
    let exported = export::export_config(&runtime.export_base_url, &token).await?;
    let version =
        pipeline::parse_stable_version(&exported.version).map_err(|e| refused(e.to_string()))?;
    let repository = args
        .repository
        .clone()
        .map(Ok)
        .unwrap_or_else(crate::deployment::config::resolve_default_choice)?;
    repository.clone().resolve()?;
    Ok(MigrateRequest {
        source,
        version: version.to_string(),
        repository,
    })
}
pub(super) fn verify(
    args: &MigrateRequest,
    source: &Path,
    instance: &Process,
    systemd: &Systemd,
) -> Result<(), InstallError> {
    if identity(source, instance, systemd)? != args.source {
        return Err(refused(
            "migration source changed since confirmation; inspect and confirm it again",
        ));
    }
    Ok(())
}
pub(super) fn wait_for_source(
    args: &MigrateRequest,
    timeout: Duration,
) -> Result<(), InstallError> {
    control::phase(Stage::WaitingForSourceStop)?;
    control::message(&format!(
        "Waiting for source PID {} to exit; stop the old instance in another terminal",
        args.source.pid
    ))?;
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match process_start(args.source.pid) {
            Ok((alive, started)) if !alive || started != args.source.started => return Ok(()),
            Err(InstallError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
            _ => (),
        }
        if std::time::Instant::now() >= deadline {
            return Err(refused(
                "source process did not stop before the migration deadline",
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
pub(crate) async fn run_operation(
    runtime: &InstallRuntime,
    record: &Record,
) -> Result<u8, InstallError> {
    let args = record
        .migrate
        .as_ref()
        .ok_or_else(|| refused("missing migration request"))?;
    let root = crate::deployment::root::normalize_install_root(Path::new(&record.install_root))?;
    if root.canonical.to_string_lossy() != record.install_root {
        return Err(refused("migration target changed"));
    }
    validate_root(&root, &args.source.source)?;
    let health = runtime.health_options()?;
    let workflow = MigrateArgs {
        config_dir: args.source.source.clone(),
        yes: true,
        console_confirmed: true,
        repository: Some(args.repository.clone()),
    };
    let options = MigrateOptions {
        export_base_url: runtime.export_base_url.clone(),
        managed_uid: runtime.managed_uid,
        confirm: &|_| Ok(false),
        health: &health,
        probe_ports: &health.ports,
        interrupted: &|| false,
    };
    let systemd = systemd::downcast(runtime.service_manager.as_ref())?;
    let prepared = prepare_migration_bound(&root, systemd, &workflow, &options, Some(args)).await?;
    match migrate_switch_outcome_bound(&root, systemd, &workflow, &options, &prepared, Some(args))
        .await?
    {
        MigrateOutcome::Committed { version, backup_id } => {
            control::message(&format!(
                "Migrated Landscape {version}; backup {backup_id}; source directory retained"
            ))?;
            Ok(0)
        }
        MigrateOutcome::RolledBack { .. } => Ok(5),
        MigrateOutcome::Cancelled { .. } => Ok(130),
        MigrateOutcome::RollbackFailed { reason, .. } => {
            control::message(&reason)?;
            Ok(6)
        }
    }
}

pub(crate) fn recover(
    root: &InstallRoot,
    tx: &TransactionFile,
    manager: &dyn ServiceManager,
) -> Result<(), InstallError> {
    let systemd = systemd::downcast(manager)?;
    if let Some(installed) = super::super::state::load_state(root)? {
        if installed.last_transaction_id.as_deref() == Some(&tx.transaction_id)
            && Some(installed.active_version.as_str()) == tx.target_version.as_deref()
            && installed.service.manager == StateServiceManager::Systemd
        {
            return super::super::transaction::mark_phase(root, tx, Phase::Committed);
        }
        return Err(refused(
            "installation state does not belong to this migration",
        ));
    }
    if matches!(tx.phase, Phase::Preparing | Phase::Prepared) && tx.legacy_unit.is_none() {
        return super::super::transaction::mark_phase(root, tx, Phase::Failed);
    }
    super::super::transaction::mark_phase(root, tx, Phase::RollingBack)?;
    let service = ManagedService::LandscapeRouter;
    let origin = root
        .canonical
        .join("service")
        .join(manager.service_name(service));
    // An original plain-file unit may already be restored, including when the
    // daemon crashed after restoration but before recording the terminal phase.
    let original_restored = tx.legacy_unit.as_ref().is_some_and(|before| {
        before.file_path.as_deref()
            == systemd
                .system_unit_dir
                .join(manager.service_name(service))
                .to_str()
            && before.file_sha256.as_ref().is_some_and(|digest| {
                let path = Path::new(before.file_path.as_deref().unwrap_or_default());
                std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file())
                    && hash_file(path).is_ok_and(|(hash, _)| hash == *digest)
            })
    });
    if !original_restored {
        let registration = manager.query_registration(service)?;
        if registration != super::super::manager::SystemRegistration::Missing {
            if registration
                != (super::super::manager::SystemRegistration::Symlink {
                    target: origin.clone(),
                })
            {
                return Err(refused(
                    "migration recovery found an unrelated service registration",
                ));
            }
            if manager.is_active(service)? {
                manager.stop_and_wait(service, &|| {
                    manager.is_active(service).is_ok_and(|active| !active)
                })?;
            }
            manager.disable(service)?;
            manager.unregister(service, &origin)?;
        }
    }
    if let Some(path) = &tx.resolv_conf_backup {
        super::super::resolv::restore(manager.resolv_conf(), &layout::territory_relative(path))?;
    }
    let current = root.canonical.join("current");
    match std::fs::symlink_metadata(&current) {
        Ok(metadata)
            if metadata.file_type().is_symlink()
                && std::fs::read_link(&current)
                    .map_err(InstallError::Io)?
                    .to_str()
                    == tx.target_release.as_deref() =>
        {
            std::fs::remove_file(&current).map_err(InstallError::Io)?;
        }
        Ok(_) => return Err(refused("migration current link has changed")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(InstallError::Io(e)),
    }
    for name in ["data", "service", "releases"] {
        let path = root.canonical.join(name);
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_dir() => {
                std::fs::remove_dir_all(path).map_err(InstallError::Io)?
            }
            Ok(_) => return Err(refused(format!("migration {name} directory has changed"))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(InstallError::Io(e)),
        }
    }
    restore_legacy_unit(root, tx, manager)?;
    if root.canonical.exists() {
        std::fs::File::open(&root.canonical)
            .and_then(|f| f.sync_all())
            .map_err(InstallError::Io)?;
    }
    super::super::transaction::mark_phase(root, tx, Phase::RolledBack)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn migration_target_rejects_overlap_and_dangling_managed_entries() {
        let temp =
            std::env::temp_dir().join(format!("lkit-migrate-target-{}", uuid::Uuid::now_v7()));
        let _guard = layout::test_territory(&temp.join("territory"));
        let root = crate::deployment::root::normalize_install_root(&temp.join("target")).unwrap();
        assert!(validate_root(&root, &temp.join("source")).is_ok());
        assert!(validate_root(&root, &temp).is_err());
        assert!(validate_root(&root, &root.canonical.join("source")).is_err());
        std::fs::create_dir_all(&root.canonical).unwrap();
        std::os::unix::fs::symlink("missing", root.canonical.join("data")).unwrap();
        assert!(validate_root(&root, &temp.join("source")).is_err());
        std::fs::remove_dir_all(temp).unwrap();
    }
    #[test]
    fn process_identity_parses_parentheses_and_zombies() {
        let tail = (4..=21).map(|_| "0").collect::<Vec<_>>().join(" ");
        assert_eq!(
            parse_start(&format!("123 (a)b c) S {tail} 456 0")).unwrap(),
            (true, 456)
        );
        assert_eq!(
            parse_start(&format!("123 (a) Z {tail} 456")).unwrap(),
            (false, 456)
        );
        assert!(parse_start("123 (bad) S").is_err());
    }
}
