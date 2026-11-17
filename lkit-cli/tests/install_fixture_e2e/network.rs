use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use super::support::*;

#[test]
fn network_takeover_confirms_from_any_ssh_session() {
    if !e2e_enabled() {
        return;
    }
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
        !harness.state_path().exists(),
        "a takeover install must not commit state before confirmation"
    );
    let transaction = read_only_transaction(&harness.territory);
    assert_eq!(transaction["phase"], "awaiting_network_confirmation");
    assert_eq!(
        transaction["network_takeover"]["plan"]["mode"]["mode"],
        "routed_lan"
    );
    assert!(
        !harness.config_path().exists(),
        "the repository record must not be written before network confirmation"
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
    assert!(!calls.contains("\"start\",\"lkit-network-"));
    let pending = read_only_transaction(&harness.territory);
    assert!(pending["network_takeover"]["boot_id"].as_str().is_some());
    let resolved_stop = calls.find("\"stop\",\"systemd-resolved.service\"").unwrap();
    let network_manager_stop = calls.find("\"stop\",\"NetworkManager.service\"").unwrap();

    assert!(resolved_stop < network_manager_stop);

    let daemon_config = format!("[flare]\npsk = \"{}\"\n", "ab".repeat(32));
    std::fs::write(harness.config_path(), &daemon_config).unwrap();
    let runtime = harness.daemon_runtime();
    let _daemon = harness.spawn_daemon(&runtime);
    let confirm = harness
        .command()
        .args(["network", "confirm", "--test-runtime"])
        .arg(&runtime)
        .output()
        .unwrap();
    assert_success(&confirm);
    assert_eq!(
        std::fs::read_to_string(&harness.ip_state).unwrap(),
        "pre\n",
        "confirmation removed the WAN address managed by the static plan"
    );
    let state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(harness.state_path()).unwrap()).unwrap();
    assert_eq!(state["active_version"], VERSION);
    assert_eq!(
        std::fs::read_to_string(harness.config_path()).unwrap(),
        daemon_config,
        "network confirm must preserve daemon config"
    );
    let transaction = read_only_transaction(&harness.territory);
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
fn console_blocks_on_pending_network_takeover() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    if unsafe { libc::geteuid() } != 0 {
        // 非 root 下控制台快照显示 RootRequired，不进入阻塞屏。
        return;
    }
    let harness = InstallHarness::new("console-pending-takeover", "healthy", 10_000);
    harness.seed_host_services();
    let output = harness.run_takeover();
    assert!(
        output.status.success(),
        "takeover install failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let transaction = read_only_transaction(&harness.territory);
    assert_eq!(transaction["phase"], "awaiting_network_confirmation");

    let mut pty = Pty::open();
    let mut command = Command::new(LKIT);
    attach_pty(&mut command, &pty);
    command.env("LKIT_TERRITORY", &harness.territory);
    let mut child = command.spawn().unwrap();
    // 阻塞屏文本在 ratatui 增量 diff 的字节流中按词分片(见 console_screen.rs 的
    // termlens 说明),只能锚定逐字节连续的片段;动作行最后绘制,等它到达时
    // phase 行与回滚提示已在流中,后续断言不与绘制进度竞态。
    let entered = pty.read_until("Confirm now", Duration::from_secs(10));
    assert!(
        entered.contains("awaiting_network_confirmation"),
        "blocking screen phase missing: {entered:?}"
    );
    assert!(
        entered.contains("auto rollback"),
        "blocking screen rollback hint missing: {entered:?}"
    );
    assert!(
        !entered.contains("Navigation"),
        "menu rendered instead of the blocking screen: {entered:?}"
    );
    pty.master.write_all(b"\r").unwrap();
    let exited = pty.read_until("\x1b[?1049l", Duration::from_secs(5));
    let status = child.wait().unwrap();
    assert!(status.success(), "later exit failed: {exited:?}");
    assert!(
        pty.echo_enabled(),
        "blocking screen exit did not restore terminal echo"
    );
}

#[test]
fn automatic_network_rollback_restores_host_services() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("network-rollback", "healthy", 10_000);
    harness.seed_host_services();
    let output = harness.run_takeover();
    assert!(
        output.status.success(),
        "takeover install failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let pending = read_only_transaction(&harness.territory);
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
    let retry_before_rollback = harness.run();
    assert_eq!(retry_before_rollback.status.code(), Some(1));
    let retry_error = String::from_utf8_lossy(&retry_before_rollback.stderr);
    assert!(retry_error.contains("lkit network status"));
    assert!(retry_error.contains("lkit network confirm"));
    assert!(retry_error.contains("lkit network rollback"));
    assert!(harness.install_root.join("data").exists());
    let rollback = harness.network_command(&["rollback", "--automatic"]);
    assert_success(&rollback);
    assert!(!harness.state_path().exists());
    assert!(!harness.install_root.join("current").exists());
    assert!(!harness.install_root.join("data").exists());
    let transaction = read_only_transaction(&harness.territory);
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
    std::fs::write(&harness.password, b"DifferentSecret456\n").unwrap();
    let reinstall = harness.run();
    assert_success(&reinstall);
}

#[test]
fn network_rollback_failure_preserves_scene_for_retry() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("network-rollback-failure", "healthy", 10_000);
    harness.seed_host_services();
    let output = harness.run_takeover();
    assert_success(&output);

    let current = harness.install_root.join("current");
    std::fs::remove_file(&current).unwrap();
    std::os::unix::fs::symlink("releases/not-the-takeover-target", &current).unwrap();

    let rollback = harness.network_command(&["rollback", "--automatic"]);
    assert_eq!(rollback.status.code(), Some(6));
    let transaction = read_only_transaction(&harness.territory);
    assert_eq!(transaction["phase"], "rolling_back");
    assert_eq!(
        std::fs::read_link(&current).unwrap(),
        PathBuf::from("releases/not-the-takeover-target")
    );
    assert!(harness.install_root.join("data").exists());
    assert!(
        harness
            .install_root
            .join(format!("releases/{VERSION}"))
            .exists()
    );
    std::fs::remove_file(&current).unwrap();
    std::os::unix::fs::symlink(format!("releases/{VERSION}"), &current).unwrap();
    assert_success(&harness.network_command(&["rollback", "--automatic"]));
    assert_eq!(
        read_only_transaction(&harness.territory)["phase"],
        "rolled_back"
    );
    assert!(!harness.install_root.join("data").exists());
}

#[test]
fn network_takeover_supports_ifupdown_without_network_manager() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("network-ifupdown", "healthy", 10_000);
    harness.seed_host_service("networking.service");

    let output = harness.run_takeover();
    assert!(
        output.status.success(),
        "takeover with ifupdown failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let pending = read_only_transaction(&harness.territory);
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
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("network-unknown-manager", "healthy", 10_000);
    harness.seed_host_service("wicked.service");

    let output = harness.run_takeover();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("preflight check failed: unknown network manager wicked.service is active")
    );
    assert!(
        !harness.transactions_dir().exists(),
        "preflight created a transaction before rejecting an unknown manager"
    );
}

#[test]
fn native_network_actions_survive_observer_disconnect_and_daemon_crash() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for (action, point, outcome) in [
        ("confirm", "network-finalizing", "committed"),
        ("confirm", "before-result", "committed"),
        ("rollback", "network-rolling-back", "rolled_back"),
        ("rollback", "before-result", "rolled_back"),
    ] {
        let h = InstallHarness::new("network-native-crash", "healthy", 10_000);
        h.seed_host_services();
        h.seed_host_service("systemd-networkd.service");
        h.seed_host_service("systemd-networkd.socket");
        let mut config: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&h.runtime_config).unwrap()).unwrap();
        config["network_confirm_timeout_ms"] = 180_000.into();
        write_json(&h.runtime_config, &config);
        assert_success(&h.run_takeover());
        let tx = read_only_transaction(&h.territory);
        let runtime = h.daemon_runtime();
        let gate = h.run_dir().join(format!("test-repair-{point}.gate"));
        std::fs::write(&gate, b"pause").unwrap();
        let mut daemon = h.spawn_daemon_at_checkpoint(&runtime, point);
        struct Observer(std::process::Child);
        impl Drop for Observer {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let observer = Observer(
            h.command()
                .args(["network", action, "--test-runtime"])
                .arg(&runtime)
                .spawn()
                .unwrap(),
        );
        let wait = |condition: &mut dyn FnMut() -> bool| {
            let deadline = std::time::Instant::now() + Duration::from_secs(50);
            while !condition() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "network {action} timed out at {point}"
                );
                std::thread::sleep(Duration::from_millis(50));
            }
        };
        wait(&mut || {
            h.run_dir()
                .join(format!("test-repair-{point}.reached"))
                .exists()
        });
        let record = || -> serde_json::Value {
            let paths: Vec<_> = std::fs::read_dir(h.run_dir().join("operations"))
                .unwrap()
                .filter_map(Result::ok)
                .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
                .collect();
            assert_eq!(paths.len(), 1);
            serde_json::from_slice(&std::fs::read(paths[0].path()).unwrap()).unwrap()
        };
        let before = record();
        assert_eq!(before["network_transaction"], tx["transaction_id"]);
        assert!(
            !h.command()
                .args(["operation", "cancel", before["id"].as_str().unwrap()])
                .output()
                .unwrap()
                .status
                .success()
        );
        drop(observer);
        daemon.crash();
        std::fs::remove_file(gate).unwrap();
        let _daemon = h.spawn_daemon(&runtime);
        wait(&mut || record()["status"] == "succeeded");
        assert_eq!(record()["transaction_id"], tx["transaction_id"]);
        assert_eq!(record()["exit_code"], 0);
        assert_eq!(read_only_transaction(&h.territory)["phase"], outcome);
        assert_eq!(h.state_path().exists(), action == "confirm");
        if action == "rollback" {
            assert!(!h.install_root.join("data").exists());
            for unit in [
                "systemd-networkd.service",
                "systemd-networkd.socket",
                "NetworkManager.service",
                "firewalld.service",
                "systemd-resolved.service",
            ] {
                assert_eq!(
                    std::fs::read_to_string(
                        h.host.join("systemd-state/units").join(unit).join("active")
                    )
                    .unwrap()
                    .trim(),
                    "active"
                );
            }
        }
    }
}

#[test]
fn networkd_takeover_preserves_startup_choices_and_restores_the_group() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for confirm in [false, true] {
        let h = InstallHarness::new("networkd-takeover", "healthy", 10_000);
        for unit in [
            "systemd-networkd.socket",
            "systemd-networkd.service",
            "systemd-networkd-wait-online.service",
            "systemd-networkd-wait-online@ens3.service",
            "systemd-networkd-wait-online@.service",
        ] {
            h.seed_host_service(unit);
        }
        let states = h.host.join("systemd-state/units");
        std::fs::remove_file(states.join("systemd-networkd-wait-online@.service/active")).unwrap();
        let socket = states.join("systemd-networkd.socket");
        std::fs::remove_file(socket.join("enabled")).unwrap();
        std::fs::remove_file(socket.join("active")).unwrap();
        let wait = states.join("systemd-networkd-wait-online.service");
        std::fs::remove_file(wait.join("active")).unwrap();
        std::fs::write(wait.join("masked"), "masked-runtime\n").unwrap();
        assert_success(&h.run_takeover());
        for unit in [
            "systemd-networkd.socket",
            "systemd-networkd.service",
            "systemd-networkd-wait-online.service",
            "systemd-networkd-wait-online@ens3.service",
            "systemd-networkd-wait-online@.service",
        ] {
            assert!(states.join(unit).join("masked").exists());
            assert!(!states.join(unit).join("active").exists());
        }
        assert!(!socket.join("enabled").exists());
        assert!(states.join("systemd-networkd.service/enabled").exists());
        if confirm {
            assert_success(&h.network_command(&["confirm"]));
            assert!(states.join("systemd-networkd.service/masked").exists());
        } else {
            assert_success(&h.network_command(&["rollback", "--automatic"]));
            assert!(states.join("systemd-networkd.service/active").exists());
            assert!(!states.join("systemd-networkd.service/masked").exists());
            assert!(!socket.join("active").exists());
            assert!(!socket.join("enabled").exists());
            assert!(!socket.join("masked").exists());
            assert_eq!(
                std::fs::read_to_string(wait.join("masked")).unwrap().trim(),
                "masked-runtime"
            );
        }
        let calls = std::fs::read_to_string(h.world.path("systemctl-calls.jsonl")).unwrap();
        for line in calls
            .lines()
            .filter(|line| line.contains("systemd-networkd"))
        {
            assert!(
                !line.contains("[\"disable\"") && !line.contains("[\"enable\""),
                "{line}"
            );
        }
    }
}
