// 测试经 `interactive_guard` 持有的 std Mutex 串行化执行,跨 await 持有是
// 刻意为之(与 restore/mod.rs tests 模块同一先例),此处统一豁免该 lint。
#![allow(clippy::await_holding_lock)]

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
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::SeqCst);
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

/// 建立 lkit 地盘(由 `test_territory` 指向临时目录),返回守卫,须存活整个测试。
fn territory_guard(root: &TempRoot) -> crate::deployment::layout::TerritoryOverride {
    let territory = root.join("territory");
    std::fs::create_dir_all(&territory).unwrap();
    crate::deployment::layout::test_territory(&territory)
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
        if let Ok(content) = std::fs::read_to_string(self.state_dir.join("main.pid"))
            && let Ok(pid) = content.trim().parse::<i32>()
        {
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }
    }
}

/// 构造一个"手工部署"现场:配置目录 + static + static.zip + 运行中的 fixture 实例。
/// fixture 与真实 landscape-webserver 一样是 clap 短/长形式双参数,
/// 这里用真实手工部署常用的短形式 `-c`/`-w` 启动,覆盖实例身份确认路径。
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
    crate::release::repository::archive::pack_static_zip(&static_dir, &source.join("static.zip"))
        .unwrap();

    let config_path = root.join("fixture.json");
    std::fs::write(
        &config_path,
        serde_json::to_vec_pretty(&fixture_config(ports, scenario)).unwrap(),
    )
    .unwrap();
    let child = Command::new(landscape_fixture())
        .env(FIXTURE_CONFIG_ENV, &config_path)
        .args([
            "-c",
            source.to_str().unwrap(),
            "-w",
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

fn available_systemd(dir: &Path) -> Systemd {
    std::fs::create_dir_all(dir.join("run")).unwrap();
    let script = dir.join("systemctl");
    std::fs::write(
        &script,
        r#"#!/bin/sh
case "$*" in
  "show --property=Version") echo "Version=252.fake";;
  *) exit 0;;
esac
"#,
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    Systemd {
        systemctl: script,
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

static NO_INTERRUPT: fn() -> bool = || false;

fn migrate_args(source: &Path) -> MigrateArgs {
    MigrateArgs {
        config_dir: source.to_path_buf(),
        yes: true,
        console_confirmed: false,
        repository: None,
        resume_transaction: None,
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
fn set_systemctl_env(path: &Path) -> SystemctlEnvGuard {
    // SAFETY: 测试进程内、串行区间的环境变量设置,由返回的守卫移除。
    unsafe { std::env::set_var(SYSTEMCTL_CONFIG_ENV, path) };
    SystemctlEnvGuard
}

struct SystemctlEnvGuard;

impl Drop for SystemctlEnvGuard {
    fn drop(&mut self) {
        // SAFETY: 与设置配对,见调用处。
        unsafe { std::env::remove_var(SYSTEMCTL_CONFIG_ENV) };
    }
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
async fn migrate_requires_yes_in_non_interactive() {
    let _guard = interactive_guard().await;
    interactive::configure(true);
    let _reset = NonInteractiveGuard;
    let root = TempRoot::new("migrate-yes");
    let _territory = territory_guard(&root);
    let ports = FixturePorts::unique();
    let (source, _static_dir, _instance) =
        spawn_manual_install(&root, &ports, Scenario::Healthy).await;

    let install_root = new_root(&root.join("install"));
    let systemd = available_systemd(&root.path);
    let options = MigrateOptions {
        export_base_url: format!("https://127.0.0.1:{}", ports.https),
        managed_uid: unsafe { libc::geteuid() },
        confirm: &YES,
        health: &health(&ports),
        probe_ports: &ports.checks(),
        interrupted: &NO_INTERRUPT,
    };
    let args = MigrateArgs {
        config_dir: source.clone(),
        yes: false,
        console_confirmed: false,
        repository: None,
        resume_transaction: None,
    };
    assert!(matches!(
        migrate_version(&install_root, &systemd, &args, &options).await,
        Err(InstallError::ParameterUsage(_))
    ));
    assert!(find_unfinished(&install_root).unwrap().is_none());
    assert!(
        !crate::deployment::layout::territory_transactions_dir()
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
    let _territory = territory_guard(&root);
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
            spawn_units: Vec::new(),
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
        interrupted: &NO_INTERRUPT,
    };
    let outcome = migrate_version(&install_root, &systemd, &migrate_args(&source), &options)
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
    let tx_dir = crate::deployment::layout::territory_transactions_dir()
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

/// fake systemctl 接管现场:旧 unit 指向运行中的 fixture,新受管实例配置、
/// systemd 句柄与事务守卫(守卫须存活整个测试)。
struct SystemdEnvironment {
    systemd: Systemd,
    legacy_unit: PathBuf,
    systemctl_config: PathBuf,
    _managed: ManagedInstanceGuard,
    _systemctl_env: SystemctlEnvGuard,
}

fn systemd_environment(root: &TempRoot, source: &Path, ports: &FixturePorts) -> SystemdEnvironment {
    let units = root.join("units");
    let state_dir = root.join("systemd-state");
    let run_dir = root.join("run");
    std::fs::create_dir_all(&units).unwrap();
    std::fs::create_dir_all(&run_dir).unwrap();
    // 旧 unit 用真实手工部署常见的短形式 `-c`/`-w` 书写,覆盖旧 unit 发现路径。
    let legacy_unit = units.join("legacy-landscape.service");
    std::fs::write(
        &legacy_unit,
        format!(
            "[Unit]\nDescription=Legacy Landscape\n\n[Service]\nExecStart={0} -c {1} -w {1}/static\nRestart=always\nUser=root\nLimitMEMLOCK=infinity\n\n[Install]\nWantedBy=multi-user.target\n",
            landscape_fixture().display(),
            source.display()
        ),
    )
    .unwrap();

    let new_config = root.join("new-fixture.json");
    std::fs::write(
        &new_config,
        serde_json::to_vec_pretty(&fixture_config(ports, Scenario::Healthy)).unwrap(),
    )
    .unwrap();
    let systemctl_config = root.join("systemctl.json");
    let systemctl_env = set_systemctl_env(&systemctl_config);
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
            spawn_units: Vec::new(),
        })
        .unwrap(),
    )
    .unwrap();
    SystemdEnvironment {
        systemd: Systemd {
            systemctl: systemctl_fixture(),
            system_unit_dir: units,
            run_systemd_dir: run_dir,
            pid1_is_systemd: true,
            resolv_conf: root.join("resolv.conf"),
        },
        legacy_unit,
        systemctl_config,
        _managed: ManagedInstanceGuard { state_dir },
        _systemctl_env: systemctl_env,
    }
}

/// 前台 `prepare_migration` 把事务标记为 prepared 后,daemon worker 以事务 id
/// 认领并只执行切换阶段(`resume_migrate_switch`):前置检查不触碰运行态,
/// 切换在同一事务内完成接管与提交。
#[tokio::test]
async fn prepared_migration_resumes_the_switch_phase_in_the_worker() {
    let _guard = interactive_guard().await;
    interactive::configure(true);
    let _reset = NonInteractiveGuard;
    let root = TempRoot::new("prepare-resume");
    let _territory = territory_guard(&root);
    let old_ports = FixturePorts::unique();
    let new_ports = FixturePorts::unique();
    let (source, _static_dir, _old_instance) =
        spawn_manual_install(&root, &old_ports, Scenario::Healthy).await;

    let install_root = new_root(&root.join("install"));
    let environment = systemd_environment(&root, &source, &new_ports);
    let options = MigrateOptions {
        export_base_url: format!("https://127.0.0.1:{}", old_ports.https),
        managed_uid: unsafe { libc::geteuid() },
        confirm: &YES,
        health: &health(&new_ports),
        probe_ports: &old_ports.checks(),
        interrupted: &NO_INTERRUPT,
    };

    // 前台阶段:只做前置检查、API 检查与备份,不触碰旧实例运行态。
    let prepared = prepare_migration(
        &install_root,
        &environment.systemd,
        &migrate_args(&source),
        &options,
    )
    .await
    .unwrap();
    let tx = find_unfinished(&install_root).unwrap().unwrap();
    assert_eq!(tx.phase, crate::deployment::transaction::Phase::Prepared);
    assert!(tx.backup.is_some(), "the migration backup must be recorded");
    assert!(
        environment.legacy_unit.is_file(),
        "pre-checks must not touch the legacy unit"
    );
    assert!(
        !install_root.canonical.join("releases").exists(),
        "pre-checks must not create the managed release"
    );

    // worker 恢复路径:以事务 id 认领,只执行切换阶段。
    let mut args = migrate_args(&source);
    args.resume_transaction = Some(prepared.transaction_id);
    let outcome = resume_migrate_switch(&install_root, &environment.systemd, &args, &options)
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
        !environment.legacy_unit.exists(),
        "the switch must take over the legacy unit"
    );
    assert!(find_unfinished(&install_root).unwrap().is_none());

    let systemctl = Command::new(systemctl_fixture())
        .env(SYSTEMCTL_CONFIG_ENV, &environment.systemctl_config)
        .args(["stop", "landscape-router.service"])
        .output()
        .unwrap();
    assert!(systemctl.status.success());
}

/// 旧部署 unit 以普通文件直接写在受管注册路径(旧安装器形态)时,所有权保护
/// 会拒绝覆盖;`preempt_registration_conflict` 先停止并把它移入事务目录。
#[test]
fn preempts_a_plain_file_legacy_unit_at_the_managed_path() {
    let root = TempRoot::new("preempt-plain");
    let _territory = territory_guard(&root);
    let units = root.join("units");
    let state_dir = root.join("systemd-state");
    let run_dir = root.join("run");
    std::fs::create_dir_all(&units).unwrap();
    std::fs::create_dir_all(&run_dir).unwrap();
    let source = root.join("deploy");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(
        units.join("landscape-router.service"),
        format!(
            "[Unit]\nDescription=Legacy Landscape\n\n[Service]\nExecStart={} -c {}\n",
            landscape_fixture().display(),
            source.display()
        ),
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
            landscape_config: None,
            log_path: root.join("fixture.log"),
            call_log: None,
            systemd_version: "252.fixture".into(),
            spawn_units: Vec::new(),
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
    let install_root = new_root(&root.join("install"));
    let transaction = crate::deployment::transaction::TransactionFile::new_migrate(
        &install_root,
        &semver::Version::parse(EXPORT_VERSION).unwrap(),
    )
    .unwrap();
    let instance = crate::service::process::Process {
        pid: 0,
        exe_link: String::new(),
        exe_sha256: None,
        args: Vec::new(),
    };

    let before =
        preempt_registration_conflict(&install_root, &transaction, &systemd, &source, &instance)
            .unwrap()
            .expect("plain file at the managed path must be preempted");
    assert!(before.file_moved);
    assert!(
        !units.join("landscape-router.service").exists(),
        "the plain file unit must be moved out of the unit dir"
    );
    assert!(
        crate::deployment::layout::territory_transactions_dir()
            .join(&transaction.transaction_id)
            .join("legacy-unit/landscape-router.service")
            .is_file(),
        "the plain file unit must be preserved in the transaction directory"
    );

    // 符号链接接管形态不预清,交由 capture_before 记录事务前事实。
    let origin = install_root
        .canonical
        .join("service/landscape-router.service");
    std::fs::create_dir_all(origin.parent().unwrap()).unwrap();
    std::fs::write(
        &origin,
        format!(
            "[Service]\nExecStart={} -c {}\n",
            landscape_fixture().display(),
            source.display()
        ),
    )
    .unwrap();
    std::os::unix::fs::symlink(origin, units.join("landscape-router.service")).unwrap();
    let transaction2 = crate::deployment::transaction::TransactionFile::new_migrate(
        &install_root,
        &semver::Version::parse(EXPORT_VERSION).unwrap(),
    )
    .unwrap();
    assert!(
        preempt_registration_conflict(&install_root, &transaction2, &systemd, &source, &instance)
            .unwrap()
            .is_none(),
        "symlink registrations are handled by the normal takeover path"
    );

    // 其他 unit 名的普通文件不属于受管注册路径,不预清。
    std::fs::write(
        units.join("legacy-landscape.service"),
        format!(
            "[Service]\nExecStart={} -c {}\n",
            landscape_fixture().display(),
            source.display()
        ),
    )
    .unwrap();
    let transaction3 = crate::deployment::transaction::TransactionFile::new_migrate(
        &install_root,
        &semver::Version::parse(EXPORT_VERSION).unwrap(),
    )
    .unwrap();
    assert!(
        preempt_registration_conflict(&install_root, &transaction3, &systemd, &source, &instance,)
            .unwrap()
            .is_none(),
        "units with other names are not the managed registration path"
    );
}

/// 用户机器形态的完整迁移:旧 unit 以普通文件直接写在受管
/// `landscape-router.service` 路径上,preempt 接管后整条切换应提交成功。
#[tokio::test]
async fn migrates_a_plain_file_legacy_unit_at_the_managed_path() {
    let _guard = interactive_guard().await;
    interactive::configure(true);
    let _reset = NonInteractiveGuard;
    let root = TempRoot::new("plain-file-managed");
    let _territory = territory_guard(&root);
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
    // 旧安装器直接写入受管路径的普通文件 unit(短形式参数,见 MIG-08)。
    std::fs::write(
        units.join("landscape-router.service"),
        format!(
            "[Unit]\nDescription=Legacy Landscape\n\n[Service]\nExecStart={0} -c {1} -w {1}/static\nRestart=always\nUser=root\nLimitMEMLOCK=infinity\n\n[Install]\nWantedBy=multi-user.target\n",
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
            spawn_units: Vec::new(),
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
        interrupted: &NO_INTERRUPT,
    };
    let outcome = migrate_version(&install_root, &systemd, &migrate_args(&source), &options)
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

    assert!(
        std::fs::symlink_metadata(units.join("landscape-router.service"))
            .unwrap()
            .file_type()
            .is_symlink(),
        "the managed registration must take over the managed path"
    );
    let tx_dir = crate::deployment::layout::territory_transactions_dir()
        .join(state.last_transaction_id.as_deref().unwrap());
    assert!(
        tx_dir
            .join("legacy-unit/landscape-router.service")
            .is_file(),
        "the plain file unit must be preserved in the transaction directory"
    );
    assert!(find_unfinished(&install_root).unwrap().is_none());

    let systemctl = Command::new(systemctl_fixture())
        .env(SYSTEMCTL_CONFIG_ENV, &systemctl_config)
        .args(["stop", "landscape-router.service"])
        .output()
        .unwrap();
    assert!(systemctl.status.success());
}

/// 切换期间取消(内联路径的 ^C 语义):检查点触发回滚,产出 Cancelled 结果
/// 而不是 RolledBack,旧 unit 恢复、不写状态。
#[tokio::test]
async fn switch_cancellation_rolls_back_with_the_cancelled_outcome() {
    let _guard = interactive_guard().await;
    interactive::configure(true);
    let _reset = NonInteractiveGuard;
    let root = TempRoot::new("switch-cancel");
    let _territory = territory_guard(&root);
    let old_ports = FixturePorts::unique();
    let new_ports = FixturePorts::unique();
    let (source, _static_dir, _old_instance) =
        spawn_manual_install(&root, &old_ports, Scenario::Healthy).await;

    let install_root = new_root(&root.join("install"));
    let environment = systemd_environment(&root, &source, &new_ports);
    // prepare 消耗 4 次检查,切换开始 1 次,停止阶段后第 6 次检查触发取消。
    let calls = std::cell::Cell::new(0usize);
    let interrupted = &|| {
        calls.set(calls.get() + 1);
        calls.get() >= 6
    };
    let options = MigrateOptions {
        export_base_url: format!("https://127.0.0.1:{}", old_ports.https),
        managed_uid: unsafe { libc::geteuid() },
        confirm: &YES,
        health: &health(&new_ports),
        probe_ports: &old_ports.checks(),
        interrupted,
    };
    let outcome = migrate_version(
        &install_root,
        &environment.systemd,
        &migrate_args(&source),
        &options,
    )
    .await
    .unwrap();
    let MigrateOutcome::Cancelled { version } = outcome else {
        panic!(
            "expected cancelled, got {outcome:?}\nfixture log:\n{}",
            std::fs::read_to_string(root.join("fixture.log")).unwrap_or_default()
        );
    };
    assert_eq!(version.to_string(), EXPORT_VERSION);
    assert!(
        environment.legacy_unit.is_file(),
        "the rollback must restore the legacy unit file"
    );
    assert!(
        !install_root.canonical.join("releases").exists(),
        "the cancelled switch must not leave managed content"
    );
    assert!(!install_root.canonical.join("data").exists());
    assert!(
        !installed_state_file_exists(&install_root),
        "the cancelled switch must not write state"
    );
    assert!(find_unfinished(&install_root).unwrap().is_none());
}

fn installed_state_file_exists(install_root: &InstallRoot) -> bool {
    crate::deployment::state::load_state(install_root)
        .ok()
        .flatten()
        .is_some()
}

/// resume 要求事务 id 精确匹配当前未完成事务;未知 id 拒绝继续,
/// 已准备好的事务保持 prepared 可被正确 id 续跑。
#[tokio::test]
async fn resume_rejects_an_unknown_prepared_transaction() {
    let _guard = interactive_guard().await;
    interactive::configure(true);
    let _reset = NonInteractiveGuard;
    let root = TempRoot::new("resume-unknown");
    let _territory = territory_guard(&root);
    let old_ports = FixturePorts::unique();
    let new_ports = FixturePorts::unique();
    let (source, _static_dir, _old_instance) =
        spawn_manual_install(&root, &old_ports, Scenario::Healthy).await;

    let install_root = new_root(&root.join("install"));
    let environment = systemd_environment(&root, &source, &new_ports);
    let options = MigrateOptions {
        export_base_url: format!("https://127.0.0.1:{}", old_ports.https),
        managed_uid: unsafe { libc::geteuid() },
        confirm: &YES,
        health: &health(&new_ports),
        probe_ports: &old_ports.checks(),
        interrupted: &NO_INTERRUPT,
    };
    let prepared = prepare_migration(
        &install_root,
        &environment.systemd,
        &migrate_args(&source),
        &options,
    )
    .await
    .unwrap();

    let mut args = migrate_args(&source);
    args.resume_transaction = Some("unknown-transaction".into());
    assert!(matches!(
        resume_migrate_switch(&install_root, &environment.systemd, &args, &options).await,
        Err(InstallError::BlockedByTransaction(_))
    ));
    let tx = find_unfinished(&install_root).unwrap().unwrap();
    assert_eq!(
        tx.transaction_id, prepared.transaction_id,
        "the prepared transaction must remain untouched"
    );

    // 正确 id 仍可续跑,失败后的 abort 是干净的错误(回滚路径不进入)。
    let mut args = migrate_args(&source);
    args.resume_transaction = Some(prepared.transaction_id);
    assert!(matches!(
        resume_migrate_switch(&install_root, &environment.systemd, &args, &options).await,
        Ok(MigrateOutcome::Committed { .. })
    ));
}

#[tokio::test]
async fn systemd_mode_rolls_back_and_restores_legacy_unit_on_activation_failure() {
    let _guard = interactive_guard().await;
    interactive::configure(true);
    let _reset = NonInteractiveGuard;
    let root = TempRoot::new("rollback");
    let _territory = territory_guard(&root);
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
            spawn_units: Vec::new(),
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
        interrupted: &NO_INTERRUPT,
    };
    let outcome = migrate_version(&install_root, &systemd, &migrate_args(&source), &options)
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
