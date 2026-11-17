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
        .env("LKIT_TERRITORY", &harness.territory)
        .env("LKIT_TEST_TTY", &pty.slave_path)
        .args(["reinit", "--admin-user", "admin", "--password-file"])
        .arg(password)
        .args(["--test-runtime"])
        .arg(&harness.runtime_config)
        .output()
        .unwrap()
}

#[test]
fn reinit_rebuilds_network_config_and_commits_after_confirmation() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (harness, _territory) =
        InstallHarness::new("reinit", "healthy", 10_000).with_tmpfs_territory();
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

    let backups_before = std::fs::read_dir(harness.backups_dir()).unwrap().count();
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
        transaction_of_operation(&harness.territory, "reinit")["phase"],
        "awaiting_network_confirmation",
        "reinit must always enter the network confirmation window"
    );
    assert!(
        harness.state_path().exists(),
        "the pending state must be written before confirmation"
    );
    let backups_after = std::fs::read_dir(harness.backups_dir()).unwrap().count();
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
    let state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(harness.state_path()).unwrap()).unwrap();
    assert_eq!(state["active_version"], VERSION);
    assert_eq!(
        transaction_of_operation(&harness.territory, "reinit")["phase"],
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

/// REI-01:无有效状态(未安装现场)时拒绝,退出码 2,不写任何文件。
#[test]
fn reinit_rejects_without_an_installation() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("reinit-not-installed", "healthy", 10_000);

    let output = harness
        .command()
        .args(["reinit", "--admin-user", "admin", "--password-file"])
        .arg(&harness.password)
        .args(["--test-runtime"])
        .arg(&harness.runtime_config)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(2),
        "reinit without an installation must be a usage error\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("install-state.json"),
        "the refusal must explain the missing installation\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !harness.state_path().exists(),
        "no state may be created by a rejected reinit"
    );
}

/// REI-01:宿主网络服务未接管时拒绝,退出码 2,不创建 reinit 事务。
#[test]
fn reinit_rejects_when_network_is_not_taken_over() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("reinit-no-takeover", "healthy", 10_000);
    assert_success(&harness.run());

    let output = harness
        .command()
        .args(["reinit", "--admin-user", "admin", "--password-file"])
        .arg(&harness.password)
        .args(["--test-runtime"])
        .arg(&harness.runtime_config)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(2),
        "reinit without network takeover must be a usage error\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("handed over"),
        "the refusal must explain the missing takeover\nstderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let has_reinit_transaction = std::fs::read_dir(harness.transactions_dir())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("json"))
        .any(|entry| {
            serde_json::from_slice::<serde_json::Value>(&std::fs::read(entry.path()).unwrap())
                .unwrap()["operation"]
                == "reinit"
        });
    assert!(
        !has_reinit_transaction,
        "a rejected reinit must not create a transaction"
    );
    assert!(
        harness.state_path().exists(),
        "the existing installation state must be untouched"
    );
}

/// REI-02:交互确认拒绝时先于任何修改返回(退出码 1),不创建事务、不创建
/// `.lkb`、不停止服务、不改写数据。
#[test]
fn reinit_refused_confirmation_leaves_no_side_effects() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("reinit-refused", "healthy", 10_000);
    harness.seed_host_services();
    let output = harness.run_takeover();
    assert!(
        output.status.success(),
        "takeover install failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_success(&harness.network_command(&["confirm"]));

    let backups_before = std::fs::read_dir(harness.backups_dir()).unwrap().count();
    let init_path = harness.install_root.join("data/landscape_init.toml");
    let init_before = std::fs::read_to_string(&init_path).unwrap();
    let new_password = harness.world.path("reinit-refused-password");
    std::fs::write(&new_password, b"NewSecret456\n").unwrap();
    std::fs::set_permissions(&new_password, std::fs::Permissions::from_mode(0o600)).unwrap();

    // 网络计划向导后,第一个确认(重配置计划)输入 no 拒绝。
    let mut pty = Pty::open();
    pty.master.write_all(b"1\n1\n\n\n\nno\n").unwrap();
    let refused = Command::new(LKIT)
        .env(
            lkit_test_fixture::SYSTEMCTL_CONFIG_ENV,
            &harness.world.systemctl_config,
        )
        .env("LKIT_TERRITORY", &harness.territory)
        .env("LKIT_TEST_TTY", &pty.slave_path)
        .args(["reinit", "--admin-user", "admin", "--password-file"])
        .arg(&new_password)
        .args(["--test-runtime"])
        .arg(&harness.runtime_config)
        .output()
        .unwrap();
    assert_eq!(
        refused.status.code(),
        Some(1),
        "a refused reinit must fail\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&refused.stdout),
        String::from_utf8_lossy(&refused.stderr)
    );
    assert_eq!(
        std::fs::read_dir(harness.backups_dir()).unwrap().count(),
        backups_before,
        "a refused reinit must not create a protection .lkb"
    );
    assert_eq!(
        std::fs::read_to_string(&init_path).unwrap(),
        init_before,
        "a refused reinit must not touch the data directory"
    );
    let has_reinit_transaction = std::fs::read_dir(harness.transactions_dir())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("json"))
        .any(|entry| {
            serde_json::from_slice::<serde_json::Value>(&std::fs::read(entry.path()).unwrap())
                .unwrap()["operation"]
                == "reinit"
        });
    assert!(
        !has_reinit_transaction,
        "a refused reinit must not create a transaction"
    );
    let active = systemctl(&harness.world, &["is-active", "landscape-router.service"]);
    assert_eq!(
        String::from_utf8_lossy(&active.stdout).trim(),
        "active",
        "the service must keep running after a refused reinit"
    );
}

/// REI-08:激活后健康检查失败 → 自动回滚恢复原数据与服务状态
/// (退出码 5),事务终止且不进入确认窗口。
#[test]
fn reinit_rolls_back_when_activation_health_check_fails() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("reinit-health-fail", "healthy", 10_000);
    harness.seed_host_services();
    let output = harness.run_takeover();
    assert!(
        output.status.success(),
        "takeover install failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_success(&harness.network_command(&["confirm"]));
    let init_path = harness.install_root.join("data/landscape_init.toml");
    let original_init = std::fs::read_to_string(&init_path).unwrap();
    assert!(original_init.contains("Secret123"));

    let new_password = harness.world.path("reinit-health-password");
    std::fs::write(&new_password, b"NewSecret456\n").unwrap();
    std::fs::set_permissions(&new_password, std::fs::Permissions::from_mode(0o600)).unwrap();

    // 健康检查注入失败:base_url 指向不可达端口,其余探针保持 fixture 现场。
    let mut runtime: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&harness.runtime_config).unwrap()).unwrap();
    runtime["health"]["base_url"] = serde_json::Value::String("https://127.0.0.1:1".into());
    let broken_runtime = harness.world.path("reinit-broken-health.json");
    std::fs::write(
        &broken_runtime,
        serde_json::to_vec_pretty(&runtime).unwrap(),
    )
    .unwrap();

    let mut pty = Pty::open();
    pty.master
        .write_all(b"1\n1\n\n\n\nyes\nyes\nyes\n")
        .unwrap();
    let reinit = Command::new(LKIT)
        .env(
            lkit_test_fixture::SYSTEMCTL_CONFIG_ENV,
            &harness.world.systemctl_config,
        )
        .env("LKIT_TERRITORY", &harness.territory)
        .env("LKIT_TEST_TTY", &pty.slave_path)
        .args(["reinit", "--admin-user", "admin", "--password-file"])
        .arg(&new_password)
        .args(["--test-runtime"])
        .arg(&broken_runtime)
        .output()
        .unwrap();
    assert_eq!(
        reinit.status.code(),
        Some(6),
        "the rollback health check must fail with the injected dead base_url\nstdout:\n{}\nstderr:\n{}\nservice log:\n{}",
        String::from_utf8_lossy(&reinit.stdout),
        String::from_utf8_lossy(&reinit.stderr),
        harness.service_log()
    );
    assert_eq!(
        std::fs::read_to_string(&init_path).unwrap(),
        original_init,
        "the rollback must restore the previous data directory"
    );
    let transaction = transaction_of_operation(&harness.territory, "reinit");
    assert_ne!(
        transaction["phase"], "awaiting_network_confirmation",
        "a failed reinit must not enter the confirmation window"
    );
    assert_ne!(transaction["phase"], "committed");
    let active = systemctl(&harness.world, &["is-active", "landscape-router.service"]);
    assert_eq!(
        String::from_utf8_lossy(&active.stdout).trim(),
        "active",
        "the rollback must restore the running service"
    );
}

#[test]
fn reinit_rollback_restores_previous_data() {
    if !e2e_enabled() {
        return;
    }
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
    let transaction = transaction_of_operation(&harness.territory, "reinit");
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
    let state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(harness.state_path()).unwrap()).unwrap();
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

#[test]
fn native_reinit_recovers_activation_and_releases_the_queue_for_network_confirmation() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for point in ["reinit-activated", "reinit-awaiting"] {
        let h = InstallHarness::new("reinit-native-crash", "healthy", 10_000);
        let mut config: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&h.runtime_config).unwrap()).unwrap();
        config["network_confirm_timeout_ms"] = 180_000.into();
        write_json(&h.runtime_config, &config);
        h.seed_host_services();
        assert_success(&h.run_takeover());
        assert_success(&h.network_command(&["confirm"]));
        std::fs::write(h.install_root.join("data/original-marker"), b"original").unwrap();
        let runtime = h.daemon_runtime();
        let gate = h.run_dir().join(format!("test-repair-{point}.gate"));
        std::fs::write(&gate, b"pause").unwrap();
        let mut daemon = h.spawn_daemon_at_checkpoint(&runtime, point);
        let mut pty = Pty::open();
        pty.master
            .write_all(b"1\n1\n\n\n\nyes\nyes\nyes\n")
            .unwrap();
        struct Observer(std::process::Child);
        impl Drop for Observer {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let observer = Observer(
            h.command()
                .env("LKIT_TEST_TTY", &pty.slave_path)
                .args(["reinit", "--admin-user", "admin", "--password-file"])
                .arg(&h.password)
                .arg("--test-runtime")
                .arg(&runtime)
                .spawn()
                .unwrap(),
        );
        let wait = |condition: &mut dyn FnMut() -> bool| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(50);
            while !condition() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "reinit timed out at {point}"
                );
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        };
        wait(&mut || {
            h.run_dir()
                .join(format!("test-repair-{point}.reached"))
                .exists()
        });
        let reinit_record = || -> serde_json::Value {
            std::fs::read_dir(h.run_dir().join("operations"))
                .unwrap()
                .filter_map(Result::ok)
                .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
                .map(|e| {
                    serde_json::from_slice::<serde_json::Value>(&std::fs::read(e.path()).unwrap())
                        .unwrap()
                })
                .find(|r| r["kind"] == "reinit")
                .unwrap()
        };
        let before = reinit_record();
        assert!(!before.to_string().contains("Secret123"));
        assert!(
            !h.run_dir()
                .join("operations")
                .join(format!("{}.password", before["id"].as_str().unwrap()))
                .exists()
        );
        let before_tx = transaction_of_operation(&h.territory, "reinit");
        assert_eq!(before_tx["schema_version"], 11);
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
        if point == "reinit-activated" {
            wait(&mut || reinit_record()["status"] == "failed");
            assert_eq!(reinit_record()["exit_code"], 5);
            assert_eq!(
                std::fs::read(h.install_root.join("data/original-marker")).unwrap(),
                b"original"
            );
        } else {
            wait(&mut || reinit_record()["status"] == "awaiting_confirmation");
            assert_eq!(
                transaction_of_operation(&h.territory, "reinit")["network_takeover"]["confirmation_deadline"],
                before_tx["network_takeover"]["confirmation_deadline"]
            );
            assert_success(
                &h.command()
                    .args(["network", "confirm", "--test-runtime"])
                    .arg(&runtime)
                    .output()
                    .unwrap(),
            );
            wait(&mut || reinit_record()["status"] == "succeeded");
            assert_eq!(reinit_record()["exit_code"], 0);
        }
        assert_eq!(reinit_record()["id"], before["id"]);
        assert_eq!(reinit_record()["transaction_id"], before["transaction_id"]);
    }
}

#[test]
fn reinit_rejects_reactivated_host_manager_after_committed_takeover() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("reinit-host-manager-drift", "healthy", 10_000);
    harness.seed_host_services();
    assert_success(&harness.run_takeover());
    assert_success(&harness.network_command(&["confirm"]));
    let state = std::fs::read(harness.state_path()).unwrap();
    let data = std::fs::read(harness.install_root.join("data/landscape_init.toml")).unwrap();
    let before = transaction_count(&harness.territory);
    // Firewalld remains stopped, but that alone must not authorize reinit.
    assert_success(&systemctl(
        &harness.world,
        &["unmask", "NetworkManager.service"],
    ));
    assert_success(&systemctl(
        &harness.world,
        &["start", "NetworkManager.service"],
    ));
    let output = run_reinit(&harness, &harness.password);
    assert!(
        !output.status.success(),
        "reactivated host manager must block reinit"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("NetworkManager.service"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(transaction_count(&harness.territory), before);
    assert_eq!(std::fs::read(harness.state_path()).unwrap(), state);
    assert_eq!(
        std::fs::read(harness.install_root.join("data/landscape_init.toml")).unwrap(),
        data
    );
}

#[test]
fn reinit_accepts_proven_takeover_without_installed_host_network_services() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("reinit-no-host-services", "healthy", 10_000);
    assert_success(&harness.run_takeover());
    assert_success(&harness.network_command(&["confirm"]));
    let installed = transaction_of_operation(&harness.territory, "install");
    assert!(
        installed["network_takeover"]["host_services"]
            .as_array()
            .unwrap()
            .iter()
            .all(|service| service["installed"] == false)
    );
    assert_success(&run_reinit(&harness, &harness.password));
    assert_eq!(
        transaction_of_operation(&harness.territory, "reinit")["phase"],
        "awaiting_network_confirmation"
    );
    assert_success(&harness.network_command(&["rollback"]));
    assert_eq!(
        transaction_of_operation(&harness.territory, "reinit")["phase"],
        "rolled_back"
    );
}
