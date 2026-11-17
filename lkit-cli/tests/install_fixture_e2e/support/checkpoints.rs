use super::InstallHarness;
use std::time::{Duration, Instant};

pub(crate) fn print_operation_diagnostics(h: &InstallHarness) {
    // Read evidence directly: a stuck CLI must not block timeout diagnostics.
    // TestWorld exports bounded, redacted logs during panic cleanup.
    for directory in [h.run_dir().join("operations"), h.transactions_dir()] {
        for entry in std::fs::read_dir(directory).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            if let Ok(bytes) = std::fs::read(&path)
                && let Ok(record) = serde_json::from_slice::<serde_json::Value>(&bytes)
            {
                eprintln!(
                    "{}: status={} stage={} phase={} message={}",
                    path.display(),
                    record["status"],
                    record["stage"],
                    record["phase"],
                    record["message"]
                );
            }
        }
    }
}

#[track_caller]
pub(crate) fn wait_for_checkpoint(h: &InstallHarness, name: &str, operation_kind: &str) {
    // Late checkpoints include queueing, hashing the debug binary, downloading,
    // compressing its protection backup, and health checks. The gate's own
    // timeout starts only after the reached marker is written.
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if h.run_dir()
            .join(format!("test-repair-{name}.reached"))
            .exists()
        {
            return;
        }
        // A completed workflow cannot reach its checkpoint. Inspect durable
        // results so neither a stuck CLI nor the longer budget hides failures.
        for entry in std::fs::read_dir(h.run_dir().join("operations"))
            .into_iter()
            .flatten()
            .flatten()
        {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("json")
                && let Ok(bytes) = std::fs::read(&path)
                && let Ok(record) = serde_json::from_slice::<serde_json::Value>(&bytes)
                && record["kind"] == operation_kind
                && matches!(
                    record["status"].as_str(),
                    Some("succeeded" | "failed" | "cancelled" | "interrupted")
                )
            {
                print_operation_diagnostics(h);
                panic!(
                    "{operation_kind} finished before checkpoint {name}: status={} stage={} message={}",
                    record["status"], record["stage"], record["message"]
                );
            }
        }
        if Instant::now() >= deadline {
            print_operation_diagnostics(h);
            panic!("timed out waiting for {operation_kind} checkpoint {name}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}
