use std::time::{Duration, Instant};

use super::support::{E2E_LOCK, InstallHarness, assert_success, e2e_enabled, systemctl};

/// `lkit self install` 把 lkit 注册为全局常驻服务(docs/commands/self.md):
/// unit 原件渲染到 `/usr/local/lib/lkit/lkit.service`
/// (`ExecStart=/usr/local/bin/lkit daemon`),注册链接指向全局原件,启用并启动;
/// daemon 写 pidfile 到 lkit 地盘 `run/lkit.pid`。`lkit self remove` 停止、
/// 注销并删除原件,幂等可重复。`self` 命令不接收 `--install-dir`。
#[test]
fn self_installs_and_removes_the_lkit_service() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap();
    let harness = InstallHarness::new("self", "healthy", 30_000);
    harness.seed_global_lkit_binary();

    let installed = harness
        .command()
        .args(["self", "install", "--test-runtime"])
        .arg(&harness.runtime_config)
        .output()
        .unwrap();
    assert_success(&installed);

    let origin = harness.host.join("usr/local/lib/lkit/lkit.service");
    let unit = std::fs::read_to_string(&origin).unwrap();
    assert!(
        unit.contains("ExecStart=/usr/local/bin/lkit daemon"),
        "unit ExecStart must point at the global lkit binary: {unit}"
    );
    assert!(unit.contains("User=root"));
    assert!(unit.contains("Restart=always"));
    assert!(unit.contains("WantedBy=multi-user.target"));

    // 注册链接:/etc/systemd/system/lkit.service → 全局原件(fixture 世界映射到
    // host/units/),启用并启动。
    let link = harness.host.join("units/lkit.service");
    assert!(
        link.is_symlink(),
        "the registration link must point at the global unit origin"
    );
    let active = systemctl(&harness.world, &["is-active", "lkit.service"]);
    assert_eq!(String::from_utf8_lossy(&active.stdout).trim(), "active");
    let enabled = systemctl(&harness.world, &["is-enabled", "lkit.service"]);
    assert_eq!(String::from_utf8_lossy(&enabled.stdout).trim(), "enabled");

    // daemon 写 pidfile 到 lkit 地盘 run/ 且进程真实存活。
    let pidfile = harness.run_dir().join("lkit.pid");
    let started = Instant::now();
    let pid: u32 = loop {
        if let Ok(content) = std::fs::read_to_string(&pidfile)
            && let Ok(pid) = content.trim().parse::<u32>()
        {
            break pid;
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "lkit daemon must write its pidfile"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(
        process_alive(pid),
        "lkit daemon must be a real running process"
    );

    let calls = std::fs::read_to_string(harness.world.path("systemctl-calls.jsonl")).unwrap();
    assert!(calls.contains("\"enable\",\"lkit.service\""), "{calls}");
    assert!(calls.contains("\"start\",\"lkit.service\""), "{calls}");

    let removed = harness
        .command()
        .args(["self", "remove", "--test-runtime"])
        .arg(&harness.runtime_config)
        .output()
        .unwrap();
    assert_success(&removed);

    assert!(!process_alive(pid), "lkit daemon must exit after removal");
    assert!(!origin.exists(), "unit origin must be removed");
    assert!(!link.exists(), "registration link must be removed");
    let inactive = systemctl(&harness.world, &["is-active", "lkit.service"]);
    assert_eq!(String::from_utf8_lossy(&inactive.stdout).trim(), "inactive");
}

/// 参数错误路径:systemd 不可用时返回退出码 2,不遗留任何现场
/// (unit 原件与 daemon pidfile 均不得存在)。
#[test]
fn self_install_rejects_unavailable_systemd() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap();
    let harness = InstallHarness::new("self-reject", "healthy", 30_000);
    harness.seed_global_lkit_binary();

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

    let unavailable = harness
        .command()
        .args(["self", "install", "--test-runtime"])
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
        !harness
            .host
            .join("usr/local/lib/lkit/lkit.service")
            .exists(),
        "the unit origin must not be left behind"
    );
    assert!(
        !harness.run_dir().join("lkit.pid").exists(),
        "no daemon may be started"
    );
}

fn process_alive(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}
