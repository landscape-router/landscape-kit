use std::io::Write;

use super::support::*;

#[test]
fn uninstalls_an_existing_installation_through_full_cli() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("uninstall", "healthy", 10_000);
    assert_success(&harness.run());
    let config_path = harness.config_path();
    std::fs::write(&config_path, b"[repository]\n").unwrap();

    let output = harness
        .command()
        .args(["uninstall", "--non-interactive", "--yes", "--test-runtime"])
        .arg(&harness.runtime_config)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "uninstall failed with {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("uninstalled Landscape version"),
        "uninstall output must report the removed version\nstdout:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        !harness.state_path().exists(),
        "install-state.json must be removed"
    );
    assert!(!harness.install_root.join("current").exists());
    assert!(!harness.install_root.join("releases").exists());
    assert!(!harness.install_root.join("data").exists());
    assert!(!harness.install_root.join("service").exists());
    assert_eq!(
        std::fs::read_to_string(&config_path).unwrap(),
        "[repository]\n",
        "config.toml must be preserved byte-for-byte"
    );
    assert!(
        harness.backups_dir().is_dir(),
        "backups/ must be preserved as the protection backup location"
    );
    assert!(
        harness.transactions_dir().is_dir(),
        "transactions/ must be preserved for diagnosis"
    );
    assert!(
        harness.logs_dir().is_dir(),
        "logs/ must be preserved in the lkit territory"
    );
    assert!(
        harness.run_dir().is_dir(),
        "run/ must be preserved in the lkit territory"
    );
    assert!(
        !harness.host.join("units/landscape-router.service").exists(),
        "the systemd registration link must be removed"
    );
    let active = systemctl(&harness.world, &["is-active", "landscape-router.service"]);
    assert!(!active.status.success());
    assert_eq!(String::from_utf8_lossy(&active.stdout).trim(), "inactive");
    let lkb_count = std::fs::read_dir(harness.backups_dir())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("lkb"))
        .count();
    assert_eq!(lkb_count, 1, "the uninstall protection backup must be kept");

    let leftover_transactions: Vec<_> = std::fs::read_dir(harness.transactions_dir())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("json"))
        .collect();
    assert!(
        leftover_transactions.is_empty(),
        "transactions of the uninstalled root must be purged, found: {leftover_transactions:?}"
    );

    let again = harness
        .command()
        .args(["uninstall", "--non-interactive", "--yes", "--test-runtime"])
        .arg(&harness.runtime_config)
        .output()
        .unwrap();
    assert_eq!(
        again.status.code(),
        Some(2),
        "a second uninstall must be rejected with exit 2\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&again.stdout),
        String::from_utf8_lossy(&again.stderr)
    );
    assert!(
        String::from_utf8_lossy(&again.stderr).contains("install-state.json"),
        "a second uninstall must explain the missing installation\nstderr:\n{}",
        String::from_utf8_lossy(&again.stderr)
    );
}

/// UNI-08:网络接管特征(宿主网络服务被 stop/disable/mask)时交互确认警告,
/// 确认后继续卸载,服务停止、状态删除。
#[test]
fn uninstall_confirms_network_takeover_before_continuing() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("uninstall-takeover", "healthy", 10_000);
    harness.seed_host_services();
    let output = harness.run_takeover();
    assert!(
        output.status.success(),
        "takeover install failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_success(&harness.network_command(&["confirm"]));
    assert_host_services_masked(
        &harness,
        &[
            "NetworkManager.service",
            "firewalld.service",
            "systemd-resolved.service",
        ],
    );

    let mut pty = Pty::open();
    let mut command = harness.command();
    command
        .args(["uninstall", "--test-runtime"])
        .arg(&harness.runtime_config);
    attach_pty(&mut command, &pty);
    let mut child = command.spawn().unwrap();
    pty.read_until("type yes to continue", std::time::Duration::from_secs(60));
    pty.master.write_all(b"yes\nyes\n").unwrap();
    let prompt = pty
        .read_until("host network services", std::time::Duration::from_secs(60))
        .replace('\x1b', "");
    assert!(
        prompt.contains("NetworkManager"),
        "the confirmation must describe the masked host services:\n{prompt}"
    );
    pty.master.write_all(b"yes\n").unwrap();
    let status = child.wait().unwrap();
    assert!(
        status.success(),
        "uninstall failed with {status:?}\npty output:\n{prompt}"
    );
    assert!(!harness.state_path().exists());
    let active = systemctl(&harness.world, &["is-active", "landscape-router.service"]);
    assert!(!active.status.success());
}

/// UNI-11:安装状态损坏时拒绝卸载(退出码非 0),不触碰任何现场。
#[test]
fn uninstall_rejects_corrupted_installation_state() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("uninstall-corrupted", "healthy", 10_000);
    assert_success(&harness.run());
    let config_path = harness.config_path();
    std::fs::write(&config_path, b"[repository]\n").unwrap();
    let config_before = std::fs::read_to_string(&config_path).unwrap();
    std::fs::write(harness.state_path(), b"{not valid json").unwrap();

    let output = harness
        .command()
        .args(["uninstall", "--non-interactive", "--yes", "--test-runtime"])
        .arg(&harness.runtime_config)
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "uninstall with corrupted state must be rejected\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(&config_path).unwrap(),
        config_before,
        "a rejected uninstall must not touch config.toml"
    );
    assert!(
        harness.install_root.join("current").exists(),
        "the installation must be untouched"
    );
}

/// UNI-14:接管安装(ifupdown 摘除)卸载时按 backups/hostnet 的 manifest 逐字
/// 恢复宿主 ifupdown 配置,重启 networking.service 重新套用原配置。
#[test]
fn uninstall_restores_ifupdown_host_config_after_takeover() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("uninstall-hostnet", "healthy", 10_000);
    harness.seed_host_service("networking.service");
    let interfaces = harness.host.join("network-interfaces");
    let original = "auto ens3 ens4\n\
iface ens3 inet static\n\
    address 192.0.2.10/24\n\
    gateway 192.0.2.1\n\
\n\
iface ens4 inet dhcp\n";
    std::fs::write(&interfaces, original).unwrap();
    assert_success(&harness.run_takeover());
    assert_success(&harness.network_command(&["confirm"]));
    assert!(
        std::fs::read_to_string(&interfaces)
            .unwrap()
            .contains("iface ens3 inet manual"),
        "the takeover must have rewritten the selected interfaces"
    );
    assert!(
        harness
            .backups_dir()
            .join("hostnet/ifupdown/manifest.json")
            .is_file(),
        "the standing hostnet backup must exist while the takeover is committed"
    );

    let output = harness
        .command()
        .args(["uninstall", "--non-interactive", "--yes", "--test-runtime"])
        .arg(&harness.runtime_config)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "uninstall after takeover failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(&interfaces).unwrap(),
        original,
        "uninstall must restore the interfaces file byte for byte"
    );
    assert!(
        !harness.backups_dir().join("hostnet").exists(),
        "the hostnet backup must be removed after the restore"
    );
    let calls = std::fs::read_to_string(harness.world.path("systemctl-calls.jsonl")).unwrap();
    assert!(
        calls.contains("[\"restart\",\"networking.service\"]"),
        "uninstall must restart networking.service to re-apply the host config:\n{calls}"
    );
}

/// UNI-15:接管安装(NM drop-in + firewalld zone 摘除)卸载时删除 drop-in、逐字
/// 恢复 zone,并 reload 运行中的 NM/firewalld;两者全程不被停止。
#[test]
fn uninstall_restores_nm_and_firewalld_after_takeover() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("uninstall-nm-firewalld", "healthy", 10_000);
    harness.seed_host_services();
    assert_success(&harness.run_takeover());
    assert_success(&harness.network_command(&["confirm"]));
    let drop_in = harness.host.join("nm-conf.d/lkit-unmanage.conf");
    assert!(
        drop_in.is_file(),
        "the takeover must have written the NM drop-in"
    );
    let zone = std::fs::read_to_string(harness.host.join("firewalld-zones/public.xml")).unwrap();
    assert!(
        !zone.contains("ens3"),
        "the takeover must have removed ens3:\n{zone}"
    );
    assert!(
        harness
            .backups_dir()
            .join("hostnet/nm/manifest.json")
            .is_file()
            && harness
                .backups_dir()
                .join("hostnet/firewalld/manifest.json")
                .is_file(),
        "per-adapter hostnet backups must exist while the takeover is committed"
    );

    let output = harness
        .command()
        .args(["uninstall", "--non-interactive", "--yes", "--test-runtime"])
        .arg(&harness.runtime_config)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "uninstall after multi-adapter takeover failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !drop_in.exists(),
        "uninstall must delete the NM drop-in it created"
    );
    assert_eq!(
        std::fs::read_to_string(harness.host.join("firewalld-zones/public.xml")).unwrap(),
        SEEDED_FIREWALLD_ZONE,
        "uninstall must restore the zone byte for byte"
    );
    assert!(
        !harness.backups_dir().join("hostnet").exists(),
        "the hostnet backups must be removed after the restore"
    );
    let calls = std::fs::read_to_string(harness.world.path("systemctl-calls.jsonl")).unwrap();
    assert!(
        !calls.contains("stop\",\"NetworkManager.service"),
        "NetworkManager must never be stopped:\n{calls}"
    );
    assert!(
        !calls.contains("stop\",\"firewalld.service"),
        "firewalld must never be stopped:\n{calls}"
    );
    let nmcli_calls = std::fs::read_to_string(harness.world.path("nmcli-calls.log")).unwrap();
    assert!(
        nmcli_calls.matches("general reload").count() >= 2,
        "NM must be reloaded after unmanage and again after the restore:\n{nmcli_calls}"
    );
    let firewall_cmd_calls =
        std::fs::read_to_string(harness.world.path("firewall-cmd-calls.log")).unwrap();
    assert!(
        firewall_cmd_calls.matches("--reload").count() >= 2,
        "firewalld must be reloaded after unmanage and again after the restore:\n{firewall_cmd_calls}"
    );
}
