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
    pty.master.write_all(b"yes\n").unwrap();
    let prompt = pty
        .read_until("host network services", std::time::Duration::from_secs(60))
        .replace('\x1b', "");
    assert!(
        prompt.contains("NetworkManager"),
        "the confirmation must describe the masked host services:\n{prompt}"
    );
    pty.master.write_all(b"yes\nyes\n").unwrap();
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
