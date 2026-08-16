use std::os::unix::fs::PermissionsExt;
use std::process::Command;

use super::support::{E2E_LOCK, InstallHarness, LKIT, assert_success, e2e_enabled, systemctl};

/// lkit 自装服务垂直切片:`self-service install` 复制当前二进制到
/// `<root>/service/lkit`,通过 fake systemctl 真实拉起 `lkit daemon`,
/// 验证 `ServiceManager` trait 的 `LkitDaemon` 定义渲染、注册、启用与启动;
/// `self-service remove` 停止并注销服务、删除二进制。
#[test]
fn self_installs_and_removes_the_lkit_service() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap();
    let harness = InstallHarness::new("self-service", "healthy", 30_000);
    let root = &harness.install_root;
    std::fs::create_dir_all(root).unwrap();
    let canonical = std::fs::canonicalize(root).unwrap();

    let installed = Command::new(LKIT)
        .env(
            lkit_test_fixture::SYSTEMCTL_CONFIG_ENV,
            &harness.world.systemctl_config,
        )
        .args([
            "self-service",
            "install",
            "--install-dir",
            root.to_str().unwrap(),
            "--test-runtime",
        ])
        .arg(&harness.runtime_config)
        .output()
        .unwrap();
    assert_success(&installed);

    let binary = canonical.join("service/lkit");
    assert!(
        binary.is_file(),
        "lkit binary must be copied into the install root"
    );
    let mode = std::fs::metadata(&binary).unwrap().permissions().mode();
    assert_ne!(mode & 0o111, 0, "lkit binary must be executable");

    let origin = canonical.join("service/lkit.service");
    let unit = std::fs::read_to_string(&origin).unwrap();
    assert!(
        unit.contains(&format!(
            "ExecStart={}/service/lkit daemon --config-dir {}/data",
            canonical.display(),
            canonical.display()
        )),
        "unit ExecStart must point at the copied binary: {unit}"
    );
    assert!(unit.contains("User=root"));
    assert!(unit.contains("Restart=always"));
    assert!(unit.contains("WantedBy=multi-user.target"));

    let active = systemctl(&harness.world, &["is-active", "lkit.service"]);
    assert_eq!(String::from_utf8_lossy(&active.stdout).trim(), "active");
    let enabled = systemctl(&harness.world, &["is-enabled", "lkit.service"]);
    assert_eq!(String::from_utf8_lossy(&enabled.stdout).trim(), "enabled");

    let state_dir = harness.host.join("systemd-state/units/lkit.service");
    let pid: u32 = std::fs::read_to_string(state_dir.join("main.pid"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(
        process_alive(pid),
        "lkit daemon must be a real running process"
    );
    let pidfile = canonical.join("run/lkit.pid");
    let started = std::time::Instant::now();
    loop {
        if let Ok(content) = std::fs::read_to_string(&pidfile)
            && content.trim().parse::<u32>().is_ok()
        {
            break;
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "lkit daemon must write its pidfile"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    let calls = std::fs::read_to_string(harness.world.path("systemctl-calls.jsonl")).unwrap();
    assert!(calls.contains("\"enable\",\"lkit.service\""), "{calls}");
    assert!(calls.contains("\"start\",\"lkit.service\""), "{calls}");

    let removed = Command::new(LKIT)
        .env(
            lkit_test_fixture::SYSTEMCTL_CONFIG_ENV,
            &harness.world.systemctl_config,
        )
        .args([
            "self-service",
            "remove",
            "--install-dir",
            root.to_str().unwrap(),
            "--test-runtime",
        ])
        .arg(&harness.runtime_config)
        .output()
        .unwrap();
    assert_success(&removed);

    assert!(!process_alive(pid), "lkit daemon must exit after removal");
    assert!(!binary.exists(), "lkit binary must be removed");
    assert!(!origin.exists(), "unit origin must be removed");
    let inactive = systemctl(&harness.world, &["is-active", "lkit.service"]);
    assert_eq!(String::from_utf8_lossy(&inactive.stdout).trim(), "inactive");
}

/// 参数错误路径:systemd 不可用时返回退出码 2,不写任何文件。
#[test]
fn self_install_rejects_unavailable_systemd() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap();
    let harness = InstallHarness::new("self-service-reject", "healthy", 30_000);
    let root = &harness.install_root;
    std::fs::create_dir_all(root).unwrap();

    // systemd 不可用:改写测试运行时的 systemctl 路径指向不存在文件。
    let mut runtime: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&harness.runtime_config).unwrap()).unwrap();
    runtime["systemd"]["systemctl"] = serde_json::Value::String(
        harness
            .world
            .path("missing-systemctl")
            .display()
            .to_string(),
    );
    let broken_runtime = harness.world.path("broken-runtime.json");
    std::fs::write(
        &broken_runtime,
        serde_json::to_vec_pretty(&runtime).unwrap(),
    )
    .unwrap();

    let unavailable = Command::new(LKIT)
        .env(
            lkit_test_fixture::SYSTEMCTL_CONFIG_ENV,
            &harness.world.systemctl_config,
        )
        .args([
            "self-service",
            "install",
            "--install-dir",
            root.to_str().unwrap(),
            "--test-runtime",
        ])
        .arg(&broken_runtime)
        .output()
        .unwrap();
    assert_eq!(
        unavailable.status.code(),
        Some(2),
        "unavailable systemd must be a usage error: {}",
        String::from_utf8_lossy(&unavailable.stderr)
    );
    assert!(
        !root.join("service/lkit").exists(),
        "nothing may be written"
    );
}

fn process_alive(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}
