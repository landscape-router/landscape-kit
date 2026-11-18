use super::*;
use std::sync::Mutex;

#[derive(Clone, Copy)]
enum Change {
    Apply,
    Fail,
    Ignore,
}

struct State {
    enabled: bool,
    queries: usize,
    calls: Vec<&'static str>,
}

struct Fake {
    backend: ServiceManagerKind,
    state: Mutex<State>,
    change: Change,
    fail_query: Option<usize>,
}

impl Fake {
    fn new(backend: ServiceManagerKind, enabled: bool) -> Self {
        Self {
            backend,
            state: Mutex::new(State {
                enabled,
                queries: 0,
                calls: Vec::new(),
            }),
            change: Change::Apply,
            fail_query: None,
        }
    }

    fn restore(&self, enabled: bool) -> Result<(), InstallError> {
        self.restore_enablement(
            ManagedService::LandscapeRouter,
            Path::new("unused-origin"),
            &Enablement {
                enabled,
                systemd_links: None,
            },
        )
    }

    fn set(&self, enabled: bool) -> Result<(), InstallError> {
        let mut state = self.state.lock().unwrap();
        state.calls.push(if enabled { "enable" } else { "disable" });
        match self.change {
            Change::Apply => state.enabled = enabled,
            Change::Fail => return Err(InstallError::Preflight("fake change failed".into())),
            Change::Ignore => {}
        }
        Ok(())
    }

    fn calls(&self) -> Vec<&'static str> {
        self.state.lock().unwrap().calls.clone()
    }
}

impl ServiceManager for Fake {
    fn kind(&self) -> ServiceManagerKind {
        self.backend
    }
    fn service_name(&self, _: ManagedService) -> &str {
        "landscape-router"
    }
    fn is_enabled(&self, _: ManagedService) -> Result<bool, InstallError> {
        let mut state = self.state.lock().unwrap();
        state.calls.push("query");
        state.queries += 1;
        if self.fail_query == Some(state.queries) {
            return Err(InstallError::Preflight("fake query failed".into()));
        }
        Ok(state.enabled)
    }
    fn enable(&self, _: ManagedService) -> Result<(), InstallError> {
        self.set(true)
    }
    fn disable(&self, _: ManagedService) -> Result<(), InstallError> {
        self.set(false)
    }
    fn probe(&self) -> Availability {
        panic!("unexpected probe")
    }
    fn render_definition(&self, _: ManagedService, _: &Path) -> Result<String, InstallError> {
        panic!("unexpected definition rendering")
    }
    fn validate_definition(
        &self,
        _: ManagedService,
        _: &str,
        _: &Path,
    ) -> Result<(), InstallError> {
        panic!("unexpected definition validation")
    }
    fn query_registration(&self, _: ManagedService) -> Result<SystemRegistration, InstallError> {
        panic!("unexpected registration query")
    }
    fn register(&self, _: ManagedService, _: &Path) -> Result<(), InstallError> {
        panic!("unexpected registration")
    }
    fn unregister(&self, _: ManagedService, _: &Path) -> Result<(), InstallError> {
        panic!("unexpected unregistration")
    }
    fn is_active(&self, _: ManagedService) -> Result<bool, InstallError> {
        panic!("unexpected active query")
    }
    fn active_state(&self, _: ManagedService) -> Result<String, InstallError> {
        panic!("unexpected active query")
    }
    fn start(&self, _: ManagedService) -> Result<(), InstallError> {
        panic!("unexpected start")
    }
    fn stop(&self, _: ManagedService) -> Result<(), InstallError> {
        panic!("unexpected stop")
    }
    fn restart(&self, _: ManagedService) -> Result<(), InstallError> {
        panic!("unexpected restart")
    }
    fn main_pid(&self, _: ManagedService) -> Result<u32, InstallError> {
        panic!("unexpected pid query")
    }
    fn restore_registration(
        &self,
        _: ManagedService,
        _: &ServiceBefore,
        _: &Path,
    ) -> Result<(), InstallError> {
        panic!("unexpected registration restoration")
    }
    fn resolv_conf(&self) -> &Path {
        panic!("unexpected DNS access")
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

const BACKENDS: [ServiceManagerKind; 2] =
    [ServiceManagerKind::Openrc, ServiceManagerKind::Sysvinit];

#[test]
fn matching_enablement_does_not_rewrite_startup_settings() {
    for backend in BACKENDS {
        for enabled in [false, true] {
            let manager = Fake::new(backend, enabled);
            manager.restore(enabled).unwrap();
            assert_eq!(manager.calls(), ["query"]);
            assert_eq!(manager.state.lock().unwrap().enabled, enabled);
        }
    }
}

#[test]
fn changes_enablement_once_and_retries_without_replaying_commands() {
    for backend in BACKENDS {
        for enabled in [false, true] {
            let manager = Fake::new(backend, !enabled);
            manager.restore(enabled).unwrap();
            manager.restore(enabled).unwrap();
            assert_eq!(
                manager.calls(),
                [
                    "query",
                    if enabled { "enable" } else { "disable" },
                    "query",
                    "query"
                ]
            );
            assert_eq!(manager.state.lock().unwrap().enabled, enabled);
        }
    }
}

#[test]
fn initial_query_failure_prevents_changes() {
    for backend in BACKENDS {
        let mut manager = Fake::new(backend, false);
        manager.fail_query = Some(1);
        assert!(
            manager
                .restore(true)
                .unwrap_err()
                .to_string()
                .contains("fake query failed")
        );
        assert_eq!(manager.calls(), ["query"]);
        assert!(!manager.state.lock().unwrap().enabled);
    }
}

#[test]
fn change_failure_propagates_without_claiming_restoration() {
    for backend in BACKENDS {
        for enabled in [false, true] {
            let mut manager = Fake::new(backend, !enabled);
            manager.change = Change::Fail;
            assert!(
                manager
                    .restore(enabled)
                    .unwrap_err()
                    .to_string()
                    .contains("fake change failed")
            );
            assert_eq!(
                manager.calls(),
                ["query", if enabled { "enable" } else { "disable" }]
            );
            assert_eq!(manager.state.lock().unwrap().enabled, !enabled);
        }
    }
}

#[test]
fn successful_command_without_state_change_is_an_error() {
    for backend in BACKENDS {
        for enabled in [false, true] {
            let mut manager = Fake::new(backend, !enabled);
            manager.change = Change::Ignore;
            let error = manager.restore(enabled).unwrap_err().to_string();
            assert!(
                error.contains("landscape-router did not reach its captured enabled state"),
                "{error}"
            );
            assert_eq!(
                manager.calls(),
                ["query", if enabled { "enable" } else { "disable" }, "query"]
            );
        }
    }
}

#[test]
fn verification_query_failure_is_reported_even_after_the_change() {
    for backend in BACKENDS {
        let mut manager = Fake::new(backend, false);
        manager.fail_query = Some(2);
        assert!(
            manager
                .restore(true)
                .unwrap_err()
                .to_string()
                .contains("fake query failed")
        );
        assert_eq!(manager.calls(), ["query", "enable", "query"]);
        assert!(manager.state.lock().unwrap().enabled);
    }
}

#[test]
fn systemd_snapshot_is_rejected_before_any_init_backend_calls() {
    for backend in BACKENDS {
        let manager = Fake::new(backend, false);
        let before = Enablement {
            enabled: false,
            systemd_links: Some(SystemdStartupLinks {
                version: 1,
                boot_id: "11111111-1111-4111-8111-111111111111".into(),
                persistent: vec![],
                runtime: vec![],
            }),
        };
        assert!(
            manager
                .restore_enablement(
                    ManagedService::LandscapeRouter,
                    Path::new("unused-origin"),
                    &before
                )
                .is_err()
        );
        assert!(manager.calls().is_empty());
    }
}
