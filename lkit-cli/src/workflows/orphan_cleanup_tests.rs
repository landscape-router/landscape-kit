//! Pure caller-contract tests shared by uninstall and migration recovery.
use crate::deployment::plan::InstallError;
use crate::service::manager::{
    Availability, ManagedService, ServiceBefore, ServiceManager, ServiceManagerKind,
    SystemRegistration,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Fake {
    directory: PathBuf,
    origin: PathBuf,
    orphan: PathBuf,
    registration: SystemRegistration,
    unregister_calls: AtomicUsize,
}

impl Fake {
    fn new() -> Self {
        let directory =
            std::env::temp_dir().join(format!("lkit-orphan-cleanup-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&directory).unwrap();
        let origin = directory.join("installation/service/landscape-router.service");
        let orphan = directory.join("boot-copy");
        std::fs::write(&orphan, b"owned").unwrap();
        Self {
            directory,
            origin,
            orphan,
            registration: SystemRegistration::Missing,
            unregister_calls: AtomicUsize::new(0),
        }
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

macro_rules! forbidden {
    ($name:ident($($argument:ident: $kind:ty),*) -> $result:ty) => {
        fn $name(&self, $(_: $kind),*) -> $result {
            panic!("orphan cleanup must not call {}", stringify!($name))
        }
    };
}

impl ServiceManager for Fake {
    forbidden!(kind() -> ServiceManagerKind);
    forbidden!(probe() -> Availability);
    fn service_name(&self, service: ManagedService) -> &str {
        assert_eq!(service, ManagedService::LandscapeRouter);
        "landscape-router.service"
    }
    forbidden!(render_definition(service: ManagedService, root: &Path) -> Result<String, InstallError>);
    forbidden!(validate_definition(service: ManagedService, content: &str, root: &Path) -> Result<(), InstallError>);
    fn query_registration(
        &self,
        service: ManagedService,
    ) -> Result<SystemRegistration, InstallError> {
        assert_eq!(service, ManagedService::LandscapeRouter);
        Ok(self.registration.clone())
    }
    forbidden!(register(service: ManagedService, origin: &Path) -> Result<(), InstallError>);
    fn unregister(&self, service: ManagedService, origin: &Path) -> Result<(), InstallError> {
        assert_eq!(service, ManagedService::LandscapeRouter);
        assert_eq!(origin, self.origin);
        self.unregister_calls.fetch_add(1, Ordering::SeqCst);
        let content = match std::fs::read(&self.orphan) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(InstallError::Io(error)),
        };
        if content != b"owned" {
            return Err(InstallError::Systemd("foreign boot definition".into()));
        }
        std::fs::remove_file(&self.orphan).map_err(InstallError::Io)
    }
    forbidden!(is_enabled(service: ManagedService) -> Result<bool, InstallError>);
    forbidden!(enable(service: ManagedService) -> Result<(), InstallError>);
    forbidden!(disable(service: ManagedService) -> Result<(), InstallError>);
    fn is_active(&self, service: ManagedService) -> Result<bool, InstallError> {
        assert_eq!(service, ManagedService::LandscapeRouter);
        Ok(false)
    }
    forbidden!(active_state(service: ManagedService) -> Result<String, InstallError>);
    forbidden!(start(service: ManagedService) -> Result<(), InstallError>);
    forbidden!(stop(service: ManagedService) -> Result<(), InstallError>);
    forbidden!(restart(service: ManagedService) -> Result<(), InstallError>);
    forbidden!(main_pid(service: ManagedService) -> Result<u32, InstallError>);
    forbidden!(restore_registration(service: ManagedService, before: &ServiceBefore, origin: &Path) -> Result<(), InstallError>);
    forbidden!(resolv_conf() -> &Path);
    forbidden!(as_any() -> &dyn std::any::Any);
}

type Deactivate = fn(&dyn ServiceManager, &Path) -> Result<(), InstallError>;

pub(super) fn missing_registration_cleans_owned_orphan_without_original_or_runtime_mutation(
    deactivate: Deactivate,
) {
    let fake = Fake::new();
    assert!(!fake.origin.exists());
    deactivate(&fake, &fake.origin).unwrap();
    assert!(!fake.orphan.exists());
    deactivate(&fake, &fake.origin).unwrap();
    assert_eq!(fake.unregister_calls.load(Ordering::SeqCst), 2);
}

pub(super) fn missing_registration_preserves_unknown_copy_and_propagates_backend_refusal(
    deactivate: Deactivate,
) {
    let fake = Fake::new();
    std::fs::write(&fake.orphan, b"foreign").unwrap();
    let error = deactivate(&fake, &fake.origin).unwrap_err();
    assert!(error.to_string().contains("foreign boot definition"));
    assert_eq!(std::fs::read(&fake.orphan).unwrap(), b"foreign");
    assert_eq!(fake.unregister_calls.load(Ordering::SeqCst), 1);
}

pub(super) fn foreign_registration_does_not_unregister_or_remove_any_copy(deactivate: Deactivate) {
    let mut fake = Fake::new();
    std::fs::create_dir_all(fake.origin.parent().unwrap()).unwrap();
    std::fs::write(&fake.origin, b"original").unwrap();
    for registration in [
        SystemRegistration::Symlink {
            target: fake.directory.join("foreign.service"),
        },
        SystemRegistration::Conflict {
            file_type: "regular file".into(),
        },
    ] {
        fake.registration = registration;
        assert!(deactivate(&fake, &fake.origin).is_err());
        assert_eq!(std::fs::read(&fake.orphan).unwrap(), b"owned");
        assert_eq!(std::fs::read(&fake.origin).unwrap(), b"original");
        assert_eq!(fake.unregister_calls.load(Ordering::SeqCst), 0);
    }
}
