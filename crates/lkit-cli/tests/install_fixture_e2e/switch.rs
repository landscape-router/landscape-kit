use std::io::Write;
use std::time::Duration;

use super::support::*;

/// SW-03:目标版本已经 active 时拒绝创建无意义事务
/// (`workflows/switch.rs` 的 `SWITCH_TARGET_VERSION_ALREADY_ACTIVE`),
/// 退出码 2,不创建 switch 事务,服务保持运行。
#[test]
fn switch_rejects_an_already_active_target_version() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("switch-already-active", "healthy", 10_000);
    assert_success(&harness.run());

    let output = harness
        .command()
        .args(["switch", "--version", VERSION, "--repository"])
        .arg(&harness.repository.base_url)
        .args(["--test-runtime"])
        .arg(&harness.runtime_config)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(2),
        "switching to the active version must be a usage error\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
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
        "a rejected switch must not create a transaction"
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

    let mut pty = Pty::open();
    let mut command = harness.command();
    command
        .args(["switch", "--version", "2.0.0", "--repository"])
        .arg(&next.base_url)
        .arg("--allow-no-backup")
        .args(["--test-runtime"])
        .arg(&harness.runtime_config);
    attach_pty(&mut command, &pty);
    let mut child = command.spawn().unwrap();
    let prompt = pty.read_until("警告", Duration::from_secs(60));
    assert!(
        prompt.contains("--allow-no-backup"),
        "the warning must mention the ignored flag:\n{prompt}"
    );
    pty.master.write_all(b"yes\n").unwrap();
    let status = child.wait().unwrap();
    assert!(
        status.success(),
        "switch failed with {status:?}\npty output:\n{prompt}"
    );

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
