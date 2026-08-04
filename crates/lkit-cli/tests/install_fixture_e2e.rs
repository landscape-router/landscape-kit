#![cfg(feature = "test-support")]

use std::collections::HashMap;
use std::ffi::CStr;
use std::fs::File;
use std::io::{Read, Write};
use std::net::{TcpListener, UdpSocket};
use std::os::fd::FromRawFd;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

const VERSION: &str = "1.2.3";
const LKIT: &str = env!("CARGO_BIN_EXE_lkit");
const LANDSCAPE_FIXTURE: &str = env!("CARGO_BIN_EXE_lkit-landscape-fixture");
const SYSTEMCTL_FIXTURE: &str = env!("CARGO_BIN_EXE_lkit-test-systemctl");
static E2E_LOCK: Mutex<()> = Mutex::new(());

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
    ip_state: PathBuf,
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
        let sys_class_net = host.join("sys/class/net");
        for (name, mac) in [("ens3", "52:54:00:12:34:01"), ("ens4", "52:54:00:12:34:02")] {
            let iface = sys_class_net.join(name);
            std::fs::create_dir_all(&iface).unwrap();
            std::fs::write(iface.join("type"), b"1\n").unwrap();
            std::fs::write(iface.join("address"), format!("{mac}\n")).unwrap();
            std::fs::write(iface.join("operstate"), b"up\n").unwrap();
        }
        let ip_state = host.join("ip-state");
        std::fs::write(&ip_state, b"pre\n").unwrap();
        let ip_command = host.join("fake-ip");
        std::fs::write(
            &ip_command,
            format!(
                r#"#!/bin/sh
state=$(tr -d '\n' < '{}')
case "$*" in
  "-j -4 addr show dev br_lan")
    printf '%s\n' '[{{"ifname":"br_lan","addr_info":[{{"family":"inet","local":"192.168.10.1","prefixlen":24,"scope":"global"}}]}}]'
    ;;
  "-j link show master br_lan")
    printf '%s\n' '[{{"ifname":"ens4"}}]'
    ;;
  "-j -4 addr show")
    if [ "$state" = post ]; then
      printf '%s\n' '[{{"ifname":"ens3","addr_info":[]}},{{"ifname":"ens4","addr_info":[]}},{{"ifname":"br_lan","addr_info":[{{"family":"inet","local":"192.168.10.1","prefixlen":24,"scope":"global"}}]}}]'
    else
      printf '%s\n' '[{{"ifname":"ens3","addr_info":[{{"family":"inet","local":"198.51.100.20","prefixlen":24,"scope":"global"}}]}},{{"ifname":"ens4","addr_info":[]}}]'
    fi
    ;;
  "-j -4 route show default")
    printf '%s\n' '[{{"dev":"ens3","gateway":"198.51.100.1"}}]'
    ;;
  "-4 address flush dev ens3")
    printf '%s\n' post > '{}'
    ;;
  *)
    echo "unsupported fake ip arguments: $*" >&2
    exit 2
    ;;
esac
"#,
                ip_state.display(),
                ip_state.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&ip_command, std::fs::Permissions::from_mode(0o755)).unwrap();

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
                "sys_class_net": sys_class_net,
                "ip_command": ip_command,
                "selinux_fs_path": host.join("sys/fs/selinux"),
                "selinux_config_path": host.join("selinux/config"),
                "network_confirm_timeout_ms": 30000,
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
            ip_state,
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

    fn seed_host_services(&self) {
        for unit in [
            "NetworkManager.service",
            "firewalld.service",
            "systemd-resolved.service",
        ] {
            std::fs::write(self.host.join("units").join(unit), b"[Unit]\n").unwrap();
            let state = self.host.join("systemd-state/units").join(unit);
            std::fs::create_dir_all(&state).unwrap();
            std::fs::write(state.join("active"), b"active\n").unwrap();
            std::fs::write(state.join("enabled"), b"enabled\n").unwrap();
        }
    }

    fn run_takeover(&self) -> Output {
        let mut pty = Pty::open();
        pty.master.write_all(b"1\n1\n\n\n\n").unwrap();
        Command::new(LKIT)
            .env(
                lkit_test_fixture::SYSTEMCTL_CONFIG_ENV,
                &self.world.systemctl_config,
            )
            .env("LKIT_INTERNAL_SYSTEMD_WORKER_TTY", &pty.slave_path)
            .args([
                "install",
                "--takeover-network",
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

    fn network_command(&self, action: &[&str]) -> Output {
        Command::new(LKIT)
            .env(
                lkit_test_fixture::SYSTEMCTL_CONFIG_ENV,
                &self.world.systemctl_config,
            )
            .env("SSH_CONNECTION", "203.0.113.9 41000 192.168.10.1 22")
            .arg("network")
            .args(action)
            .arg("--install-dir")
            .arg(&self.install_root)
            .arg("--test-runtime")
            .arg(&self.runtime_config)
            .output()
            .unwrap()
    }
}

struct Pty {
    master: File,
    _slave: File,
    slave_path: PathBuf,
}

impl Pty {
    fn open() -> Self {
        let mut master = 0;
        let mut slave = 0;
        let mut name = [0 as libc::c_char; 128];
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    name.as_mut_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                )
            },
            0
        );
        let slave_path = unsafe { CStr::from_ptr(name.as_ptr()) }
            .to_str()
            .unwrap()
            .into();
        Self {
            master: unsafe { File::from_raw_fd(master) },
            _slave: unsafe { File::from_raw_fd(slave) },
            slave_path,
        }
    }
}

#[test]
fn installs_and_verifies_fixture_through_full_cli() {
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
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
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
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

#[test]
fn network_takeover_waits_for_reconnected_ssh_confirmation() {
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("network-confirm", "healthy", 10_000);
    harness.seed_host_services();
    let output = harness.run_takeover();
    assert!(
        output.status.success(),
        "takeover install failed with {:?}\nstdout:\n{}\nstderr:\n{}\nservice log:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        harness.service_log()
    );
    assert!(
        !harness
            .install_root
            .join("state/install-state.json")
            .exists()
    );
    let transaction = read_only_transaction(&harness.install_root);
    assert_eq!(transaction["phase"], "awaiting_network_confirmation");
    assert_eq!(
        transaction["network_takeover"]["plan"]["mode"]["mode"],
        "routed_lan"
    );

    let init: toml::Value = toml::from_str(
        &std::fs::read_to_string(harness.install_root.join("data/landscape_init.toml")).unwrap(),
    )
    .unwrap();
    assert!(init.get("ipconfigs").is_none());
    assert!(init.get("static_nat_mappings_v4").is_none());
    assert_eq!(init["route_wans"][0]["iface_name"].as_str(), Some("ens3"));
    assert_eq!(init["route_lans"][0]["iface_name"].as_str(), Some("br_lan"));
    assert_eq!(
        init["dhcpv4_services"][0]["config"]["server_ip_addr"].as_str(),
        Some("192.168.10.1")
    );
    assert_eq!(
        init["dhcpv4_services"][0]["config"]["ip_range_start"].as_str(),
        Some("192.168.10.100")
    );
    assert_eq!(
        init["dhcpv4_services"][0]["config"]["ip_range_end"].as_str(),
        Some("192.168.10.254")
    );
    assert_host_services_masked(&harness);

    let calls = std::fs::read_to_string(harness.world.path("systemctl-calls.jsonl")).unwrap();
    let timer_start = calls.find("\"start\",\"lkit-network-").unwrap();
    let resolved_stop = calls.find("\"stop\",\"systemd-resolved.service\"").unwrap();
    let network_manager_stop = calls.find("\"stop\",\"NetworkManager.service\"").unwrap();
    assert!(timer_start < resolved_stop);
    assert!(resolved_stop < network_manager_stop);

    let confirm = harness.network_command(&["confirm"]);
    assert_success(&confirm);
    assert_eq!(
        std::fs::read_to_string(&harness.ip_state).unwrap(),
        "post\n",
        "confirmation did not remove the inherited WAN IPv4 address"
    );
    let state: serde_json::Value = serde_json::from_slice(
        &std::fs::read(harness.install_root.join("state/install-state.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(state["active_version"], VERSION);
    let transaction = read_only_transaction(&harness.install_root);
    assert_eq!(transaction["phase"], "committed");
    assert!(
        std::fs::read_dir(harness.host.join("units"))
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("lkit-network-"))
    );
}

#[test]
fn automatic_network_rollback_restores_host_services() {
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("network-rollback", "healthy", 10_000);
    harness.seed_host_services();
    let output = harness.run_takeover();
    assert!(
        output.status.success(),
        "takeover install failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let pending = read_only_transaction(&harness.install_root);
    let recovery_units = [
        pending["network_takeover"]["rollback_service"]
            .as_str()
            .unwrap()
            .to_string(),
        pending["network_takeover"]["rollback_timer"]
            .as_str()
            .unwrap()
            .to_string(),
        pending["network_takeover"]["boot_rollback_service"]
            .as_str()
            .unwrap()
            .to_string(),
    ];
    let rollback = harness.network_command(&["rollback", "--automatic"]);
    assert_success(&rollback);
    assert!(
        !harness
            .install_root
            .join("state/install-state.json")
            .exists()
    );
    assert!(!harness.install_root.join("current").exists());
    let transaction = read_only_transaction(&harness.install_root);
    assert_eq!(transaction["phase"], "rolled_back");
    for unit in [
        "NetworkManager.service",
        "firewalld.service",
        "systemd-resolved.service",
    ] {
        let state = harness.host.join("systemd-state/units").join(unit);
        assert!(state.join("active").is_file(), "{unit} was not restarted");
        assert!(state.join("enabled").is_file(), "{unit} was not re-enabled");
        assert!(!state.join("masked").exists(), "{unit} remains masked");
        assert!(harness.host.join("units").join(unit).is_file());
    }
    let calls = std::fs::read_to_string(harness.world.path("systemctl-calls.jsonl")).unwrap();
    for unit in recovery_units {
        assert!(
            !calls.contains(&format!("[\"stop\",\"{unit}\"]")),
            "automatic recovery attempted to stop its own recovery unit {unit}"
        );
    }
}

fn read_only_transaction(install_root: &Path) -> serde_json::Value {
    let paths: Vec<PathBuf> = std::fs::read_dir(install_root.join("transactions"))
        .unwrap()
        .filter_map(|entry| {
            let path = entry.unwrap().path();
            (path.extension().and_then(|value| value.to_str()) == Some("json")).then_some(path)
        })
        .collect();
    assert_eq!(paths.len(), 1);
    serde_json::from_slice(&std::fs::read(&paths[0]).unwrap()).unwrap()
}

fn assert_host_services_masked(harness: &InstallHarness) {
    for unit in [
        "NetworkManager.service",
        "firewalld.service",
        "systemd-resolved.service",
    ] {
        let state = harness.host.join("systemd-state/units").join(unit);
        assert!(state.join("masked").is_file(), "{unit} was not masked");
        assert!(!state.join("active").exists(), "{unit} remains active");
        assert!(!state.join("enabled").exists(), "{unit} remains enabled");
        assert!(harness.host.join("units").join(unit).is_file());
    }
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
