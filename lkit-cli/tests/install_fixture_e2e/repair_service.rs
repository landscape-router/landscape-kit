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
