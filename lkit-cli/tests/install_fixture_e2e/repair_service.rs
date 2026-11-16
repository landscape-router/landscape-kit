use super::support::{E2E_LOCK, InstallHarness, e2e_enabled};
use sha2::{Digest, Sha256};

#[test]
fn repair_service_migrates_legacy_definition_without_restarting_landscape() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("repair-service", "healthy", 30_000);
    let installed = harness.run();
    assert!(
        installed.status.success(),
        "{}",
        String::from_utf8_lossy(&installed.stderr)
    );
    let definition = harness
        .install_root
        .join("service/landscape-router.service");
    let current = std::fs::read_to_string(&definition).unwrap();
    assert!(current.contains("network startup-check"));
    let legacy = current
        .lines()
        .filter(|line| !line.contains("network startup-check"))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    std::fs::write(&definition, &legacy).unwrap();
    let mut state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(harness.state_path()).unwrap()).unwrap();
    state["service"]["definition_sha256"] = Sha256::digest(legacy.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
        .into();
    std::fs::write(
        harness.state_path(),
        serde_json::to_vec_pretty(&state).unwrap(),
    )
    .unwrap();
    let output = harness
        .command()
        .args(["repair", "service", "--test-runtime"])
        .arg(&harness.runtime_config)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(std::fs::read_to_string(&definition).unwrap(), current);
    let committed = std::fs::read(harness.state_path()).unwrap();
    let repeated = harness
        .command()
        .args(["repair", "service", "--test-runtime"])
        .arg(&harness.runtime_config)
        .output()
        .unwrap();
    assert!(repeated.status.success());
    assert_eq!(std::fs::read(harness.state_path()).unwrap(), committed);
}

/// Real CLI/daemon boundary; execution is deliberately paused in fake systemctl.
/// Parent PID proves refresh is invoked by daemon itself, without an lkit worker.
#[test]
fn repair_service_daemon_survives_frontend_disconnect_and_recovers_crash() {
    use std::os::{fd::AsRawFd, unix::fs::PermissionsExt};
    use std::process::Stdio;
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for crash in [false, true] {
        let harness = InstallHarness::new("repair-service-daemon", "healthy", 30_000);
        let installed = harness.run();
        assert!(
            installed.status.success(),
            "{}",
            String::from_utf8_lossy(&installed.stderr)
        );
        let definition = harness
            .install_root
            .join("service/landscape-router.service");
        let current = std::fs::read_to_string(&definition).unwrap();
        let legacy = current
            .lines()
            .filter(|line| !line.contains("network startup-check"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        std::fs::write(&definition, &legacy).unwrap();
        let mut state: serde_json::Value =
            serde_json::from_slice(&std::fs::read(harness.state_path()).unwrap()).unwrap();
        state["service"]["definition_sha256"] = Sha256::digest(legacy.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
            .into();
        std::fs::write(harness.state_path(), serde_json::to_vec(&state).unwrap()).unwrap();

        let runtime = harness.daemon_runtime();
        let mut config: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&runtime).unwrap()).unwrap();
        let fixture = config["systemd"]["systemctl"].as_str().unwrap().to_string();
        let wrapper = harness.world.path("systemctl-paused.sh");
        let entered = harness.world.path("refresh-parent");
        let release = harness.world.path("refresh-release");
        // Bounded wait ensures an assertion failure cannot leak a blocked fixture.
        let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
        std::fs::write(&wrapper, format!(
            "#!/bin/sh\nif [ \"$1\" = daemon-reload ]; then\n echo \"$PPID\" > {}\n n=0\n while [ ! -f {} ] && [ \"$n\" -lt 200 ]; do sleep 0.1; n=$((n+1)); done\nfi\nexec {} \"$@\"\n",
            quote(entered.to_str().unwrap()), quote(release.to_str().unwrap()), quote(&fixture)
        )).unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
        config["systemd"]["systemctl"] = wrapper.to_str().unwrap().into();
        std::fs::write(&runtime, serde_json::to_vec(&config).unwrap()).unwrap();

        let install_lock = std::fs::OpenOptions::new()
            .write(true)
            .open(harness.run_dir().join("install.lock"))
            .unwrap();
        assert_eq!(
            unsafe { libc::flock(install_lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
        let mut daemon = harness.spawn_daemon(&runtime);
        let mut frontend = harness
            .command()
            .args(["repair", "service", "--test-runtime"])
            .arg(&runtime)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let operations = harness.run_dir().join("operations");
        wait_until(|| {
            std::fs::read_dir(&operations).is_ok_and(|entries| {
                entries
                    .flatten()
                    .any(|e| e.path().extension().is_some_and(|ext| ext == "json"))
            })
        });
        let record_path = std::fs::read_dir(&operations)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|ext| ext == "json"))
            .unwrap();
        let record = || -> serde_json::Value {
            serde_json::from_slice(&std::fs::read(&record_path).unwrap()).unwrap()
        };
        let id = record()["id"].as_str().unwrap().to_string();
        assert_eq!(record()["status"], "queued");
        frontend.kill().unwrap();
        frontend.wait().unwrap();
        let query = harness
            .command()
            .args(["operation", "status", &id])
            .output()
            .unwrap();
        assert!(
            query.status.success(),
            "status must not wait for installation lock"
        );
        drop(install_lock);
        wait_until(|| entered.is_file());
        assert_eq!(
            std::fs::read_to_string(&entered).unwrap().trim(),
            daemon.id().to_string()
        );
        assert_eq!(record()["status"], "running");
        assert!(record()["transaction_id"].is_string());
        assert!(
            !harness
                .command()
                .args(["operation", "cancel", &id])
                .output()
                .unwrap()
                .status
                .success()
        );
        if crash {
            daemon.crash();
        }
        std::fs::write(&release, b"release").unwrap();
        if crash {
            daemon = harness.spawn_daemon(&runtime);
        }
        wait_until(|| record()["status"] == if crash { "failed" } else { "succeeded" });
        let tx_id = record()["transaction_id"].as_str().unwrap().to_string();
        let tx: serde_json::Value = serde_json::from_slice(
            &std::fs::read(harness.transactions_dir().join(format!("{tx_id}.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(tx["phase"], if crash { "rolled_back" } else { "committed" });
        assert_eq!(
            std::fs::read_to_string(&definition).unwrap(),
            if crash { legacy } else { current }
        );
        let before = std::fs::read(&record_path).unwrap();
        drop(daemon);
        let _restarted = harness.spawn_daemon(&runtime);
        std::thread::sleep(std::time::Duration::from_secs(3));
        assert_eq!(
            std::fs::read(&record_path).unwrap(),
            before,
            "terminal operations must not be replayed"
        );
    }
}

fn wait_until(mut ready: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !ready() {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for daemon operation"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}
