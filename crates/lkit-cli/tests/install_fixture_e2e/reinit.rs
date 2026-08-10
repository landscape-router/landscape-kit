use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};

use super::support::*;

/// 复用 run_takeover 的向导输入(网络计划),后续追加 3 个 yes 确认。
fn run_reinit(harness: &InstallHarness, password: &Path) -> Output {
    let mut pty = Pty::open();
    pty.master
        .write_all(b"1\n1\n\n\n\nyes\nyes\nyes\n")
        .unwrap();
    Command::new(LKIT)
        .env(
            lkit_test_fixture::SYSTEMCTL_CONFIG_ENV,
            &harness.world.systemctl_config,
        )
        .env("LKIT_INTERNAL_SYSTEMD_WORKER_TTY", &pty.slave_path)
        .args(["reinit", "--install-dir"])
        .arg(&harness.install_root)
        .args(["--admin-user", "admin", "--password-file"])
        .arg(password)
        .args(["--test-runtime"])
        .arg(&harness.runtime_config)
        .output()
        .unwrap()
}

#[test]
fn reinit_rebuilds_network_config_and_commits_after_confirmation() {
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("reinit", "healthy", 10_000);
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

    let backups_before = std::fs::read_dir(harness.install_root.join("backups"))
        .unwrap()
        .count();
    let new_password = harness.world.path("reinit-password");
    std::fs::write(&new_password, b"NewSecret456\n").unwrap();
    std::fs::set_permissions(&new_password, std::fs::Permissions::from_mode(0o600)).unwrap();

    let reinit = run_reinit(&harness, &new_password);
    assert!(
        reinit.status.success(),
        "reinit failed with {:?}\nstdout:\n{}\nstderr:\n{}\nservice log:\n{}",
        reinit.status.code(),
        String::from_utf8_lossy(&reinit.stdout),
        String::from_utf8_lossy(&reinit.stderr),
        harness.service_log()
    );
    assert_eq!(
        transaction_of_operation(&harness.install_root, "reinit")["phase"],
        "awaiting_network_confirmation",
        "reinit must always enter the network confirmation window"
    );
    assert!(
        harness
            .install_root
            .join("state/install-state.json")
            .exists(),
        "the pending state must be written before confirmation"
    );
    let backups_after = std::fs::read_dir(harness.install_root.join("backups"))
        .unwrap()
        .count();
    assert!(
        backups_after > backups_before,
        "reinit must create a protection .lkb backup"
    );
    let init: toml::Value = toml::from_str(
        &std::fs::read_to_string(harness.install_root.join("data/landscape_init.toml")).unwrap(),
    )
    .unwrap();
    assert_eq!(init["version"].as_str(), Some(VERSION));
    assert_eq!(
        init["config"]["auth"]["admin_pass"].as_str(),
        Some("NewSecret456"),
        "the new init config must carry the newly entered credentials"
    );

    assert_success(&harness.network_command(&["confirm"]));
    let state: serde_json::Value = serde_json::from_slice(
        &std::fs::read(harness.install_root.join("state/install-state.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(state["active_version"], VERSION);
    assert_eq!(
        transaction_of_operation(&harness.install_root, "reinit")["phase"],
        "committed"
    );
    assert!(
        harness
            .install_root
            .join("data/landscape_init.lock")
            .is_file(),
        "the rebuilt data directory must be initialized"
    );
}

#[test]
fn reinit_rollback_restores_previous_data() {
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("reinit-rollback", "healthy", 10_000);
    harness.seed_host_services();
    let output = harness.run_takeover();
    assert!(output.status.success());
    assert_success(&harness.network_command(&["confirm"]));
    let original_init =
        std::fs::read_to_string(harness.install_root.join("data/landscape_init.toml")).unwrap();
    assert!(original_init.contains("Secret123"));

    let new_password = harness.world.path("reinit-password");
    std::fs::write(&new_password, b"NewSecret456\n").unwrap();
    std::fs::set_permissions(&new_password, std::fs::Permissions::from_mode(0o600)).unwrap();
    let reinit = run_reinit(&harness, &new_password);
    assert!(
        reinit.status.success(),
        "reinit failed:\n{}",
        String::from_utf8_lossy(&reinit.stderr)
    );
    let rebuilt =
        std::fs::read_to_string(harness.install_root.join("data/landscape_init.toml")).unwrap();
    assert!(rebuilt.contains("NewSecret456"));

    let rollback = harness.network_command(&["rollback"]);
    assert_success(&rollback);
    let transaction = transaction_of_operation(&harness.install_root, "reinit");
    assert_eq!(transaction["phase"], "rolled_back");
    assert_eq!(transaction["operation"], "reinit");
    let restored =
        std::fs::read_to_string(harness.install_root.join("data/landscape_init.toml")).unwrap();
    assert_eq!(
        restored, original_init,
        "the rollback must restore the previous data directory byte-for-byte"
    );
    assert!(
        harness
            .install_root
            .join("data/landscape_db.sqlite")
            .is_file(),
        "the restored data must contain the original database"
    );
    let state: serde_json::Value = serde_json::from_slice(
        &std::fs::read(harness.install_root.join("state/install-state.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(state["active_version"], VERSION);
    assert!(
        std::fs::read_dir(harness.host.join("units"))
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("lkit-network-")),
        "the rollback must remove the recovery units"
    );
}
