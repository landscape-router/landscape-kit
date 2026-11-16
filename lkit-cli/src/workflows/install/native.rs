use super::*;
use crate::daemon_operations::{Record, Stage};
use crate::deployment::{
    state as installation,
    transaction::{self, TransactionFile},
};
use crate::service::manager::ServiceManagerKind;
use crate::workflows::{operation_control as control, repair_snapshot};
use serde::{Deserialize, Serialize};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

fn failure(message: &str) -> InstallError {
    InstallError::Preflight(message.into())
}
fn directory(tx: &TransactionFile) -> PathBuf {
    layout::territory_transactions_dir().join(&tx.transaction_id)
}
fn remove(path: &Path) -> Result<(), InstallError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(InstallError::Io(e)),
    };
    if metadata.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
    .map_err(InstallError::Io)
}
fn copy_tree(source: &Path, target: &Path) -> Result<(), InstallError> {
    let metadata = std::fs::symlink_metadata(source).map_err(InstallError::Io)?;
    if !metadata.is_dir() {
        return Err(failure("snapshot source must be a directory"));
    }
    std::fs::create_dir_all(target).map_err(InstallError::Io)?;
    for entry in std::fs::read_dir(source).map_err(InstallError::Io)? {
        let entry = entry.map_err(InstallError::Io)?;
        let kind = entry.file_type().map_err(InstallError::Io)?;
        let output = target.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &output)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), output).map_err(InstallError::Io)?;
        } else {
            return Err(failure("snapshot contains a non-regular entry"));
        }
    }
    std::fs::set_permissions(target, metadata.permissions()).map_err(InstallError::Io)
}
pub(crate) fn validate_root(root: &InstallRoot) -> Result<(), InstallError> {
    if installation::read_state()?.is_some() {
        return Err(InstallError::ParameterUsage(
            "Landscape is already installed; uninstall it before installing again".into(),
        ));
    }
    if root.canonical.exists() {
        crate::deployment::root::verify_landscape_root_clean(root)?;
    }
    if std::fs::symlink_metadata(root.canonical.join("current")).is_ok() {
        return Err(failure(
            "installation root already contains current; recover the existing deployment first",
        ));
    }
    crate::deployment::plan::validate_applicability(
        crate::deployment::plan::StatePresence::FirstInstall,
        &root.canonical,
        crate::deployment::plan::UsageFlags {
            admin_user: true,
            password_file: true,
            ..Default::default()
        },
    )
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Preimage {
    schema_version: u32,
    transaction_id: String,
    root: String,
    backend: ServiceManagerKind,
    // Ordered data, service, target release. None means absent before installation.
    directories: Vec<Option<String>>,
}
fn targets(tx: &TransactionFile) -> Result<[&str; 3], InstallError> {
    Ok([
        "data",
        "service",
        tx.target_release
            .as_deref()
            .ok_or_else(|| failure("missing install target"))?,
    ])
}
fn snapshot(
    root: &InstallRoot,
    tx: &TransactionFile,
    backend: ServiceManagerKind,
) -> Result<(), InstallError> {
    let dir = directory(tx).join("before");
    std::fs::create_dir_all(&dir).map_err(InstallError::Io)?;
    let mut directories = vec![];
    for (index, name) in targets(tx)?.iter().enumerate() {
        let source = root.canonical.join(name);
        let saved = dir.join(index.to_string());
        match std::fs::symlink_metadata(&source) {
            Ok(meta) if meta.is_dir() => {
                copy_tree(&source, &saved)?;
                directories.push(Some(repair_snapshot::tree_digest(&saved)?));
            }
            Ok(_) => return Err(failure("install preimage must contain regular directories")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => directories.push(None),
            Err(e) => return Err(InstallError::Io(e)),
        }
    }
    repair_snapshot::sync_tree(&dir)?;
    let snapshot = Preimage {
        schema_version: 1,
        transaction_id: tx.transaction_id.clone(),
        root: tx.canonical_install_root.clone(),
        backend,
        directories,
    };
    crate::workflows::repair_service::atomic_write(
        &directory(tx).join("install-preimage.json"),
        &serde_json::to_vec(&snapshot).map_err(InstallError::StateWrite)?,
        0o600,
    )
}
fn load_preimage(
    root: &InstallRoot,
    tx: &TransactionFile,
    backend: ServiceManagerKind,
) -> Result<Preimage, InstallError> {
    let before: Preimage = serde_json::from_slice(
        &std::fs::read(directory(tx).join("install-preimage.json")).map_err(InstallError::Io)?,
    )
    .map_err(|_| failure("invalid install recovery snapshot"))?;
    if before.schema_version != 1
        || before.transaction_id != tx.transaction_id
        || before.root != root.canonical.to_string_lossy()
        || before.backend != backend
        || before.directories.len() != 3
    {
        return Err(failure("install recovery identity or backend mismatch"));
    }
    for (index, hash) in before.directories.iter().enumerate() {
        if let Some(hash) = hash
            && repair_snapshot::tree_digest(&directory(tx).join("before").join(index.to_string()))?
                != *hash
        {
            return Err(failure("install preimage corrupted"));
        }
    }
    Ok(before)
}

pub(crate) fn recover(
    root: &InstallRoot,
    tx: &TransactionFile,
    manager: &dyn ServiceManager,
) -> Result<(), InstallError> {
    if let Some(state) = installation::load_state(root)? {
        if state.last_transaction_id.as_deref() == Some(&tx.transaction_id)
            && Some(&state.active_version) == tx.target_version.as_ref()
            && state.service.manager == manager.kind()
        {
            return transaction::mark_phase(root, tx, Phase::Committed);
        }
        return Err(failure("another committed installation prevents rollback"));
    }
    if tx.phase == Phase::Preparing {
        return crate::workflows::switch::cleanup_preparing(root, tx);
    }
    let before = load_preimage(root, tx, manager.kind())?;
    let service = tx
        .systemd_before
        .as_ref()
        .ok_or_else(|| failure("install service snapshot missing"))?;
    match std::fs::symlink_metadata(root.canonical.join("current")) {
        Ok(meta) if meta.file_type().is_symlink() => {
            let target =
                std::fs::read_link(root.canonical.join("current")).map_err(InstallError::Io)?;
            if Some(target.as_path()) != tx.target_release.as_deref().map(Path::new) {
                return Err(failure("current changed outside the install transaction"));
            }
        }
        Ok(_) => return Err(failure("current is not an installation symlink")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(InstallError::Io(e)),
    }
    transaction::mark_phase(root, tx, Phase::RollingBack)?;
    if manager.is_active(ManagedService::LandscapeRouter)? {
        manager.stop_and_wait(ManagedService::LandscapeRouter, &|| {
            manager
                .is_active(ManagedService::LandscapeRouter)
                .is_ok_and(|active| !active)
        })?;
    }
    remove(&root.canonical.join("current"))?;
    for (index, name) in targets(tx)?.iter().enumerate() {
        let target = root.canonical.join(name);
        remove(&target)?;
        if before.directories[index].is_some() {
            copy_tree(
                &directory(tx).join("before").join(index.to_string()),
                &target,
            )?;
            repair_snapshot::sync_tree(&target)?;
        }
    }
    manager.restore_before(
        ManagedService::LandscapeRouter,
        service,
        &root.canonical.join("service/landscape-router.service"),
    )?;
    if let Some(path) = &tx.resolv_conf_backup {
        resolv::restore(manager.resolv_conf(), &layout::territory_relative(path))?;
    }
    if let Some(network) = &tx.network_takeover {
        crate::network::takeover::cleanup_failed_takeover(root, network, manager)?;
        remove(&layout::territory_relative(&network.pending_state))?;
    }
    remove(
        &root
            .canonical
            .join("releases")
            .join(format!(".native-install-{}.tmp", tx.transaction_id)),
    )?;
    std::fs::File::open(&root.canonical)
        .and_then(|f| f.sync_all())
        .map_err(InstallError::Io)?;
    transaction::mark_phase(root, tx, Phase::RolledBack)
}

pub(crate) async fn run_operation(
    runtime: &InstallRuntime,
    record: &Record,
) -> Result<u8, InstallError> {
    let args = record
        .install
        .as_ref()
        .ok_or_else(|| failure("missing install parameters"))?;
    let root = crate::deployment::root::normalize_install_root(Path::new(&record.install_root))?;
    if root.canonical.to_string_lossy() != record.install_root {
        return Err(failure("install root changed since submission"));
    }
    validate_root(&root)?;
    let manager = runtime.service_manager.as_ref();
    require_manager(manager)?;
    if !matches!(
        manager.query_registration(ManagedService::LandscapeRouter)?,
        crate::service::manager::SystemRegistration::Missing
    ) {
        return Err(failure(
            "existing Landscape service registration prevents first install",
        ));
    }
    crate::commands::manage::check_host_conflicts(
        crate::deployment::plan::StatePresence::FirstInstall,
        &root.canonical,
        &None,
        runtime,
        false,
        args.network.is_some(),
    )?;
    run_preflight(
        &root.canonical,
        None,
        false,
        runtime,
        args.network.is_some(),
    )?;
    if let Some(network) = &args.network {
        crate::network::takeover::preflight(runtime)?;
        crate::network::takeover::verify_interfaces(network, runtime)?;
        ensure_network_takeover_data_empty(&root)?;
    }
    let password = crate::daemon_operations::reinit::take_password(record, runtime.managed_uid)?;
    let credentials = Credentials {
        admin_user: args.admin_user.clone(),
        password,
    };
    let source = args.repository.clone().resolve()?;
    let provider = crate::release::repository::provider_for(source.kind, &source.location)?;
    let architecture = Architecture::host().ok_or_else(|| failure("unsupported architecture"))?;
    let release = match TargetVersion::parse(&args.target)? {
        TargetVersion::Latest => provider
            .latest(architecture)
            .await?
            .ok_or(InstallError::NoStableVersion)?,
        TargetVersion::Version(version) => provider.release(&version, architecture).await?,
    };
    control::resolved_version(&release.version.to_string())?;
    let mut tx = TransactionFile::new_install(&root, &release.version)?;
    tx.schema_version = 12;
    control::associate(&tx)?;
    transaction::begin(&root, &tx)?;
    std::fs::create_dir_all(directory(&tx)).map_err(InstallError::Io)?;
    std::fs::set_permissions(directory(&tx), std::fs::Permissions::from_mode(0o700))
        .map_err(InstallError::Io)?;
    let result: Result<bool, InstallError> = async {
        let built =
            control::download(crate::workflows::switch::build_staged(&root, &tx, &release)).await?;
        control::phase(Stage::Preparing)?;
        snapshot(&root, &tx, manager.kind())?;
        tx.systemd_before = Some(capture_before(manager, ManagedService::LandscapeRouter)?);
        let backup = format!("backups/{}/host/resolv.conf", tx.transaction_id);
        resolv::backup(manager.resolv_conf(), &layout::territory_relative(&backup))?;
        tx.resolv_conf_backup = Some(backup);
        if let Some(network) = &args.network {
            tx.network_takeover = Some(crate::network::takeover::prepare_transaction(
                &tx.transaction_id,
                network,
                runtime,
            )?);
        }
        transaction::persist(&root, &tx)?;
        transaction::mark_phase(&root, &tx, Phase::Prepared)?;
        let staged = directory(&tx).join("target");
        let staged_root = InstallRoot {
            canonical: staged.clone(),
            install_root: staged,
        };
        let init = build_init_config(
            &staged_root,
            &release.version,
            &credentials,
            args.network.as_ref(),
        )?;
        let source = staged_root
            .canonical
            .join(tx.target_release.as_ref().unwrap());
        let target = root.canonical.join(tx.target_release.as_ref().unwrap());
        let temp = root
            .canonical
            .join("releases")
            .join(format!(".native-install-{}.tmp", tx.transaction_id));
        copy_tree(&source, &temp)?;
        repair_snapshot::sync_tree(&temp)?;
        remove(&target)?;
        std::fs::rename(&temp, &target).map_err(InstallError::Io)?;
        write_init_config(&root, &init)?;
        let unit_sha = write_unit_origin(
            &root,
            manager,
            ManagedService::LandscapeRouter,
            &manager.render_definition(ManagedService::LandscapeRouter, &root.canonical)?,
        )?;
        if let Some(network) = &tx.network_takeover {
            crate::network::takeover::arm_recovery(&root, network, runtime)?;
            transaction::mark_phase(&root, &tx, Phase::Stopping)?;
            control::phase(Stage::Stopping)?;
            crate::network::takeover::stop_host_services(network, manager)?;
            crate::network::takeover::clear_selected_lan_addresses(
                &network.plan,
                &runtime.ip_command,
            )?;
        }
        transaction::mark_phase(&root, &tx, Phase::Activating)?;
        control::phase(Stage::Activating)?;
        activate_current(&root, &release.version)?;
        #[cfg(feature = "test-support")]
        control::checkpoint("install-activated")?;
        manager.register(
            ManagedService::LandscapeRouter,
            &root.canonical.join("service/landscape-router.service"),
        )?;
        manager.enable(ManagedService::LandscapeRouter)?;
        manager.start(ManagedService::LandscapeRouter)?;
        transaction::mark_phase(&root, &tx, Phase::Verifying)?;
        control::phase(Stage::Verifying)?;
        let expected_pid = manager.main_pid(ManagedService::LandscapeRouter)?;
        if expected_pid == 0 {
            return Err(failure("installation did not start Landscape"));
        }
        let health = runtime.health_options()?;
        let data = root.canonical.join("data");
        let options = health::StartupOptions {
            ports: &health.ports,
            expected_pid,
            docs: &health.docs,
            unit_state: Some(&|| manager.active_state(ManagedService::LandscapeRouter).ok()),
            init_required: true,
            data_dir: &data,
            startup_timeout: health.startup_timeout,
            stable_duration: health.stable_duration,
        };
        health::wait_for_startup(&options).await?;
        health::observe_stable(&options).await?;
        let activation = UnitActivation {
            unit_sha: unit_sha.clone(),
            initialization: InitializationState {
                status: InitStatus::Complete,
                lock_present: true,
                initialized_at: Some(Utc::now()),
            },
            service: ServiceState {
                manager: manager.kind(),
                registered: true,
                enabled: true,
                verified: true,
                definition_path: Some("service/landscape-router.service".into()),
                definition_sha256: Some(unit_sha),
            },
        };
        let mut installed = build_state(&root, &release, architecture, &built, &activation);
        installed.last_transaction_id = Some(tx.transaction_id.clone());
        if let Some(network) = &tx.network_takeover {
            crate::network::takeover::write_pending_state(&root, network, &installed)?;
            transaction::mark_phase(&root, &tx, Phase::AwaitingNetworkConfirmation)?;
            #[cfg(feature = "test-support")]
            control::checkpoint("install-awaiting")?;
            Ok(true)
        } else {
            installation::write_state(&root, &installed)?;
            #[cfg(feature = "test-support")]
            control::checkpoint("install-state-written")?;
            transaction::mark_phase(&root, &tx, Phase::Committed)?;
            Ok(false)
        }
    }
    .await;
    match result {
        Ok(pending) => {
            control::message(&format!(
                "Landscape {} {}; {}",
                release.version,
                if pending {
                    "awaits network confirmation"
                } else {
                    "installed"
                },
                if pending {
                    "run lkit network confirm or lkit network rollback before the deadline"
                } else {
                    "flare recovery channel remains available"
                }
            ))?;
            Ok(0)
        }
        Err(error) => {
            let stored = transaction::find_unfinished(&root)?;
            if let Some(tx) = stored {
                recover(&root, &tx, manager)?;
            }
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn install_preimages_preserve_cached_files_and_modes_and_reject_corruption() {
        let temp =
            std::env::temp_dir().join(format!("lkit-install-preimage-{}", uuid::Uuid::now_v7()));
        let _guard = layout::test_territory(&temp.join("territory"));
        let root =
            crate::deployment::root::normalize_install_root(&temp.join("landscape")).unwrap();
        let data = root.canonical.join("data/private");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(data.join("marker"), b"original").unwrap();
        let mut tx = TransactionFile::new_install(&root, &semver::Version::new(1, 0, 0)).unwrap();
        tx.schema_version = 12;
        let cache = root.canonical.join("releases/1.0.0");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("static.zip"), b"cached").unwrap();
        snapshot(&root, &tx, ServiceManagerKind::Openrc).unwrap();
        let before = load_preimage(&root, &tx, ServiceManagerKind::Openrc).unwrap();
        assert!(before.directories[0].is_some());
        assert!(before.directories[1].is_none());
        assert!(before.directories[2].is_some());
        assert_eq!(
            std::fs::metadata(directory(&tx).join("before/0/private"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert!(load_preimage(&root, &tx, ServiceManagerKind::Systemd).is_err());
        std::fs::write(directory(&tx).join("before/2/static.zip"), b"corrupted").unwrap();
        assert!(load_preimage(&root, &tx, ServiceManagerKind::Openrc).is_err());
        assert_eq!(std::fs::read(cache.join("static.zip")).unwrap(), b"cached");
        std::fs::remove_dir_all(temp).unwrap();
    }
    #[test]
    fn root_with_current_is_never_treated_as_a_clean_installation() {
        let temp = std::env::temp_dir().join(format!("lkit-install-root-{}", uuid::Uuid::now_v7()));
        let _guard = layout::test_territory(&temp.join("territory"));
        let root =
            crate::deployment::root::normalize_install_root(&temp.join("landscape")).unwrap();
        assert!(validate_root(&root).is_ok());
        std::fs::create_dir_all(&root.canonical).unwrap();
        std::os::unix::fs::symlink("releases/missing", root.canonical.join("current")).unwrap();
        assert!(validate_root(&root).is_err());
        std::fs::remove_dir_all(temp).unwrap();
    }
}
