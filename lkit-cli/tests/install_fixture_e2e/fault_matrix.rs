use super::support::*;

/// Boundary errors return through normal rollback; crash tests exercise abrupt termination separately.
#[test]
fn install_and_switch_boundary_errors_restore_the_original_state() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for point in [
        "install-before-publish",
        "install-published",
        "install-before-start",
        "switch-before-publish",
        "switch-before-start",
        "switch-before-state",
    ] {
        let h = InstallHarness::new(point, "healthy", 10_000);
        let switch = point.starts_with("switch-");
        if switch {
            assert_success(&h.run());
        } else {
            std::fs::create_dir_all(h.install_root.join("data")).unwrap();
            std::fs::write(h.install_root.join("data/original-marker"), b"original").unwrap();
        }
        let state = std::fs::read(h.state_path()).ok();
        let current = std::fs::read_link(h.install_root.join("current")).ok();
        let repository = RepositoryServer::start(repository_files_for("2.0.0"));
        let runtime = h.daemon_runtime();
        let mut daemon = h.spawn_daemon_with_fault(&runtime, point);
        let mut command = h.command();
        if switch {
            command.args([
                "switch",
                "--version",
                "2.0.0",
                "--repository",
                &repository.base_url,
            ]);
        } else {
            command
                .args([
                    "install",
                    "--non-interactive",
                    "--version",
                    VERSION,
                    "--repository",
                    &h.repository.base_url,
                    "--install-dir",
                ])
                .arg(&h.install_root)
                .args(["--admin-user", "admin", "--password-file"])
                .arg(&h.password);
        }
        let output = command
            .arg("--test-runtime")
            .arg(&runtime)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{point} must fail");
        assert!(
            h.run_dir()
                .join(format!("test-repair-{point}.failed"))
                .is_file(),
            "fault was not reached: {point}"
        );
        // Install surfaces the error through its operation record; switch may report rollback.
        let operation_output = h.command().args(["operation", "list"]).output().unwrap();
        assert_success(&operation_output);
        let records: Vec<serde_json::Value> =
            serde_json::from_slice(&operation_output.stdout).unwrap();
        assert_eq!(records.len(), 1, "{point}: {records:?}");
        assert_eq!(records[0]["status"], "failed", "{point}: {records:?}");
        assert_eq!(records[0]["exit_code"], if switch { 5 } else { 1 });
        assert_eq!(std::fs::read(h.state_path()).ok(), state, "{point}");
        assert_eq!(
            std::fs::read_link(h.install_root.join("current")).ok(),
            current,
            "{point}"
        );
        let tx = transaction_of_operation(&h.territory, if switch { "switch" } else { "install" });
        assert_eq!(tx["phase"], "rolled_back", "{point}: {tx}");
        let active = systemctl(&h.world, &["is-active", "landscape-router.service"]);
        assert_eq!(active.status.success(), switch, "{point}");
        if !switch {
            assert_eq!(
                std::fs::read(h.install_root.join("data/original-marker")).unwrap(),
                b"original"
            );
        }
        daemon.crash();
        let _daemon = h.spawn_daemon(&runtime);
        std::thread::sleep(std::time::Duration::from_secs(3));
        let after = h.command().args(["operation", "list"]).output().unwrap();
        assert_success(&after);
        let after: Vec<serde_json::Value> = serde_json::from_slice(&after.stdout).unwrap();
        assert_eq!(
            after, records,
            "terminal operation must not execute again: {point}"
        );
        assert_eq!(std::fs::read(h.state_path()).ok(), state);
    }
}

#[test]
fn failed_world_exports_diagnostics_and_still_cleans_its_temporary_directory() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let destination =
        std::env::temp_dir().join(format!("lkit-export-test-{}", uuid::Uuid::now_v7()));
    let old = std::env::var_os("LKIT_TEST_ARTIFACT_DIR");
    unsafe {
        std::env::set_var("LKIT_TEST_ARTIFACT_DIR", &destination);
    }
    let world = TestWorld::new("expected-diagnostic-failure");
    let root = world.path("");
    let result = std::panic::catch_unwind(|| {
        let _world = world;
        panic!("intentional fixture diagnostic probe");
    });
    unsafe {
        match old {
            Some(value) => std::env::set_var("LKIT_TEST_ARTIFACT_DIR", value),
            None => std::env::remove_var("LKIT_TEST_ARTIFACT_DIR"),
        }
    }
    assert!(result.is_err());
    assert!(
        !root.exists(),
        "artifact retention must not retain the live fixture world"
    );
    assert!(
        destination
            .join(root.file_name().unwrap())
            .join("snapshot.json")
            .is_file()
    );
    std::fs::remove_dir_all(destination).unwrap();
}
