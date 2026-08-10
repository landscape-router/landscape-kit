use std::process::Command;

use super::support::*;

#[test]
fn uninstalls_an_existing_installation_through_full_cli() {
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("uninstall", "healthy", 10_000);
    assert_success(&harness.run());
    std::fs::write(harness.install_root.join("config.toml"), b"[repository]\n").unwrap();

    let output = Command::new(LKIT)
        .env(
            lkit_test_fixture::SYSTEMCTL_CONFIG_ENV,
            &harness.world.systemctl_config,
        )
        .args(["uninstall", "--non-interactive", "--yes", "--install-dir"])
        .arg(&harness.install_root)
        .args(["--test-runtime"])
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
        !harness
            .install_root
            .join("state/install-state.json")
            .exists(),
        "install-state.json must be removed"
    );
    assert!(!harness.install_root.join("current").exists());
    assert!(!harness.install_root.join("releases").exists());
    assert!(!harness.install_root.join("data").exists());
    assert!(!harness.install_root.join("service").exists());
    assert!(!harness.install_root.join("logs").exists());
    assert!(!harness.install_root.join("run").exists());
    assert_eq!(
        std::fs::read_to_string(harness.install_root.join("config.toml")).unwrap(),
        "[repository]\n",
        "config.toml must be preserved byte-for-byte"
    );
    assert!(
        harness.install_root.join("backups").is_dir(),
        "backups/ must be preserved as the protection backup location"
    );
    assert!(
        harness.install_root.join("transactions").is_dir(),
        "transactions/ must be preserved for diagnosis"
    );
    assert!(
        !harness.host.join("units/landscape-router.service").exists(),
        "the systemd registration link must be removed"
    );
    let active = systemctl(&harness.world, &["is-active", "landscape-router.service"]);
    assert!(!active.status.success());
    assert_eq!(String::from_utf8_lossy(&active.stdout).trim(), "inactive");
    let lkb_count = std::fs::read_dir(harness.install_root.join("backups"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("lkb"))
        .count();
    assert_eq!(lkb_count, 1, "the uninstall protection backup must be kept");
    let state_dir = harness.install_root.join("state");
    assert!(
        !state_dir.exists(),
        "state/ directory must be removed with the state file"
    );
}
