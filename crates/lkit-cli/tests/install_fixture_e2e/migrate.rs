use std::path::{Path, PathBuf};
use std::process::{Child, Command};

use super::support::*;

struct OldInstance {
    child: Child,
}

impl Drop for OldInstance {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// 从 runtime.json 读取固定端口,构造并启动运行中的旧手工部署:
/// 配置目录(特征文件 + static + static.zip)+ fixture 实例 + 旧 unit 状态标记。
fn start_manual_install(harness: &InstallHarness) -> (PathBuf, OldInstance) {
    let runtime: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&harness.runtime_config).unwrap()).unwrap();
    let health = &runtime["health"];
    let dns = health["dns_udp_port"].as_u64().unwrap() as u16;
    let http = health["http_port"].as_u64().unwrap() as u16;
    let https = health["https_port"].as_u64().unwrap() as u16;

    let source = harness.world.path("manual");
    let static_dir = source.join("static");
    std::fs::create_dir_all(static_dir.join("assets")).unwrap();
    std::fs::write(static_dir.join("index.html"), "manual static").unwrap();
    std::fs::write(static_dir.join("assets/app.js"), "manual asset").unwrap();
    std::fs::write(source.join("static.zip"), b"manual static archive").unwrap();
    std::fs::write(
        source.join("landscape.toml"),
        format!("version = \"{VERSION}\"\n"),
    )
    .unwrap();
    std::fs::write(source.join("landscape_init.lock"), b"").unwrap();

    let landscape_config = harness.world.path("manual-landscape.json");
    std::fs::write(
        &landscape_config,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 1,
            "scenario": "healthy",
            "listen_address": "127.0.0.1",
            "dns_tcp_port": dns,
            "dns_udp_port": dns,
            "http_port": http,
            "https_port": https,
            "ready_delay_ms": 500,
            "exit_after_ms": 2000,
            "start_exit_code": 1,
            "export_version": VERSION,
            "export_content": format!("version = \"{VERSION}\"\n"),
        }))
        .unwrap(),
    )
    .unwrap();
    let child = Command::new(LANDSCAPE_FIXTURE)
        .env(lkit_test_fixture::FIXTURE_CONFIG_ENV, &landscape_config)
        .args([
            "--config-dir",
            source.to_str().unwrap(),
            "--web",
            static_dir.to_str().unwrap(),
        ])
        .spawn()
        .unwrap();
    let instance = OldInstance { child };

    // 等待导出 API 就绪,确保识别与导出阶段可用。
    wait_for_docs(https);

    // 旧 unit:ExecStart 指向运行中的 fixture,并预置 active/enabled/pid 状态,
    // 使 fake systemctl 的 stop 能真正结束旧实例进程。
    let legacy_unit = harness.host.join("units/legacy-landscape.service");
    std::fs::write(
        &legacy_unit,
        format!(
            "[Unit]\nDescription=Legacy Landscape\n\n[Service]\nExecStart={LANDSCAPE_FIXTURE} --config-dir {0} --web {0}/static\nRestart=always\nUser=root\nLimitMEMLOCK=infinity\n\n[Install]\nWantedBy=multi-user.target\n",
            source.display()
        ),
    )
    .unwrap();
    let legacy_state = harness
        .host
        .join("systemd-state/units/legacy-landscape.service");
    std::fs::create_dir_all(&legacy_state).unwrap();
    std::fs::write(legacy_state.join("active"), b"active\n").unwrap();
    std::fs::write(legacy_state.join("enabled"), b"enabled\n").unwrap();
    std::fs::write(
        legacy_state.join("main.pid"),
        format!("{}\n", instance.child.id()),
    )
    .unwrap();
    (source, instance)
}

fn wait_for_docs(https_port: u16) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .build()
            .unwrap();
        let url = format!("https://127.0.0.1:{https_port}/api/docs");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while std::time::Instant::now() < deadline {
            if client
                .get(&url)
                .send()
                .await
                .ok()
                .is_some_and(|r| r.status().is_success())
            {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        panic!("old instance did not become ready on port {https_port}");
    });
}

fn migrate_command(harness: &InstallHarness, source: &Path) -> Command {
    let mut command = Command::new(LKIT);
    command
        .env(
            lkit_test_fixture::SYSTEMCTL_CONFIG_ENV,
            &harness.world.systemctl_config,
        )
        .args(["migrate", "--non-interactive", "--yes", "--from"])
        .arg(source)
        .args(["--repository"])
        .arg(&harness.repository.base_url)
        .args(["--install-dir"])
        .arg(&harness.install_root)
        .args(["--test-runtime"])
        .arg(&harness.runtime_config);
    command
}

#[test]
fn migrates_manual_deployment_through_full_cli() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("migrate", "healthy", 10_000);
    let (source, _old_instance) = start_manual_install(&harness);
    let output = migrate_command(&harness, &source).output().unwrap();
    assert!(
        output.status.success(),
        "lkit migrate failed with {:?}\nstdout:\n{}\nstderr:\n{}\nservice log:\n{}",
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
    assert_eq!(
        state["active_version"], VERSION,
        "migration must not upgrade"
    );
    assert_eq!(state["initialization"]["status"], "complete");
    assert_eq!(state["service"]["manager"], "systemd");
    assert_eq!(state["service"]["verified"], true);

    let release = harness.install_root.join("releases").join(VERSION);
    assert!(release.join("landscape-webserver").is_file());
    assert!(release.join("static/index.html").is_file());
    assert_eq!(
        std::fs::read_link(harness.install_root.join("current")).unwrap(),
        PathBuf::from(format!("releases/{VERSION}"))
    );
    assert_eq!(
        std::fs::read_to_string(harness.install_root.join("data/landscape_init.toml")).unwrap(),
        format!("version = \"{VERSION}\"\n")
    );
    let lkb_count = std::fs::read_dir(harness.install_root.join("backups"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("lkb"))
        .count();
    assert_eq!(lkb_count, 1, "the migration backup must be preserved");

    assert!(
        !harness.host.join("units/legacy-landscape.service").exists(),
        "the legacy unit file must be moved out of the unit dir"
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
    let executable = std::fs::read_link(format!("/proc/{pid}/exe")).unwrap();
    assert_eq!(
        executable,
        release.join("landscape-webserver").canonicalize().unwrap(),
        "the new managed instance must run from the migrated release"
    );
}

#[test]
fn migrate_rolls_back_and_restores_legacy_unit_on_activation_failure() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("migrate-rollback", "healthy", 10_000);
    let (source, _old_instance) = start_manual_install(&harness);

    // 新实例启动即退出(start_exit_code 1):激活失败进入回滚。
    let runtime: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&harness.runtime_config).unwrap()).unwrap();
    let health = &runtime["health"];
    let failing_config = harness.world.path("failing-landscape.json");
    std::fs::write(
        &failing_config,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 1,
            "scenario": "start_exit",
            "listen_address": "127.0.0.1",
            "dns_tcp_port": health["dns_udp_port"],
            "dns_udp_port": health["dns_udp_port"],
            "http_port": health["http_port"],
            "https_port": health["https_port"],
            "ready_delay_ms": 500,
            "exit_after_ms": 2000,
            "start_exit_code": 1,
            "export_version": VERSION,
            "export_content": format!("version = \"{VERSION}\"\n"),
        }))
        .unwrap(),
    )
    .unwrap();
    let systemctl_config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&harness.world.systemctl_config).unwrap()).unwrap();
    let mut updated = systemctl_config.clone();
    updated["landscape_config"] = serde_json::json!(failing_config);
    std::fs::write(
        &harness.world.systemctl_config,
        serde_json::to_vec_pretty(&updated).unwrap(),
    )
    .unwrap();

    let output = migrate_command(&harness, &source).output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(5),
        "expected exit code 5 after rollback\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(
        harness
            .host
            .join("units/legacy-landscape.service")
            .is_file(),
        "the legacy unit file must be restored on rollback"
    );
    assert!(!harness.install_root.join("releases").join(VERSION).exists());
    assert!(!harness.install_root.join("data").exists());
    assert!(!harness.install_root.join("current").exists());
    assert!(
        !harness
            .install_root
            .join("state/install-state.json")
            .exists()
    );
}
