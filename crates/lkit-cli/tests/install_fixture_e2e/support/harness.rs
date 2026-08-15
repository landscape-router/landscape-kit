use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

use super::ports::TestPorts;
use super::{
    LKIT, Pty, RepositoryServer, SYSTEMCTL_FIXTURE, TestWorld, VERSION, attach_pty,
    repository_files, write_json,
};

pub(crate) struct InstallHarness {
    pub(crate) world: TestWorld,
    pub(crate) install_root: PathBuf,
    pub(crate) host: PathBuf,
    pub(crate) runtime_config: PathBuf,
    pub(crate) password: PathBuf,
    pub(crate) ip_state: PathBuf,
    pub(crate) repository: RepositoryServer,
}

impl InstallHarness {
    pub(crate) fn new(name: &str, scenario: &str, startup_timeout_ms: u64) -> Self {
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
                "spawn_units": ["lkit.service"],
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

    pub(crate) fn run(&self) -> Output {
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
            .args(["--test-runtime"])
            .arg(&self.runtime_config)
            .output()
            .unwrap()
    }

    pub(crate) fn password_prompt_command(&self, pty: &Pty) -> Command {
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
            .args(["--admin-user", "admin", "--test-runtime"])
            .arg(&self.runtime_config);
        attach_pty(&mut command, pty);
        command
    }

    pub(crate) fn update_command(&self) -> Command {
        let mut command = Command::new(LKIT);
        command
            .env(
                lkit_test_fixture::SYSTEMCTL_CONFIG_ENV,
                &self.world.systemctl_config,
            )
            .arg("update")
            .arg("--install-dir")
            .arg(&self.install_root)
            .arg("--test-runtime")
            .arg(&self.runtime_config);
        command
    }

    pub(crate) fn service_log(&self) -> String {
        std::fs::read_to_string(self.world.path("landscape.log")).unwrap_or_default()
    }

    pub(crate) fn seed_host_services(&self) {
        for unit in [
            "NetworkManager.service",
            "firewalld.service",
            "systemd-resolved.service",
        ] {
            self.seed_host_service(unit);
        }
    }

    pub(crate) fn seed_host_service(&self, unit: &str) {
        std::fs::write(self.host.join("units").join(unit), b"[Unit]\n").unwrap();
        let state = self.host.join("systemd-state/units").join(unit);
        std::fs::create_dir_all(&state).unwrap();
        std::fs::write(state.join("active"), b"active\n").unwrap();
        std::fs::write(state.join("enabled"), b"enabled\n").unwrap();
    }

    pub(crate) fn run_takeover(&self) -> Output {
        let mut pty = Pty::open();
        pty.master.write_all(b"1\n1\n\n\n\n").unwrap();
        Command::new(LKIT)
            .env(
                lkit_test_fixture::SYSTEMCTL_CONFIG_ENV,
                &self.world.systemctl_config,
            )
            .env("LKIT_INTERNAL_DAEMON_TTY", &pty.slave_path)
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
            .args(["--test-runtime"])
            .arg(&self.runtime_config)
            .output()
            .unwrap()
    }

    pub(crate) fn network_command(&self, action: &[&str]) -> Output {
        Command::new(LKIT)
            .env(
                lkit_test_fixture::SYSTEMCTL_CONFIG_ENV,
                &self.world.systemctl_config,
            )
            .env("SSH_CONNECTION", "203.0.113.9 41000 10.1.1.105 22")
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
