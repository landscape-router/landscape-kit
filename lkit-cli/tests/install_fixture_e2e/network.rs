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
    assert_host_services_masked(&harness, &["systemd-resolved.service"]);
    // NM/firewalld/systemd-networkd 不再整体停止:drop-in 摘除、zone 接口行
    // 删除、`.network` 移出,服务保持运行。
    let calls = std::fs::read_to_string(harness.world.path("systemctl-calls.jsonl")).unwrap();
    let timer_start = calls.find("\"start\",\"lkit-network-").unwrap();
    let resolved_stop = calls.find("\"stop\",\"systemd-resolved.service\"").unwrap();
    assert!(timer_start < resolved_stop);
    // 只读探测(LoadState/is-active/is-enabled)允许;变更操作被禁止。
    for unit in [
        "NetworkManager.service",
        "firewalld.service",
        "systemd-networkd.service",
    ] {
        for verb in ["stop", "disable", "mask"] {
            assert!(
                !calls.contains(&format!("[\"{verb}\",\"{unit}\"]")),
                "{unit} must not be {verb}ed:\n{calls}"
            );
        }
    }
    let drop_in =
        std::fs::read_to_string(harness.host.join("nm-conf.d/lkit-unmanage.conf")).unwrap();
    assert!(drop_in.contains("interface-name:ens3;"));
    assert!(drop_in.contains("interface-name:ens4"));
    let zone = std::fs::read_to_string(harness.host.join("firewalld-zones/public.xml")).unwrap();
    assert!(!zone.contains("ens3"), "ens3 must leave the zone:\n{zone}");
    assert!(zone.contains("ens9"), "unselected interface stays:\n{zone}");
    let nmcli_calls = std::fs::read_to_string(harness.world.path("nmcli-calls.log")).unwrap();
    assert!(
        nmcli_calls.contains("general reload"),
        "the drop-in must be applied to the running NetworkManager:\n{nmcli_calls}"
    );
    let firewall_cmd_calls =
        std::fs::read_to_string(harness.world.path("firewall-cmd-calls.log")).unwrap();
    assert!(
        firewall_cmd_calls.contains("--reload"),
        "the zone edit must be applied to the running firewalld:\n{firewall_cmd_calls}"
    );
    let wan_network = harness
        .host
        .join(format!("systemd-network/{SEEDED_NETWORKD_FILE}"));
    assert!(
        !wan_network.exists(),
        "the .network file bound to the selected WAN must be removed"
    );
    let networkctl_calls =
        std::fs::read_to_string(harness.world.path("networkctl-calls.log")).unwrap();
    assert!(
        networkctl_calls.contains("reload"),
        "the removal must be applied to the running systemd-networkd:\n{networkctl_calls}"
    );

    let confirm = harness.network_command(&["confirm"]);
    assert_success(&confirm);
    assert_eq!(
        std::fs::read_to_string(&harness.ip_state).unwrap(),
        "pre\n",
        "confirmation removed the WAN address managed by the static plan"
    );
    let state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(harness.state_path()).unwrap()).unwrap();
    assert_eq!(state["active_version"], VERSION);
    assert!(
        !harness.config_path().exists(),
        "network confirm must not create config.toml"
    );
    eprintln!("DEBUG SECOND READ");
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
    assert_host_services_restored(&harness, &["systemd-resolved.service"]);
    let calls = std::fs::read_to_string(harness.world.path("systemctl-calls.jsonl")).unwrap();
    for unit in [
        "NetworkManager.service",
        "firewalld.service",
        "systemd-networkd.service",
    ] {
        assert!(
            !calls.contains(&format!("\"stop\",\"{unit}\"")),
            "{unit} must never be stopped:\n{calls}"
        );
    }
    // 摘除的文件被恢复:drop-in 删除,zone 逐字还原,`.network` 逐字重建。
    assert!(
        !harness.host.join("nm-conf.d/lkit-unmanage.conf").exists(),
        "rollback must delete the NM drop-in"
    );
    assert_eq!(
        std::fs::read_to_string(harness.host.join("firewalld-zones/public.xml")).unwrap(),
        SEEDED_FIREWALLD_ZONE,
        "rollback must restore the zone byte for byte"
    );
    assert_eq!(
        std::fs::read_to_string(
            harness
                .host
                .join(format!("systemd-network/{SEEDED_NETWORKD_FILE}"))
        )
        .unwrap(),
        SEEDED_NETWORKD_WAN,
        "rollback must restore the .network file byte for byte"
    );
    let nmcli_calls = std::fs::read_to_string(harness.world.path("nmcli-calls.log")).unwrap();
    assert_eq!(
        nmcli_calls.matches("general reload").count(),
        2,
        "NM must be reloaded after unmanage and after restore:\n{nmcli_calls}"
    );
    let firewall_cmd_calls =
        std::fs::read_to_string(harness.world.path("firewall-cmd-calls.log")).unwrap();
    assert_eq!(
        firewall_cmd_calls.matches("--reload").count(),
        2,
        "firewalld must be reloaded after unmanage and after restore:\n{firewall_cmd_calls}"
    );
    let networkctl_calls =
        std::fs::read_to_string(harness.world.path("networkctl-calls.log")).unwrap();
    assert_eq!(
        networkctl_calls.matches("reload").count(),
        2,
        "systemd-networkd must be reloaded after unmanage and after restore:\n{networkctl_calls}"
    );
    assert!(
        !harness.backups_dir().join("hostnet").exists(),
        "rollback must clear the hostnet backups"
    );
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
    let transaction = read_only_transaction(&harness.territory);
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
    let interfaces = harness.host.join("network-interfaces");
    let original = "auto ens3 ens4 ens5\n\
iface ens3 inet static\n\
    address 192.0.2.10/24\n\
    gateway 192.0.2.1\n\
\n\
iface ens4 inet dhcp\n\
\n\
iface ens5 inet static\n\
    address 198.51.100.10/24\n";
    std::fs::write(&interfaces, original).unwrap();

    let output = harness.run_takeover();
    assert!(
        output.status.success(),
        "takeover with ifupdown failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    // networking.service 不再整体 stop/disable/mask:摘除改为改写 ifupdown 配置。
    let pending = read_only_transaction(&harness.territory);
    let host_services = pending["network_takeover"]["host_services"]
        .as_array()
        .unwrap();
    assert!(
        host_services
            .iter()
            .all(|service| service["unit"] != "networking.service"),
        "networking.service must not be recorded as a wholesale-stopped service"
    );
    assert!(
        host_services
            .iter()
            .all(|service| service["unit"] != "NetworkManager.service"),
        "NetworkManager is not a wholesale-stopped service anymore"
    );
    assert!(
        !harness.host.join("units/NetworkManager.service").exists(),
        "NetworkManager was unexpectedly installed"
    );

    // 选中接口(ens3 WAN + ens4 LAN)改写为 manual 并从 auto 行摘除,
    // 未选接口 ens5 原样保留;dry-run 契约走假 ifup。
    let rewritten = std::fs::read_to_string(&interfaces).unwrap();
    assert!(
        rewritten.contains("iface ens3 inet manual"),
        "selected WAN must be manual:\n{rewritten}"
    );
    assert!(
        rewritten.contains("iface ens4 inet manual"),
        "selected LAN must be manual:\n{rewritten}"
    );
    assert!(
        rewritten.contains("auto ens5"),
        "unselected interface must stay in auto:\n{rewritten}"
    );
    assert!(
        !rewritten.contains("ens3 inet static"),
        "selected WAN options must be stripped:\n{rewritten}"
    );
    assert!(
        rewritten.contains("address 198.51.100.10/24"),
        "unselected interface stanza must be untouched:\n{rewritten}"
    );
    assert!(
        harness
            .backups_dir()
            .join("hostnet/ifupdown/manifest.json")
            .is_file(),
        "the verbatim ifupdown backup manifest is missing"
    );
    let ifup_calls = std::fs::read_to_string(harness.world.path("ifup-calls.log")).unwrap();
    assert!(
        ifup_calls.contains("--no-act"),
        "the unmanaged config must be dry-run validated: {ifup_calls}"
    );

    // networking.service 全程不动:不 stop、不掩蔽,保持 active/enabled。
    let state = harness.host.join("systemd-state/units/networking.service");
    assert!(
        state.join("active").is_file(),
        "networking.service must stay active"
    );
    assert!(
        state.join("enabled").is_file(),
        "networking.service must stay enabled"
    );
    assert!(
        !state.join("masked").exists(),
        "networking.service must not be masked"
    );
    let calls = std::fs::read_to_string(harness.world.path("systemctl-calls.jsonl")).unwrap();
    assert!(
        !calls.contains("networking.service"),
        "networking.service must not receive any systemctl call during takeover:\n{calls}"
    );
    assert!(
        !calls.contains("stop\",\"NetworkManager.service"),
        "the missing NetworkManager unit was stopped"
    );

    // 回滚:原文件按 manifest 逐字恢复,备份清除,networking.service 重启
    // (fake systemctl 记录 restart 调用并回到 active)。
    let rollback = harness.network_command(&["rollback", "--automatic"]);
    assert_success(&rollback);
    assert_eq!(
        std::fs::read_to_string(&interfaces).unwrap(),
        original,
        "rollback must restore the interfaces file byte for byte"
    );
    assert!(
        !harness.backups_dir().join("hostnet").exists(),
        "the hostnet backup must be removed after a successful restore"
    );
    let calls = std::fs::read_to_string(harness.world.path("systemctl-calls.jsonl")).unwrap();
    assert!(
        calls.contains("[\"restart\",\"networking.service\"]"),
        "rollback must restart networking.service to re-apply the original config:\n{calls}"
    );
    assert!(
        state.join("active").is_file(),
        "networking.service must be active again"
    );
}

/// systemd-networkd 是受支持的摘除目标,专门场景覆盖;真正未知的活动网络
/// 管理器(如 wicked)仍在 preflight 被整体拒绝。
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

/// HNET-14:netplan 管理的宿主(`/etc/netplan/*.yaml` 存在)接管在 preflight
/// 拒绝——netplan 渲染进搜索路径的配置会被重新生成,文件摘除无法保持稳定;
/// 拒绝发生在触碰任何宿主配置之前。
#[test]
fn network_takeover_rejects_netplan_configured_hosts() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("network-netplan", "healthy", 10_000);
    harness.seed_netplan();
    let netplan_yaml = harness.host.join("netplan/10-config.yaml");

    let output = harness.run_takeover();
    assert!(
        !output.status.success(),
        "a netplan-configured host must be rejected"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("netplan configuration is present"),
        "unexpected rejection reason:\n{stderr}"
    );
    assert!(
        netplan_yaml.is_file(),
        "the rejection must not touch the netplan config"
    );
    assert!(
        !harness.backups_dir().join("hostnet").exists(),
        "no hostnet backup may be created"
    );
}

/// NET-15:systemd-networkd 主机(无 NM/firewalld)的接管移出引用选中接口的
/// `.network` 文件、reload 运行中的 networkd;回滚按备份逐字重建文件。
/// 服务全程不被停止。
#[test]
fn network_takeover_supports_systemd_networkd_without_network_manager() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("network-networkd", "healthy", 10_000);
    harness.seed_host_service("systemd-resolved.service");
    harness.seed_systemd_networkd();
    let wan_network = harness
        .host
        .join(format!("systemd-network/{SEEDED_NETWORKD_FILE}"));

    let output = harness.run_takeover();
    assert!(
        output.status.success(),
        "takeover with systemd-networkd failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Name=ens3 精确集 ⊆ 选中集(ens3+ens4):文件整个移出,备份持有逐字副本。
    assert!(
        !wan_network.exists(),
        "the .network file bound to the selected WAN must be removed"
    );
    assert!(
        harness
            .backups_dir()
            .join("hostnet/networkd/manifest.json")
            .is_file(),
        "the networkd backup manifest is missing"
    );
    let networkctl_calls =
        std::fs::read_to_string(harness.world.path("networkctl-calls.log")).unwrap();
    assert!(
        networkctl_calls.contains("reload"),
        "the removal must be applied to the running networkd:\n{networkctl_calls}"
    );
    let calls = std::fs::read_to_string(harness.world.path("systemctl-calls.jsonl")).unwrap();
    for verb in ["stop", "disable", "mask"] {
        assert!(
            !calls.contains(&format!("[\"{verb}\",\"systemd-networkd.service\"]")),
            "systemd-networkd must not be {verb}ed:\n{calls}"
        );
    }

    // 回滚:文件按 manifest 逐字重建,networkd 再次 reload,备份清除。
    let rollback = harness.network_command(&["rollback", "--automatic"]);
    assert_success(&rollback);
    assert_eq!(
        std::fs::read_to_string(&wan_network).unwrap(),
        SEEDED_NETWORKD_WAN,
        "rollback must restore the .network file byte for byte"
    );
    assert!(
        !harness.backups_dir().join("hostnet").exists(),
        "the hostnet backups must be removed after the restore"
    );
    let networkctl_calls =
        std::fs::read_to_string(harness.world.path("networkctl-calls.log")).unwrap();
    assert_eq!(
        networkctl_calls.matches("reload").count(),
        2,
        "networkd must be reloaded after unmanage and again after the restore:\n{networkctl_calls}"
    );
}
