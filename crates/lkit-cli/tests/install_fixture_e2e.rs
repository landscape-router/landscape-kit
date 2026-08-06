#![cfg(feature = "test-support")]

use std::collections::HashMap;
use std::ffi::CStr;
use std::fs::File;
use std::io::{Read, Write};
use std::net::{TcpListener, UdpSocket};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
  "-4 address flush dev ens4"|"-6 address flush dev ens4")
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
                "--non-interactive",
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

    fn password_prompt_command(&self, pty: &Pty) -> Command {
        let mut command = Command::new(LKIT);
        command
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
            .args([
                "--admin-user",
                "admin",
                "--service-manager",
                "none",
                "--test-runtime",
            ])
            .arg(&self.runtime_config);
        attach_pty(&mut command, pty);
        command
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
            self.seed_host_service(unit);
        }
    }

    fn seed_host_service(&self, unit: &str) {
        std::fs::write(self.host.join("units").join(unit), b"[Unit]\n").unwrap();
        let state = self.host.join("systemd-state/units").join(unit);
        std::fs::create_dir_all(&state).unwrap();
        std::fs::write(state.join("active"), b"active\n").unwrap();
        std::fs::write(state.join("enabled"), b"enabled\n").unwrap();
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
    slave: File,
    slave_path: PathBuf,
}

impl Pty {
    fn open() -> Self {
        let mut master = 0;
        let mut slave = 0;
        let mut name = [0 as libc::c_char; 128];
        let size = libc::winsize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    name.as_mut_ptr(),
                    std::ptr::null(),
                    &size,
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
            slave: unsafe { File::from_raw_fd(slave) },
            slave_path,
        }
    }

    fn read_until(&mut self, expected: &str, timeout: Duration) -> String {
        let deadline = Instant::now() + timeout;
        let mut output = Vec::new();
        while Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let mut descriptor = libc::pollfd {
                fd: self.master.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let timeout_ms = remaining.as_millis().min(100) as libc::c_int;
            let ready = unsafe { libc::poll(&mut descriptor, 1, timeout_ms) };
            if ready < 0 {
                panic!("poll pty: {}", std::io::Error::last_os_error());
            }
            if ready == 0 || descriptor.revents & libc::POLLIN == 0 {
                continue;
            }
            let mut buffer = [0_u8; 4096];
            let size = self.master.read(&mut buffer).unwrap();
            output.extend_from_slice(&buffer[..size]);
            if String::from_utf8_lossy(&output).contains(expected) {
                return String::from_utf8_lossy(&output).into_owned();
            }
        }
        panic!(
            "timed out waiting for {expected:?}; pty output:\n{}",
            String::from_utf8_lossy(&output)
        );
    }

    fn echo_enabled(&self) -> bool {
        let mut termios: libc::termios = unsafe { std::mem::zeroed() };
        assert_eq!(
            unsafe { libc::tcgetattr(self.slave.as_raw_fd(), &mut termios) },
            0
        );
        termios.c_lflag & libc::ECHO != 0
    }
}

fn attach_pty(command: &mut Command, pty: &Pty) {
    command
        .stdin(Stdio::from(pty.slave.try_clone().unwrap()))
        .stdout(Stdio::from(pty.slave.try_clone().unwrap()))
        .stderr(Stdio::from(pty.slave.try_clone().unwrap()));
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::ioctl(libc::STDIN_FILENO, libc::TIOCSCTTY, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
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
    assert!(
        !output.stdout.contains(&0x1b) && !output.stderr.contains(&0x1b),
        "non-interactive output contains terminal control sequences"
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
fn ctrl_c_during_password_restores_terminal_echo() {
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("password-sigint", "healthy", 10_000);
    let mut pty = Pty::open();
    assert!(pty.echo_enabled());
    let mut child = harness.password_prompt_command(&pty).spawn().unwrap();
    let output = pty.read_until("Enter admin password: ", Duration::from_secs(10));
    assert!(!pty.echo_enabled(), "password input did not disable echo");
    assert_eq!(
        unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGINT) },
        0
    );
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(130), "pty output:\n{output}");
    assert!(pty.echo_enabled(), "Ctrl+C did not restore terminal echo");
}

#[test]
fn explicit_non_interactive_mode_ignores_available_tty() {
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("explicit-non-interactive", "healthy", 10_000);
    let mut pty = Pty::open();
    let mut command = harness.password_prompt_command(&pty);
    command.arg("--non-interactive");
    let mut child = command.spawn().unwrap();
    let output = pty.read_until(
        "--password-file is required in non-interactive mode",
        Duration::from_secs(10),
    );
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(2), "pty output:\n{output}");
    assert!(!output.contains("Enter admin password"));
    assert!(pty.echo_enabled());
}

#[test]
fn bare_lkit_console_restores_terminal_on_exit() {
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let mut pty = Pty::open();
    let mut command = Command::new(LKIT);
    attach_pty(&mut command, &pty);
    let mut child = command.spawn().unwrap();
    let entered = pty.read_until("Landscape Kit", Duration::from_secs(5));
    assert!(
        entered.contains("\x1b[?1049h"),
        "console did not enter alternate screen: {entered:?}"
    );
    pty.master.write_all(b"\x1b").unwrap();
    let armed = pty.read_until("Exit armed", Duration::from_secs(5));
    assert!(
        child.try_wait().unwrap().is_none(),
        "console exited after one Esc: {armed:?}"
    );
    assert!(!armed.contains("Confirm exit"));
    pty.master.write_all(b"\x1b").unwrap();
    let confirmation = pty.read_until("Confirm exit", Duration::from_secs(5));
    assert!(
        child.try_wait().unwrap().is_none(),
        "console exited while showing confirmation: {confirmation:?}"
    );
    pty.master.write_all(b"\r").unwrap();
    let exited = pty.read_until("\x1b[?1049l", Duration::from_secs(5));
    let status = child.wait().unwrap();
    assert!(status.success(), "console exit failed: {exited:?}");
    assert!(
        pty.echo_enabled(),
        "console exit did not restore terminal echo"
    );
}

#[test]
fn ctrl_c_leaves_bare_lkit_console_and_restores_terminal() {
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let mut pty = Pty::open();
    let mut command = Command::new(LKIT);
    attach_pty(&mut command, &pty);
    let mut child = command.spawn().unwrap();
    let entered = pty.read_until("Landscape Kit", Duration::from_secs(5));
    assert!(
        entered.contains("\x1b[?1049h"),
        "console did not enter alternate screen: {entered:?}"
    );
    assert_eq!(
        unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGINT) },
        0
    );
    let exited = pty.read_until("\x1b[?1049l", Duration::from_secs(5));
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(130), "console output: {exited:?}");
    assert!(
        pty.echo_enabled(),
        "console Ctrl+C did not restore terminal echo"
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
    assert_eq!(init["ipconfigs"][0]["iface_name"].as_str(), Some("ens3"));
    assert_eq!(
        init["ipconfigs"][0]["ip_model"]["t"].as_str(),
        Some("static")
    );
    assert_eq!(
        init["ipconfigs"][0]["ip_model"]["ipv4"].as_str(),
        Some("198.51.100.20")
    );
    assert_eq!(
        init["ipconfigs"][0]["ip_model"]["default_router_ip"].as_str(),
        Some("198.51.100.1")
    );
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
    assert_host_services_masked(
        &harness,
        &[
            "NetworkManager.service",
            "firewalld.service",
            "systemd-resolved.service",
        ],
    );

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
        "pre\n",
        "confirmation removed the WAN address managed by the static plan"
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
    assert_host_services_restored(
        &harness,
        &[
            "NetworkManager.service",
            "firewalld.service",
            "systemd-resolved.service",
        ],
    );
    let calls = std::fs::read_to_string(harness.world.path("systemctl-calls.jsonl")).unwrap();
    for unit in recovery_units {
        assert!(
            !calls.contains(&format!("[\"stop\",\"{unit}\"]")),
            "automatic recovery attempted to stop its own recovery unit {unit}"
        );
    }
}

#[test]
fn network_takeover_supports_ifupdown_without_network_manager() {
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("network-ifupdown", "healthy", 10_000);
    harness.seed_host_service("networking.service");

    let output = harness.run_takeover();
    assert!(
        output.status.success(),
        "takeover with ifupdown failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let pending = read_only_transaction(&harness.install_root);
    let host_services = pending["network_takeover"]["host_services"]
        .as_array()
        .unwrap();
    let networking = host_services
        .iter()
        .find(|service| service["unit"] == "networking.service")
        .unwrap();
    assert_eq!(networking["installed"], true);
    assert_eq!(networking["active"], true);
    assert_eq!(networking["enable_state"], "enabled");
    let network_manager = host_services
        .iter()
        .find(|service| service["unit"] == "NetworkManager.service")
        .unwrap();
    assert_eq!(network_manager["installed"], false);
    assert_host_services_masked(&harness, &["networking.service"]);
    assert!(
        !harness.host.join("units/NetworkManager.service").exists(),
        "NetworkManager was unexpectedly installed"
    );

    let calls = std::fs::read_to_string(harness.world.path("systemctl-calls.jsonl")).unwrap();
    assert!(calls.contains("[\"stop\",\"networking.service\"]"));
    assert!(
        !calls.contains("[\"stop\",\"NetworkManager.service\"]"),
        "the missing NetworkManager unit was stopped"
    );

    let rollback = harness.network_command(&["rollback", "--automatic"]);
    assert_success(&rollback);
    assert_host_services_restored(&harness, &["networking.service"]);
}

#[test]
fn network_takeover_rejects_other_active_network_manager() {
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("network-unknown-manager", "healthy", 10_000);
    harness.seed_host_service("systemd-networkd.service");

    let output = harness.run_takeover();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains(
        "preflight check failed: unknown network manager systemd-networkd.service is active"
    ));
    assert!(
        !harness.install_root.join("transactions").exists(),
        "preflight created a transaction before rejecting an unknown manager"
    );
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

fn assert_host_services_masked(harness: &InstallHarness, units: &[&str]) {
    for unit in units {
        let state = harness.host.join("systemd-state/units").join(unit);
        assert!(state.join("masked").is_file(), "{unit} was not masked");
        assert!(!state.join("active").exists(), "{unit} remains active");
        assert!(!state.join("enabled").exists(), "{unit} remains enabled");
        assert!(harness.host.join("units").join(unit).is_file());
    }
}

fn assert_host_services_restored(harness: &InstallHarness, units: &[&str]) {
    for unit in units {
        let state = harness.host.join("systemd-state/units").join(unit);
        assert!(state.join("active").is_file(), "{unit} was not restarted");
        assert!(state.join("enabled").is_file(), "{unit} was not re-enabled");
        assert!(!state.join("masked").exists(), "{unit} remains masked");
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
