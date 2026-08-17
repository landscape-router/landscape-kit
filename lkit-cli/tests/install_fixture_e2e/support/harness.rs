use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use super::ports::TestPorts;
use super::{
    LKIT, Pty, RepositoryServer, SYSTEMCTL_FIXTURE, TestWorld, VERSION, attach_pty,
    repository_files, write_json,
};

pub(crate) struct InstallHarness {
    pub(crate) world: TestWorld,
    /// lkit 地盘:config/state/transactions/backups/logs/run 全部位于此处,
    /// 通过 `LKIT_TERRITORY` 指向。landscape 安装根是独立目录 `install_root`。
    pub(crate) territory: PathBuf,
    pub(crate) install_root: PathBuf,
    pub(crate) host: PathBuf,
    pub(crate) runtime_config: PathBuf,
    pub(crate) password: PathBuf,
    pub(crate) flare_psk: PathBuf,
    pub(crate) ip_state: PathBuf,
    pub(crate) repository: RepositoryServer,
}

impl InstallHarness {
    pub(crate) fn new(name: &str, scenario: &str, startup_timeout_ms: u64) -> Self {
        let world = TestWorld::new(name);
        let territory = world.path("territory");
        let install_root = world.path("install");
        let host = world.path("host");
        let unit_dir = host.join("units");
        let run_systemd_dir = host.join("run/systemd/system");
        let systemd_state = host.join("systemd-state");
        std::fs::create_dir_all(&territory).unwrap();
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
        let flare_psk = world.path("flare-psk");
        std::fs::write(&flare_psk, b"fixture-flare-recovery-secret\n").unwrap();
        std::fs::set_permissions(&flare_psk, std::fs::Permissions::from_mode(0o600)).unwrap();
        let repository = RepositoryServer::start(repository_files());
        Self {
            world,
            territory,
            install_root,
            host,
            runtime_config,
            password,
            flare_psk,
            ip_state,
            repository,
        }
    }

    /// lkit 地盘路径:config/state/transactions/backups/logs/run。
    pub(crate) fn state_path(&self) -> PathBuf {
        self.territory.join("state/install-state.json")
    }

    pub(crate) fn config_path(&self) -> PathBuf {
        self.territory.join("config.toml")
    }

    pub(crate) fn transactions_dir(&self) -> PathBuf {
        self.territory.join("transactions")
    }

    pub(crate) fn backups_dir(&self) -> PathBuf {
        self.territory.join("backups")
    }

    pub(crate) fn logs_dir(&self) -> PathBuf {
        self.territory.join("logs")
    }

    pub(crate) fn run_dir(&self) -> PathBuf {
        self.territory.join("run")
    }

    /// 统一的 lkit 命令构造点:注入 fake systemctl 配置与指向 fixture 世界的
    /// `LKIT_TERRITORY`。除 install/migrate(显式传 `--install-dir`)外,
    /// 所有命令都经由此处且不接收 install-dir,从地盘状态发现 landscape 根。
    pub(crate) fn command(&self) -> Command {
        let mut command = Command::new(LKIT);
        command
            .env(
                lkit_test_fixture::SYSTEMCTL_CONFIG_ENV,
                &self.world.systemctl_config,
            )
            .env("LKIT_TERRITORY", &self.territory)
            .env("LKIT_GLOBAL_DIR", self.host.join("usr/local"));
        command
    }

    /// fixture 世界里的全局 lkit 二进制 `/usr/local/bin/lkit`:`lkit self install`
    /// 校验它可执行,daemon unit 的 `ExecStart` 直接引用它。
    pub(crate) fn seed_global_lkit_binary(&self) {
        let dir = self.host.join("usr/local/bin");
        std::fs::create_dir_all(&dir).unwrap();
        let binary = dir.join("lkit");
        if !binary.exists() {
            std::os::unix::fs::symlink(LKIT, &binary).unwrap();
        }
    }

    pub(crate) fn run(&self) -> Output {
        self.command()
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
            .args(["--flare-psk-file"])
            .arg(&self.flare_psk)
            .args(["--test-runtime"])
            .arg(&self.runtime_config)
            .output()
            .unwrap()
    }

    pub(crate) fn password_prompt_command(&self, pty: &Pty) -> Command {
        let mut command = self.command();
        command
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
        let mut command = self.command();
        command
            .arg("update")
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
        self.command()
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
            .args(["--flare-psk-file"])
            .arg(&self.flare_psk)
            .args(["--test-runtime"])
            .arg(&self.runtime_config)
            .output()
            .unwrap()
    }

    pub(crate) fn network_command(&self, action: &[&str]) -> Output {
        self.command()
            .env("SSH_CONNECTION", "203.0.113.9 41000 10.1.1.105 22")
            .arg("network")
            .args(action)
            .arg("--test-runtime")
            .arg(&self.runtime_config)
            .output()
            .unwrap()
    }
}

/// 写一份可被 `read_state` 接受的有效安装状态(用于无真实安装的命令路径,
/// 如 daemon 恢复现场的根发现)。`canonical_install_root` 指向 `install_root`。
pub(crate) fn write_valid_state_at(state_path: &Path, install_root: &Path, active_version: &str) {
    std::fs::create_dir_all(state_path.parent().unwrap()).unwrap();
    let state = serde_json::json!({
        "schema_version": 1,
        "layout_version": 2,
        "install_root": install_root.display().to_string(),
        "canonical_install_root": install_root.display().to_string(),
        "active_version": active_version,
        "assets": {
            "webserver": {
                "architecture": "x86_64",
                "sha256": "a".repeat(64),
                "size": 10,
            },
            "static_archive": {
                "sha256": "b".repeat(64),
                "size": 20,
            },
        },
        "initialization": {
            "status": "complete",
            "lock_present": true,
            "initialized_at": "2026-08-01T16:30:00Z",
        },
        "service": {
            "manager": "systemd",
            "registered": true,
            "enabled": true,
            "verified": true,
            "definition_path": "service/landscape-router.service",
            "definition_sha256": "c".repeat(64),
        },
        "last_transaction_id": null,
        "committed_at": "2026-08-01T16:30:00Z",
    });
    write_json(state_path, &state);
}
