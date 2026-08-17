use std::io::Write;

use super::support::*;

/// 以第二版本仓库 + pty 确认运行 `lkit update`。
fn update_with_confirmation(harness: &InstallHarness, args: &[&str]) -> std::process::Child {
    let next = RepositoryServer::start(repository_files_for("2.0.0"));
    let mut pty = Pty::open();
    pty.master.write_all(b"yes\nyes\n").unwrap();
    let mut command = harness.command();
    command
        .arg("update")
        .args(args)
        .arg("--repository")
        .arg(&next.base_url)
        .args(["--test-runtime"])
        .arg(&harness.runtime_config);
    attach_pty(&mut command, &pty);
    command.spawn().unwrap()
}

/// UP-02:确认后执行 latest 目标的实际升级,新版本进入 active。
#[test]
fn update_upgrades_to_latest_after_confirmation() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("update-latest", "healthy", 10_000);
    assert_success(&harness.run());

    let mut child = update_with_confirmation(&harness, &[]);
    let status = child.wait().unwrap();
    assert!(
        status.success(),
        "update failed with {status:?}\nservice log:\n{}",
        harness.service_log()
    );
    let state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(harness.state_path()).unwrap()).unwrap();
    assert_eq!(state["active_version"], "2.0.0");
    let active = systemctl(&harness.world, &["is-active", "landscape-router.service"]);
    assert_eq!(String::from_utf8_lossy(&active.stdout).trim(), "active");
}

/// UP-03:确认后执行固定版本目标的实际升级。
#[test]
fn update_upgrades_to_a_pinned_version_after_confirmation() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("update-pinned", "healthy", 10_000);
    assert_success(&harness.run());

    let mut child = update_with_confirmation(&harness, &["--version", "2.0.0"]);
    let status = child.wait().unwrap();
    assert!(
        status.success(),
        "update failed with {status:?}\nservice log:\n{}",
        harness.service_log()
    );
    let state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(harness.state_path()).unwrap()).unwrap();
    assert_eq!(state["active_version"], "2.0.0");
}

/// update 确认被拒时返回退出码 1,不创建事务、版本不变。
#[test]
fn update_refused_confirmation_leaves_the_installation_untouched() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("update-refused", "healthy", 10_000);
    assert_success(&harness.run());
    let transactions_before = std::fs::read_dir(harness.transactions_dir())
        .unwrap()
        .count();

    let next = RepositoryServer::start(repository_files_for("2.0.0"));
    let mut pty = Pty::open();
    pty.master.write_all(b"no\n").unwrap();
    let mut command = harness.command();
    command
        .arg("update")
        .arg("--repository")
        .arg(&next.base_url)
        .args(["--test-runtime"])
        .arg(&harness.runtime_config);
    attach_pty(&mut command, &pty);
    let output = command.output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "a refused update must exit 1\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(harness.state_path()).unwrap()).unwrap();
    assert_eq!(state["active_version"], VERSION);
    assert_eq!(
        std::fs::read_dir(harness.transactions_dir())
            .unwrap()
            .count(),
        transactions_before,
        "a refused update must not create a transaction"
    );
    let active = systemctl(&harness.world, &["is-active", "landscape-router.service"]);
    assert_eq!(String::from_utf8_lossy(&active.stdout).trim(), "active");
}
