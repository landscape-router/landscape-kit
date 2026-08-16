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
    assert!(
        !harness.install_root.join("config.toml").exists(),
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
    assert!(
        !harness.install_root.join("config.toml").exists(),
        "network confirm must not create config.toml"
    );
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
    let transaction = read_only_transaction(&harness.install_root);
    assert_eq!(transaction["phase"], "awaiting_network_confirmation");

    let mut pty = Pty::open();
    let mut command = Command::new(LKIT);
    attach_pty(&mut command, &pty);
    command.env("LKIT_INSTALL_DIR", &harness.install_root);
    let mut child = command.spawn().unwrap();
    let entered = pty.read_until(
        "Network takeover awaiting confirmation",
        Duration::from_secs(10),
    );
    assert!(
        entered.contains("awaiting network confirmation"),
        "blocking screen badge missing: {entered:?}"
    );
    assert!(
        entered.contains("Confirm now"),
        "blocking screen action missing: {entered:?}"
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
    let retry_before_rollback = harness.run();
    assert_eq!(retry_before_rollback.status.code(), Some(1));
    let retry_error = String::from_utf8_lossy(&retry_before_rollback.stderr);
    assert!(retry_error.contains("lkit network status"));
    assert!(retry_error.contains("lkit network confirm"));
    assert!(retry_error.contains("lkit network rollback"));
    assert!(harness.install_root.join("data").exists());
    let rollback = harness.network_command(&["rollback", "--automatic"]);
    assert_success(&rollback);
    assert!(
        !harness
            .install_root
            .join("state/install-state.json")
            .exists()
    );
    assert!(!harness.install_root.join("current").exists());
    assert!(!harness.install_root.join("data").exists());
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
    std::fs::write(&harness.password, b"DifferentSecret456\n").unwrap();
    let reinstall = harness.run();
    assert_success(&reinstall);
}

#[test]
fn network_rollback_failure_preserves_scene_and_marks_transaction_failed() {
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
    let transaction = read_only_transaction(&harness.install_root);
    assert_eq!(transaction["phase"], "failed");
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
    if !e2e_enabled() {
        return;
    }
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
