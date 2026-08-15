use std::collections::HashMap;
use std::io::Write;
use std::net::UdpSocket;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use super::super::health::PortCheck;
use crate::service::systemd::Systemd;

use sha2::{Digest, Sha256};

use super::super::repository::provider_for;
use super::super::repository::test_server::{TestResponse, TestServer};
use super::*;

pub(crate) const WEBSERVER_PAYLOAD: &[u8] = b"landscape webserver payload";

pub(crate) fn temp_root(name: &str) -> std::path::PathBuf {
    let root =
        std::env::temp_dir().join(format!("lkit-pipeline-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

pub(crate) fn sha256_bytes(bytes: &[u8]) -> (String, u64) {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    (hex(&hasher.finalize()), bytes.len() as u64)
}

pub(crate) fn build_static_zip() -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    writer.start_file("static/index.html", options).unwrap();
    writer.write_all(b"<h1>hello</h1>").unwrap();
    writer.start_file("static/app.js", options).unwrap();
    writer.write_all(b"console.log(1);").unwrap();
    writer.finish().unwrap().into_inner()
}

pub(crate) fn repository_files_for(version: &str, payload: &[u8]) -> HashMap<String, Vec<u8>> {
    let webserver_zst = zstd::encode_all(payload, 3).unwrap();
    let (webserver_sha, webserver_size) = sha256_bytes(&webserver_zst);
    let static_zip = build_static_zip();
    let (static_sha, static_size) = sha256_bytes(&static_zip);
    let manifest = serde_json::json!({
        "protocol_version": 1,
        "version": version,
        "assets": {
            "webserver": {
                "x86_64": {
                    "url": "landscape-webserver-x86_64.zst",
                    "sha256": webserver_sha,
                    "size": webserver_size,
                }
            },
            "static": {
                "url": "static.zip",
                "sha256": static_sha,
                "size": static_size,
            }
        }
    })
    .to_string();
    HashMap::from([
        (
            "/repository.json".into(),
            br#"{"protocol_version":1}"#.to_vec(),
        ),
        (
            "/channels/stable.json".to_string(),
            format!(r#"{{"protocol_version":1,"version":"{version}"}}"#).into_bytes(),
        ),
        (
            format!("/releases/{version}/manifest.json"),
            manifest.into_bytes(),
        ),
        (
            format!("/releases/{version}/landscape-webserver-x86_64.zst"),
            webserver_zst,
        ),
        (format!("/releases/{version}/static.zip"), static_zip),
    ])
}

pub(crate) fn repository_files() -> (HashMap<String, Vec<u8>>, Vec<u8>) {
    (
        repository_files_for("1.2.3", WEBSERVER_PAYLOAD),
        WEBSERVER_PAYLOAD.to_vec(),
    )
}

pub(crate) fn start_repository(
    name: &str,
    files: HashMap<String, Vec<u8>>,
) -> (TestServer, InstallRoot, ReleaseProvider) {
    let server = TestServer::start(move |path| match files.get(path) {
        Some(body) => TestResponse::ok(body.clone()),
        None => TestResponse::status(404, "Not Found", Vec::new()),
    });
    let root = temp_root(name);
    let install_root = InstallRoot {
        install_root: root.clone(),
        canonical: root,
    };
    let provider = provider_for(ProviderKind::Http, &server.base).unwrap();
    (server, install_root, provider)
}

pub(crate) fn credentials() -> Credentials {
    Credentials {
        admin_user: "admin".into(),
        password: "Secret123".into(),
    }
}

pub(crate) struct FakeDocs;

impl DocsProbe for FakeDocs {
    async fn docs_ok(&self) -> bool {
        true
    }
}

pub(crate) struct ToggleDocs {
    pub(crate) ok: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl DocsProbe for ToggleDocs {
    async fn docs_ok(&self) -> bool {
        self.ok.load(std::sync::atomic::Ordering::Relaxed)
    }
}

pub(crate) struct FailSecondDocs {
    pub(crate) calls: std::sync::atomic::AtomicUsize,
}

impl DocsProbe for FailSecondDocs {
    async fn docs_ok(&self) -> bool {
        // Target startup succeeds, target observation fails, then rollback probes recover.
        self.calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            != 1
    }
}

/// 以 `current` 符号链接目标作为阶段信号:目标版本激活期间指向
/// `releases/1.3.0`,`/api/docs` 持续失败;回滚恢复为 1.2.3 后通过。
/// 事件驱动,不依赖墙钟,消除并行负载下的时序竞态。
pub(crate) struct CurrentSymlinkDocs {
    pub(crate) root: std::path::PathBuf,
    pub(crate) rollback_target: std::path::PathBuf,
}

impl DocsProbe for CurrentSymlinkDocs {
    async fn docs_ok(&self) -> bool {
        std::fs::read_link(self.root.join("current"))
            .map(|target| target == self.rollback_target)
            .unwrap_or(false)
    }
}

pub(crate) fn test_options() -> HealthOptions<FakeDocs> {
    HealthOptions {
        docs: FakeDocs,
        ports: Vec::new(),
        startup_timeout: Duration::from_secs(10),
        stable_duration: Duration::from_millis(100),
    }
}

pub(crate) fn none_options() -> HealthOptions<FakeDocs> {
    test_options()
}

/// 探测返回 Available 的 systemd(不提供 systemctl 脚本,不执行注册/启动)。
/// 探测返回 Available 的 systemd(仅响应版本查询,不执行注册/启动)。
pub(crate) fn available_systemd(dir: &std::path::Path) -> Systemd {
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

pub(crate) fn version() -> semver::Version {
    semver::Version::new(1, 2, 3)
}

pub(crate) fn fake_systemd(dir: &std::path::Path, main_pid: u32) -> Systemd {
    let script = dir.join("systemctl");
    std::fs::write(
        &script,
        format!(
            r#"#!/bin/sh
case "$*" in
  "show --property=MainPID --value landscape-router.service") echo {main_pid};;
  "show --property=ActiveState --value landscape-router.service") echo active;;
  "is-enabled landscape-router.service") echo enabled;;
  "is-active landscape-router.service") echo active;;
  *) exit 0;;
esac
"#
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::create_dir_all(dir.join("run")).unwrap();
    std::fs::create_dir_all(dir.join("units")).unwrap();
    Systemd {
        systemctl: script,
        system_unit_dir: dir.join("units"),
        run_systemd_dir: dir.join("run"),
        pid1_is_systemd: true,
        resolv_conf: dir.join("resolv.conf"),
    }
}

#[tokio::test]
async fn performs_first_install_from_http_repository() {
    use std::net::{TcpListener, UdpSocket};

    let (server, root, provider) = start_repository("e2e-explicit", repository_files().0);
    let dir = std::env::temp_dir().join(format!(
        "lkit-pipeline-test-explicit-{}",
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

    let systemd = fake_systemd(&dir, std::process::id());
    let data_dir = root.canonical.join("data");
    let watcher = std::thread::spawn(move || {
        loop {
            if data_dir.join("landscape_init.toml").is_file() {
                std::fs::write(data_dir.join("landscape_init.lock"), b"").unwrap();
                std::fs::write(data_dir.join("landscape.toml"), b"").unwrap();
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    });

    let options = HealthOptions {
        docs: FakeDocs,
        ports: ports.clone(),
        startup_timeout: Duration::from_secs(15),
        stable_duration: Duration::from_millis(100),
    };
    let outcome = first_install(
        &root,
        &provider,
        &TargetVersion::Version(version()),
        &credentials(),
        &systemd,
        &options,
    )
    .await
    .unwrap();
    watcher.join().unwrap();
    assert_eq!(outcome.release.version, version());

    let binary = root.canonical.join("releases/1.2.3/landscape-webserver");
    assert_eq!(std::fs::read(&binary).unwrap(), WEBSERVER_PAYLOAD);
    let mode = std::fs::metadata(&binary).unwrap().permissions().mode();
    assert_eq!(mode & 0o111, 0o111);

    let index = root.canonical.join("releases/1.2.3/static/index.html");
    assert_eq!(std::fs::read_to_string(&index).unwrap(), "<h1>hello</h1>");

    assert_eq!(
        std::fs::read_link(root.canonical.join("current")).unwrap(),
        std::path::PathBuf::from("releases/1.2.3")
    );

    let init_config =
        std::fs::read_to_string(root.canonical.join("data/landscape_init.toml")).unwrap();
    assert!(init_config.contains("version = \"1.2.3\""));
    assert!(init_config.contains("admin_user = \"admin\""));
    assert!(init_config.contains("admin_pass = \"Secret123\""));

    let state = super::super::state::load_state(&root).unwrap().unwrap();
    assert_eq!(state.active_version, "1.2.3");
    assert_eq!(state.initialization.status, InitStatus::Complete);
    assert!(state.initialization.lock_present);
    assert!(state.initialization.initialized_at.is_some());
    assert_eq!(state.service.manager, StateServiceManager::Systemd);
    assert!(state.service.registered);
    assert!(state.service.enabled);
    assert!(state.service.verified);
    assert_eq!(
        state.assets.webserver.sha256,
        super::hex(&{
            let mut hasher = Sha256::new();
            hasher.update(WEBSERVER_PAYLOAD);
            hasher.finalize()
        })
    );

    assert!(
        server
            .request_paths()
            .contains(&"/releases/1.2.3/landscape-webserver-x86_64.zst".into())
    );
    assert!(
        server
            .request_paths()
            .contains(&"/releases/1.2.3/static.zip".into())
    );

    assert!(
        super::super::transaction::find_unfinished(&root)
            .unwrap()
            .is_none()
    );
    let tx_files: Vec<_> = std::fs::read_dir(root.canonical.join("transactions"))
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert_eq!(tx_files.len(), 1);
    drop(tcp1);
    drop(tcp2);
    drop(tcp3);
    drop(udp);
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&root.install_root);
}

#[tokio::test]
async fn first_install_fails_without_available_systemd() {
    let (files, _) = repository_files();
    let (_server, root, provider) = start_repository("e2e-systemd-unavail", files);
    let dir = std::env::temp_dir().join(format!(
        "lkit-pipeline-test-systemd-unavail-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let systemd = Systemd {
        systemctl: dir.join("systemctl"),
        system_unit_dir: dir.join("units"),
        run_systemd_dir: dir.join("missing-run"),
        pid1_is_systemd: true,
        resolv_conf: dir.join("resolv.conf"),
    };
    assert!(matches!(
        first_install(
            &root,
            &provider,
            &TargetVersion::Version(version()),
            &credentials(),
            &systemd,
            &none_options(),
        )
        .await,
        Err(InstallError::UnsupportedPlatform(_))
    ));
    assert!(!root.canonical.join("state/install-state.json").exists());
    let _ = std::fs::remove_dir_all(&root.install_root);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn first_install_with_systemd_registers_and_verifies() {
    assert_systemd_first_install("e2e-systemd").await;
}

pub(crate) async fn assert_systemd_first_install(case: &str) {
    use std::net::{TcpListener, UdpSocket};

    let (files, payload) = repository_files();
    let (server, root, provider) = start_repository(case, files);
    let dir =
        std::env::temp_dir().join(format!("lkit-pipeline-test-{case}-{}", std::process::id()));
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

    let systemd = fake_systemd(&dir, std::process::id());
    let data_dir = root.canonical.join("data");
    let watcher = std::thread::spawn(move || {
        loop {
            if data_dir.join("landscape_init.toml").is_file() {
                std::fs::write(data_dir.join("landscape_init.lock"), b"").unwrap();
                std::fs::write(data_dir.join("landscape.toml"), b"").unwrap();
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    });

    let options = HealthOptions {
        docs: FakeDocs,
        ports: ports.clone(),
        startup_timeout: Duration::from_secs(15),
        stable_duration: Duration::from_millis(100),
    };
    let outcome = first_install(
        &root,
        &provider,
        &TargetVersion::Version(version()),
        &credentials(),
        &systemd,
        &options,
    )
    .await
    .unwrap();
    watcher.join().unwrap();
    assert_eq!(outcome.release.version, version());

    assert!(dir.join("units/landscape-router.service").is_symlink());
    let unit_origin = root.canonical.join("service/landscape-router.service");
    assert!(unit_origin.is_file());
    assert_eq!(
        std::fs::read_link(dir.join("units/landscape-router.service")).unwrap(),
        unit_origin.canonicalize().unwrap()
    );

    let state = super::super::state::load_state(&root).unwrap().unwrap();
    assert_eq!(state.service.manager, StateServiceManager::Systemd);
    assert!(state.service.registered);
    assert!(state.service.enabled);
    assert!(state.service.verified);
    assert_eq!(
        state.service.definition_path.as_deref(),
        Some("service/landscape-router.service")
    );
    assert_eq!(state.initialization.status, InitStatus::Complete);
    assert!(state.initialization.lock_present);
    assert!(state.initialization.initialized_at.is_some());

    assert!(
        super::super::transaction::find_unfinished(&root)
            .unwrap()
            .is_none()
    );
    let tx = load_transaction_json(&root);
    assert_eq!(tx["phase"], "committed");
    assert!(tx["systemd_before"]["registration"]["kind"] == "missing");
    let resolv_backup = tx["resolv_conf_backup"].as_str().unwrap();
    assert!(
        root.canonical.join(resolv_backup).is_dir(),
        "resolv backup dir missing: {resolv_backup}"
    );

    assert!(!server.request_paths().is_empty());
    let binary = root.canonical.join("releases/1.2.3/landscape-webserver");
    assert_eq!(std::fs::read(&binary).unwrap(), payload);

    drop(tcp1);
    drop(tcp2);
    drop(tcp3);
    drop(udp);
    let _ = std::fs::remove_dir_all(&root.install_root);
    let _ = std::fs::remove_dir_all(&dir);
}

pub(crate) fn load_transaction_json(root: &InstallRoot) -> serde_json::Value {
    let entries: Vec<_> = std::fs::read_dir(root.canonical.join("transactions"))
        .unwrap()
        .filter_map(Result::ok)
        .collect();
    assert!(!entries.is_empty());
    // uuid v7 按时间排序,取最新的交易。
    let newest = entries
        .into_iter()
        .max_by(|a, b| a.file_name().cmp(&b.file_name()))
        .unwrap();
    let bytes = std::fs::read(newest.path()).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn resolves_latest_stable_from_channel() {
    use std::net::{TcpListener, UdpSocket};

    let (_server, root, provider) = start_repository("e2e-latest", repository_files().0);
    let dir =
        std::env::temp_dir().join(format!("lkit-pipeline-test-latest-{}", std::process::id()));
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

    let systemd = fake_systemd(&dir, std::process::id());
    let data_dir = root.canonical.join("data");
    let watcher = std::thread::spawn(move || {
        loop {
            if data_dir.join("landscape_init.toml").is_file() {
                std::fs::write(data_dir.join("landscape_init.lock"), b"").unwrap();
                std::fs::write(data_dir.join("landscape.toml"), b"").unwrap();
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    });

    let options = HealthOptions {
        docs: FakeDocs,
        ports: ports.clone(),
        startup_timeout: Duration::from_secs(15),
        stable_duration: Duration::from_millis(100),
    };
    let outcome = first_install(
        &root,
        &provider,
        &TargetVersion::Latest,
        &credentials(),
        &systemd,
        &options,
    )
    .await
    .unwrap();
    watcher.join().unwrap();
    assert_eq!(outcome.release.version, version());
    drop(tcp1);
    drop(tcp2);
    drop(tcp3);
    drop(udp);
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&root.install_root);
}

#[tokio::test]
async fn fails_without_stable_channel() {
    let (files, _) = repository_files();
    let mut files = files;
    files.remove("/channels/stable.json");
    let (_server, root, provider) = start_repository("e2e-missing", files);
    let dir =
        std::env::temp_dir().join(format!("lkit-pipeline-test-missing-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let systemd = available_systemd(&dir);
    assert!(matches!(
        first_install(
            &root,
            &provider,
            &TargetVersion::Latest,
            &credentials(),
            &systemd,
            &none_options(),
        )
        .await,
        Err(InstallError::NoStableVersion)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&root.install_root);
}

#[tokio::test]
async fn cleans_up_on_asset_download_failure() {
    let (mut files, _) = repository_files();
    let asset_path = "/releases/1.2.3/landscape-webserver-x86_64.zst";
    files.remove(asset_path);
    let (server, root, provider) = start_repository("e2e-download-failure", files);
    let dir = std::env::temp_dir().join(format!(
        "lkit-pipeline-test-download-failure-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let systemd = available_systemd(&dir);

    assert!(
        first_install(
            &root,
            &provider,
            &TargetVersion::Version(version()),
            &credentials(),
            &systemd,
            &none_options(),
        )
        .await
        .is_err()
    );
    let _ = std::fs::remove_dir_all(&dir);
    assert!(server.request_paths().contains(&asset_path.to_string()));
    assert_failed_first_install_cleanup(&root);
    let _ = std::fs::remove_dir_all(&root.install_root);
}

#[tokio::test]
async fn cleans_up_on_corrupted_webserver_archive() {
    let (files, _) = repository_files();
    let (webserver_sha, webserver_size) = sha256_bytes(b"garbage");
    let manifest = serde_json::json!({
        "protocol_version": 1,
        "version": "1.2.3",
        "assets": {
            "webserver": {
                "x86_64": {
                    "url": "landscape-webserver-x86_64.zst",
                    "sha256": webserver_sha,
                    "size": webserver_size,
                }
            },
            "static": {
                "url": "static.zip",
                "sha256": "b".repeat(64),
                "size": 1,
            }
        }
    })
    .to_string();
    let mut files = files;
    files.insert(
        "/releases/1.2.3/manifest.json".into(),
        manifest.into_bytes(),
    );
    files.insert(
        "/releases/1.2.3/landscape-webserver-x86_64.zst".into(),
        b"garbage".to_vec(),
    );
    let (_server, root, provider) = start_repository("e2e-corrupt", files);
    let dir =
        std::env::temp_dir().join(format!("lkit-pipeline-test-corrupt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let systemd = available_systemd(&dir);
    assert!(
        first_install(
            &root,
            &provider,
            &TargetVersion::Version(version()),
            &credentials(),
            &systemd,
            &none_options(),
        )
        .await
        .is_err()
    );
    assert_failed_first_install_cleanup(&root);
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&root.install_root);
}

#[tokio::test]
async fn cleans_up_on_invalid_static_archive() {
    let (files, _) = repository_files();
    let invalid_static = b"not a zip archive";
    let (static_sha, static_size) = sha256_bytes(invalid_static);
    let webserver_zst = files
        .get("/releases/1.2.3/landscape-webserver-x86_64.zst")
        .unwrap();
    let (webserver_sha, webserver_size) = sha256_bytes(webserver_zst);
    let manifest = serde_json::json!({
        "protocol_version": 1,
        "version": "1.2.3",
        "assets": {
            "webserver": {
                "x86_64": {
                    "url": "landscape-webserver-x86_64.zst",
                    "sha256": webserver_sha,
                    "size": webserver_size,
                }
            },
            "static": {
                "url": "static.zip",
                "sha256": static_sha,
                "size": static_size,
            }
        }
    })
    .to_string();
    let mut files = files;
    files.insert(
        "/releases/1.2.3/manifest.json".into(),
        manifest.into_bytes(),
    );
    files.insert("/releases/1.2.3/static.zip".into(), invalid_static.to_vec());
    let (server, root, provider) = start_repository("e2e-invalid-static", files);
    let dir = std::env::temp_dir().join(format!(
        "lkit-pipeline-test-invalid-static-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let systemd = available_systemd(&dir);

    assert!(
        first_install(
            &root,
            &provider,
            &TargetVersion::Version(version()),
            &credentials(),
            &systemd,
            &none_options(),
        )
        .await
        .is_err()
    );
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        server
            .request_paths()
            .contains(&"/releases/1.2.3/static.zip".to_string())
    );
    assert_failed_first_install_cleanup(&root);
    let _ = std::fs::remove_dir_all(&root.install_root);
}

pub(crate) fn assert_failed_first_install_cleanup(root: &InstallRoot) {
    assert!(!root.canonical.join("current").exists());
    assert!(!root.canonical.join("releases/1.2.3").exists());
    assert!(!root.canonical.join("data/landscape_init.toml").exists());
    assert!(!root.canonical.join("state/install-state.json").exists());
    assert!(!root.canonical.join("releases/.install-1.2.3.tmp").exists());
    assert!(
        super::super::transaction::find_unfinished(root)
            .unwrap()
            .is_none()
    );
    assert_eq!(load_transaction_json(root)["phase"], "failed");
}

#[tokio::test]
async fn rejects_existing_release_directory() {
    let (_server, root, provider) = start_repository("e2e-exists", repository_files().0);
    std::fs::create_dir_all(root.canonical.join("releases/1.2.3")).unwrap();
    let dir =
        std::env::temp_dir().join(format!("lkit-pipeline-test-exists-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let systemd = available_systemd(&dir);
    assert!(matches!(
        first_install(
            &root,
            &provider,
            &TargetVersion::Version(version()),
            &credentials(),
            &systemd,
            &none_options(),
        )
        .await,
        Err(InstallError::ReleaseExists(_))
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&root.install_root);
}

pub(crate) fn export_body(version: &str) -> Vec<u8> {
    serde_json::json!({
            "data": {
                "filename": format!("landscape_init_v{version}.toml"),
                "version": version,
                "content": format!("version = \"{version}\"\n\n[config.auth]\nadmin_user = \"admin\"\nadmin_pass = \"Secret123\"\n"),
            }
        })
        .to_string()
        .into_bytes()
}
