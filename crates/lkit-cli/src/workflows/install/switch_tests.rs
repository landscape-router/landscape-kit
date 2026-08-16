use std::net::UdpSocket;
use std::time::Duration;

use super::super::health::PortCheck;
use super::super::repository::provider_for;
use super::super::repository::test_server::{TestResponse, TestServer};
use crate::service::systemd::Systemd;
use std::os::unix::fs::PermissionsExt;

use super::first_install_tests::{
    CurrentSymlinkDocs, FailSecondDocs, FakeDocs, ToggleDocs, WEBSERVER_PAYLOAD, credentials,
    export_body, fake_systemd, load_transaction_json, none_options, repository_files,
    repository_files_for, sha256_bytes, start_repository, temp_root, test_options, version,
};
use super::*;

fn start_switch_repository(
    name: &str,
    from: &str,
    to: &str,
    payload_to: &[u8],
) -> (TestServer, InstallRoot, ReleaseProvider) {
    let mut files = repository_files_for(from, WEBSERVER_PAYLOAD);
    files.extend(repository_files_for(to, payload_to));
    let export = export_body(from);
    let server = TestServer::start(move |path| match path {
        "/api/v1/system/config/export" => TestResponse::ok(export.clone()),
        other => match files.get(other) {
            Some(body) => TestResponse::ok(body.clone()),
            None => TestResponse::status(404, "Not Found", Vec::new()),
        },
    });
    let root = temp_root(name);
    let install_root = InstallRoot {
        install_root: root.clone(),
        canonical: root,
    };
    let provider = provider_for(ProviderKind::Http, &server.base).unwrap();
    (server, install_root, provider)
}

fn switch_options<'a, P: DocsProbe>(
    base_url: &str,
    health: &'a HealthOptions<P>,
    confirmed: bool,
) -> SwitchOptions<'a, P> {
    static TOKEN: fn() -> Result<String, InstallError> = || Ok("tok".into());
    static YES: fn(&str) -> Result<bool, InstallError> = |_| Ok(true);
    static NO: fn(&str) -> Result<bool, InstallError> = |_| Ok(false);
    SwitchOptions {
        export_base_url: base_url.to_string(),
        token: &TOKEN,
        confirm: if confirmed { &YES } else { &NO },
        health,
    }
}

/// 系统化 first-install + switch 测试世界:假 systemd、监听端口、init watcher。
struct SwitchWorld {
    _server: TestServer,
    root: InstallRoot,
    provider: ReleaseProvider,
    systemd: Systemd,
    dir: std::path::PathBuf,
    options: HealthOptions<FakeDocs>,
    _tcp: Vec<std::net::TcpListener>,
    _udp: UdpSocket,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

fn switch_world(name: &str) -> SwitchWorld {
    use std::net::{TcpListener, UdpSocket};

    let (server, root, provider) =
        start_switch_repository(name, "1.2.3", "1.3.0", b"webserver 1.3.0 payload");
    let dir = std::env::temp_dir().join(format!("lkit-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let tcp1 = TcpListener::bind("127.0.0.1:0").unwrap();
    let tcp2 = TcpListener::bind("127.0.0.1:0").unwrap();
    let tcp3 = TcpListener::bind("127.0.0.1:0").unwrap();
    let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
    let ports = vec![
        PortCheck {
            protocol: super::super::process::Protocol::Tcp,
            port: tcp1.local_addr().unwrap().port(),
        },
        PortCheck {
            protocol: super::super::process::Protocol::Tcp,
            port: tcp2.local_addr().unwrap().port(),
        },
        PortCheck {
            protocol: super::super::process::Protocol::Tcp,
            port: tcp3.local_addr().unwrap().port(),
        },
        PortCheck {
            protocol: super::super::process::Protocol::Udp,
            port: udp.local_addr().unwrap().port(),
        },
    ];
    let systemd = fake_systemd_stateful(&dir, std::process::id());
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    init_watcher(root.canonical.join("data"), stop.clone());
    let options = HealthOptions {
        docs: FakeDocs,
        ports: ports.clone(),
        startup_timeout: Duration::from_secs(15),
        stable_duration: Duration::from_millis(100),
    };
    SwitchWorld {
        _server: server,
        root,
        provider,
        systemd,
        dir,
        options,
        _tcp: vec![tcp1, tcp2, tcp3],
        _udp: udp,
        stop,
    }
}

async fn install_v1(world: &SwitchWorld) {
    first_install(
        &world.root,
        &world.provider,
        &TargetVersion::Version(version()),
        &credentials(),
        &world.systemd,
        &world.options,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn switches_version_after_first_install() {
    let world = switch_world("e2e-switch-after-install");
    install_v1(&world).await;
    let state = super::super::state::load_state(&world.root)
        .unwrap()
        .unwrap();
    assert_eq!(state.active_version, "1.2.3");

    let target = world
        .provider
        .release(&semver::Version::new(1, 3, 0), Architecture::X86_64)
        .await
        .unwrap();
    let health = test_options();
    let outcome = switch_version(
        &world.root,
        &state,
        target,
        &SwitchArgs {
            allow_no_backup: false,
        },
        &world.systemd,
        &switch_options(&world._server.base, &health, true),
    )
    .await
    .unwrap();
    let SwitchOutcome::Committed { version, backup_id } = outcome else {
        panic!("expected committed switch, got {outcome:?}");
    };
    assert_eq!(version.to_string(), "1.3.0");
    assert!(backup_id.is_some());

    assert_eq!(
        std::fs::read_link(world.root.canonical.join("current")).unwrap(),
        std::path::PathBuf::from("releases/1.3.0")
    );
    assert!(world.root.canonical.join("releases/1.2.3").is_dir());
    assert!(
        world
            .root
            .canonical
            .join("backups")
            .join(format!("{}.lkb", backup_id.as_ref().unwrap()))
            .is_file()
    );
    let state = super::super::state::load_state(&world.root)
        .unwrap()
        .unwrap();
    assert_eq!(state.active_version, "1.3.0");
    assert_eq!(state.service.manager, StateServiceManager::Systemd);
    assert!(state.service.verified);
    assert!(
        super::super::transaction::find_unfinished(&world.root)
            .unwrap()
            .is_none()
    );
    let tx = load_transaction_json(&world.root);
    assert_eq!(tx["phase"], "committed");
    assert_eq!(tx["operation"], "switch");
    assert_eq!(tx["from_version"], "1.2.3");
    assert_eq!(tx["target_version"], "1.3.0");
    assert_eq!(tx["previous_current"], "releases/1.2.3");
    assert!(tx["backup"]["backup_id"].as_str().unwrap() == backup_id.as_deref().unwrap());
    assert!(
        world
            ._server
            .request_paths()
            .contains(&"/api/v1/system/config/export".to_string())
    );

    world.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = std::fs::remove_dir_all(&world.dir);
    let _ = std::fs::remove_dir_all(&world.root.install_root);
}

#[tokio::test]
async fn reuses_existing_target_release_directory_without_redownloading() {
    let world = switch_world("e2e-switch-reuse-existing");
    install_v1(&world).await;
    let state = super::super::state::load_state(&world.root)
        .unwrap()
        .unwrap();
    assert_eq!(state.active_version, "1.2.3");

    // 模拟上次切换回滚后残留的目标版本目录:内容与 manifest 一致。
    let target = world
        .provider
        .release(&semver::Version::new(1, 3, 0), Architecture::X86_64)
        .await
        .unwrap();
    let final_dir = world.root.canonical.join("releases/1.3.0");
    std::fs::create_dir_all(&final_dir).unwrap();
    fetch_webserver_asset(&target, &final_dir).await.unwrap();
    fetch_static_asset(&target, &final_dir).await.unwrap();
    let asset_requests_before = world
        ._server
        .request_paths()
        .iter()
        .filter(|path| path.starts_with("/releases/1.3.0/"))
        .count();

    let health = test_options();
    let outcome = switch_version(
        &world.root,
        &state,
        target,
        &SwitchArgs {
            allow_no_backup: false,
        },
        &world.systemd,
        &switch_options(&world._server.base, &health, true),
    )
    .await
    .unwrap();
    let SwitchOutcome::Committed { version, .. } = outcome else {
        panic!("expected committed switch, got {outcome:?}");
    };
    assert_eq!(version.to_string(), "1.3.0");
    let asset_requests_after = world
        ._server
        .request_paths()
        .iter()
        .filter(|path| path.starts_with("/releases/1.3.0/"))
        .count();
    assert_eq!(
        asset_requests_after, asset_requests_before,
        "the existing trusted release must be reused without re-downloading its assets"
    );
    assert_eq!(
        std::fs::read_link(world.root.canonical.join("current")).unwrap(),
        std::path::PathBuf::from("releases/1.3.0")
    );
    let state = super::super::state::load_state(&world.root)
        .unwrap()
        .unwrap();
    assert_eq!(state.active_version, "1.3.0");
    assert_eq!(
        std::fs::read(final_dir.join("landscape-webserver")).unwrap(),
        b"webserver 1.3.0 payload"
    );

    world.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = std::fs::remove_dir_all(&world.dir);
    let _ = std::fs::remove_dir_all(&world.root.install_root);
}

#[tokio::test]
async fn rejects_version_downgrade_before_transaction_or_asset_download() {
    let mut world = switch_world("e2e-switch-downgrade");
    // 仓库内容不同:降级路径使用 0.22.2 → 0.21.1。
    let _ = std::fs::remove_dir_all(&world.root.install_root);
    let (server, root, provider) = start_switch_repository(
        "e2e-switch-downgrade",
        "0.22.2",
        "0.21.1",
        b"webserver 0.21.1 payload",
    );
    world._server = server;
    world.root = root;
    world.provider = provider;
    first_install(
        &world.root,
        &world.provider,
        &TargetVersion::Version(semver::Version::new(0, 22, 2)),
        &credentials(),
        &world.systemd,
        &world.options,
    )
    .await
    .unwrap();
    let state = super::super::state::load_state(&world.root)
        .unwrap()
        .unwrap();
    let target = world
        .provider
        .release(&semver::Version::new(0, 21, 1), Architecture::X86_64)
        .await
        .unwrap();
    let health = test_options();

    let result = switch_version(
        &world.root,
        &state,
        target,
        &SwitchArgs {
            allow_no_backup: false,
        },
        &world.systemd,
        &switch_options(&world._server.base, &health, true),
    )
    .await;

    let Err(InstallError::ParameterUsage(reason)) = result else {
        panic!("expected downgrade to be rejected, got {result:?}");
    };
    assert!(reason.contains("0.22.2"));
    assert!(reason.contains("0.21.1"));
    assert_eq!(
        std::fs::read_link(world.root.canonical.join("current")).unwrap(),
        std::path::PathBuf::from("releases/0.22.2")
    );
    assert_eq!(
        super::super::state::load_state(&world.root)
            .unwrap()
            .unwrap()
            .active_version,
        "0.22.2"
    );
    assert!(!world.root.canonical.join("releases/0.21.1").exists());
    assert!(
        super::super::transaction::find_unfinished(&world.root)
            .unwrap()
            .is_none()
    );
    let transaction = load_transaction_json(&world.root);
    assert_eq!(transaction["operation"], "install");
    assert_eq!(transaction["target_version"], "0.22.2");
    let requests = world._server.request_paths();
    assert!(!requests.contains(&"/releases/0.21.1/landscape-webserver-x86_64.zst".to_string()));
    assert!(!requests.contains(&"/releases/0.21.1/static.zip".to_string()));

    world.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = std::fs::remove_dir_all(&world.dir);
    let _ = std::fs::remove_dir_all(&world.root.install_root);
}

#[tokio::test]
async fn switches_without_confirmation_when_systemd() {
    let world = switch_world("e2e-switch-no-confirm");
    install_v1(&world).await;
    let state = super::super::state::load_state(&world.root)
        .unwrap()
        .unwrap();
    let target = world
        .provider
        .release(&semver::Version::new(1, 3, 0), Architecture::X86_64)
        .await
        .unwrap();
    let health = test_options();
    // systemd 路径不请求用户确认:confirm 返回 false 也应完成切换。
    let outcome = switch_version(
        &world.root,
        &state,
        target,
        &SwitchArgs {
            allow_no_backup: false,
        },
        &world.systemd,
        &switch_options(&world._server.base, &health, false),
    )
    .await
    .unwrap();
    assert!(matches!(outcome, SwitchOutcome::Committed { .. }));
    assert_eq!(
        std::fs::read_link(world.root.canonical.join("current")).unwrap(),
        std::path::PathBuf::from("releases/1.3.0")
    );

    world.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = std::fs::remove_dir_all(&world.dir);
    let _ = std::fs::remove_dir_all(&world.root.install_root);
}

#[tokio::test]
async fn repository_override_does_not_require_a_second_confirmation() {
    let first = switch_world("e2e-switch-repo-a");
    install_v1(&first).await;
    let state = super::super::state::load_state(&first.root)
        .unwrap()
        .unwrap();
    assert!(
        super::super::config::load_repository(&first.root)
            .unwrap()
            .is_none(),
        "first install must not create config.toml"
    );

    let second = switch_world("e2e-switch-repo-b");
    let target = second
        .provider
        .release(&semver::Version::new(1, 3, 0), Architecture::X86_64)
        .await
        .unwrap();
    let health = test_options();
    let token = || Ok("tok".to_string());
    let prompts = std::cell::RefCell::new(Vec::new());
    let confirm = |prompt: &str| {
        prompts.borrow_mut().push(prompt.to_string());
        Ok(true)
    };
    let options = SwitchOptions {
        export_base_url: second._server.base.clone(),
        token: &token,
        confirm: &confirm,
        health: &health,
    };
    let outcome = switch_version(
        &first.root,
        &state,
        target,
        &SwitchArgs {
            allow_no_backup: false,
        },
        &first.systemd,
        &options,
    )
    .await
    .unwrap();
    assert!(matches!(outcome, SwitchOutcome::Committed { .. }));
    assert!(
        prompts.into_inner().is_empty(),
        "systemd 路径不得要求用户确认停止服务"
    );
    assert_eq!(
        std::fs::read_link(first.root.canonical.join("current")).unwrap(),
        std::path::PathBuf::from("releases/1.3.0")
    );
    assert!(
        super::super::transaction::find_unfinished(&first.root)
            .unwrap()
            .is_none()
    );
    let state = super::super::state::load_state(&first.root)
        .unwrap()
        .unwrap();
    assert_eq!(state.active_version, "1.3.0");
    assert!(
        super::super::config::load_repository(&first.root)
            .unwrap()
            .is_none(),
        "switch must not write config.toml"
    );
    first.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    second
        .stop
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = std::fs::remove_dir_all(&first.dir);
    let _ = std::fs::remove_dir_all(&second.dir);
    let _ = std::fs::remove_dir_all(&first.root.install_root);
    let _ = std::fs::remove_dir_all(&second.root.install_root);
}

/// 有状态假 systemctl:start/stop 维护 state 文件,stop 后 ActiveState 为 inactive。
fn fake_systemd_stateful(dir: &std::path::Path, main_pid: u32) -> Systemd {
    std::fs::create_dir_all(dir.join("units")).unwrap();
    std::fs::create_dir_all(dir.join("run")).unwrap();
    std::fs::write(dir.join("state"), b"active").unwrap();
    let script = dir.join("systemctl");
    std::fs::write(
        &script,
        format!(
            r#"#!/bin/sh
STATE_FILE="{}"
case "$*" in
  "start landscape-router.service") echo active > "$STATE_FILE"; exit 0;;
  "stop landscape-router.service") echo inactive > "$STATE_FILE"; exit 0;;
  "show --property=ActiveState --value landscape-router.service") cat "$STATE_FILE";;
  "show --property=MainPID --value landscape-router.service") echo {main_pid};;
  "is-enabled landscape-router.service") echo enabled;;
  "is-active landscape-router.service") cat "$STATE_FILE";;
  *) exit 0;;
esac
"#,
            dir.join("state").display()
        ),
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

fn init_watcher(data_dir: std::path::PathBuf, stop: std::sync::Arc<std::sync::atomic::AtomicBool>) {
    std::thread::spawn(move || {
        while !stop.load(std::sync::atomic::Ordering::Relaxed) {
            if data_dir.join("landscape_init.toml").is_file()
                && !data_dir.join("landscape_init.lock").exists()
            {
                std::fs::write(data_dir.join("landscape_init.lock"), b"").unwrap();
                std::fs::write(data_dir.join("landscape.toml"), b"").unwrap();
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    });
}

#[tokio::test]
async fn switches_version_with_systemd() {
    use std::net::{TcpListener, UdpSocket};

    let (server, root, provider) = start_switch_repository(
        "e2e-switch-systemd",
        "1.2.3",
        "1.3.0",
        b"webserver 1.3.0 payload",
    );
    let dir = std::env::temp_dir().join(format!(
        "lkit-pipeline-test-switch-systemd-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let tcp1 = TcpListener::bind("127.0.0.1:0").unwrap();
    let tcp2 = TcpListener::bind("127.0.0.1:0").unwrap();
    let tcp3 = TcpListener::bind("127.0.0.1:0").unwrap();
    let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
    let ports = vec![
        PortCheck {
            protocol: super::super::process::Protocol::Tcp,
            port: tcp1.local_addr().unwrap().port(),
        },
        PortCheck {
            protocol: super::super::process::Protocol::Tcp,
            port: tcp2.local_addr().unwrap().port(),
        },
        PortCheck {
            protocol: super::super::process::Protocol::Tcp,
            port: tcp3.local_addr().unwrap().port(),
        },
        PortCheck {
            protocol: super::super::process::Protocol::Udp,
            port: udp.local_addr().unwrap().port(),
        },
    ];
    let systemd = fake_systemd_stateful(&dir, std::process::id());
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    init_watcher(root.canonical.join("data"), stop.clone());

    let options = HealthOptions {
        docs: FakeDocs,
        ports: ports.clone(),
        startup_timeout: Duration::from_secs(15),
        stable_duration: Duration::from_millis(100),
    };
    first_install(
        &root,
        &provider,
        &TargetVersion::Version(version()),
        &credentials(),
        &systemd,
        &options,
    )
    .await
    .unwrap();
    let state = super::super::state::load_state(&root).unwrap().unwrap();
    assert_eq!(state.active_version, "1.2.3");
    let retained_init = b"externally_modified = true\n";
    std::fs::write(
        root.canonical.join("data/landscape_init.toml"),
        retained_init,
    )
    .unwrap();

    let target = provider
        .release(&semver::Version::new(1, 3, 0), Architecture::X86_64)
        .await
        .unwrap();
    let outcome = switch_version(
        &root,
        &state,
        target,
        &SwitchArgs {
            allow_no_backup: false,
        },
        &systemd,
        &switch_options(&server.base, &options, true),
    )
    .await
    .unwrap();
    let SwitchOutcome::Committed { version, backup_id } = outcome else {
        panic!("expected committed switch, got {outcome:?}");
    };
    assert_eq!(version.to_string(), "1.3.0");
    assert!(backup_id.is_some());

    let state = super::super::state::load_state(&root).unwrap().unwrap();
    assert_eq!(state.active_version, "1.3.0");
    assert_eq!(state.service.manager, StateServiceManager::Systemd);
    assert!(state.service.verified);
    assert!(state.initialization.lock_present);
    assert_eq!(
        std::fs::read(root.canonical.join("data/landscape_init.toml")).unwrap(),
        retained_init
    );
    assert!(
        root.canonical
            .join("backups")
            .join(format!("{}.lkb", backup_id.as_ref().unwrap()))
            .is_file()
    );
    let tx = load_transaction_json(&root);
    assert_eq!(tx["phase"], "committed");
    assert_eq!(tx["operation"], "switch");
    assert!(tx["systemd_before"]["registration"]["kind"] == "symlink");
    assert!(tx["resolv_conf_backup"].is_string());

    drop(tcp1);
    drop(tcp2);
    drop(tcp3);
    drop(udp);
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = std::fs::remove_dir_all(&root.install_root);
    let _ = std::fs::remove_dir_all(&dir);
}

type StoppedServiceWorld = (
    TestServer,
    InstallRoot,
    ReleaseProvider,
    Systemd,
    std::path::PathBuf,
    HealthOptions<ToggleDocs>,
    Vec<std::net::TcpListener>,
    UdpSocket,
    std::sync::Arc<std::sync::atomic::AtomicBool>,
);

fn stopped_service_world(name: &str) -> StoppedServiceWorld {
    use std::net::{TcpListener, UdpSocket};

    let (server, root, provider) =
        start_switch_repository(name, "1.2.3", "1.3.0", b"webserver 1.3.0 payload");
    let dir =
        std::env::temp_dir().join(format!("lkit-pipeline-host-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let tcp1 = TcpListener::bind("127.0.0.1:0").unwrap();
    let tcp2 = TcpListener::bind("127.0.0.1:0").unwrap();
    let tcp3 = TcpListener::bind("127.0.0.1:0").unwrap();
    let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
    let ports = vec![
        PortCheck {
            protocol: super::super::process::Protocol::Tcp,
            port: tcp1.local_addr().unwrap().port(),
        },
        PortCheck {
            protocol: super::super::process::Protocol::Tcp,
            port: tcp2.local_addr().unwrap().port(),
        },
        PortCheck {
            protocol: super::super::process::Protocol::Tcp,
            port: tcp3.local_addr().unwrap().port(),
        },
        PortCheck {
            protocol: super::super::process::Protocol::Udp,
            port: udp.local_addr().unwrap().port(),
        },
    ];
    let systemd = fake_systemd_stateful(&dir, std::process::id());
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    init_watcher(root.canonical.join("data"), stop.clone());
    let docs = ToggleDocs {
        ok: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
    };
    let options = HealthOptions {
        docs,
        ports: ports.clone(),
        startup_timeout: Duration::from_secs(15),
        stable_duration: Duration::from_millis(100),
    };
    (
        server,
        root,
        provider,
        systemd,
        dir,
        options,
        vec![tcp1, tcp2, tcp3],
        udp,
        stop,
    )
}

#[tokio::test]
async fn refuses_switch_when_stopped_service_without_allow_no_backup() {
    use std::os::unix::fs::PermissionsExt;

    let (server, root, provider, systemd, dir, options, _tcp, _udp, stop) =
        stopped_service_world("e2e-switch-stopped-refuse");
    first_install(
        &root,
        &provider,
        &TargetVersion::Version(version()),
        &credentials(),
        &systemd,
        &options,
    )
    .await
    .unwrap();
    // 模拟 systemctl stop:unit 仍注册,但 ActiveState 为 inactive。
    std::fs::write(dir.join("state"), b"inactive").unwrap();
    let state = super::super::state::load_state(&root).unwrap().unwrap();
    let target = provider
        .release(&semver::Version::new(1, 3, 0), Architecture::X86_64)
        .await
        .unwrap();
    let result = switch_version(
        &root,
        &state,
        target,
        &SwitchArgs {
            allow_no_backup: false,
        },
        &systemd,
        &switch_options(&server.base, &options, true),
    )
    .await;
    assert!(matches!(result, Err(InstallError::ServiceNotRunning(_))));
    assert!(
        super::super::transaction::find_unfinished(&root)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        std::fs::read_link(root.canonical.join("current")).unwrap(),
        std::path::PathBuf::from("releases/1.2.3")
    );
    assert_eq!(state.active_version, "1.2.3");
    assert!(
        std::fs::metadata(dir.join("state"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777
            > 0
    );

    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = std::fs::remove_dir_all(&root.install_root);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn switches_stopped_service_without_backup_when_allowed() {
    let (server, root, provider, systemd, dir, options, _tcp, _udp, stop) =
        stopped_service_world("e2e-switch-stopped-ok");
    first_install(
        &root,
        &provider,
        &TargetVersion::Version(version()),
        &credentials(),
        &systemd,
        &options,
    )
    .await
    .unwrap();
    std::fs::write(dir.join("state"), b"inactive").unwrap();
    let state = super::super::state::load_state(&root).unwrap().unwrap();
    let target = provider
        .release(&semver::Version::new(1, 3, 0), Architecture::X86_64)
        .await
        .unwrap();
    let outcome = switch_version(
        &root,
        &state,
        target,
        &SwitchArgs {
            allow_no_backup: true,
        },
        &systemd,
        &switch_options(&server.base, &options, true),
    )
    .await
    .unwrap();
    let SwitchOutcome::Committed { version, backup_id } = outcome else {
        panic!("expected committed switch, got {outcome:?}");
    };
    assert_eq!(version.to_string(), "1.3.0");
    assert!(backup_id.is_none());

    let state = super::super::state::load_state(&root).unwrap().unwrap();
    assert_eq!(state.active_version, "1.3.0");
    assert_eq!(state.service.manager, StateServiceManager::Systemd);
    assert!(state.service.verified);
    let tx = load_transaction_json(&root);
    assert_eq!(tx["phase"], "committed");
    assert_eq!(tx["operation"], "switch");
    assert!(tx["no_backup"] == true);
    assert!(tx["backup"].is_null());
    let lkb_count = std::fs::read_dir(root.canonical.join("backups"))
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "lkb"))
                .count()
        })
        .unwrap_or(0);
    assert_eq!(lkb_count, 0, "no .lkb backup must be created");
    assert!(
        !server
            .request_paths()
            .contains(&"/api/v1/system/config/export".to_string()),
        "the stopped service must not be queried for a config snapshot"
    );
    assert!(
        super::super::transaction::find_unfinished(&root)
            .unwrap()
            .is_none()
    );

    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = std::fs::remove_dir_all(&root.install_root);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn rolls_back_stopped_service_switch_without_backup_on_health_failure() {
    let (server, root, provider, systemd, dir, _options, _tcp, _udp, stop) =
        stopped_service_world("e2e-switch-stopped-rollback");
    // 首次安装阶段 /api/docs 保持可用。
    let install_options = HealthOptions {
        docs: FakeDocs,
        ports: _options.ports.clone(),
        startup_timeout: Duration::from_secs(3),
        stable_duration: Duration::from_millis(100),
    };
    first_install(
        &root,
        &provider,
        &TargetVersion::Version(version()),
        &credentials(),
        &systemd,
        &install_options,
    )
    .await
    .unwrap();
    std::fs::write(dir.join("state"), b"inactive").unwrap();
    let state = super::super::state::load_state(&root).unwrap().unwrap();
    // 目标版本启动验证期间 current 指向 1.3.0,/api/docs 持续失败,
    // 启动轮询超时触发无备份回滚;回滚恢复 current 为 1.2.3 后通过。
    let switch_health = HealthOptions {
        docs: CurrentSymlinkDocs {
            root: root.canonical.clone(),
            rollback_target: "releases/1.2.3".into(),
        },
        ports: _options.ports.clone(),
        startup_timeout: Duration::from_secs(3),
        stable_duration: Duration::from_millis(100),
    };
    let target = provider
        .release(&semver::Version::new(1, 3, 0), Architecture::X86_64)
        .await
        .unwrap();
    let outcome = switch_version(
        &root,
        &state,
        target,
        &SwitchArgs {
            allow_no_backup: true,
        },
        &systemd,
        &switch_options(&server.base, &switch_health, true),
    )
    .await
    .unwrap();
    let SwitchOutcome::RolledBack { version, backup_id } = outcome else {
        panic!("expected rolled back switch, got {outcome:?}");
    };
    assert_eq!(version.to_string(), "1.2.3");
    assert!(backup_id.is_none());

    let state = super::super::state::load_state(&root).unwrap().unwrap();
    assert_eq!(state.active_version, "1.2.3");
    assert_eq!(state.service.manager, StateServiceManager::Systemd);
    assert!(state.service.verified);
    assert_eq!(
        std::fs::read_link(root.canonical.join("current")).unwrap(),
        std::path::PathBuf::from("releases/1.2.3")
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("state")).unwrap().trim(),
        "inactive",
        "the previous version stays stopped, matching the pre-switch state"
    );
    let tx = load_transaction_json(&root);
    assert_eq!(tx["phase"], "rolled_back");
    assert_eq!(tx["operation"], "switch");
    assert!(tx["no_backup"] == true);
    assert!(tx["backup"].is_null());
    assert!(
        super::super::transaction::find_unfinished(&root)
            .unwrap()
            .is_none()
    );

    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = std::fs::remove_dir_all(&root.install_root);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn switches_and_rolls_back_via_lkb_on_health_failure() {
    use std::net::{TcpListener, UdpSocket};

    let (server, root, provider) =
        start_switch_repository("e2e-rollback", "1.2.3", "1.3.0", b"webserver 1.3.0 payload");
    let dir = std::env::temp_dir().join(format!(
        "lkit-pipeline-test-rollback-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let tcp1 = TcpListener::bind("127.0.0.1:0").unwrap();
    let tcp2 = TcpListener::bind("127.0.0.1:0").unwrap();
    let tcp3 = TcpListener::bind("127.0.0.1:0").unwrap();
    let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
    let ports = vec![
        PortCheck {
            protocol: super::super::process::Protocol::Tcp,
            port: tcp1.local_addr().unwrap().port(),
        },
        PortCheck {
            protocol: super::super::process::Protocol::Tcp,
            port: tcp2.local_addr().unwrap().port(),
        },
        PortCheck {
            protocol: super::super::process::Protocol::Tcp,
            port: tcp3.local_addr().unwrap().port(),
        },
        PortCheck {
            protocol: super::super::process::Protocol::Udp,
            port: udp.local_addr().unwrap().port(),
        },
    ];
    let systemd = fake_systemd_stateful(&dir, std::process::id());
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    init_watcher(root.canonical.join("data"), stop.clone());

    let install_options = HealthOptions {
        docs: FakeDocs,
        ports: ports.clone(),
        startup_timeout: Duration::from_secs(5),
        stable_duration: Duration::from_millis(100),
    };
    let options = HealthOptions {
        docs: FailSecondDocs {
            calls: std::sync::atomic::AtomicUsize::new(0),
        },
        ports: ports.clone(),
        startup_timeout: Duration::from_secs(5),
        stable_duration: Duration::from_millis(100),
    };
    first_install(
        &root,
        &provider,
        &TargetVersion::Version(version()),
        &credentials(),
        &systemd,
        &install_options,
    )
    .await
    .unwrap();
    let state = super::super::state::load_state(&root).unwrap().unwrap();
    assert_eq!(state.active_version, "1.2.3");

    let target = provider
        .release(&semver::Version::new(1, 3, 0), Architecture::X86_64)
        .await
        .unwrap();
    let outcome = switch_version(
        &root,
        &state,
        target,
        &SwitchArgs {
            allow_no_backup: false,
        },
        &systemd,
        &switch_options(&server.base, &options, true),
    )
    .await
    .unwrap();
    let SwitchOutcome::RolledBack { version, backup_id } = outcome else {
        panic!("expected rolled back switch, got {outcome:?}");
    };
    assert_eq!(version.to_string(), "1.2.3");
    assert!(backup_id.is_some());

    let state = super::super::state::load_state(&root).unwrap().unwrap();
    assert_eq!(state.active_version, "1.2.3");
    assert_eq!(state.service.manager, StateServiceManager::Systemd);
    assert!(state.service.verified);
    assert_eq!(
        std::fs::read_link(root.canonical.join("current")).unwrap(),
        std::path::PathBuf::from("releases/1.2.3")
    );
    let init_config =
        std::fs::read_to_string(root.canonical.join("data/landscape_init.toml")).unwrap();
    assert!(
        init_config.contains("admin_pass = \"Secret123\""),
        "restored init config: {init_config}"
    );

    let tx = load_transaction_json(&root);
    assert_eq!(tx["phase"], "rolled_back");
    assert_eq!(tx["operation"], "switch");
    let tx_dir = root
        .canonical
        .join("transactions")
        .join(tx["transaction_id"].as_str().unwrap());
    assert!(tx_dir.join("failed-data").is_dir());
    assert!(tx_dir.join("replaced-release").is_dir());
    assert!(tx_dir.join("restore").is_dir());
    assert!(
        root.canonical
            .join("backups")
            .join(tx["transaction_id"].as_str().unwrap())
            .join("host/resolv.conf")
            .is_dir()
    );

    drop(tcp1);
    drop(tcp2);
    drop(tcp3);
    drop(udp);
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = std::fs::remove_dir_all(&root.install_root);
    let _ = std::fs::remove_dir_all(&dir);
}

struct FailingDocs;

impl DocsProbe for FailingDocs {
    async fn docs_ok(&self) -> bool {
        false
    }
}

#[tokio::test]
async fn switch_rollback_failure_returns_rollback_failed_and_preserves_diagnostics() {
    use std::net::{TcpListener, UdpSocket};

    // RB-06:激活验证失败触发 `.lkb` 回滚,回滚自身的健康检查也失败时
    // 返回 RollbackFailed(命令层映射退出码 6),事务标记 `failed` 且诊断
    // 现场保留。探测恒失败,事件驱动,不依赖墙钟。
    let (server, root, provider) = start_switch_repository(
        "e2e-switch-rollback-failed",
        "1.2.3",
        "1.3.0",
        b"webserver 1.3.0 payload",
    );
    let dir = std::env::temp_dir().join(format!(
        "lkit-pipeline-test-rollback-failed-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let tcp1 = TcpListener::bind("127.0.0.1:0").unwrap();
    let tcp2 = TcpListener::bind("127.0.0.1:0").unwrap();
    let tcp3 = TcpListener::bind("127.0.0.1:0").unwrap();
    let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
    let ports = vec![
        PortCheck {
            protocol: super::super::process::Protocol::Tcp,
            port: tcp1.local_addr().unwrap().port(),
        },
        PortCheck {
            protocol: super::super::process::Protocol::Tcp,
            port: tcp2.local_addr().unwrap().port(),
        },
        PortCheck {
            protocol: super::super::process::Protocol::Tcp,
            port: tcp3.local_addr().unwrap().port(),
        },
        PortCheck {
            protocol: super::super::process::Protocol::Udp,
            port: udp.local_addr().unwrap().port(),
        },
    ];
    let systemd = fake_systemd_stateful(&dir, std::process::id());
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    init_watcher(root.canonical.join("data"), stop.clone());

    let install_options = HealthOptions {
        docs: FakeDocs,
        ports: ports.clone(),
        startup_timeout: Duration::from_secs(5),
        stable_duration: Duration::from_millis(100),
    };
    first_install(
        &root,
        &provider,
        &TargetVersion::Version(version()),
        &credentials(),
        &systemd,
        &install_options,
    )
    .await
    .unwrap();
    let state = super::super::state::load_state(&root).unwrap().unwrap();
    assert_eq!(state.active_version, "1.2.3");

    let options = HealthOptions {
        docs: FailingDocs,
        ports: ports.clone(),
        startup_timeout: Duration::from_secs(2),
        stable_duration: Duration::from_millis(100),
    };
    let target = provider
        .release(&semver::Version::new(1, 3, 0), Architecture::X86_64)
        .await
        .unwrap();
    let outcome = switch_version(
        &root,
        &state,
        target,
        &SwitchArgs {
            allow_no_backup: false,
        },
        &systemd,
        &switch_options(&server.base, &options, true),
    )
    .await
    .unwrap();
    let SwitchOutcome::RollbackFailed { version, reason } = outcome else {
        panic!("expected rollback failed switch, got {outcome:?}");
    };
    assert_eq!(version.to_string(), "1.2.3");
    assert!(!reason.is_empty());

    let tx = load_transaction_json(&root);
    assert_eq!(
        tx["phase"], "failed",
        "a failed rollback must leave the transaction in the failed phase"
    );
    assert_eq!(tx["operation"], "switch");
    let tx_dir = root
        .canonical
        .join("transactions")
        .join(tx["transaction_id"].as_str().unwrap());
    assert!(
        tx_dir.join("failed-data").is_dir(),
        "the interrupted data must be preserved for manual recovery"
    );
    assert!(
        tx_dir.join("replaced-release").is_dir(),
        "the failed target release must be preserved for manual recovery"
    );
    assert!(
        tx_dir.join("restore").is_dir(),
        "the extracted .lkb must be preserved for manual recovery"
    );
    assert!(
        root.canonical
            .join("backups")
            .join(tx["transaction_id"].as_str().unwrap())
            .join("host/resolv.conf")
            .is_dir()
    );
    assert_eq!(
        std::fs::read_link(root.canonical.join("current")).unwrap(),
        std::path::PathBuf::from("releases/1.2.3"),
        "the current link is restored before the rollback health check"
    );
    assert!(
        super::super::transaction::find_unfinished(&root)
            .unwrap()
            .is_none()
    );

    drop(tcp1);
    drop(tcp2);
    drop(tcp3);
    drop(udp);
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = std::fs::remove_dir_all(&root.install_root);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn network_takeover_requires_an_empty_data_directory() {
    let dir = temp_root("network-data-empty");
    let root = InstallRoot {
        install_root: dir.clone(),
        canonical: dir.clone(),
    };
    assert!(ensure_network_takeover_data_empty(&root).is_ok());

    std::fs::create_dir_all(dir.join("data")).unwrap();
    assert!(ensure_network_takeover_data_empty(&root).is_ok());
    std::fs::write(dir.join("data/existing"), b"keep").unwrap();
    assert!(matches!(
        ensure_network_takeover_data_empty(&root),
        Err(InstallError::ParameterUsage(_))
    ));
    let _ = std::fs::remove_dir_all(dir);
}
