use std::fs;

#[test]
fn export_retains_recovery_evidence_but_excludes_credentials_and_symlink_contents() {
    let temp = std::env::temp_dir().join(format!("lkit-diagnostics-{}", uuid::Uuid::now_v7()));
    let root = temp.join("world");
    let operations = root.join("territory/run/operations");
    fs::create_dir_all(&operations).unwrap();
    fs::write(operations.join("op.json"), br#"{"id":"operation-1","status":"recovering","stage":"rolling_back","message":"message-sentinel","install":{"password":"payload-sentinel"}}"#).unwrap();
    fs::write(
        root.join("territory/config.toml"),
        b"psk = 'config-sentinel'",
    )
    .unwrap();
    fs::write(root.join("territory/run/daemon.log"), b"network recovery failed; will retry\nPSK: log-sentinel\nAuthorization: credential-sentinel\n").unwrap();
    fs::write(
        root.join("territory/run/test-repair-stop.reached"),
        b"ready",
    )
    .unwrap();
    fs::write(temp.join("outside"), b"outside-sentinel").unwrap();
    std::os::unix::fs::symlink(temp.join("outside"), root.join("landscape.log")).unwrap();
    lkit_test_fixture::diagnostics::export(&root, &temp.join("artifacts")).unwrap();
    let output = fs::read_to_string(temp.join("artifacts/snapshot.json")).unwrap();
    for secret in [
        "event-sentinel",
        "message-sentinel",
        "payload-sentinel",
        "config-sentinel",
        "log-sentinel",
        "credential-sentinel",
        "outside-sentinel",
    ] {
        assert!(!output.contains(secret), "leaked {secret}");
    }
    assert!(output.contains("recovering"));
    assert!(output.contains("network recovery failed; will retry"));
    assert!(output.contains("test-repair-stop.reached"));
    fs::remove_dir_all(temp).unwrap();
}

#[test]
fn corrupt_or_oversized_diagnostics_do_not_prevent_other_evidence_from_being_saved() {
    let temp = std::env::temp_dir().join(format!("lkit-diagnostics-{}", uuid::Uuid::now_v7()));
    let root = temp.join("world");
    fs::create_dir_all(root.join("territory/transactions")).unwrap();
    fs::write(root.join("territory/transactions/bad.json"), b"{broken").unwrap();
    fs::write(root.join("landscape.log"), vec![b'x'; 256 * 1024 + 1]).unwrap();
    lkit_test_fixture::diagnostics::export(&root, &temp.join("out")).unwrap();
    let result = fs::read_to_string(temp.join("out/snapshot.json")).unwrap();
    assert!(result.contains("unreadable"));
    assert!(result.len() < 4096);
    fs::remove_dir_all(temp).unwrap();
}
