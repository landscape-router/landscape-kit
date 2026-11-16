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
    let paths: Vec<_> = std::fs::read_dir(h.run_dir().join("operations"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|e| e == "json"))
        .collect();
    assert_eq!(paths.len(), 1);
    serde_json::from_slice(&std::fs::read(paths[0].path()).unwrap()).unwrap()
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
    let runtime = h.daemon_runtime();
    let _daemon = h.spawn_daemon(&runtime);
    let requests = h.repository.request_paths();
    assert_success(
        &command(&h, &runtime, &backup)
            .args(["--yes", "--non-interactive"])
            .output()
            .unwrap(),
    );
    let record = record(&h);
    assert_eq!(record["kind"], "restore");
    assert_eq!(record["status"], "succeeded");
    let tx: serde_json::Value = serde_json::from_slice(
        &std::fs::read(h.transactions_dir().join(format!(
            "{}.json",
            record["transaction_id"].as_str().unwrap()
        )))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(tx["schema_version"], 9);
    assert_eq!(tx["phase"], "committed");
    assert!(
        tx["restore_backup"]["path"]
            .as_str()
            .unwrap()
            .starts_with("transactions/")
    );
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
fn native_restore_recovers_after_activation_and_committed_result_loss() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for point in ["restore-activated", "before-result"] {
        let h = InstallHarness::new("restore-crash", "healthy", 30_000);
        assert_success(&h.run());
        let backup = backup(&h);
        // Same-version recovery must restore the exact original data, not re-import the target.
        std::fs::write(h.install_root.join("data/original-marker"), b"original").unwrap();
        let runtime = h.daemon_runtime();
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
            "succeeded"
        } else {
            "failed"
        };
        wait(|| record(&h)["status"] == expected);
        let result = record(&h);
        assert_eq!(
            result["exit_code"],
            if point == "before-result" { 0 } else { 5 }
        );
        assert_eq!(result["transaction_id"], active["transaction_id"]);
        if point == "restore-activated" {
            assert_eq!(
                std::fs::read(h.install_root.join("data/original-marker")).unwrap(),
                b"original"
            );
        }
    }
}
