use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use lkit_test_fixture::contract;
use lkit_test_fixture::{
    FIXTURE_CONFIG_ENV, LandscapeFixtureConfig, SYSTEMCTL_CONFIG_ENV, Scenario,
    SystemctlFixtureConfig,
};

use super::*;
use crate::deployment::state::load_state;
use crate::deployment::state::{InitStatus, StateServiceManager};
use crate::deployment::transaction::find_unfinished;
use crate::interaction::interactive;
use crate::service::health::{HealthOptions, PortCheck};
use crate::service::process::Protocol;
use crate::service::systemd::Systemd;

/// 单元测试中没有 CARGO_BIN_EXE_*;测试二进制位于 `target/debug/deps/`,
/// fixture 二进制位于 `target/debug/`。
fn fixture_binary(name: &str) -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop();
    if path.file_name().is_some_and(|name| name == "deps") {
        path.pop();
    }
    path.join(name)
}

fn landscape_fixture() -> PathBuf {
    fixture_binary("lkit-landscape-fixture")
}

fn systemctl_fixture() -> PathBuf {
    fixture_binary("lkit-test-systemctl")
}

const EXPORT_VERSION: &str = "0.22.0";

static PORT_COUNTER: AtomicUsize = AtomicUsize::new(0);

#[derive(Clone, Copy)]
struct FixturePorts {
    dns_tcp: u16,
    dns_udp: u16,
    http: u16,
    https: u16,
}

impl FixturePorts {
    fn unique() -> Self {
        let base = 22_000 + PORT_COUNTER.fetch_add(1, Ordering::SeqCst) as u16 * 100;
        Self {
            dns_tcp: base,
            dns_udp: base,
            http: base + 10,
            https: base + 20,
        }
    }

    fn checks(self) -> Vec<PortCheck> {
        vec![
            PortCheck {
                protocol: Protocol::Tcp,
                port: self.dns_tcp,
            },
            PortCheck {
                protocol: Protocol::Udp,
                port: self.dns_udp,
            },
            PortCheck {
                protocol: Protocol::Tcp,
                port: self.http,
            },
            PortCheck {
                protocol: Protocol::Tcp,
                port: self.https,
            },
        ]
    }
}

fn fixture_config(ports: &FixturePorts, scenario: Scenario) -> LandscapeFixtureConfig {
    LandscapeFixtureConfig {
        schema_version: 1,
        scenario,
        listen_address: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        dns_tcp_port: ports.dns_tcp,
        dns_udp_port: ports.dns_udp,
        http_port: ports.http,
        https_port: ports.https,
        ready_delay_ms: 200,
        exit_after_ms: 2_000,
        start_exit_code: 1,
        export_version: EXPORT_VERSION.into(),
        export_content: format!("version = \"{EXPORT_VERSION}\"\n"),
    }
}

struct TempRoot {
    path: PathBuf,
}

impl TempRoot {
    fn new(name: &str) -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "lkit-migrate-test-{name}-{}-{unique}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn join(&self, path: &str) -> PathBuf {
        self.path.join(path)
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

struct Instance {
    child: Child,
}

impl Drop for Instance {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// fake systemctl 派生的受管实例(set sid 脱离会话),由测试结束后按
/// `state_dir/main.pid` 清理,避免残留进程占用端口。
struct ManagedInstanceGuard {
    state_dir: PathBuf,
}

impl Drop for ManagedInstanceGuard {
    fn drop(&mut self) {
        if let Ok(content) = std::fs::read_to_string(self.state_dir.join("main.pid")) {
            if let Ok(pid) = content.trim().parse::<i32>() {
                unsafe {
                    libc::kill(pid, libc::SIGKILL);
                }
            }
        }
    }
}

/// 构造一个"手工部署"现场:配置目录 + static + static.zip + 运行中的 fixture 实例。
async fn spawn_manual_install(
    root: &TempRoot,
    ports: &FixturePorts,
    scenario: Scenario,
) -> (PathBuf, PathBuf, Instance) {
    let source = root.join("source");
    std::fs::create_dir_all(&source).unwrap();
    let static_dir = source.join("static");
    std::fs::create_dir_all(static_dir.join("assets")).unwrap();
    std::fs::write(static_dir.join("index.html"), "manual static").unwrap();
    std::fs::write(static_dir.join("assets/app.js"), "manual asset").unwrap();
    pack_static_zip(&static_dir, &source.join("static.zip")).unwrap();

    let config_path = root.join("fixture.json");
    std::fs::write(
        &config_path,
        serde_json::to_vec_pretty(&fixture_config(ports, scenario)).unwrap(),
    )
    .unwrap();
    let child = Command::new(landscape_fixture())
        .env(FIXTURE_CONFIG_ENV, &config_path)
        .args([
            "--config-dir",
            source.to_str().unwrap(),
            "--web",
            static_dir.to_str().unwrap(),
        ])
        .spawn()
        .unwrap();
    let instance = Instance { child };
    wait_for_export(ports.https).await;
    (source, static_dir, instance)
}

async fn wait_for_export(https_port: u16) {
    let client = reqwest::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap();
    let url = format!("https://127.0.0.1:{https_port}{}", contract::DOCS_PATH);
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while std::time::Instant::now() < deadline {
        if client
            .get(&url)
            .send()
            .await
            .ok()
            .is_some_and(|response| response.status().is_success())
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("fixture did not become ready on port {https_port}");
}

fn new_root(path: &Path) -> InstallRoot {
    InstallRoot {
        install_root: path.to_path_buf(),
        canonical: path.to_path_buf(),
    }
}

fn none_systemd(dir: &Path) -> Systemd {
    Systemd {
        systemctl: dir.join("missing-systemctl"),
        system_unit_dir: dir.join("units"),
        run_systemd_dir: dir.join("run"),
        pid1_is_systemd: true,
        resolv_conf: dir.join("resolv.conf"),
    }
}

struct FakeDocs;

impl DocsProbe for FakeDocs {
    async fn docs_ok(&self) -> bool {
        true
    }
}

fn health(ports: &FixturePorts) -> HealthOptions<FakeDocs> {
    HealthOptions {
        docs: FakeDocs,
        ports: ports.checks(),
        startup_timeout: Duration::from_secs(15),
        stable_duration: Duration::from_millis(100),
    }
}

static YES: fn(&str) -> Result<bool, InstallError> = |_| Ok(true);

fn migrate_args(source: &Path, manager: MigrateManager) -> MigrateArgs {
    MigrateArgs {
        config_dir: source.to_path_buf(),
        manager,
        yes: true,
        console_confirmed: false,
        repository: None,
    }
}

struct NonInteractiveGuard;

impl Drop for NonInteractiveGuard {
    fn drop(&mut self) {
        interactive::configure(false);
    }
}

/// 为 fake systemctl 设置配置环境变量。migrate 测试经 `interactive_guard` 串行执行,
/// 进程级环境变量不会与其他测试交叉。
fn set_systemctl_env(path: &Path) -> impl Drop {
    // SAFETY: 测试进程内、串行区间的环境变量设置,由返回的守卫移除。
    unsafe { std::env::set_var(SYSTEMCTL_CONFIG_ENV, path) };
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            // SAFETY: 与设置配对,见调用处。
            unsafe { std::env::remove_var(SYSTEMCTL_CONFIG_ENV) };
        }
    }
    Reset
}

async fn interactive_guard() -> std::sync::MutexGuard<'static, ()> {
    interactive::test_guard()
}

fn installed_state(install_root: &InstallRoot) -> crate::deployment::state::InstallState {
    load_state(install_root).unwrap().unwrap()
}

fn committed_version(install_root: &InstallRoot) -> String {
    installed_state(install_root).active_version
}

#[test]
fn validates_source_directories() {
    let root = TempRoot::new("source-validation");
    let empty = root.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    assert!(validate_source_dir(&empty).is_err());

    let featureless = root.join("featureless");
    std::fs::create_dir_all(&featureless).unwrap();
    std::fs::write(featureless.join("random.txt"), b"x").unwrap();
    assert!(validate_source_dir(&featureless).is_err());

    let managed = root.join("managed-root");
    std::fs::create_dir_all(managed.join("state")).unwrap();
    std::fs::write(managed.join("state/install-state.json"), b"{}").unwrap();
    std::fs::create_dir_all(managed.join("data")).unwrap();
    std::fs::write(managed.join("data/landscape.toml"), b"x").unwrap();
    assert!(validate_source_dir(&managed.join("data")).is_err());

    let valid = root.join("valid");
    std::fs::create_dir_all(&valid).unwrap();
    std::fs::write(valid.join("landscape_init.lock"), b"").unwrap();
    assert!(validate_source_dir(&valid).is_ok());
    std::fs::remove_file(valid.join("landscape_init.lock")).unwrap();
    std::fs::write(valid.join("landscape.toml"), b"").unwrap();
    assert!(validate_source_dir(&valid).is_ok());
}

#[tokio::test]
async fn migrates_in_none_mode_with_running_instance() {
    let _guard = interactive_guard().await;
    interactive::configure(true);
    let _reset = NonInteractiveGuard;
    let root = TempRoot::new("none-mode");
    let ports = FixturePorts::unique();
    let (source, static_dir, _instance) =
        spawn_manual_install(&root, &ports, Scenario::Healthy).await;

    let install_root = new_root(&root.join("install"));
    let systemd = none_systemd(&root.path);
    let options = MigrateOptions {
        export_base_url: format!("https://127.0.0.1:{}", ports.https),
        managed_uid: unsafe { libc::geteuid() },
        confirm: &YES,
        health: &health(&ports),
        probe_ports: &ports.checks(),
    };
    let outcome = migrate_version(
        &install_root,
        &systemd,
        &migrate_args(&source, MigrateManager::None),
        &options,
    )
    .await
    .unwrap();
    let MigrateOutcome::Committed { version, backup_id } = outcome else {
        panic!("expected committed, got {outcome:?}");
    };
    assert_eq!(version.to_string(), EXPORT_VERSION);
    assert_eq!(committed_version(&install_root), EXPORT_VERSION);

    let release = install_root.canonical.join("releases").join(EXPORT_VERSION);
    assert!(release.join("landscape-webserver").is_file());
    assert_eq!(
        std::fs::read_to_string(release.join("static/index.html")).unwrap(),
        "manual static"
    );
    assert!(release.join("static.zip").is_file());
    assert_eq!(
        std::fs::read_link(install_root.canonical.join("current")).unwrap(),
        PathBuf::from(format!("releases/{EXPORT_VERSION}"))
    );
    assert_eq!(
        std::fs::read_to_string(install_root.canonical.join("data/landscape_init.toml")).unwrap(),
        format!("version = \"{EXPORT_VERSION}\"\n")
    );
    assert!(
        install_root
            .canonical
            .join("backups")
            .join(format!("{backup_id}.lkb"))
            .is_file()
    );
    let state = installed_state(&install_root);
    assert_eq!(state.initialization.status, InitStatus::Pending);
    assert_eq!(state.service.manager, StateServiceManager::None);
    assert!(!state.service.verified);
    assert!(find_unfinished(&install_root).unwrap().is_none());
    assert!(
        !install_root
            .canonical
            .join("data/landscape_init.lock")
            .exists()
    );
    let _ = static_dir;
}

#[tokio::test]
async fn none_mode_requires_yes_in_non_interactive() {
    let _guard = interactive_guard().await;
    interactive::configure(true);
    let _reset = NonInteractiveGuard;
    let root = TempRoot::new("none-yes");
    let ports = FixturePorts::unique();
    let (source, _static_dir, _instance) =
        spawn_manual_install(&root, &ports, Scenario::Healthy).await;

    let install_root = new_root(&root.join("install"));
    let systemd = none_systemd(&root.path);
    let options = MigrateOptions {
        export_base_url: format!("https://127.0.0.1:{}", ports.https),
        managed_uid: unsafe { libc::geteuid() },
        confirm: &YES,
        health: &health(&ports),
        probe_ports: &ports.checks(),
    };
    let args = MigrateArgs {
        config_dir: source.clone(),
        manager: MigrateManager::None,
        yes: false,
        console_confirmed: false,
        repository: None,
    };
    assert!(matches!(
        migrate_version(&install_root, &systemd, &args, &options).await,
        Err(InstallError::ParameterUsage(_))
    ));
    assert!(find_unfinished(&install_root).unwrap().is_none());
    assert!(
        !install_root
            .canonical
            .join("transactions")
            .join(".tmp")
            .exists()
    );
}

#[tokio::test]
async fn migrates_in_systemd_mode_with_legacy_unit_adoption() {
    let _guard = interactive_guard().await;
    interactive::configure(true);
    let _reset = NonInteractiveGuard;
    let root = TempRoot::new("systemd-mode");
    let old_ports = FixturePorts::unique();
    let new_ports = FixturePorts::unique();
    let (source, _static_dir, _old_instance) =
        spawn_manual_install(&root, &old_ports, Scenario::Healthy).await;

    let install_root = new_root(&root.join("install"));
    let units = root.join("units");
    let state_dir = root.join("systemd-state");
    let run_dir = root.join("run");
    std::fs::create_dir_all(&units).unwrap();
    std::fs::create_dir_all(&run_dir).unwrap();
    let _managed = ManagedInstanceGuard {
        state_dir: state_dir.clone(),
    };
    let legacy_unit = units.join("legacy-landscape.service");
    std::fs::write(
        &legacy_unit,
        format!(
            "[Unit]\nDescription=Legacy Landscape\n\n[Service]\nExecStart={0} --config-dir {1} --web {1}/static\nRestart=always\nUser=root\nLimitMEMLOCK=infinity\n\n[Install]\nWantedBy=multi-user.target\n",
            landscape_fixture().display(),
            source.display()
        ),
    )
    .unwrap();

    let new_config = root.join("new-fixture.json");
    std::fs::write(
        &new_config,
        serde_json::to_vec_pretty(&fixture_config(&new_ports, Scenario::Healthy)).unwrap(),
    )
    .unwrap();
    let systemctl_config = root.join("systemctl.json");
    let _systemctl_env = set_systemctl_env(&systemctl_config);
    std::fs::write(
        &systemctl_config,
        serde_json::to_vec_pretty(&SystemctlFixtureConfig {
            schema_version: 1,
            unit_dir: units.clone(),
            state_dir: state_dir.clone(),
            landscape_config: Some(new_config),
            log_path: root.join("fixture.log"),
            call_log: None,
            systemd_version: "252.fixture".into(),
        })
        .unwrap(),
    )
    .unwrap();
    let systemd = Systemd {
        systemctl: systemctl_fixture(),
        system_unit_dir: units.clone(),
        run_systemd_dir: run_dir,
        pid1_is_systemd: true,
        resolv_conf: root.join("resolv.conf"),
    };
    let options = MigrateOptions {
        export_base_url: format!("https://127.0.0.1:{}", old_ports.https),
        managed_uid: unsafe { libc::geteuid() },
        confirm: &YES,
        health: &health(&new_ports),
        probe_ports: &old_ports.checks(),
    };
    let outcome = migrate_version(
        &install_root,
        &systemd,
        &migrate_args(&source, MigrateManager::Systemd),
        &options,
    )
    .await
    .unwrap();
    let MigrateOutcome::Committed { version, .. } = outcome else {
        panic!(
            "expected committed, got {outcome:?}\nfixture log:\n{}",
            std::fs::read_to_string(root.join("fixture.log")).unwrap_or_default()
        );
    };
    assert_eq!(version.to_string(), EXPORT_VERSION);
    let state = installed_state(&install_root);
    assert_eq!(state.service.manager, StateServiceManager::Systemd);
    assert!(state.service.verified);
    assert_eq!(state.initialization.status, InitStatus::Complete);

    assert!(
        std::fs::symlink_metadata(units.join("landscape-router.service"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(
        !legacy_unit.exists(),
        "legacy unit must be moved out of the unit dir"
    );
    let tx_dir = install_root
        .canonical
        .join("transactions")
        .join(state.last_transaction_id.as_deref().unwrap());
    assert!(
        tx_dir
            .join("legacy-unit/legacy-landscape.service")
            .is_file(),
        "legacy unit file must be preserved in the transaction directory"
    );
    assert!(find_unfinished(&install_root).unwrap().is_none());

    let systemctl = Command::new(systemctl_fixture())
        .env(SYSTEMCTL_CONFIG_ENV, &systemctl_config)
        .args(["stop", "landscape-router.service"])
        .output()
        .unwrap();
    assert!(systemctl.status.success());
}

#[tokio::test]
async fn systemd_mode_rolls_back_and_restores_legacy_unit_on_activation_failure() {
    let _guard = interactive_guard().await;
    interactive::configure(true);
    let _reset = NonInteractiveGuard;
    let root = TempRoot::new("rollback");
    let old_ports = FixturePorts::unique();
    let new_ports = FixturePorts::unique();
    let (source, _static_dir, _old_instance) =
        spawn_manual_install(&root, &old_ports, Scenario::Healthy).await;

    let install_root = new_root(&root.join("install"));
    let units = root.join("units");
    let state_dir = root.join("systemd-state");
    let run_dir = root.join("run");
    std::fs::create_dir_all(&units).unwrap();
    std::fs::create_dir_all(&run_dir).unwrap();
    let _managed = ManagedInstanceGuard {
        state_dir: state_dir.clone(),
    };
    let legacy_unit = units.join("legacy-landscape.service");
    std::fs::write(
        &legacy_unit,
        format!(
            "[Unit]\nDescription=Legacy Landscape\n\n[Service]\nExecStart={0} --config-dir {1} --web {1}/static\nRestart=always\nUser=root\nLimitMEMLOCK=infinity\n\n[Install]\nWantedBy=multi-user.target\n",
            landscape_fixture().display(),
            source.display()
        ),
    )
    .unwrap();

    // 新实例启动即退出(start_exit_code 1):main_pid 为 0,激活失败进入回滚。
    let new_config = root.join("new-fixture.json");
    std::fs::write(
        &new_config,
        serde_json::to_vec_pretty(&fixture_config(&new_ports, Scenario::StartExit)).unwrap(),
    )
    .unwrap();
    let systemctl_config = root.join("systemctl.json");
    let _systemctl_env = set_systemctl_env(&systemctl_config);
    std::fs::write(
        &systemctl_config,
        serde_json::to_vec_pretty(&SystemctlFixtureConfig {
            schema_version: 1,
            unit_dir: units.clone(),
            state_dir: state_dir.clone(),
            landscape_config: Some(new_config),
            log_path: root.join("fixture.log"),
            call_log: None,
            systemd_version: "252.fixture".into(),
        })
        .unwrap(),
    )
    .unwrap();
    let systemd = Systemd {
        systemctl: systemctl_fixture(),
        system_unit_dir: units.clone(),
        run_systemd_dir: run_dir,
        pid1_is_systemd: true,
        resolv_conf: root.join("resolv.conf"),
    };
    let options = MigrateOptions {
        export_base_url: format!("https://127.0.0.1:{}", old_ports.https),
        managed_uid: unsafe { libc::geteuid() },
        confirm: &YES,
        health: &health(&new_ports),
        probe_ports: &old_ports.checks(),
    };
    let outcome = migrate_version(
        &install_root,
        &systemd,
        &migrate_args(&source, MigrateManager::Systemd),
        &options,
    )
    .await
    .unwrap();
    assert!(
        matches!(outcome, MigrateOutcome::RolledBack { .. }),
        "expected rolled back, got {outcome:?}"
    );

    assert!(
        legacy_unit.is_file(),
        "legacy unit file must be restored on rollback"
    );
    assert!(
        !install_root
            .canonical
            .join("releases")
            .join(EXPORT_VERSION)
            .exists()
    );
    assert!(!install_root.canonical.join("data").exists());
    assert!(!install_root.canonical.join("current").exists());
    assert!(load_state(&install_root).unwrap().is_none());
    assert!(find_unfinished(&install_root).unwrap().is_none());

    let systemctl = Command::new(systemctl_fixture())
        .env(SYSTEMCTL_CONFIG_ENV, &systemctl_config)
        .args(["stop", "landscape-router.service"])
        .output()
        .unwrap();
    assert!(systemctl.status.success());
}
