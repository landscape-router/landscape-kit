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
    }
}

/// 整屏快照与布局不变量测试的确定性 daemon 状态:隔离 lkit 地盘并写入指定
/// 状态的 pidfile(运行 = 当前测试进程的 pid,未运行 = 必定不存在的 pid)。
/// 守卫存活期间(header 徽标、Overview 常驻服务状态都读该 pidfile),渲染
/// 结果由夹具唯一决定;`test_territory` 的进程级锁保证与其他 territory 测试串行。
pub(crate) struct DaemonTerritory {
    _guard: crate::deployment::layout::TerritoryOverride,
    path: PathBuf,
}

impl DaemonTerritory {
    pub(crate) fn new(name: &str, running: bool) -> Self {
        let territory = std::env::temp_dir().join(format!(
            "lkit-console-screen-{name}-{}-{running}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&territory);
        std::fs::create_dir_all(territory.join("run")).unwrap();
        let guard = crate::deployment::layout::test_territory(&territory);
        let pidfile = crate::deployment::layout::territory_pidfile();
        if running {
            std::fs::write(&pidfile, format!("{}\n", std::process::id())).unwrap();
        } else {
            std::fs::write(&pidfile, "99999999\n").unwrap();
        }
        Self {
            _guard: guard,
            path: territory,
        }
    }
}

impl Drop for DaemonTerritory {
    fn drop(&mut self) {
        // Drop 体先删临时目录,随后 guard 字段自动释放 territory 锁。
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
