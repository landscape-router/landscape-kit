use std::io::Write;

use super::support::*;

/// Hold the terminal and repository until the command finishes.
fn update_with_confirmation(harness: &InstallHarness, args: &[&str]) -> std::process::ExitStatus {
    let next = RepositoryServer::start(repository_files_for("2.0.0"));
    let runtime = harness.daemon_runtime();
    let _daemon = harness.spawn_daemon(&runtime);
    let mut pty = Pty::open();
    pty.master.write_all(b"yes\n").unwrap();
    let mut command = harness.command();
    command
        .arg("update")
        .args(args)
        .arg("--repository")
        .arg(&next.base_url)
        .arg("--test-runtime")
        .arg(&runtime);
    attach_pty(&mut command, &pty);
    let status = command.spawn().unwrap().wait().unwrap();
    if status.success() {
        let operation = records(harness).pop().unwrap();
        assert_eq!(operation["kind"], "switch");
        assert_eq!(operation["schema_version"], 3);
        assert_eq!(operation["status"], "succeeded");
        assert_eq!(operation["switch"]["target"], "2.0.0");
        assert_eq!(operation["switch"]["repository"]["http"], next.base_url);
        let tx: serde_json::Value = serde_json::from_slice(
            &std::fs::read(harness.transactions_dir().join(format!(
                "{}.json",
                operation["transaction_id"].as_str().unwrap()
            )))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(tx["schema_version"], 8);
        assert_eq!(tx["phase"], "committed");
    }
    status
}

/// UP-02:确认后执行 latest 目标的实际升级,新版本进入 active。
#[test]
fn update_upgrades_to_latest_after_confirmation() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("update-latest", "healthy", 10_000);
    assert_success(&harness.run());

    let status = update_with_confirmation(&harness, &[]);
    assert!(
        status.success(),
        "update failed with {status:?}\nservice log:\n{}",
        harness.service_log()
    );
    let state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(harness.state_path()).unwrap()).unwrap();
    assert_eq!(state["active_version"], "2.0.0");
    let active = systemctl(&harness.world, &["is-active", "landscape-router.service"]);
    assert_eq!(String::from_utf8_lossy(&active.stdout).trim(), "active");
}

/// UP-03:确认后执行固定版本目标的实际升级。
#[test]
fn update_upgrades_to_a_pinned_version_after_confirmation() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("update-pinned", "healthy", 10_000);
    assert_success(&harness.run());

    let status = update_with_confirmation(&harness, &["--version", "2.0.0"]);
    assert!(
        status.success(),
        "update failed with {status:?}\nservice log:\n{}",
        harness.service_log()
    );
    let state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(harness.state_path()).unwrap()).unwrap();
    assert_eq!(state["active_version"], "2.0.0");
}

/// update 确认被拒时返回退出码 1,不创建事务、版本不变。
#[test]
fn update_refused_confirmation_leaves_the_installation_untouched() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("update-refused", "healthy", 10_000);
    assert_success(&harness.run());
    let transactions_before = std::fs::read_dir(harness.transactions_dir())
        .unwrap()
        .count();

    let next = RepositoryServer::start(repository_files_for("2.0.0"));
    let mut pty = Pty::open();
    pty.master.write_all(b"no\n").unwrap();
    let mut command = harness.command();
    command
        .arg("update")
        .arg("--repository")
        .arg(&next.base_url)
        .args(["--test-runtime"])
        .arg(&harness.runtime_config);
    attach_pty(&mut command, &pty);
    let output = command.output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "a refused update must exit 1\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(harness.state_path()).unwrap()).unwrap();
    assert_eq!(state["active_version"], VERSION);
    assert_eq!(
        std::fs::read_dir(harness.transactions_dir())
            .unwrap()
            .count(),
        transactions_before,
        "a refused update must not create a transaction"
    );
    assert!(records(&harness).is_empty());
    let active = systemctl(&harness.world, &["is-active", "landscape-router.service"]);
    assert_eq!(String::from_utf8_lossy(&active.stdout).trim(), "active");
}

fn records(h: &InstallHarness) -> Vec<serde_json::Value> {
    let output = h.command().args(["operation", "list"]).output().unwrap();
    assert_success(&output);
    serde_json::from_slice(&output.stdout).unwrap()
}
fn wait_until(mut ready: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(45);
    while !ready() {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for update"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}
struct Frontend(std::process::Child);
impl Drop for Frontend {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn start(
    h: &InstallHarness,
    runtime: &std::path::Path,
    pty: &Pty,
    repository: Option<&str>,
) -> Frontend {
    let mut command = h.command();
    command.args(["update", "--test-runtime"]).arg(runtime);
    if let Some(repository) = repository {
        command.arg("--repository").arg(repository);
    }
    attach_pty(&mut command, pty);
    Frontend(command.spawn().unwrap())
}

#[test]
fn confirmation_pins_latest_and_default_source_across_configuration_changes() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let h = InstallHarness::new("update-pinned-confirmation", "healthy", 30_000);
    assert_success(&h.run());
    let next = RepositoryServer::start(repository_files_for("2.0.0"));
    let changed = RepositoryServer::start(repository_files_for("3.0.0"));
    let config = h.territory.join("config.toml");
    std::fs::write(
        &config,
        format!(
            "[repository]\nkind = \"http\"\nlocation = \"{}\"\n",
            next.base_url
        ),
    )
    .unwrap();
    let runtime = h.daemon_runtime();
    let _daemon = h.spawn_daemon(&runtime);
    let mut pty = Pty::open();
    pty.master.write_all(b"\n").unwrap();
    let mut frontend = start(&h, &runtime, &pty, None);
    let prompt = pty.read_until("yes", std::time::Duration::from_secs(30));
    assert!(prompt.contains("2.0.0"), "{prompt}");
    assert!(prompt.contains(&next.base_url), "{prompt}");
    assert!(records(&h).is_empty());
    next.replace_file(
        "/channels/stable.json",
        br#"{"protocol_version":1,"version":"3.0.0"}"#.to_vec(),
    );
    std::fs::write(
        &config,
        format!(
            "[repository]\nkind = \"http\"\nlocation = \"{}\"\n",
            changed.base_url
        ),
    )
    .unwrap();
    pty.master.write_all(b"yes\n").unwrap();
    assert!(frontend.0.wait().unwrap().success());
    let operation = records(&h).pop().unwrap();
    assert_eq!(operation["status"], "succeeded");
    assert_eq!(operation["switch"]["target"], "2.0.0");
    assert_eq!(operation["switch"]["repository"]["http"], next.base_url);
    assert!(changed.request_paths().is_empty());
    assert_eq!(
        next.request_paths()
            .iter()
            .filter(|p| p.as_str() == "/channels/stable.json")
            .count(),
        1
    );
}

#[test]
fn update_rejects_installation_changes_during_confirmation() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let h = InstallHarness::new("update-state-changed", "healthy", 30_000);
    assert_success(&h.run());
    let next = RepositoryServer::start(repository_files_for("2.0.0"));
    let runtime = h.daemon_runtime();
    let _daemon = h.spawn_daemon(&runtime);
    let mut pty = Pty::open();
    let mut frontend = start(&h, &runtime, &pty, Some(&next.base_url));
    let prompt = pty.read_until("yes", std::time::Duration::from_secs(30));
    assert!(prompt.contains("2.0.0"), "{prompt}");
    let mut state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(h.state_path()).unwrap()).unwrap();
    state["service"]["verified"] = false.into();
    std::fs::write(h.state_path(), serde_json::to_vec(&state).unwrap()).unwrap();
    pty.master.write_all(b"yes\n").unwrap();
    assert!(!frontend.0.wait().unwrap().success());
    let operation = records(&h).pop().unwrap();
    assert_eq!(operation["status"], "failed");
    assert!(operation["transaction_id"].is_null());
    assert!(
        operation["message"]
            .as_str()
            .unwrap()
            .contains("state changed")
    );
    assert!(
        !next
            .request_paths()
            .iter()
            .any(|p| p.ends_with(".zst") || p.ends_with("static.zip"))
    );
}

#[test]
fn disconnected_update_download_can_be_cancelled_and_observed_after_restart() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let h = InstallHarness::new("update-disconnect", "healthy", 30_000);
    assert_success(&h.run());
    let original_state = std::fs::read(h.state_path()).unwrap();
    let gate = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let next = RepositoryServer::start_gated(
        repository_files_for("2.0.0"),
        Some((".zst".into(), gate.clone())),
    );
    let runtime = h.daemon_runtime();
    let daemon = h.spawn_daemon(&runtime);
    let mut pty = Pty::open();
    pty.master.write_all(b"yes\n").unwrap();
    let mut frontend = start(&h, &runtime, &pty, Some(&next.base_url));
    wait_until(|| next.request_paths().iter().any(|p| p.ends_with(".zst")));
    unsafe {
        libc::kill(frontend.0.id() as i32, libc::SIGINT);
    }
    assert_eq!(frontend.0.wait().unwrap().code(), Some(130));
    let operation = records(&h).pop().unwrap();
    assert_eq!(operation["status"], "running");
    assert_eq!(operation["cancel_requested"], false);
    let id = operation["id"].as_str().unwrap();
    assert_success(
        &h.command()
            .args(["operation", "status", id])
            .output()
            .unwrap(),
    );
    assert_success(
        &h.command()
            .args(["operation", "cancel", id])
            .output()
            .unwrap(),
    );
    wait_until(|| records(&h)[0]["status"] == "cancelled");
    let result = records(&h).pop().unwrap();
    assert_eq!(result["exit_code"], 130);
    assert_eq!(result["cancel_cleaned"], true);
    assert_eq!(std::fs::read(h.state_path()).unwrap(), original_state);
    let requests = next.request_paths();
    gate.store(true, std::sync::atomic::Ordering::SeqCst);
    drop(daemon);
    let _daemon = h.spawn_daemon(&runtime);
    std::thread::sleep(std::time::Duration::from_secs(3));
    assert_eq!(records(&h), vec![result]);
    assert_eq!(next.request_paths(), requests);
}

#[test]
fn update_operations_recover_after_frontend_and_daemon_exit() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for point in ["switch-activated", "before-result"] {
        let h = InstallHarness::new("update-crash", "healthy", 30_000);
        assert_success(&h.run());
        let next = RepositoryServer::start(repository_files_for("2.0.0"));
        let runtime = h.daemon_runtime();
        let gate = h.run_dir().join(format!("test-repair-{point}.gate"));
        std::fs::write(&gate, b"pause").unwrap();
        let mut daemon = h.spawn_daemon_at_checkpoint(&runtime, point);
        let mut pty = Pty::open();
        pty.master.write_all(b"yes\n").unwrap();
        let frontend = start(&h, &runtime, &pty, Some(&next.base_url));
        wait_until(|| {
            h.run_dir()
                .join(format!("test-repair-{point}.reached"))
                .exists()
        });
        let operation = records(&h).pop().unwrap();
        assert_eq!(operation["status"], "running");
        assert!(
            !h.command()
                .args(["operation", "cancel", operation["id"].as_str().unwrap()])
                .output()
                .unwrap()
                .status
                .success()
        );
        let requests = next.request_paths();
        drop(frontend);
        daemon.crash();
        std::fs::remove_file(gate).unwrap();
        let _daemon = h.spawn_daemon(&runtime);
        let committed = point == "before-result";
        wait_until(|| records(&h)[0]["status"] == if committed { "succeeded" } else { "failed" });
        let result = records(&h).pop().unwrap();
        assert_eq!(result["transaction_id"], operation["transaction_id"]);
        assert_eq!(result["exit_code"], if committed { 0 } else { 5 });
        let state: serde_json::Value =
            serde_json::from_slice(&std::fs::read(h.state_path()).unwrap()).unwrap();
        assert_eq!(
            state["active_version"],
            if committed { "2.0.0" } else { VERSION }
        );
        assert_eq!(next.request_paths(), requests);
    }
}
