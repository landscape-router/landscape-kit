use super::support::*;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

struct Observer(std::process::Child);
impl Drop for Observer {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn backup(h: &InstallHarness) -> PathBuf {
    assert_success(
        &h.command()
            .args(["backup", "create", "--test-runtime"])
            .arg(&h.runtime_config)
            .output()
            .unwrap(),
    );
    std::fs::read_dir(h.backups_dir())
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|e| e == "lkb"))
        .unwrap()
}
fn command(h: &InstallHarness, runtime: &Path, backup: &Path) -> std::process::Command {
    let mut command = h.command();
    command
        .args(["restore", "--file"])
        .arg(backup)
        .arg("--test-runtime")
        .arg(runtime);
    command
}
fn record(h: &InstallHarness) -> serde_json::Value {
    let records: Vec<serde_json::Value> = std::fs::read_dir(h.run_dir().join("operations"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|e| e == "json"))
        .map(|entry| serde_json::from_slice(&std::fs::read(entry.path()).unwrap()).unwrap())
        .filter(|record: &serde_json::Value| record["kind"] == "restore")
        .collect();
    assert_eq!(records.len(), 1);
    records.into_iter().next().unwrap()
}
fn transaction_path(h: &InstallHarness) -> PathBuf {
    h.transactions_dir().join(format!(
        "{}.json",
        record(h)["transaction_id"].as_str().unwrap()
    ))
}
fn transaction(h: &InstallHarness) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(transaction_path(h)).unwrap()).unwrap()
}
fn guarded_runtime(h: &InstallHarness) -> PathBuf {
    let runtime = h.daemon_runtime();
    let mut config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&runtime).unwrap()).unwrap();
    config["network_confirm_timeout_ms"] = 120_000.into();
    write_json(&runtime, &config);
    runtime
}
fn confirm_command(h: &InstallHarness, runtime: &Path) -> std::process::Command {
    let mut command = h.command();
    command
        .args(["network", "confirm", "--test-runtime"])
        .arg(runtime);
    command
}
fn wait(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(50);
    while !condition() {
        assert!(Instant::now() < deadline, "restore timed out");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn native_restore_commits_without_repository_downloads() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let h = InstallHarness::new("restore-native", "healthy", 30_000);
    assert_success(&h.run());
    let backup = backup(&h);
    let old_state = std::fs::read(h.state_path()).unwrap();
    std::fs::write(h.install_root.join("data/after-backup"), b"later data").unwrap();
    let runtime = guarded_runtime(&h);
    let _daemon = h.spawn_daemon(&runtime);
    let requests = h.repository.request_paths();
    assert_success(
        &command(&h, &runtime, &backup)
            .args(["--yes", "--non-interactive"])
            .output()
            .unwrap(),
    );
    assert_eq!(record(&h)["status"], "awaiting_confirmation");
    let tx = transaction(&h);
    assert_eq!(tx["schema_version"], 14);
    assert_eq!(tx["phase"], "awaiting_network_confirmation");
    assert!(tx["restore_confirmation"]["boot_id"].is_string());
    assert_eq!(std::fs::read(h.state_path()).unwrap(), old_state);
    assert!(!h.install_root.join("data/after-backup").exists());
    assert!(
        tx["restore_backup"]["path"]
            .as_str()
            .unwrap()
            .starts_with("transactions/")
    );
    let backup_count = std::fs::read_dir(h.backups_dir()).unwrap().count();
    let rejected = h
        .command()
        .args(["backup", "create", "--test-runtime"])
        .arg(&h.runtime_config)
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("lkit daemon"));
    assert_eq!(
        std::fs::read_dir(h.backups_dir()).unwrap().count(),
        backup_count
    );
    assert_eq!(transaction(&h), tx);
    assert_eq!(std::fs::read(h.state_path()).unwrap(), old_state);

    assert_success(&confirm_command(&h, &runtime).output().unwrap());
    wait(|| record(&h)["status"] == "succeeded");
    assert_eq!(transaction(&h)["phase"], "committed");
    let installed: serde_json::Value =
        serde_json::from_slice(&std::fs::read(h.state_path()).unwrap()).unwrap();
    assert_eq!(installed["last_transaction_id"], tx["transaction_id"]);
    assert_eq!(h.repository.request_paths(), requests);
}

#[test]
fn native_restore_rejects_backup_replacement_after_confirmation() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let h = InstallHarness::new("restore-replaced", "healthy", 30_000);
    assert_success(&h.run());
    let backup = backup(&h);
    let state = std::fs::read(h.state_path()).unwrap();
    let runtime = h.daemon_runtime();
    let _daemon = h.spawn_daemon(&runtime);
    let mut pty = Pty::open();
    let mut cmd = command(&h, &runtime, &backup);
    attach_pty(&mut cmd, &pty);
    let mut observer = Observer(cmd.spawn().unwrap());
    pty.read_until("yes", Duration::from_secs(10));
    std::fs::write(&backup, b"replaced backup").unwrap();
    pty.master.write_all(b"yes\nyes\n").unwrap();
    assert!(!observer.0.wait().unwrap().success());
    let result = record(&h);
    assert_eq!(result["status"], "failed");
    assert!(result["transaction_id"].is_null());
    assert_eq!(std::fs::read(h.state_path()).unwrap(), state);
}

#[test]
fn native_restore_recovers_after_activation_and_pending_result_loss() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for point in ["restore-activated", "before-result"] {
        let (h, _territory) =
            InstallHarness::new("restore-crash", "healthy", 30_000).with_tmpfs_territory();
        assert_success(&h.run());
        let backup = backup(&h);
        // Same-version recovery must restore the exact original data, not re-import the target.
        std::fs::write(h.install_root.join("data/original-marker"), b"original").unwrap();
        let runtime = guarded_runtime(&h);
        let gate = h.run_dir().join(format!("test-repair-{point}.gate"));
        std::fs::write(&gate, b"pause").unwrap();
        let mut daemon = h.spawn_daemon_at_checkpoint(&runtime, point);
        let observer = Observer(
            command(&h, &runtime, &backup)
                .args(["--yes", "--non-interactive"])
                .spawn()
                .unwrap(),
        );
        wait(|| {
            h.run_dir()
                .join(format!("test-repair-{point}.reached"))
                .exists()
        });
        let active = record(&h);
        let deadline = transaction(&h)["restore_confirmation"]["confirmation_deadline"].clone();
        assert!(
            !h.command()
                .args(["operation", "cancel", active["id"].as_str().unwrap()])
                .output()
                .unwrap()
                .status
                .success()
        );
        drop(observer);
        daemon.crash();
        std::fs::remove_file(gate).unwrap();
        let _daemon = h.spawn_daemon(&runtime);
        let expected = if point == "before-result" {
            "awaiting_confirmation"
        } else {
            "failed"
        };
        wait(|| record(&h)["status"] == expected);
        let result = record(&h);
        assert_eq!(result["transaction_id"], active["transaction_id"]);
        if point == "restore-activated" {
            assert_eq!(result["exit_code"], 5);
            assert_eq!(
                std::fs::read(h.install_root.join("data/original-marker")).unwrap(),
                b"original"
            );
        } else {
            // The interrupted executor never saved a result. Recovery discovers a
            // pending transaction, so the receipt has no terminal exit code yet.
            assert!(result["exit_code"].is_null());
            assert_eq!(transaction(&h)["phase"], "awaiting_network_confirmation");
            assert_eq!(
                transaction(&h)["restore_confirmation"]["confirmation_deadline"],
                deadline
            );
            assert_success(&confirm_command(&h, &runtime).output().unwrap());
            wait(|| record(&h)["status"] == "succeeded");
            assert_eq!(record(&h)["exit_code"], 0);
        }
    }
}

#[test]
fn unconfirmed_restore_timeout_restores_original_data_and_committed_state() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (h, _territory) =
        InstallHarness::new("restore-timeout", "healthy", 30_000).with_tmpfs_territory();
    assert_success(&h.run());
    let backup = backup(&h);
    std::fs::write(h.install_root.join("data/original-marker"), b"original").unwrap();
    let runtime = guarded_runtime(&h);
    let _daemon = h.spawn_daemon(&runtime);
    assert_success(
        &command(&h, &runtime, &backup)
            .args(["--yes", "--non-interactive"])
            .output()
            .unwrap(),
    );
    assert_eq!(record(&h)["status"], "awaiting_confirmation");
    assert!(!h.install_root.join("data/original-marker").exists());
    let mut tx = transaction(&h);
    tx["restore_confirmation"]["confirmation_deadline"] = "2000-01-01T00:00:00Z".into();
    let path = transaction_path(&h);
    let temporary = path.with_extension("expired.tmp");
    write_json(&temporary, &tx);
    std::fs::rename(temporary, path).unwrap();
    wait(|| record(&h)["status"] == "failed");
    assert_eq!(record(&h)["exit_code"], 5);
    assert_eq!(transaction(&h)["phase"], "rolled_back");
    assert_eq!(
        std::fs::read(h.install_root.join("data/original-marker")).unwrap(),
        b"original"
    );
    assert_success(
        &h.command()
            .args(["backup", "create", "--test-runtime"])
            .arg(&h.runtime_config)
            .output()
            .unwrap(),
    );
}

#[test]
fn accepted_restore_confirmation_finishes_after_daemon_crash() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for checkpoint in ["restore-finalizing", "restore-state-written"] {
        let h = InstallHarness::new("restore-confirm-crash", "healthy", 30_000);
        assert_success(&h.run());
        let backup = backup(&h);
        std::fs::write(h.install_root.join("data/original-marker"), b"original").unwrap();
        let runtime = guarded_runtime(&h);
        let gate = h.run_dir().join(format!("test-repair-{checkpoint}.gate"));
        std::fs::write(&gate, b"pause").unwrap();
        let mut daemon = h.spawn_daemon_at_checkpoint(&runtime, checkpoint);
        assert_success(
            &command(&h, &runtime, &backup)
                .args(["--yes", "--non-interactive"])
                .output()
                .unwrap(),
        );
        let before = record(&h);
        let backups_before = std::fs::read_dir(h.backups_dir()).unwrap().count();
        let observer = Observer(confirm_command(&h, &runtime).spawn().unwrap());
        wait(|| {
            h.run_dir()
                .join(format!("test-repair-{checkpoint}.reached"))
                .exists()
        });
        assert_eq!(transaction(&h)["phase"], "finalizing");
        drop(observer);
        daemon.crash();
        std::fs::remove_file(gate).unwrap();
        // Accepted confirmation wins over a deadline crossed while daemon was down.
        let mut tx = transaction(&h);
        tx["restore_confirmation"]["confirmation_deadline"] = "2000-01-01T00:00:00Z".into();
        write_json(&transaction_path(&h), &tx);
        let _daemon = h.spawn_daemon(&runtime);
        wait(|| record(&h)["status"] == "succeeded");
        assert_eq!(record(&h)["transaction_id"], before["transaction_id"]);
        assert_eq!(record(&h)["exit_code"], 0);
        assert_eq!(transaction(&h)["phase"], "committed");
        assert_eq!(
            std::fs::read_dir(h.backups_dir()).unwrap().count(),
            backups_before
        );
        assert!(!h.install_root.join("data/original-marker").exists());
        let installed: serde_json::Value =
            serde_json::from_slice(&std::fs::read(h.state_path()).unwrap()).unwrap();
        assert_eq!(installed["last_transaction_id"], before["transaction_id"]);
    }
}
