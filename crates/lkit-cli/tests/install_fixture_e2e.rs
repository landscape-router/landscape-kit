#![cfg(feature = "test-support")]

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, UdpSocket};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

const VERSION: &str = "1.2.3";
const LKIT: &str = env!("CARGO_BIN_EXE_lkit");
const LANDSCAPE_FIXTURE: &str = env!("CARGO_BIN_EXE_lkit-landscape-fixture");
const SYSTEMCTL_FIXTURE: &str = env!("CARGO_BIN_EXE_lkit-test-systemctl");

struct TestWorld {
    root: PathBuf,
    systemctl_config: PathBuf,
}

impl TestWorld {
    fn new(name: &str) -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "lkit-cli-fixture-{name}-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        Self {
            systemctl_config: root.join("systemctl.json"),
            root,
        }
    }

    fn path(&self, path: &str) -> PathBuf {
        self.root.join(path)
    }
}

impl Drop for TestWorld {
    fn drop(&mut self) {
        if self.systemctl_config.is_file() {
            let _ = Command::new(SYSTEMCTL_FIXTURE)
                .env(
                    lkit_test_fixture::SYSTEMCTL_CONFIG_ENV,
                    &self.systemctl_config,
                )
                .args(["stop", "landscape-router.service"])
                .output();
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

struct RepositoryServer {
    base_url: String,
}

impl RepositoryServer {
    fn start(files: HashMap<String, Vec<u8>>) -> Self {
        let files = Arc::new(files);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut request = [0u8; 8192];
                let Ok(size) = stream.read(&mut request) else {
                    continue;
                };
                let request = String::from_utf8_lossy(&request[..size]);
                let path = request.split_whitespace().nth(1).unwrap_or("/");
                let (status, reason, body) = match files.get(path) {
                    Some(body) => (200, "OK", body.as_slice()),
                    None => (404, "Not Found", &[][..]),
                };
                let head = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                if stream.write_all(head.as_bytes()).is_ok() {
                    let _ = stream.write_all(body);
                    let _ = stream.flush();
                }
            }
        });
        Self {
            base_url: format!("http://{address}/"),
        }
    }
}

struct InstallHarness {
    world: TestWorld,
    install_root: PathBuf,
    host: PathBuf,
    runtime_config: PathBuf,
    password: PathBuf,
    repository: RepositoryServer,
}

impl InstallHarness {
    fn new(name: &str, scenario: &str, startup_timeout_ms: u64) -> Self {
        let world = TestWorld::new(name);
        let install_root = world.path("install");
        let host = world.path("host");
        let unit_dir = host.join("units");
        let run_systemd_dir = host.join("run/systemd/system");
        let systemd_state = host.join("systemd-state");
        std::fs::create_dir_all(&unit_dir).unwrap();
        std::fs::create_dir_all(&run_systemd_dir).unwrap();
        std::fs::create_dir_all(&systemd_state).unwrap();
        std::fs::write(host.join("resolv.conf"), b"nameserver 127.0.0.1\n").unwrap();
        std::fs::write(host.join("os-release"), b"ID=debian\n").unwrap();

        let ports = TestPorts::reserve();
        let landscape_config = world.path("landscape.json");
        write_json(
            &landscape_config,
            &serde_json::json!({
                "schema_version": 1,
                "scenario": scenario,
                "listen_address": "127.0.0.1",
                "dns_tcp_port": ports.dns,
                "dns_udp_port": ports.dns,
                "http_port": ports.http,
                "https_port": ports.https,
                "export_version": VERSION,
                "export_content": format!("version = \"{VERSION}\"\n"),
            }),
        );
        write_json(
            &world.systemctl_config,
            &serde_json::json!({
                "schema_version": 1,
                "unit_dir": unit_dir,
                "state_dir": systemd_state,
                "landscape_config": landscape_config,
                "log_path": world.path("landscape.log"),
                "call_log": world.path("systemctl-calls.jsonl"),
                "systemd_version": "252.fixture",
            }),
        );

        let runtime_config = world.path("runtime.json");
        let current_uid = unsafe { libc::geteuid() };
        write_json(
            &runtime_config,
            &serde_json::json!({
                "schema_version": 1,
                "allow_non_root": true,
                "preflight": "skip",
                "execution": "inline",
                "managed_uid": current_uid,
                "os_release_path": host.join("os-release"),
                "systemd": {
                    "systemctl": SYSTEMCTL_FIXTURE,
                    "system_unit_dir": host.join("units"),
                    "run_systemd_dir": host.join("run/systemd/system"),
                    "pid1_is_systemd": true,
                    "resolv_conf": host.join("resolv.conf"),
                },
                "health": {
                    "base_url": format!("https://127.0.0.1:{}", ports.https),
                    "dns_tcp_port": ports.dns,
                    "dns_udp_port": ports.dns,
                    "http_port": ports.http,
                    "https_port": ports.https,
                    "startup_timeout_ms": startup_timeout_ms,
                    "stable_duration_ms": 1200,
                },
                "export_base_url": format!("https://127.0.0.1:{}", ports.https),
            }),
        );

        let password = world.path("password");
        std::fs::write(&password, b"Secret123\n").unwrap();
        std::fs::set_permissions(&password, std::fs::Permissions::from_mode(0o600)).unwrap();
        let repository = RepositoryServer::start(repository_files());
        Self {
            world,
            install_root,
            host,
            runtime_config,
            password,
            repository,
        }
    }

    fn run(&self) -> Output {
        Command::new(LKIT)
            .env(
                lkit_test_fixture::SYSTEMCTL_CONFIG_ENV,
                &self.world.systemctl_config,
            )
            .args([
                "install",
                "--version",
                VERSION,
                "--repository",
                &self.repository.base_url,
                "--install-dir",
            ])
            .arg(&self.install_root)
            .args(["--admin-user", "admin", "--password-file"])
            .arg(&self.password)
            .args(["--service-manager", "systemd", "--test-runtime"])
            .arg(&self.runtime_config)
            .output()
            .unwrap()
    }

    fn service_log(&self) -> String {
        std::fs::read_to_string(self.world.path("landscape.log")).unwrap_or_default()
    }
}

#[test]
fn installs_and_verifies_fixture_through_full_cli() {
    let harness = InstallHarness::new("first-install", "healthy", 10_000);
    let output = harness.run();
    assert!(
        output.status.success(),
        "lkit failed with {:?}\nstdout:\n{}\nstderr:\n{}\nservice log:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        harness.service_log()
    );

    let state: serde_json::Value = serde_json::from_slice(
        &std::fs::read(harness.install_root.join("state/install-state.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(state["active_version"], VERSION);
    assert_eq!(state["initialization"]["status"], "complete");
    assert_eq!(state["service"]["manager"], "systemd");
    assert_eq!(state["service"]["verified"], true);
    assert!(
        harness
            .install_root
            .join("data/landscape_init.lock")
            .is_file()
    );
    assert!(harness.install_root.join("data/landscape.toml").is_file());
    assert!(
        harness
            .install_root
            .join("data/landscape_db.sqlite")
            .is_file()
    );
    assert!(
        harness
            .install_root
            .join("current/landscape-webserver")
            .is_file()
    );

    let active = systemctl(&harness.world, &["is-active", "landscape-router.service"]);
    assert_success(&active);
    assert_eq!(String::from_utf8_lossy(&active.stdout).trim(), "active");
    let main_pid = systemctl(
        &harness.world,
        &[
            "show",
            "--property=MainPID",
            "--value",
            "landscape-router.service",
        ],
    );
    assert_success(&main_pid);
    let pid: u32 = String::from_utf8_lossy(&main_pid.stdout)
        .trim()
        .parse()
        .unwrap();
    assert!(Path::new(&format!("/proc/{pid}")).is_dir());
    let executable = std::fs::read_link(format!("/proc/{pid}/exe")).unwrap();
    assert_eq!(
        executable,
        harness
            .install_root
            .join(format!("releases/{VERSION}/landscape-webserver"))
            .canonicalize()
            .unwrap()
    );
}

#[test]
fn cleans_up_after_fixture_health_failure() {
    let harness = InstallHarness::new("health-failure", "health_error", 2_500);
    let output = harness.run();
    assert!(
        !output.status.success(),
        "health failure unexpectedly succeeded\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("health check failed"),
        "unexpected error:\n{}\nservice log:\n{}",
        String::from_utf8_lossy(&output.stderr),
        harness.service_log()
    );
    let active = systemctl(&harness.world, &["is-active", "landscape-router.service"]);
    assert_eq!(active.status.code(), Some(3));
    assert_eq!(String::from_utf8_lossy(&active.stdout).trim(), "inactive");
    assert!(!harness.host.join("units/landscape-router.service").exists());
    assert!(!harness.install_root.join("current").exists());
    assert!(
        !harness
            .install_root
            .join(format!("releases/{VERSION}"))
            .exists()
    );
    assert!(
        !harness
            .install_root
            .join("state/install-state.json")
            .exists()
    );
    assert_eq!(
        std::fs::read(harness.host.join("resolv.conf")).unwrap(),
        b"nameserver 127.0.0.1\n"
    );
}

fn systemctl(world: &TestWorld, args: &[&str]) -> Output {
    Command::new(SYSTEMCTL_FIXTURE)
        .env(
            lkit_test_fixture::SYSTEMCTL_CONFIG_ENV,
            &world.systemctl_config,
        )
        .args(args)
        .output()
        .unwrap()
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "command failed with {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write_json(path: &Path, value: &serde_json::Value) {
    std::fs::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

fn repository_files() -> HashMap<String, Vec<u8>> {
    let executable = std::fs::read(LANDSCAPE_FIXTURE).unwrap();
    let compressed = zstd::encode_all(executable.as_slice(), 3).unwrap();
    let static_zip = static_zip();
    let (webserver_sha, webserver_size) = sha256(&compressed);
    let (static_sha, static_size) = sha256(&static_zip);
    let architecture = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        architecture => panic!("unsupported test architecture {architecture}"),
    };
    let asset_name = format!("landscape-webserver-{architecture}.zst");
    let manifest = serde_json::json!({
        "protocol_version": 1,
        "version": VERSION,
        "assets": {
            "webserver": {
                architecture: {
                    "url": asset_name,
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
    });
    HashMap::from([
        (
            "/repository.json".into(),
            br#"{"protocol_version":1}"#.to_vec(),
        ),
        (
            "/channels/stable.json".into(),
            format!(r#"{{"protocol_version":1,"version":"{VERSION}"}}"#).into_bytes(),
        ),
        (
            format!("/releases/{VERSION}/manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        ),
        (format!("/releases/{VERSION}/{asset_name}"), compressed),
        (format!("/releases/{VERSION}/static.zip"), static_zip),
    ])
}

fn static_zip() -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    writer.start_file("static/index.html", options).unwrap();
    writer.write_all(b"<h1>Landscape fixture</h1>").unwrap();
    writer.finish().unwrap().into_inner()
}

fn sha256(bytes: &[u8]) -> (String, u64) {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    (
        digest.iter().map(|byte| format!("{byte:02x}")).collect(),
        bytes.len() as u64,
    )
}

struct TestPorts {
    dns: u16,
    http: u16,
    https: u16,
}

impl TestPorts {
    fn reserve() -> Self {
        let dns_tcp = TcpListener::bind("127.0.0.1:0").unwrap();
        let dns = dns_tcp.local_addr().unwrap().port();
        let dns_udp = UdpSocket::bind(("127.0.0.1", dns)).unwrap();
        let http = free_tcp_port();
        let https = free_tcp_port();
        drop(dns_udp);
        drop(dns_tcp);
        Self { dns, http, https }
    }
}

fn free_tcp_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}
