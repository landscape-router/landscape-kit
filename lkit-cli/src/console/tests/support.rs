use super::super::backup::*;
use super::super::network_wizard::*;
use super::super::update::*;
use super::super::*;
use crate::check::model::*;
use crate::commands::update::ResolvedUpdate;
use crate::deployment::config::{RepositorySource, RepositorySourceKind};
use crate::i18n::Language;
use crate::network::config::DEFAULT_MANAGEMENT_CIDR;
use crate::network::discovery::{DefaultRoute, Interface};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use std::sync::{Arc, Mutex, mpsc};
use unicode_width::UnicodeWidthStr;

pub(crate) struct LanguageGuard(Language);

impl LanguageGuard {
    pub(crate) fn set(language: Language) -> Self {
        let previous = crate::i18n::current();
        crate::i18n::configure(language);
        Self(previous)
    }
}

impl Drop for LanguageGuard {
    fn drop(&mut self) {
        crate::i18n::configure(self.0);
    }
}

pub(crate) fn terminal_content(terminal: &Terminal<TestBackend>) -> String {
    let buffer = terminal.backend().buffer();
    let width = buffer.area.width as usize;
    let mut content = String::new();
    for row in buffer.content.chunks(width) {
        let mut column = 0;
        while column < row.len() {
            let symbol = row[column].symbol();
            content.push_str(symbol);
            column += UnicodeWidthStr::width(symbol).max(1);
        }
        content.push('\n');
    }
    content
}

pub(crate) fn sample_preflight_report() -> CheckReport {
    CheckReport {
        groups: vec![CheckGroup {
            title: "Host platform".to_string(),
            results: vec![
                CheckResult::new("platform.linux", "Operating system").set(
                    Status::Pass,
                    "linux",
                    "Linux detected",
                ),
                CheckResult::new("platform.architecture", "CPU architecture")
                    .set(
                        Status::Warning,
                        "riscv64",
                        "Release availability is unknown",
                    )
                    .suggestion("Confirm that a compatible release asset exists"),
            ],
        }],
        summary: Status::Warning,
        counts: StatusCounts {
            pass: 1,
            warning: 1,
            error: 0,
            unknown: 0,
        },
    }
}

pub(crate) fn pass_preflight_report() -> CheckReport {
    CheckReport {
        groups: vec![CheckGroup {
            title: "Host platform".to_string(),
            results: vec![CheckResult::new("platform.linux", "Operating system").set(
                Status::Pass,
                "linux",
                "Linux detected",
            )],
        }],
        summary: Status::Pass,
        counts: StatusCounts {
            pass: 1,
            warning: 0,
            error: 0,
            unknown: 0,
        },
    }
}

pub(crate) fn error_preflight_report() -> CheckReport {
    CheckReport {
        groups: vec![CheckGroup {
            title: "Ports".to_string(),
            results: vec![
                CheckResult::new("ports.6443", "Port 6443")
                    .set(Status::Error, "6443", "already in use")
                    .suggestion("stop the conflicting process"),
                CheckResult::new("ports.22", "Port 22").set(Status::Unknown, "", "cannot probe"),
            ],
        }],
        summary: Status::Error,
        counts: StatusCounts {
            pass: 0,
            warning: 0,
            error: 1,
            unknown: 1,
        },
    }
}

pub(crate) fn sample_network_wizard() -> NetworkWizard {
    let mut wizard = NetworkWizard {
        interfaces: vec![
            Interface {
                name: "ens32".into(),
                mac: "00:0c:29:a4:09:08".into(),
                operstate: "up".into(),
                addresses: vec!["10.1.1.105/24".parse().unwrap()],
            },
            Interface {
                name: "ens33".into(),
                mac: "00:0c:29:a4:09:12".into(),
                operstate: "down".into(),
                addresses: Vec::new(),
            },
        ],
        routes: Vec::new(),
        wan: 0,
        step: WizardStep::Wan,
        wan_mode: WanMode::Static,
        address: String::new(),
        gateway: String::new(),
        focus: 0,
        lan_candidates: Vec::new(),
        lan_cursor: 0,
        lan_selected: Vec::new(),
        management: DEFAULT_MANAGEMENT_CIDR.into(),
        dhcp_start: String::new(),
        dhcp_end: String::new(),
        editing: false,
        cancel_confirming: false,
    };
    wizard.set_wan(0);
    wizard
}

pub(crate) fn routes_armed_wizard() -> NetworkWizard {
    let mut wizard = sample_network_wizard();
    wizard.routes = vec![DefaultRoute {
        iface: "ens32".into(),
        gateway: "10.1.1.1".parse().unwrap(),
    }];
    wizard
}

pub(crate) fn installed_snapshot() -> Snapshot {
    Snapshot::Installed {
        version: "1.2.3".into(),
        manager: "systemd",
        initialized: true,
    }
}

/// Deterministic deployment replies without a thread or host service operation.
pub(crate) struct TestDeployOps {
    result: super::super::daemon_panel::DeployResult,
    requests: Mutex<Vec<Option<String>>>,
}

impl TestDeployOps {
    pub(crate) fn requests(&self) -> Vec<Option<String>> {
        self.requests.lock().unwrap().clone()
    }
}

impl super::super::daemon_panel::DeployOps for TestDeployOps {
    fn deploy(
        &self,
        psk: Option<String>,
    ) -> mpsc::Receiver<super::super::daemon_panel::DeployResult> {
        self.requests.lock().unwrap().push(psk);
        let (sender, receiver) = mpsc::channel();
        sender.send(self.result.clone()).unwrap();
        receiver
    }
}

/// Overview/daemon dialog tests use a fixed snapshot and isolated background operations.
pub(crate) fn overview_app_with_deploy(
    result: super::super::daemon_panel::DeployResult,
) -> (ConsoleApp, Arc<TestDeployOps>) {
    let mut app = ConsoleApp::new();
    app.snapshot = Snapshot::NotInstalled;
    // A successful deployment reruns preflight; both seams must remain isolated.
    app.preflight.ops = Arc::new(super::super::preflight::MockPreflightOps::manual());
    let deploy = Arc::new(TestDeployOps {
        result,
        requests: Mutex::new(Vec::new()),
    });
    app.deploy_ops = deploy.clone();
    (app, deploy)
}

pub(crate) fn overview_app() -> ConsoleApp {
    overview_app_with_deploy(Err("fixture deployment failed".into())).0
}

pub(crate) use super::super::backup::{backup_rows, sample_backup_entry, sample_backup_metadata};

pub(crate) fn pending_takeover_snapshot() -> Snapshot {
    Snapshot::AwaitingNetworkConfirmation {
        transaction_id: "tx-1".into(),
        phase: "awaiting_network_confirmation",
        deadline: chrono::Utc::now() + chrono::Duration::minutes(10),
        management_address: Some("192.168.10.1/24".into()),
    }
}

pub(crate) fn backup_ready_app() -> ConsoleApp {
    let mut app = ConsoleApp::new();
    app.menu_index = 2;
    app.focus = Focus::Panel;
    app.snapshot = installed_snapshot();
    app.backup.state = BackupListState::Complete(vec![sample_backup_entry()]);
    app
}

pub(crate) fn update_ready_app() -> ConsoleApp {
    let mut app = ConsoleApp::new();
    app.menu_index = 3;
    app.focus = Focus::Panel;
    app.snapshot = installed_snapshot();
    app.update.current_source = Some(RepositorySource {
        kind: RepositorySourceKind::Http,
        location: "https://example.com/releases/".into(),
    });
    app.update.repository = UpdateRepositoryMode::Current;
    app
}

pub(crate) fn resolved(current: &str, target: &str) -> ResolvedUpdate {
    ResolvedUpdate {
        current: semver::Version::parse(current).unwrap(),
        target: semver::Version::parse(target).unwrap(),
        install_root: "/fixture/landscape".into(),
        state_identity: "a".repeat(64),
        repository: crate::deployment::plan::RepositoryChoice::Http(
            "https://example.com/releases/".into(),
        ),
    }
}

/// Deterministic daemon readiness in a temporary territory. A running fixture
/// owns daemon.lock and publishes this test process's PID for the guard lifetime.
pub(crate) struct DaemonTerritory {
    _instance_lock: Option<std::fs::File>,
    _guard: crate::deployment::layout::TerritoryOverride,
    path: PathBuf,
}

impl DaemonTerritory {
    pub(crate) fn new(name: &str, running: bool) -> Self {
        let pid = if running {
            std::process::id().to_string()
        } else {
            "99999999".into()
        };
        Self::with_pidfile(name, &pid)
    }

    pub(crate) fn with_pidfile(name: &str, content: &str) -> Self {
        let territory =
            std::env::temp_dir().join(format!("lkit-console-screen-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&territory);
        std::fs::create_dir_all(territory.join("run")).unwrap();
        let guard = crate::deployment::layout::test_territory(&territory);
        let pidfile = crate::deployment::layout::territory_pidfile();
        std::fs::write(&pidfile, content).unwrap();
        let instance_lock = (content.trim() == std::process::id().to_string())
            .then(|| crate::daemon_supervisor::instance_lock("daemon.lock").unwrap());
        Self {
            _instance_lock: instance_lock,
            _guard: guard,
            path: territory,
        }
    }

    pub(crate) fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for DaemonTerritory {
    fn drop(&mut self) {
        // Drop 体先删临时目录,随后 guard 字段自动释放 territory 锁。
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
