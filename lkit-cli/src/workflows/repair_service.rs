//! Transactional service definition migration, without restarting Landscape.
use crate::deployment::plan::InstallError;
use crate::deployment::{layout, root::InstallRoot, runtime::InstallRuntime, state, transaction};
use crate::release::artifacts::hash_str;
use crate::service::manager::{
    self, ManagedService, ServiceBefore, ServiceManager, ServiceManagerKind, SystemRegistration,
};
use serde::{Deserialize, Serialize};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use transaction::{Operation, Phase, TransactionFile};

#[derive(Serialize, Deserialize)]
struct Snapshot {
    version: u32,
    backend: ServiceManagerKind,
    original: String,
    replacement: String,
    permissions: u32,
    before: ServiceBefore,
    startup_links: Option<String>,
    state: state::InstallState,
}

fn failure(message: &str) -> InstallError {
    InstallError::Preflight(message.into())
}
fn definition(root: &InstallRoot) -> PathBuf {
    root.canonical.join("service/landscape-router.service")
}
fn snapshot_path(tx: &TransactionFile) -> PathBuf {
    layout::territory_transactions_dir()
        .join(&tx.transaction_id)
        .join("service-repair.json")
}

#[cfg(feature = "test-support")]
pub(crate) fn run(runtime: &InstallRuntime) -> Result<(), InstallError> {
    if !runtime.allow_non_root && unsafe { libc::geteuid() } != 0 {
        return Err(failure("service repair requires root"));
    }
    let _lock = crate::deployment::lock::acquire_install_lock()?;
    run_locked(runtime, &mut |_| Ok(()))
}

/// The caller owns the installation lock for the entire operation.
pub(crate) fn run_locked(
    runtime: &InstallRuntime,
    associate: &mut dyn FnMut(&TransactionFile) -> Result<(), InstallError>,
) -> Result<(), InstallError> {
    let root =
        state::discover_landscape_root()?.ok_or_else(|| failure("no Landscape installation"))?;
    if transaction::find_unfinished(&root)?.is_some() {
        return Err(failure(
            "finish the pending transaction before repairing the service",
        ));
    }
    if runtime.test_runtime_path.is_none() {
        crate::daemon::require_capability("service-repair-v1.pid")?;
    }
    let manager = runtime.service_manager.as_ref();
    if !matches!(manager.probe(), manager::Availability::Available { .. }) {
        return Err(failure("service manager is unavailable"));
    }
    repair_associated(&root, manager, associate)
}

#[cfg(test)]
fn repair(root: &InstallRoot, manager: &dyn ServiceManager) -> Result<(), InstallError> {
    repair_associated(root, manager, &mut |_| Ok(()))
}

fn repair_associated(
    root: &InstallRoot,
    manager: &dyn ServiceManager,
    associate: &mut dyn FnMut(&TransactionFile) -> Result<(), InstallError>,
) -> Result<(), InstallError> {
    let installed =
        state::load_state(root)?.ok_or_else(|| failure("missing committed installation state"))?;
    if installed.service.manager != manager.kind() {
        return Err(failure("service backend does not match installation"));
    }
    let path = definition(root);
    let original = std::fs::read_to_string(&path).map_err(InstallError::Io)?;
    let before = manager::capture_before(manager, ManagedService::LandscapeRouter)?;
    check_registration(manager, &path)?;
    if installed.service.definition_sha256.as_deref() != Some(&hash_str(&original)) {
        return Err(failure(
            "service definition differs from its recorded identity; refusing to overwrite",
        ));
    }
    let replacement =
        manager.render_definition(ManagedService::LandscapeRouter, &root.canonical)?;
    manager.validate_definition(
        ManagedService::LandscapeRouter,
        &replacement,
        &root.canonical,
    )?;
    let permissions = std::fs::symlink_metadata(&path).map_err(InstallError::Io)?;
    if !permissions.file_type().is_file() {
        return Err(failure("service definition must be a regular file"));
    }
    if original == replacement {
        return Ok(());
    }
    let mut tx = TransactionFile::new_install(
        root,
        &semver::Version::parse(&installed.active_version)
            .map_err(|_| failure("invalid active version"))?,
    )?;
    tx.schema_version = 6;
    tx.operation = Operation::RepairService;
    tx.target_release = None;
    tx.systemd_before = Some(before.clone());
    associate(&tx)?;
    transaction::begin(root, &tx)?;
    let snapshot = Snapshot {
        version: 1,
        backend: manager.kind(),
        original,
        replacement,
        permissions: permissions.permissions().mode() & 0o7777,
        before,
        startup_links: manager.startup_links(ManagedService::LandscapeRouter)?,
        state: installed,
    };
    let result = (|| {
        write_snapshot(&tx, &snapshot)?;
        transaction::mark_phase(root, &tx, Phase::Prepared)?;
        transaction::mark_phase(root, &tx, Phase::Activating)?;
        atomic_write(&path, snapshot.replacement.as_bytes(), snapshot.permissions)?;
        manager.refresh()?;
        verify(root, &snapshot, manager, true)?;
        transaction::mark_phase(root, &tx, Phase::Finalizing)?;
        tx.phase = Phase::Finalizing;
        commit(root, &tx, &snapshot)
    })();
    if let Err(error) = result {
        // Reload the durable phase: a failed phase write must not choose the
        // opposite recovery direction from a successfully persisted commit.
        if let Some(pending) = transaction::find_unfinished(root)? {
            recover(root, &pending, manager)?;
        }
        return Err(error);
    }
    Ok(())
}

fn check_registration(manager: &dyn ServiceManager, path: &Path) -> Result<(), InstallError> {
    if !matches!(manager.query_registration(ManagedService::LandscapeRouter)?, SystemRegistration::Symlink {target} if target == path)
    {
        return Err(failure(
            "service registration is missing or points to a foreign definition",
        ));
    }
    Ok(())
}

pub(crate) fn atomic_write(path: &Path, bytes: &[u8], mode: u32) -> Result<(), InstallError> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::now_v7()));
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(mode)
        .open(&temporary)
        .map_err(InstallError::Io)?;
    file.write_all(bytes).map_err(InstallError::Io)?;
    file.set_permissions(std::fs::Permissions::from_mode(mode))
        .map_err(InstallError::Io)?;
    file.sync_all().map_err(InstallError::Io)?;
    std::fs::rename(&temporary, path).map_err(InstallError::Io)?;
    std::fs::File::open(path.parent().unwrap())
        .and_then(|directory| directory.sync_all())
        .map_err(InstallError::Io)
}
fn write_snapshot(tx: &TransactionFile, snapshot: &Snapshot) -> Result<(), InstallError> {
    let path = snapshot_path(tx);
    std::fs::create_dir_all(path.parent().unwrap()).map_err(InstallError::Io)?;
    atomic_write(
        &path,
        &serde_json::to_vec(snapshot).map_err(InstallError::StateWrite)?,
        0o600,
    )
}
fn verify(
    root: &InstallRoot,
    snapshot: &Snapshot,
    manager: &dyn ServiceManager,
    target: bool,
) -> Result<(), InstallError> {
    check_registration(manager, &definition(root))?;
    let expected = if target {
        &snapshot.replacement
    } else {
        &snapshot.original
    };
    if std::fs::read_to_string(definition(root)).map_err(InstallError::Io)? != *expected {
        return Err(failure("service definition verification failed"));
    }
    if manager.startup_links(ManagedService::LandscapeRouter)? != snapshot.startup_links {
        return Err(failure("service startup links changed during repair"));
    }
    if manager.is_enabled(ManagedService::LandscapeRouter)? != snapshot.before.enabled
        || manager.is_active(ManagedService::LandscapeRouter)? != snapshot.before.active
    {
        return Err(failure("service state changed during definition repair"));
    }
    Ok(())
}
fn commit(
    root: &InstallRoot,
    tx: &TransactionFile,
    snapshot: &Snapshot,
) -> Result<(), InstallError> {
    let mut installed = snapshot.state.clone();
    installed.service.definition_sha256 = Some(hash_str(&snapshot.replacement));
    installed.last_transaction_id = Some(tx.transaction_id.clone());
    installed.committed_at = Some(chrono::Utc::now());
    state::write_state(root, &installed)?;
    transaction::mark_phase(root, tx, Phase::Committed)
}

pub(crate) fn recover(
    root: &InstallRoot,
    tx: &TransactionFile,
    manager: &dyn ServiceManager,
) -> Result<(), InstallError> {
    if tx.phase == Phase::Preparing {
        // The definition cannot have changed before Prepared is persisted.
        return transaction::mark_phase(root, tx, Phase::Failed);
    }
    let snapshot: Snapshot =
        serde_json::from_slice(&std::fs::read(snapshot_path(tx)).map_err(InstallError::Io)?)
            .map_err(|e| InstallError::CorruptedTransaction(e.to_string()))?;
    if snapshot.version != 1
        || snapshot.backend != manager.kind()
        || snapshot.state.canonical_install_root != root.canonical.to_string_lossy()
        || snapshot.state.service.definition_sha256.as_deref()
            != Some(&hash_str(&snapshot.original))
    {
        return Err(failure("invalid service repair snapshot"));
    }
    state::validate_state(&snapshot.state)?;
    check_registration(manager, &definition(root))?;
    if !std::fs::symlink_metadata(definition(root))
        .map_err(InstallError::Io)?
        .file_type()
        .is_file()
    {
        return Err(failure("service definition is no longer a regular file"));
    }
    if tx.systemd_before.as_ref() != Some(&snapshot.before) {
        return Err(failure(
            "service repair snapshot does not match transaction",
        ));
    }
    let current = std::fs::read_to_string(definition(root)).map_err(InstallError::Io)?;
    if current != snapshot.original && current != snapshot.replacement {
        return Err(failure(
            "service definition changed externally; refusing recovery overwrite",
        ));
    }
    if tx.phase == Phase::Finalizing {
        manager.validate_definition(
            ManagedService::LandscapeRouter,
            &snapshot.replacement,
            &root.canonical,
        )?;
        manager.refresh()?;
        verify(root, &snapshot, manager, true)?;
        return commit(root, tx, &snapshot);
    }
    transaction::mark_phase(root, tx, Phase::RollingBack)?;
    atomic_write(
        &definition(root),
        snapshot.original.as_bytes(),
        snapshot.permissions,
    )?;
    manager.refresh()?;
    verify(root, &snapshot, manager, false)?;
    state::write_state(root, &snapshot.state)?;
    transaction::mark_phase(root, tx, Phase::RolledBack)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Fake {
        path: PathBuf,
        backend: ServiceManagerKind,
        running: std::sync::atomic::AtomicBool,
        enabled: bool,
        failures: AtomicUsize,
        stops: AtomicUsize,
        starts: AtomicUsize,
        registrations: AtomicUsize,
    }
    impl ServiceManager for Fake {
        fn kind(&self) -> ServiceManagerKind {
            self.backend
        }
        fn probe(&self) -> manager::Availability {
            manager::Availability::Available {
                version: "fake".into(),
            }
        }
        fn service_name(&self, _: ManagedService) -> &str {
            "landscape-router.service"
        }
        fn render_definition(&self, _: ManagedService, _: &Path) -> Result<String, InstallError> {
            Ok("new startup-check".into())
        }
        fn validate_definition(
            &self,
            _: ManagedService,
            text: &str,
            _: &Path,
        ) -> Result<(), InstallError> {
            if text == "new startup-check" {
                Ok(())
            } else {
                Err(failure("invalid definition"))
            }
        }
        fn query_registration(
            &self,
            _: ManagedService,
        ) -> Result<SystemRegistration, InstallError> {
            Ok(SystemRegistration::Symlink {
                target: self.path.clone(),
            })
        }
        fn register(&self, _: ManagedService, _: &Path) -> Result<(), InstallError> {
            panic!("must not change registration")
        }
        fn unregister(&self, _: ManagedService, _: &Path) -> Result<(), InstallError> {
            panic!("must not unregister")
        }
        fn is_enabled(&self, _: ManagedService) -> Result<bool, InstallError> {
            Ok(self.enabled)
        }
        fn enable(&self, _: ManagedService) -> Result<(), InstallError> {
            panic!("must not enable")
        }
        fn disable(&self, _: ManagedService) -> Result<(), InstallError> {
            panic!("must not disable")
        }
        fn is_active(&self, _: ManagedService) -> Result<bool, InstallError> {
            Ok(self.running.load(Ordering::SeqCst))
        }
        fn active_state(&self, _: ManagedService) -> Result<String, InstallError> {
            Ok(if self.running.load(Ordering::SeqCst) {
                "active"
            } else {
                "inactive"
            }
            .into())
        }
        fn start(&self, _: ManagedService) -> Result<(), InstallError> {
            self.starts.fetch_add(1, Ordering::SeqCst);
            self.running.store(true, Ordering::SeqCst);
            Ok(())
        }
        fn stop(&self, _: ManagedService) -> Result<(), InstallError> {
            self.running.store(false, Ordering::SeqCst);
            self.stops.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn restart(&self, _: ManagedService) -> Result<(), InstallError> {
            panic!("must not restart")
        }
        fn main_pid(&self, _: ManagedService) -> Result<u32, InstallError> {
            Ok(std::process::id())
        }
        fn restore_registration(
            &self,
            _: ManagedService,
            _: &ServiceBefore,
            _: &Path,
        ) -> Result<(), InstallError> {
            self.registrations.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn resolv_conf(&self) -> &Path {
            &self.path
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn refresh(&self) -> Result<(), InstallError> {
            if self
                .failures
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok()
            {
                Err(failure("injected reload failure"))
            } else {
                Ok(())
            }
        }
    }
    fn setup(
        path: &Path,
        backend: ServiceManagerKind,
        active: bool,
        enabled: bool,
    ) -> (InstallRoot, Fake) {
        std::fs::create_dir_all(path.join("service")).unwrap();
        std::fs::create_dir_all(path.join("releases/1.0.0")).unwrap();
        std::os::unix::fs::symlink("releases/1.0.0", path.join("current")).unwrap();
        let root = crate::deployment::root::normalize_install_root(path).unwrap();
        atomic_write(&definition(&root), b"old definition", 0o750).unwrap();
        let installed: state::InstallState = serde_json::from_value(serde_json::json!({
            "schema_version":1, "layout_version":2, "install_root":path, "canonical_install_root":path,
            "active_version":"1.0.0", "assets":{"webserver":{"architecture":"x86_64","sha256":"a".repeat(64),"size":1},"static_archive":{"sha256":"b".repeat(64),"size":1}},
            "initialization":{"status":"complete","lock_present":true,"initialized_at":chrono::Utc::now()},
            "service":{"manager":backend,"registered":true,"enabled":enabled,"verified":true,"definition_path":"service/landscape-router.service","definition_sha256":hash_str("old definition")},
            "last_transaction_id":null,"committed_at":chrono::Utc::now()
        })).unwrap();
        state::write_state(&root, &installed).unwrap();
        let manager = Fake {
            path: definition(&root),
            backend,
            running: std::sync::atomic::AtomicBool::new(active),
            enabled,
            failures: AtomicUsize::new(0),
            stops: AtomicUsize::new(0),
            starts: AtomicUsize::new(0),
            registrations: AtomicUsize::new(0),
        };
        (root, manager)
    }
    #[test]
    fn association_must_succeed_before_transaction_or_service_mutation() {
        let dir =
            std::env::temp_dir().join(format!("lkit-service-associate-{}", uuid::Uuid::now_v7()));
        let _guard = layout::test_territory(&dir.join("territory"));
        let (root, manager) = setup(
            &dir.join("landscape"),
            ServiceManagerKind::Systemd,
            false,
            false,
        );
        let mut calls = 0;
        let result = repair_associated(&root, &manager, &mut |tx| {
            calls += 1;
            assert!(
                !layout::territory_transactions_dir()
                    .join(format!("{}.json", tx.transaction_id))
                    .exists()
            );
            assert_eq!(
                std::fs::read_to_string(&manager.path).unwrap(),
                "old definition"
            );
            Err(failure("operation association could not be persisted"))
        });
        assert!(result.is_err());
        assert_eq!(calls, 1);
        assert!(transaction::find_unfinished(&root).unwrap().is_none());
        assert_eq!(
            std::fs::read_to_string(&manager.path).unwrap(),
            "old definition"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn migrates_all_backends_without_changing_runtime_and_is_idempotent() {
        for backend in ServiceManagerKind::supported() {
            for (active, enabled) in [(true, true), (true, false), (false, true), (false, false)] {
                let dir = std::env::temp_dir()
                    .join(format!("lkit-service-repair-{}", uuid::Uuid::now_v7()));
                let _guard = layout::test_territory(&dir.join("territory"));
                let (root, manager) = setup(&dir.join("landscape"), backend, active, enabled);
                repair(&root, &manager).unwrap();
                assert_eq!(
                    std::fs::read_to_string(&manager.path).unwrap(),
                    "new startup-check"
                );
                assert_eq!(
                    std::fs::metadata(&manager.path)
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o777,
                    0o750
                );
                let installed = state::load_state(&root).unwrap().unwrap();
                repair(&root, &manager).unwrap();
                assert_eq!(state::load_state(&root).unwrap().unwrap(), installed);
                assert!(transaction::find_unfinished(&root).unwrap().is_none());
                assert_eq!(manager.stops.load(Ordering::SeqCst), 0);
                assert_eq!(manager.starts.load(Ordering::SeqCst), 0);
                assert_eq!(manager.registrations.load(Ordering::SeqCst), 0);
                std::fs::remove_dir_all(dir).unwrap();
            }
        }
    }
    #[test]
    fn reload_failure_rolls_back_and_failed_rollback_remains_retryable() {
        let dir = std::env::temp_dir().join(format!("lkit-service-retry-{}", uuid::Uuid::now_v7()));
        let _guard = layout::test_territory(&dir.join("territory"));
        let (root, manager) = setup(
            &dir.join("landscape"),
            ServiceManagerKind::Systemd,
            false,
            false,
        );
        manager.failures.store(2, Ordering::SeqCst);
        assert!(repair(&root, &manager).is_err());
        let pending = transaction::find_unfinished(&root).unwrap().unwrap();
        assert_eq!(pending.phase, Phase::RollingBack);
        recover(&root, &pending, &manager).unwrap();
        assert_eq!(
            std::fs::read_to_string(&manager.path).unwrap(),
            "old definition"
        );
        assert!(transaction::find_unfinished(&root).unwrap().is_none());
        std::fs::write(&manager.path, "user modified").unwrap();
        assert!(repair(&root, &manager).is_err());
        assert_eq!(
            std::fs::read_to_string(&manager.path).unwrap(),
            "user modified"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn interrupted_repair_rolls_back_before_finalizing_and_commits_afterwards() {
        for phase in [
            Phase::Prepared,
            Phase::Activating,
            Phase::Verifying,
            Phase::Finalizing,
        ] {
            let dir = std::env::temp_dir().join(format!(
                "lkit-service-interruption-{}",
                uuid::Uuid::now_v7()
            ));
            let _guard = layout::test_territory(&dir.join("territory"));
            let (root, manager) = setup(
                &dir.join("landscape"),
                ServiceManagerKind::Openrc,
                true,
                false,
            );
            let installed = state::load_state(&root).unwrap().unwrap();
            let mut tx =
                TransactionFile::new_install(&root, &semver::Version::new(1, 0, 0)).unwrap();
            tx.schema_version = 6;
            tx.operation = Operation::RepairService;
            tx.target_release = None;
            tx.systemd_before =
                Some(manager::capture_before(&manager, ManagedService::LandscapeRouter).unwrap());
            transaction::begin(&root, &tx).unwrap();
            let snapshot = Snapshot {
                version: 1,
                backend: manager.backend,
                original: "old definition".into(),
                replacement: "new startup-check".into(),
                permissions: 0o750,
                before: tx.systemd_before.clone().unwrap(),
                startup_links: None,
                state: installed.clone(),
            };
            write_snapshot(&tx, &snapshot).unwrap();
            tx.phase = phase;
            transaction::persist(&root, &tx).unwrap();
            atomic_write(&manager.path, b"new startup-check", 0o750).unwrap();
            recover(&root, &tx, &manager).unwrap();
            assert!(transaction::find_unfinished(&root).unwrap().is_none());
            let expected = if phase == Phase::Finalizing {
                "new startup-check"
            } else {
                "old definition"
            };
            assert_eq!(std::fs::read_to_string(&manager.path).unwrap(), expected);
            assert_eq!(
                state::load_state(&root)
                    .unwrap()
                    .unwrap()
                    .service
                    .definition_sha256,
                Some(hash_str(expected))
            );
            std::fs::remove_dir_all(dir).unwrap();
        }
    }
    #[tokio::test]
    async fn reinit_rollback_restores_running_state_and_skips_online_checks_when_stopped() {
        struct Probe(bool);
        impl crate::service::health::DocsProbe for Probe {
            async fn docs_ok(&self) -> bool {
                assert!(self.0, "stopped service must not receive health probes");
                true
            }
        }
        for backend in ServiceManagerKind::supported() {
            for (active, enabled) in [(false, false), (false, true), (true, false), (true, true)] {
                let dir = std::env::temp_dir()
                    .join(format!("lkit-stopped-reinit-{}", uuid::Uuid::now_v7()));
                let _guard = layout::test_territory(&dir.join("territory"));
                let (root, manager) = setup(&dir.join("landscape"), backend, active, enabled);
                let installed = state::load_state(&root).unwrap().unwrap();
                let mut tx =
                    TransactionFile::new_reinit(&root, &semver::Version::new(1, 0, 0)).unwrap();
                tx.no_backup = !active;
                tx.systemd_before = Some(
                    manager::capture_before(&manager, ManagedService::LandscapeRouter).unwrap(),
                );
                transaction::begin(&root, &tx).unwrap();
                crate::backup::rollback::write_state_snapshot(
                    &root,
                    &tx.transaction_id,
                    &installed,
                )
                .unwrap();
                let previous = layout::territory_transactions_dir()
                    .join(&tx.transaction_id)
                    .join("previous-data");
                std::fs::create_dir_all(&previous).unwrap();
                std::fs::write(previous.join("original"), "retained").unwrap();
                std::fs::write(previous.join("landscape_init.lock"), "").unwrap();
                std::fs::write(previous.join("landscape.toml"), "").unwrap();
                std::fs::create_dir_all(root.canonical.join("data")).unwrap();
                std::fs::write(root.canonical.join("data/target"), "discarded").unwrap();
                let health = crate::service::health::HealthOptions {
                    docs: Probe(active),
                    ports: vec![],
                    startup_timeout: std::time::Duration::from_secs(1),
                    stable_duration: std::time::Duration::ZERO,
                };
                for _ in 0..2 {
                    crate::workflows::reinit::rollback_reinit_inner(&root, &tx, &manager, &health)
                        .await
                        .unwrap();
                    assert_eq!(manager.running.load(Ordering::SeqCst), active);
                    assert!(root.canonical.join("data/original").exists());
                    assert!(!root.canonical.join("data/target").exists());
                    assert_eq!(
                        state::load_state(&root).unwrap().unwrap().service.enabled,
                        enabled
                    );
                }
                std::fs::remove_dir_all(dir).unwrap();
            }
        }
    }
}
