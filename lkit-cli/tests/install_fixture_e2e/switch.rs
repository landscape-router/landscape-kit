use std::time::Duration;

use super::support::*;

/// SW-03:切换目标等于当前 active version 时不创建事务——existing 路径直接
/// 验证当前安装并返回 0(与 `lkit update` 的"已是最新"一致)。
#[test]
fn switch_to_the_active_version_verifies_without_a_transaction() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("switch-already-active", "healthy", 10_000);
    assert_success(&harness.run());

    let runtime = harness.daemon_runtime();
    let _daemon = harness.spawn_daemon(&runtime);
    let output = harness
        .command()
        .args(["switch", "--version", VERSION, "--repository"])
        .arg(&harness.repository.base_url)
        .args(["--test-runtime"])
        .arg(&runtime)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "switching to the active version must verify the current installation\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let operation = records(&harness).pop().unwrap();
    assert_eq!(operation["status"], "succeeded");
    assert!(operation["transaction_id"].is_null());
    let has_switch_transaction = std::fs::read_dir(harness.transactions_dir())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("json"))
        .any(|entry| {
            serde_json::from_slice::<serde_json::Value>(&std::fs::read(entry.path()).unwrap())
                .unwrap()["operation"]
                == "switch"
        });
    assert!(
        !has_switch_transaction,
        "switching to the active version must not create a transaction"
    );
    let active = systemctl(&harness.world, &["is-active", "landscape-router.service"]);
    assert_eq!(
        String::from_utf8_lossy(&active.stdout).trim(),
        "active",
        "the service must keep running"
    );
}

/// SW-10:服务运行中传入 `--allow-no-backup` → 忽略该标志、给出警告并照常创建
/// `.lkb`,切换成功,服务保持运行。
#[test]
fn switch_ignores_allow_no_backup_while_the_service_runs() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("switch-allow-no-backup", "healthy", 10_000);
    assert_success(&harness.run());
    let backups_before = std::fs::read_dir(harness.backups_dir()).unwrap().count();
    let next = RepositoryServer::start(repository_files_for("2.0.0"));

    let runtime = harness.daemon_runtime();
    let _daemon = harness.spawn_daemon(&runtime);
    let output = harness
        .command()
        .args([
            "switch",
            "--version",
            "2.0.0",
            "--repository",
            &next.base_url,
            "--allow-no-backup",
            "--test-runtime",
        ])
        .arg(&runtime)
        .output()
        .unwrap();
    assert_success(&output);
    let operation = records(&harness).pop().unwrap();
    assert_eq!(operation["status"], "succeeded");
    assert_eq!(operation["resolved_version"], "2.0.0");
    assert!(
        operation["message"]
            .as_str()
            .unwrap()
            .contains("--allow-no-backup ignored")
    );
    assert_eq!(transaction(&harness, &operation)["schema_version"], 8);
    assert_eq!(transaction(&harness, &operation)["no_backup"], false);

    let state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(harness.state_path()).unwrap()).unwrap();
    assert_eq!(state["active_version"], "2.0.0");
    let backups_after = std::fs::read_dir(harness.backups_dir()).unwrap().count();
    assert!(
        backups_after > backups_before,
        "a running service must still receive a protection .lkb despite --allow-no-backup"
    );
    let active = systemctl(&harness.world, &["is-active", "landscape-router.service"]);
    assert_eq!(
        String::from_utf8_lossy(&active.stdout).trim(),
        "active",
        "the service must be running on the new version"
    );
}

fn wait_until(mut ready: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(45);
    while !ready() {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for switch"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}
fn records(h: &InstallHarness) -> Vec<serde_json::Value> {
    let output = h.command().args(["operation", "list"]).output().unwrap();
    assert_success(&output);
    serde_json::from_slice(&output.stdout).unwrap()
}
fn transaction(h: &InstallHarness, op: &serde_json::Value) -> serde_json::Value {
    serde_json::from_slice(
        &std::fs::read(
            h.transactions_dir()
                .join(format!("{}.json", op["transaction_id"].as_str().unwrap())),
        )
        .unwrap(),
    )
    .unwrap()
}
struct Frontend(std::process::Child);
impl Drop for Frontend {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn start_switch(
    h: &InstallHarness,
    runtime: &std::path::Path,
    repository: &str,
    no_backup: bool,
) -> Frontend {
    let mut command = h.command();
    command
        .args([
            "switch",
            "--version",
            "2.0.0",
            "--repository",
            repository,
            "--test-runtime",
        ])
        .arg(runtime);
    if no_backup {
        command.arg("--allow-no-backup");
    }
    Frontend(
        command
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    )
}
fn gate(h: &InstallHarness, name: &str) -> std::path::PathBuf {
    let gate = h.run_dir().join(format!("test-repair-{name}.gate"));
    std::fs::write(&gate, b"pause").unwrap();
    gate
}
fn checkpoint(h: &InstallHarness, name: &str) {
    wait_until(|| {
        h.run_dir()
            .join(format!("test-repair-{name}.reached"))
            .exists()
    });
}
fn snapshot_state(h: &InstallHarness) -> Vec<u8> {
    std::fs::read(h.state_path()).unwrap()
}
fn service_mutations(h: &InstallHarness) -> Vec<Vec<String>> {
    std::fs::read_to_string(h.world.path("systemctl-calls.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Vec<String>>(line).unwrap())
        .filter(|args| {
            args.iter().any(|a| {
                matches!(
                    a.as_str(),
                    "start" | "stop" | "restart" | "enable" | "disable" | "daemon-reload" | "link"
                )
            })
        })
        .collect()
}

#[test]
fn daemon_switch_recovers_activation_and_commit_crashes_with_or_without_backup() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for point in ["switch-activated", "switch-state-written", "before-result"] {
        for no_backup in [false, true] {
            let h = InstallHarness::new("switch-crash", "healthy", 30_000);
            assert_success(&h.run());
            if no_backup {
                assert_success(&systemctl(&h.world, &["stop", "landscape-router.service"]));
            }
            let old_state = snapshot_state(&h);
            let old_current = std::fs::read_link(h.install_root.join("current")).unwrap();
            let old_archive = std::fs::read(h.install_root.join("current/static.zip")).unwrap();
            let next = RepositoryServer::start(repository_files_for("2.0.0"));
            let runtime = h.daemon_runtime();
            let gate = gate(&h, point);
            let mut daemon = h.spawn_daemon_at_checkpoint(&runtime, point);
            let frontend = start_switch(&h, &runtime, &next.base_url, no_backup);
            checkpoint(&h, point);
            let operation = records(&h).pop().unwrap();
            assert_eq!(operation["status"], "running");
            assert_eq!(operation["cancellable"], false);
            assert!(
                !h.command()
                    .args(["operation", "cancel", operation["id"].as_str().unwrap()])
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
            let committed = point == "before-result";
            assert_eq!(
                transaction(&h, &operation)["phase"],
                if committed {
                    "committed"
                } else if point == "switch-activated" {
                    "activating"
                } else {
                    "verifying"
                }
            );
            let target_state = snapshot_state(&h);
            let calls = service_mutations(&h);
            let requests = next.request_paths();
            drop(frontend);
            daemon.crash();
            std::fs::remove_file(gate).unwrap();
            let daemon = h.spawn_daemon(&runtime);
            wait_until(|| {
                records(&h)[0]["status"] == if committed { "succeeded" } else { "failed" }
            });
            let result = records(&h).pop().unwrap();
            assert_eq!(result["exit_code"], if committed { 0 } else { 5 });
            assert_eq!(result["transaction_id"], operation["transaction_id"]);
            if committed {
                assert_eq!(snapshot_state(&h), target_state);
                assert_eq!(
                    service_mutations(&h),
                    calls,
                    "committed switch must not restart again"
                );
            } else {
                assert_eq!(snapshot_state(&h), old_state);
                assert_eq!(
                    std::fs::read_link(h.install_root.join("current")).unwrap(),
                    old_current
                );
                assert_eq!(
                    std::fs::read(h.install_root.join("current/static.zip")).unwrap(),
                    old_archive
                );
                assert_eq!(transaction(&h, &result)["phase"], "rolled_back");
                let active = systemctl(&h.world, &["is-active", "landscape-router.service"]);
                assert_eq!(active.status.success(), !no_backup);
            }
            drop(daemon);
            let _daemon = h.spawn_daemon(&runtime);
            std::thread::sleep(Duration::from_secs(3));
            assert_eq!(records(&h), vec![result]);
            assert_eq!(next.request_paths(), requests);
        }
    }
}

#[test]
fn disconnected_switch_download_can_be_cancelled_without_changing_installation() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let h = InstallHarness::new("switch-cancel", "healthy", 30_000);
    assert_success(&h.run());
    let old_state = snapshot_state(&h);
    let calls = service_mutations(&h);
    let download_gate = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let next = RepositoryServer::start_gated(
        repository_files_for("2.0.0"),
        Some((".zst".into(), download_gate.clone())),
    );
    let runtime = h.daemon_runtime();
    let _daemon = h.spawn_daemon(&runtime);
    let frontend = start_switch(&h, &runtime, &next.base_url, false);
    wait_until(|| next.request_paths().iter().any(|p| p.ends_with(".zst")));
    drop(frontend);
    let operation = records(&h).pop().unwrap();
    assert_eq!(operation["status"], "running");
    assert_eq!(operation["stage"], "downloading");
    assert_success(
        &h.command()
            .args(["operation", "cancel", operation["id"].as_str().unwrap()])
            .output()
            .unwrap(),
    );
    wait_until(|| records(&h)[0]["status"] == "cancelled");
    download_gate.store(true, std::sync::atomic::Ordering::SeqCst);
    let result = records(&h).pop().unwrap();
    assert_eq!(result["cancel_cleaned"], true);
    assert_eq!(result["exit_code"], 130);
    assert_eq!(snapshot_state(&h), old_state);
    assert_eq!(service_mutations(&h), calls);
    assert!(!h.install_root.join("releases/2.0.0").exists());
    assert!(
        !h.transactions_dir()
            .join(result["transaction_id"].as_str().unwrap())
            .exists()
    );
}

#[test]
fn switch_rollback_failure_blocks_next_request_until_backup_is_repaired() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let h = InstallHarness::new("switch-rollback-fault", "healthy", 30_000);
    assert_success(&h.run());
    let next = RepositoryServer::start(repository_files_for("2.0.0"));
    let runtime = h.daemon_runtime();
    let gate = gate(&h, "switch-activated");
    let mut daemon = h.spawn_daemon_at_checkpoint(&runtime, "switch-activated");
    let frontend = start_switch(&h, &runtime, &next.base_url, false);
    checkpoint(&h, "switch-activated");
    let operation = records(&h).pop().unwrap();
    let tx = transaction(&h, &operation);
    let backup = h.territory.join(tx["backup"]["path"].as_str().unwrap());
    let bytes = std::fs::read(&backup).unwrap();
    drop(frontend);
    daemon.crash();
    std::fs::remove_file(gate).unwrap();
    std::fs::write(&backup, b"damaged backup").unwrap();
    let mut daemon = h.spawn_daemon(&runtime);
    wait_until(|| records(&h)[0]["status"] == "recovering");
    assert_eq!(transaction(&h, &operation)["phase"], "rolling_back");
    let queued = start_switch(&h, &runtime, &next.base_url, false);
    wait_until(|| records(&h).len() == 2);
    let requests = next.request_paths();
    std::thread::sleep(Duration::from_secs(4));
    assert_eq!(records(&h)[1]["status"], "queued");
    assert_eq!(next.request_paths(), requests);
    assert_success(
        &h.command()
            .args([
                "operation",
                "cancel",
                records(&h)[1]["id"].as_str().unwrap(),
            ])
            .output()
            .unwrap(),
    );
    drop(queued);
    daemon.crash();
    std::fs::write(&backup, bytes).unwrap();
    let _daemon = h.spawn_daemon(&runtime);
    wait_until(|| records(&h)[0]["status"] == "failed");
    assert_eq!(records(&h)[0]["exit_code"], 5);
    assert_eq!(transaction(&h, &operation)["phase"], "rolled_back");
    let state: serde_json::Value = serde_json::from_slice(&snapshot_state(&h)).unwrap();
    assert_eq!(state["active_version"], VERSION);
}

#[test]
fn queued_switch_rejects_changed_installation_without_downloading() {
    if !e2e_enabled() {
        return;
    }
    use std::os::fd::AsRawFd;
    let _guard = E2E_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let h = InstallHarness::new("switch-stale-state", "healthy", 30_000);
    assert_success(&h.run());
    let original = snapshot_state(&h);
    let next = RepositoryServer::start(repository_files_for("2.0.0"));
    let runtime = h.daemon_runtime();
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .open(h.run_dir().join("install.lock"))
        .unwrap();
    assert_eq!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    let _daemon = h.spawn_daemon(&runtime);
    let _frontend = start_switch(&h, &runtime, &next.base_url, false);
    wait_until(|| !records(&h).is_empty());
    assert_eq!(records(&h)[0]["status"], "queued");
    let mut changed: serde_json::Value = serde_json::from_slice(&original).unwrap();
    changed["service"]["verified"] = false.into();
    std::fs::write(h.state_path(), serde_json::to_vec(&changed).unwrap()).unwrap();
    drop(lock);
    wait_until(|| records(&h)[0]["status"] == "failed");
    let record = records(&h).pop().unwrap();
    assert!(
        record["message"]
            .as_str()
            .unwrap()
            .contains("state changed")
    );
    assert!(record["transaction_id"].is_null());
    assert!(next.request_paths().is_empty());
    assert!(!h.install_root.join("releases/2.0.0").exists());
}
