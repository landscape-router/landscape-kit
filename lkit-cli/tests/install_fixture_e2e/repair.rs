use super::support::{E2E_LOCK, InstallHarness, RepositoryServer, e2e_enabled, repository_files};
use std::{
    os::{fd::AsRawFd, unix::fs::PermissionsExt},
    process::{Child, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

fn wait_until(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(40);
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for repair");
        std::thread::sleep(Duration::from_millis(50));
    }
}
struct Frontend(Child);
impl Drop for Frontend {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
struct Gate(Arc<AtomicBool>);
impl Drop for Gate {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}
fn records(h: &InstallHarness) -> Vec<serde_json::Value> {
    let output = h.command().args(["operation", "list"]).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
fn assert_success(output: &std::process::Output) {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
fn alter_asset(h: &InstallHarness, target: &str) -> (std::path::PathBuf, Vec<u8>) {
    let path = h.install_root.join(if target == "static" {
        "current/static/index.html"
    } else {
        "current/landscape-webserver"
    });
    let original = std::fs::read(&path).unwrap();
    let temp = path.with_extension("tampered");
    std::fs::write(&temp, b"tampered asset").unwrap();
    std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::rename(temp, &path).unwrap();
    (path, original)
}

#[test]
fn repairs_assets_directly_in_daemon_and_keeps_terminal_results_across_restart() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for target in ["static", "binary"] {
        let h = InstallHarness::new("repair-assets", "healthy", 30_000);
        assert_success(&h.run());
        let (asset, original) = alter_asset(&h, target);
        let runtime = h.daemon_runtime();
        let mut config: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&runtime).unwrap()).unwrap();
        let fixture = config["systemd"]["systemctl"].as_str().unwrap();
        let parents = h.world.path("service-parents");
        let wrapper = h.world.path("systemctl-wrapper");
        let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
        std::fs::write(
            &wrapper,
            format!(
                "#!/bin/sh\necho \"$PPID\" >> {}\nexec {} \"$@\"\n",
                quote(parents.to_str().unwrap()),
                quote(fixture)
            ),
        )
        .unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
        config["systemd"]["systemctl"] = wrapper.to_str().unwrap().into();
        std::fs::write(&runtime, serde_json::to_vec(&config).unwrap()).unwrap();
        let daemon = h.spawn_daemon(&runtime);
        let output = h
            .command()
            .args([
                "repair",
                target,
                "--repository",
                &h.repository.base_url,
                "--test-runtime",
            ])
            .arg(&runtime)
            .output()
            .unwrap();
        assert_success(&output);
        assert_eq!(std::fs::read(&asset).unwrap(), original);
        let operation = records(&h).pop().unwrap();
        assert_eq!(operation["status"], "succeeded");
        assert_eq!(operation["exit_code"], 0);
        let id = operation["transaction_id"].as_str().unwrap();
        let tx: serde_json::Value = serde_json::from_slice(
            &std::fs::read(h.transactions_dir().join(format!("{id}.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(tx["phase"], "committed");
        assert_eq!(tx["schema_version"], 7);
        let calls = std::fs::read_to_string(&parents).unwrap_or_default();
        if target == "binary" {
            assert!(!calls.is_empty());
        }
        for parent in calls.lines() {
            assert_eq!(
                parent,
                daemon.id().to_string(),
                "service tools must be children of daemon"
            );
        }
        let requests = h.repository.request_paths();
        drop(daemon);
        let _daemon = h.spawn_daemon(&runtime);
        std::thread::sleep(Duration::from_secs(3));
        assert_eq!(records(&h), vec![operation]);
        assert_eq!(
            h.repository.request_paths(),
            requests,
            "terminal repair must not download again"
        );
    }
}

#[test]
fn disconnected_asset_downloads_can_be_cancelled_or_recovered_without_replay() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for target in ["static", "binary"] {
        for crash in [false, true] {
            let h = InstallHarness::new("repair-download", "healthy", 30_000);
            assert_success(&h.run());
            let (asset, _) = alter_asset(&h, target);
            let gate = Gate(Arc::new(AtomicBool::new(false)));
            let suffix = if target == "static" {
                "static.zip"
            } else {
                ".zst"
            };
            let server = RepositoryServer::start_gated(
                repository_files(),
                Some((suffix.into(), gate.0.clone())),
            );
            let runtime = h.daemon_runtime();
            let mut daemon = h.spawn_daemon(&runtime);
            let frontend = Frontend(
                h.command()
                    .args([
                        "repair",
                        target,
                        "--repository",
                        &server.base_url,
                        "--test-runtime",
                    ])
                    .arg(&runtime)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .unwrap(),
            );
            wait_until(|| {
                server
                    .request_paths()
                    .iter()
                    .any(|path| path.ends_with(suffix))
            });
            let operation = records(&h).pop().unwrap();
            assert_eq!(operation["stage"], "downloading");
            assert_eq!(operation["cancellable"], true);
            let id = operation["id"].as_str().unwrap().to_owned();
            drop(frontend); // Disconnecting the observer does not stop the daemon operation.
            assert_eq!(records(&h)[0]["status"], "running");
            if crash {
                daemon.crash();
                daemon = h.spawn_daemon(&runtime);
            } else {
                assert_success(
                    &h.command()
                        .args(["operation", "cancel", &id])
                        .output()
                        .unwrap(),
                );
            }
            wait_until(|| records(&h)[0]["status"] == if crash { "failed" } else { "cancelled" });
            assert_eq!(std::fs::read(&asset).unwrap(), b"tampered asset");
            let result = records(&h)[0].clone();
            if !crash {
                assert_eq!(result["cancel_cleaned"], true);
                assert_eq!(result["exit_code"], 130);
            }
            let requests = server.request_paths();
            gate.0.store(true, Ordering::SeqCst);
            drop(daemon);
            let _daemon = h.spawn_daemon(&runtime);
            std::thread::sleep(Duration::from_secs(3));
            assert_eq!(records(&h)[0], result);
            assert_eq!(server.request_paths(), requests);
        }
    }
}

#[test]
fn queued_repair_can_be_queried_and_cancelled_while_installation_is_locked() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let h = InstallHarness::new("repair-queued", "healthy", 30_000);
    assert_success(&h.run());
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
    let _frontend = Frontend(
        h.command()
            .args(["repair", "static", "--test-runtime"])
            .arg(&runtime)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    wait_until(|| !records(&h).is_empty());
    let operation = records(&h).pop().unwrap();
    assert_eq!(operation["status"], "queued");
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
    assert_eq!(records(&h)[0]["status"], "cancelled");
}

#[test]
fn crashed_daemon_rolls_back_binary_replacement_and_preserves_original_state() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let h = InstallHarness::new("repair-activation-crash", "healthy", 30_000);
    assert_success(&h.run());
    let original_state = std::fs::read(h.state_path()).unwrap();
    let binary = h.install_root.join("current/landscape-webserver");
    let original_binary = std::fs::read(&binary).unwrap();
    let archive = h.install_root.join("current/static.zip");
    let original_archive = std::fs::read(&archive).unwrap();
    let runtime = h.daemon_runtime();
    let gate = h.run_dir().join("test-repair-verify-gate");
    std::fs::write(&gate, b"pause after replacement").unwrap();
    let mut daemon = h.spawn_daemon_with_repair_pause(&runtime, true);
    let frontend = Frontend(
        h.command()
            .args([
                "repair",
                "binary",
                "--repository",
                &h.repository.base_url,
                "--test-runtime",
            ])
            .arg(&runtime)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    wait_until(|| {
        records(&h)
            .first()
            .is_some_and(|r| r["stage"] == "verifying")
    });
    let operation = records(&h).pop().unwrap();
    assert_eq!(operation["cancellable"], false);
    assert!(
        !h.command()
            .args(["operation", "cancel", operation["id"].as_str().unwrap()])
            .output()
            .unwrap()
            .status
            .success()
    );
    drop(frontend);
    daemon.crash();
    std::fs::remove_file(gate).unwrap();
    let _daemon = h.spawn_daemon(&runtime);
    wait_until(|| records(&h)[0]["status"] == "failed");
    let result = records(&h).pop().unwrap();
    assert_eq!(result["exit_code"], 5);
    assert_eq!(std::fs::read(&binary).unwrap(), original_binary);
    assert_eq!(std::fs::read(&archive).unwrap(), original_archive);
    assert_eq!(std::fs::read(h.state_path()).unwrap(), original_state);
    let tx: serde_json::Value = serde_json::from_slice(
        &std::fs::read(h.transactions_dir().join(format!(
            "{}.json",
            result["transaction_id"].as_str().unwrap()
        )))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(tx["phase"], "rolled_back");
}
