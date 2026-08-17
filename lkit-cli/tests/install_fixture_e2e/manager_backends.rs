use std::path::PathBuf;
use std::process::Command;

use super::support::world::TestWorld;
use super::support::{E2E_LOCK, LKIT, e2e_enabled, write_json};

const INIT_FIXTURE: &str = env!("CARGO_BIN_EXE_lkit-test-init");

/// `lkit self` 的服务管理器固定为 systemd(docs/commands/self.md):daemon 是
/// 全局单例(`lkit.service`),unit 原件在 `/usr/local/lib/lkit/lkit.service`。
/// 在只有 OpenRC/sysvinit 的宿主上 `self install` 必须拒绝(退出码 `2`,
/// "请求 systemd 但不可用"),且不遗留注册、原件或 daemon pidfile。
/// 这两条用例复用 OpenRC 与 sysvinit 的 fixture 世界,验证固定 systemd 语义
/// 在非 systemd 后端上的边界。
struct InitWorld {
    world: TestWorld,
    /// lkit 地盘(fixture 世界的 territory/),`self install` 经 `LKIT_TERRITORY`
    /// 指向它;daemon 若被拉起,pidfile 写这里。
    territory: PathBuf,
    runtime_config: PathBuf,
    init_config: PathBuf,
}

fn setup(name: &str, kind: &str) -> InitWorld {
    let world = TestWorld::new(name);
    let territory = world.path("territory");
    std::fs::create_dir_all(&territory).unwrap();
    let host = world.path("host");
    let init_d_dir = host.join("etc/init.d");
    let rc_d_dir = host.join("etc/rc.d");
    let init_state = host.join("init-state");
    for dir in [&init_d_dir, &rc_d_dir, &init_state] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(host.join("resolv.conf"), b"nameserver 127.0.0.1\n").unwrap();
    std::fs::write(host.join("os-release"), b"ID=alpine\n").unwrap();

    // 工具符号链接:同一 fixture 二进制按 argv[0] 分派角色。
    let tool_dir = host.join("tools");
    std::fs::create_dir_all(&tool_dir).unwrap();
    for tool in [
        "rc-service",
        "rc-update",
        "update-rc.d",
        "start-stop-daemon",
    ] {
        std::os::unix::fs::symlink(INIT_FIXTURE, tool_dir.join(tool)).unwrap();
    }

    let init_config = world.path("init.json");
    write_json(
        &init_config,
        &serde_json::json!({
            "schema_version": 1,
            "state_dir": init_state,
            "init_d_dir": init_d_dir,
            "rc_d_dir": rc_d_dir,
            "call_log": world.path("init-calls.jsonl"),
        }),
    );

    let runtime_config = world.path("runtime.json");
    let current_uid = unsafe { libc::geteuid() };
    let mut manager_block = serde_json::Map::new();
    manager_block.insert(
        "resolv_conf".into(),
        serde_json::Value::String(host.join("resolv.conf").display().to_string()),
    );
    if kind == "openrc" {
        manager_block.insert(
            "rc_service".into(),
            serde_json::Value::String(tool_dir.join("rc-service").display().to_string()),
        );
        manager_block.insert(
            "rc_update".into(),
            serde_json::Value::String(tool_dir.join("rc-update").display().to_string()),
        );
        manager_block.insert(
            "init_d_dir".into(),
            serde_json::Value::String(init_d_dir.display().to_string()),
        );
    } else {
        manager_block.insert(
            "update_rc_d".into(),
            serde_json::Value::String(tool_dir.join("update-rc.d").display().to_string()),
        );
        manager_block.insert(
            "init_d_dir".into(),
            serde_json::Value::String(init_d_dir.display().to_string()),
        );
        manager_block.insert(
            "rc_d_glob".into(),
            serde_json::Value::String(rc_d_dir.display().to_string()),
        );
    }
    write_json(
        &runtime_config,
        &serde_json::json!({
            "schema_version": 1,
            "allow_non_root": true,
            "preflight": "skip",
            "execution": "inline",
            "manager_kind": kind,
            "managed_uid": current_uid,
            "os_release_path": host.join("os-release"),
            "sys_class_net": host.join("sys/class/net"),
            "ip_command": host.join("fake-ip"),
            "selinux_fs_path": host.join("sys/fs/selinux"),
            "selinux_config_path": host.join("selinux/config"),
            "network_confirm_timeout_ms": 30000,
            kind: manager_block,
            "health": {
                "base_url": "https://127.0.0.1:1",
                "dns_tcp_port": 1,
                "dns_udp_port": 1,
                "http_port": 1,
                "https_port": 1,
                "startup_timeout_ms": 1000,
                "stable_duration_ms": 1000,
            },
            "export_base_url": "https://127.0.0.1:1",
        }),
    );
    InitWorld {
        world,
        territory,
        runtime_config,
        init_config,
    }
}

fn assert_rejected_without_writes(harness: &InitWorld) {
    let output = Command::new(LKIT)
        .env(lkit_test_fixture::INIT_CONFIG_ENV, &harness.init_config)
        .env("LKIT_TERRITORY", &harness.territory)
        .args(["self", "install", "--test-runtime"])
        .arg(&harness.runtime_config)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(2),
        "self install on a non-systemd host must be a usage error\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !harness
            .world
            .path("host/usr/local/lib/lkit/lkit.service")
            .exists(),
        "the global unit origin must not be written"
    );
    assert!(
        !harness.world.path("host/etc/init.d/lkit.service").exists(),
        "no registration may be left behind"
    );
    assert!(
        !harness.territory.join("run/lkit.pid").exists(),
        "no daemon may be started"
    );
}

#[test]
fn self_install_rejects_hosts_without_systemd_openrc() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = setup("self-reject-openrc", "openrc");
    assert_rejected_without_writes(&harness);
}

#[test]
fn self_install_rejects_hosts_without_systemd_sysvinit() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = setup("self-reject-sysvinit", "sysvinit");
    assert_rejected_without_writes(&harness);
}
