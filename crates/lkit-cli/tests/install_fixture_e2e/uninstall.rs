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
}
