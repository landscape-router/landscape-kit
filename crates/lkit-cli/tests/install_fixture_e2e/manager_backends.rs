use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::support::world::TestWorld;
use super::support::{E2E_LOCK, LKIT, assert_success, write_json};

const INIT_FIXTURE: &str = env!("CARGO_BIN_EXE_lkit-test-init");

/// 多发行版服务管理器的 daemon 安装验证:在 OpenRC 与 sysvinit 后端上执行
/// `lkit self-service install|remove`,验证定义渲染、注册链接、启用标记、
/// 真实进程拉起(main_pid)与反向清理。systemd 路径由 `self_service.rs` 覆盖。
struct InitWorld {
    world: TestWorld,
    install_root: PathBuf,
    runtime_config: PathBuf,
    init_config: PathBuf,
    /// 工具符号链接目录(rc-service/rc-update/update-rc.d/start-stop-daemon)。
    tool_dir: PathBuf,
}

fn setup(name: &str, kind: &str) -> InitWorld {
    let world = TestWorld::new(name);
    let install_root = world.path("install");
    std::fs::create_dir_all(&install_root).unwrap();
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
        install_root,
        runtime_config,
        init_config,
        tool_dir,
    }
}

fn init_fixture(env: &TestWorld, tool: &Path, args: &[&str]) -> std::process::Output {
    Command::new(tool)
        .env(lkit_test_fixture::INIT_CONFIG_ENV, &env.init_config)
        .args(args)
        .output()
        .unwrap()
}

fn self_service(harness: &InitWorld, action: &str) -> std::process::Output {
    Command::new(LKIT)
        .env(lkit_test_fixture::INIT_CONFIG_ENV, &harness.init_config)
        .args([
            "self-service",
            action,
            "--install-dir",
            harness.install_root.to_str().unwrap(),
            "--test-runtime",
        ])
        .arg(&harness.runtime_config)
        .output()
        .unwrap()
}

fn daemon_alive(harness: &InitWorld) -> bool {
    let pidfile = std::fs::canonicalize(&harness.install_root)
        .unwrap()
        .join("run/lkit.pid");
    let Ok(content) = std::fs::read_to_string(pidfile) else {
        return false;
    };
    let Ok(pid) = content.trim().parse::<i32>() else {
        return false;
    };
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn assert_installed(harness: &InitWorld, kind: &str) {
    let canonical = std::fs::canonicalize(&harness.install_root).unwrap();
    let binary = canonical.join("service/lkit");
    assert!(
        binary.is_file(),
        "lkit binary must be copied into the install root"
    );
    let mode = std::fs::metadata(&binary).unwrap().permissions().mode();
    assert_ne!(mode & 0o111, 0, "lkit binary must be executable");

    let origin = canonical.join("service/lkit.service");
    let script = std::fs::read_to_string(&origin).unwrap();
    assert!(
        script.contains(&format!("command=\"{}/service/lkit\"", canonical.display()))
            || script.contains(&format!("{}/service/lkit", canonical.display())),
        "init script must point at the copied binary: {script}"
    );

    // 注册链接:init.d/lkit.service → <root>/service/lkit.service。
    let link = harness.world.path("host/etc/init.d/lkit.service");
    assert!(link.is_symlink(), "init.d/lkit registration link missing");
    assert_eq!(
        std::fs::read_link(&link).unwrap(),
        canonical.join("service/lkit.service")
    );

    // 启用标记。
    if kind == "openrc" {
        let shown = init_fixture(
            &harness.world,
            &harness.tool_dir.join("rc-update"),
            &["show"],
        );
        assert_success(&shown);
        let table = String::from_utf8_lossy(&shown.stdout);
        assert!(
            table
                .lines()
                .any(|line| line.trim().starts_with("lkit.service")),
            "rc-update show must list lkit: {table}"
        );
    } else {
        let link = harness.world.path("host/etc/rc.d/rc3.d/S20lkit.service");
        assert!(link.exists(), "rc3.d/S20lkit enable link missing");
    }

    // daemon 真实运行:main_pid 已在 self-service install 内部校验,
    // 这里确认 daemon 自己的 pidfile 且进程存活。
    for _ in 0..50 {
        if daemon_alive(harness) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(
        daemon_alive(harness),
        "lkit daemon must be running after install"
    );
}

#[test]
fn self_service_installs_and_removes_daemon_on_openrc() {
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = setup("manager-openrc", "openrc");

    let installed = self_service(&harness, "install");
    assert_success(&installed);
    assert_installed(&harness, "openrc");

    let removed = self_service(&harness, "remove");
    assert_success(&removed);
    assert!(
        !harness.world.path("host/etc/init.d/lkit.service").exists(),
        "registration link must be removed"
    );
    assert!(
        !std::fs::canonicalize(&harness.install_root)
            .unwrap()
            .join("service/lkit")
            .exists(),
        "lkit binary must be removed"
    );
}

#[test]
fn self_service_installs_and_removes_daemon_on_sysvinit() {
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = setup("manager-sysvinit", "sysvinit");

    // start-stop-daemon 由渲染脚本调用,需要出现在 PATH 中。
    let mut path = std::env::var("PATH").unwrap_or_default();
    path = format!("{}:{}", harness.tool_dir.display(), path);
    let installed = Command::new(LKIT)
        .env(lkit_test_fixture::INIT_CONFIG_ENV, &harness.init_config)
        .env("PATH", path)
        .args([
            "self-service",
            "install",
            "--install-dir",
            harness.install_root.to_str().unwrap(),
            "--test-runtime",
        ])
        .arg(&harness.runtime_config)
        .output()
        .unwrap();
    assert_success(&installed);
    assert_installed(&harness, "sysvinit");

    let removed = Command::new(LKIT)
        .env(lkit_test_fixture::INIT_CONFIG_ENV, &harness.init_config)
        .args([
            "self-service",
            "remove",
            "--install-dir",
            harness.install_root.to_str().unwrap(),
            "--test-runtime",
        ])
        .arg(&harness.runtime_config)
        .output()
        .unwrap();
    assert_success(&removed);
    assert!(
        !harness.world.path("host/etc/init.d/lkit.service").exists(),
        "registration link must be removed"
    );
}
