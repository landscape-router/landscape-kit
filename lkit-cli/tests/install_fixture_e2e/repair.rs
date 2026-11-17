use super::support::{
    E2E_LOCK, InstallHarness, RepositoryServer, TestPorts, e2e_enabled, repository_files,
};
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

#[test]
fn fixture_health_ports_stay_reserved_until_repository_start() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let ports = TestPorts::reserve();
    let reserved = [ports.dns, ports.http, ports.https];
    assert_eq!(
        reserved
            .into_iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        3
    );
    for port in reserved {
        assert!(std::net::TcpListener::bind(("127.0.0.1", port)).is_err());
    }
    assert!(std::net::UdpSocket::bind(("127.0.0.1", ports.dns)).is_err());
    let repository = RepositoryServer::start(std::collections::HashMap::new());
    let repository_port = url::Url::parse(&repository.base_url)
        .unwrap()
        .port()
        .unwrap();
    assert!(!reserved.contains(&repository_port));
    drop(ports);
    for port in reserved {
        std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
    }
    std::net::UdpSocket::bind(("127.0.0.1", reserved[0])).unwrap();
}

#[test]
fn repository_drop_closes_idle_gated_and_incomplete_connections() {
    use std::io::Write;
    use std::net::TcpStream;

    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for scenario in ["idle", "gated", "incomplete"] {
        let gate = Gate(Arc::new(AtomicBool::new(false)));
        let repository = RepositoryServer::start_gated(
            std::collections::HashMap::new(),
            Some(("asset".into(), gate.0.clone())),
        );
        let address = repository
            .base_url
            .trim_start_matches("http://")
            .trim_end_matches('/');
        let mut client = (scenario != "idle").then(|| TcpStream::connect(address).unwrap());
        if scenario == "gated" {
            client
                .as_mut()
                .unwrap()
                .write_all(b"GET /asset HTTP/1.1\r\n\r\n")
                .unwrap();
            wait_until(|| repository.request_paths() == ["/asset"]);
        }
        if scenario == "incomplete" {
            wait_until(|| repository.has_active_connection());
        }
        let address = address.to_owned();
        let started = Instant::now();
        drop(repository);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{scenario} shutdown blocked"
        );
        assert!(
            TcpStream::connect(address).is_err(),
            "{scenario} listener leaked after drop"
        );
        drop(client);
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
        let (h, _territory) =
            InstallHarness::new("repair-assets", "healthy", 30_000).with_tmpfs_territory();
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

fn service_mutations(h: &InstallHarness) -> Vec<Vec<String>> {
    std::fs::read_to_string(h.world.path("systemctl-calls.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Vec<String>>(line).unwrap())
        .filter(|args| {
            args.iter().any(|arg| {
                matches!(
                    arg.as_str(),
                    "start"
                        | "stop"
                        | "restart"
                        | "enable"
                        | "disable"
                        | "mask"
                        | "unmask"
                        | "link"
                        | "daemon-reload"
                )
            })
        })
        .collect()
}
fn acquire_fixture_install_lock(h: &InstallHarness) -> std::fs::File {
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .open(h.run_dir().join("install.lock"))
        .unwrap();
    wait_until(|| unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0);
    lock
}

fn start_repair(
    h: &InstallHarness,
    runtime: &std::path::Path,
    target: &str,
    repository: &str,
) -> Frontend {
    Frontend(
        h.command()
            .args([
                "repair",
                target,
                "--repository",
                repository,
                "--test-runtime",
            ])
            .arg(runtime)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    )
}
fn transaction(h: &InstallHarness, operation: &serde_json::Value) -> serde_json::Value {
    serde_json::from_slice(
        &std::fs::read(h.transactions_dir().join(format!(
            "{}.json",
            operation["transaction_id"].as_str().unwrap()
        )))
        .unwrap(),
    )
    .unwrap()
}
fn checkpoint_gate(h: &InstallHarness, name: &str) -> std::path::PathBuf {
    let gate = h.run_dir().join(format!("test-repair-{name}.gate"));
    std::fs::write(&gate, b"pause").unwrap();
    gate
}
fn await_checkpoint(h: &InstallHarness, name: &str) {
    wait_until(|| {
        h.run_dir()
            .join(format!("test-repair-{name}.reached"))
            .exists()
    });
}
fn operation_by_id(h: &InstallHarness, id: &str) -> serde_json::Value {
    records(h).into_iter().find(|r| r["id"] == id).unwrap()
}

#[test]
fn static_crash_windows_restore_pages_archive_and_state_together() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for checkpoint in ["static-removed", "static-installed", "static-state-written"] {
        for archive_present in [false, true] {
            let h = InstallHarness::new("static-crash-window", "healthy", 30_000);
            assert_success(&h.run());
            let (asset, _) = alter_asset(&h, "static");
            let archive = h.install_root.join("current/static.zip");
            if archive_present {
                std::fs::write(&archive, b"previous archive").unwrap();
            } else {
                std::fs::remove_file(&archive).unwrap();
            }
            let old_archive = std::fs::read(&archive).ok();
            let old_state = std::fs::read(h.state_path()).unwrap();
            let old_service = service_mutations(&h);
            let runtime = h.daemon_runtime();
            let gate = checkpoint_gate(&h, checkpoint);
            let mut daemon = h.spawn_daemon_at_checkpoint(&runtime, checkpoint);
            let frontend = start_repair(&h, &runtime, "static", &h.repository.base_url);
            await_checkpoint(&h, checkpoint);
            let operation = records(&h).pop().unwrap();
            assert_eq!(operation["status"], "running");
            assert_eq!(transaction(&h, &operation)["phase"], "activating");
            assert_eq!(asset.exists(), checkpoint != "static-removed");
            drop(frontend);
            daemon.crash();
            std::fs::remove_file(gate).unwrap();
            let requests = h.repository.request_paths();
            let daemon = h.spawn_daemon(&runtime);
            wait_until(|| records(&h)[0]["status"] == "failed");
            let result = records(&h).pop().unwrap();
            assert_eq!(result["exit_code"], 1);
            assert_eq!(transaction(&h, &result)["phase"], "rolled_back");
            assert_eq!(std::fs::read(&asset).unwrap(), b"tampered asset");
            assert_eq!(std::fs::read(&archive).ok(), old_archive);
            assert_eq!(std::fs::read(h.state_path()).unwrap(), old_state);
            assert_eq!(
                service_mutations(&h),
                old_service,
                "static repair must not restart service"
            );
            drop(daemon);
            let _daemon = h.spawn_daemon(&runtime);
            std::thread::sleep(Duration::from_secs(3));
            assert_eq!(records(&h), vec![result]);
            assert_eq!(h.repository.request_paths(), requests);
        }
    }
}

#[test]
fn committed_repairs_recover_missing_results_without_replaying_side_effects() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for target in ["static", "binary"] {
        let h = InstallHarness::new("repair-commit-window", "healthy", 30_000);
        assert_success(&h.run());
        let (asset, original) = alter_asset(&h, target);
        let runtime = h.daemon_runtime();
        let gate = checkpoint_gate(&h, "before-result");
        let mut daemon = h.spawn_daemon_at_checkpoint(&runtime, "before-result");
        let frontend = start_repair(&h, &runtime, target, &h.repository.base_url);
        await_checkpoint(&h, "before-result");
        let operation = records(&h).pop().unwrap();
        assert_eq!(operation["status"], "running");
        assert_eq!(transaction(&h, &operation)["phase"], "committed");
        assert_eq!(std::fs::read(&asset).unwrap(), original);
        let state = std::fs::read(h.state_path()).unwrap();
        let service_log = service_mutations(&h);
        let requests = h.repository.request_paths();
        let backup_count = std::fs::read_dir(h.backups_dir()).unwrap().count();
        let transaction_count = std::fs::read_dir(h.transactions_dir()).unwrap().count();
        drop(frontend);
        daemon.crash();
        std::fs::remove_file(gate).unwrap();
        let daemon = h.spawn_daemon(&runtime);
        wait_until(|| records(&h)[0]["status"] == "succeeded");
        let result = records(&h).pop().unwrap();
        assert_eq!(result["exit_code"], 0);
        assert_eq!(result["transaction_id"], operation["transaction_id"]);
        drop(daemon);
        let _daemon = h.spawn_daemon(&runtime);
        std::thread::sleep(Duration::from_secs(3));
        assert_eq!(records(&h), vec![result]);
        assert_eq!(std::fs::read(&asset).unwrap(), original);
        assert_eq!(std::fs::read(h.state_path()).unwrap(), state);
        assert_eq!(h.repository.request_paths(), requests);
        assert_eq!(service_mutations(&h), service_log);
        assert_eq!(
            std::fs::read_dir(h.backups_dir()).unwrap().count(),
            backup_count
        );
        assert_eq!(
            std::fs::read_dir(h.transactions_dir()).unwrap().count(),
            transaction_count
        );
    }
}

/// A second accepted request must remain queued through multiple recovery cycles.
/// Cancel it before removing the fault so only recovery may modify the installation.
fn assert_recovery_blocks_next_repair(h: &InstallHarness, runtime: &std::path::Path, id: &str) {
    let _frontend = start_repair(h, runtime, "static", &h.repository.base_url);
    wait_until(|| records(h).len() == 2);
    let queued = records(h).into_iter().find(|r| r["id"] != id).unwrap();
    let queued_id = queued["id"].as_str().unwrap();
    let requests = h.repository.request_paths();
    std::thread::sleep(Duration::from_secs(4));
    assert_eq!(operation_by_id(h, id)["status"], "recovering");
    assert_eq!(operation_by_id(h, queued_id)["status"], "queued");
    assert_eq!(
        operation_by_id(h, queued_id)["transaction_id"],
        serde_json::Value::Null
    );
    assert_eq!(h.repository.request_paths(), requests);
    assert_success(
        &h.command()
            .args(["operation", "cancel", queued_id])
            .output()
            .unwrap(),
    );
}

#[test]
fn failed_static_rollback_blocks_mutations_and_resumes_after_filesystem_repair() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let h = InstallHarness::new("repair-rollback-fault", "healthy", 30_000);
    assert_success(&h.run());
    let (asset, _) = alter_asset(&h, "static");
    let old_state = std::fs::read(h.state_path()).unwrap();
    let archive = h.install_root.join("current/static.zip");
    let old_archive = std::fs::read(&archive).unwrap();
    let runtime = h.daemon_runtime();
    let gate = checkpoint_gate(&h, "static-removed");
    let mut daemon = h.spawn_daemon_at_checkpoint(&runtime, "static-removed");
    let frontend = start_repair(&h, &runtime, "static", &h.repository.base_url);
    await_checkpoint(&h, "static-removed");
    let operation = records(&h).pop().unwrap();
    let id = operation["id"].as_str().unwrap();
    drop(frontend);
    daemon.crash();
    std::fs::remove_file(gate).unwrap();
    // A regular file at the directory destination makes remove_dir_all fail,
    // even when CI runs as root. The verified preimage remains intact.
    let destination = asset.parent().unwrap();
    std::fs::write(destination, b"filesystem obstruction").unwrap();
    let mut daemon = h.spawn_daemon(&runtime);
    wait_until(|| operation_by_id(&h, id)["status"] == "recovering");
    assert_eq!(transaction(&h, &operation)["phase"], "rolling_back");
    let preimage = h
        .transactions_dir()
        .join(operation["transaction_id"].as_str().unwrap())
        .join("static-backup/index.html");
    assert_eq!(std::fs::read(&preimage).unwrap(), b"tampered asset");
    daemon.crash();
    let _daemon = h.spawn_daemon(&runtime);
    assert_recovery_blocks_next_repair(&h, &runtime, id);
    assert_eq!(
        std::fs::read(destination).unwrap(),
        b"filesystem obstruction"
    );
    std::fs::remove_file(destination).unwrap();
    wait_until(|| operation_by_id(&h, id)["status"] == "failed");
    assert_eq!(transaction(&h, &operation)["phase"], "rolled_back");
    assert_eq!(std::fs::read(asset).unwrap(), b"tampered asset");
    assert_eq!(std::fs::read(archive).unwrap(), old_archive);
    assert_eq!(std::fs::read(h.state_path()).unwrap(), old_state);
}

#[test]
fn failed_cancellation_cleanup_survives_restart_and_resumes_without_download() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for target in ["static", "binary"] {
        let h = InstallHarness::new("repair-cancel-fault", "healthy", 30_000);
        assert_success(&h.run());
        let (asset, _) = alter_asset(&h, target);
        let old_state = std::fs::read(h.state_path()).unwrap();
        let service_log = service_mutations(&h);
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
        let frontend = start_repair(&h, &runtime, target, &server.base_url);
        wait_until(|| {
            server
                .request_paths()
                .iter()
                .any(|path| path.ends_with(suffix))
        });
        let operation = records(&h).pop().unwrap();
        let id = operation["id"].as_str().unwrap();
        let dir = h
            .transactions_dir()
            .join(operation["transaction_id"].as_str().unwrap());
        let saved = h.world.path("saved-download-directory");
        std::fs::rename(&dir, &saved).unwrap();
        std::fs::write(&dir, b"cleanup obstruction").unwrap();
        assert_success(
            &h.command()
                .args(["operation", "cancel", id])
                .output()
                .unwrap(),
        );
        wait_until(|| operation_by_id(&h, id)["status"] == "recovering");
        assert_eq!(operation_by_id(&h, id)["cancel_cleaned"], false);
        assert_eq!(transaction(&h, &operation)["phase"], "preparing");
        drop(frontend);
        daemon.crash();
        let _daemon = h.spawn_daemon(&runtime);
        assert_recovery_blocks_next_repair(&h, &runtime, id);
        assert!(saved.is_dir());
        let requests = server.request_paths();
        let lock = acquire_fixture_install_lock(&h);
        std::fs::remove_file(&dir).unwrap();
        std::fs::rename(saved, &dir).unwrap();
        drop(lock);
        wait_until(|| operation_by_id(&h, id)["status"] == "cancelled");
        let result = operation_by_id(&h, id);
        assert_eq!(result["cancel_cleaned"], true);
        assert_eq!(result["exit_code"], 130);
        assert_eq!(transaction(&h, &result)["phase"], "failed");
        assert!(!dir.exists());
        assert_eq!(server.request_paths(), requests);
        assert_eq!(std::fs::read(asset).unwrap(), b"tampered asset");
        assert_eq!(std::fs::read(h.state_path()).unwrap(), old_state);
        assert_eq!(service_mutations(&h), service_log);
    }
}
