use std::io::Write;
use std::path::Path;
use std::process::Command;

use super::support::*;

#[test]
fn installs_and_verifies_fixture_through_full_cli() {
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("first-install", "healthy", 10_000);
    let output = harness.run();
    assert!(
        output.status.success(),
        "lkit failed with {:?}\nstdout:\n{}\nstderr:\n{}\nservice log:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        harness.service_log()
    );
    assert!(
        !output.stdout.contains(&0x1b) && !output.stderr.contains(&0x1b),
        "non-interactive output contains terminal control sequences"
    );

    let state: serde_json::Value = serde_json::from_slice(
        &std::fs::read(harness.install_root.join("state/install-state.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(state["active_version"], VERSION);
    assert_eq!(state["initialization"]["status"], "complete");
    assert_eq!(state["service"]["manager"], "systemd");
    assert_eq!(state["service"]["verified"], true);
    assert!(
        state.get("repository").is_none(),
        "install-state.json must not record the repository source"
    );
    assert!(
        !harness.install_root.join("config.toml").exists(),
        "lkit must not create config.toml on first install"
    );
    assert!(
        harness
            .install_root
            .join("data/landscape_init.lock")
            .is_file()
    );
    assert!(harness.install_root.join("data/landscape.toml").is_file());
    assert!(
        harness
            .install_root
            .join("data/landscape_db.sqlite")
            .is_file()
    );
    assert!(
        harness
            .install_root
            .join("current/landscape-webserver")
            .is_file()
    );

    let active = systemctl(&harness.world, &["is-active", "landscape-router.service"]);
    assert_success(&active);
    assert_eq!(String::from_utf8_lossy(&active.stdout).trim(), "active");
    let main_pid = systemctl(
        &harness.world,
        &[
            "show",
            "--property=MainPID",
            "--value",
            "landscape-router.service",
        ],
    );
    assert_success(&main_pid);
    let pid: u32 = String::from_utf8_lossy(&main_pid.stdout)
        .trim()
        .parse()
        .unwrap();
    assert!(Path::new(&format!("/proc/{pid}")).is_dir());
    let executable = std::fs::read_link(format!("/proc/{pid}/exe")).unwrap();
    assert_eq!(
        executable,
        harness
            .install_root
            .join(format!("releases/{VERSION}/landscape-webserver"))
            .canonicalize()
            .unwrap()
    );
}

#[test]
fn corrupted_config_blocks_repository_commands_but_not_plain_reconcile() {
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("config-corrupt", "healthy", 10_000);
    assert_success(&harness.run());
    let config_path = harness.install_root.join("config.toml");
    assert!(
        !config_path.exists(),
        "first install must not create config.toml"
    );

    std::fs::write(&config_path, b"not valid toml [[[").unwrap();
    let original_bytes = std::fs::read(&config_path).unwrap();

    // 普通 reconcile 不读取配置,损坏配置不影响它。
    let reconcile = Command::new(LKIT)
        .env(
            lkit_test_fixture::SYSTEMCTL_CONFIG_ENV,
            &harness.world.systemctl_config,
        )
        .arg("reconcile")
        .arg("--install-dir")
        .arg(&harness.install_root)
        .arg("--test-runtime")
        .arg(&harness.runtime_config)
        .output()
        .unwrap();
    assert!(
        reconcile.status.success(),
        "plain reconcile must ignore the corrupted config\nstderr:\n{}",
        String::from_utf8_lossy(&reconcile.stderr)
    );

    // 需要解析来源的 switch/repair/update 在无显式来源时被损坏配置阻断。
    for args in [
        vec!["switch", "--version", VERSION],
        vec!["repair", "static"],
    ] {
        let blocked = Command::new(LKIT)
            .env(
                lkit_test_fixture::SYSTEMCTL_CONFIG_ENV,
                &harness.world.systemctl_config,
            )
            .args(&args)
            .arg("--install-dir")
            .arg(&harness.install_root)
            .arg("--test-runtime")
            .arg(&harness.runtime_config)
            .output()
            .unwrap();
        assert_eq!(
            blocked.status.code(),
            Some(1),
            "{args:?} must be blocked by the corrupted config\nstderr:\n{}",
            String::from_utf8_lossy(&blocked.stderr)
        );
        assert!(
            String::from_utf8_lossy(&blocked.stderr).contains("config.toml"),
            "{args:?} stderr must identify the config file:\n{}",
            String::from_utf8_lossy(&blocked.stderr)
        );
    }

    let mut update_tty = Pty::open();
    update_tty.master.write_all(b"\n").unwrap();
    let blocked = Command::new(LKIT)
        .env(
            lkit_test_fixture::SYSTEMCTL_CONFIG_ENV,
            &harness.world.systemctl_config,
        )
        .env("LKIT_INTERNAL_DAEMON_TTY", &update_tty.slave_path)
        .arg("update")
        .arg("--install-dir")
        .arg(&harness.install_root)
        .arg("--test-runtime")
        .arg(&harness.runtime_config)
        .output()
        .unwrap();
    assert_eq!(
        blocked.status.code(),
        Some(1),
        "update must be blocked by the corrupted config\nstderr:\n{}",
        String::from_utf8_lossy(&blocked.stderr)
    );
    assert!(
        String::from_utf8_lossy(&blocked.stderr).contains("config.toml"),
        "update stderr must identify the config file:\n{}",
        String::from_utf8_lossy(&blocked.stderr)
    );

    // 显式来源完全绕过损坏配置,且不修改原文件字节。
    let bypass = Command::new(LKIT)
        .env(
            lkit_test_fixture::SYSTEMCTL_CONFIG_ENV,
            &harness.world.systemctl_config,
        )
        .arg("reconcile")
        .arg("--repository")
        .arg(&harness.repository.base_url)
        .arg("--install-dir")
        .arg(&harness.install_root)
        .arg("--test-runtime")
        .arg(&harness.runtime_config)
        .output()
        .unwrap();
    assert!(
        bypass.status.success(),
        "explicit --repository must bypass the corrupted config\nstderr:\n{}",
        String::from_utf8_lossy(&bypass.stderr)
    );
    assert_eq!(
        std::fs::read(&config_path).unwrap(),
        original_bytes,
        "explicit bypass must not modify the config file"
    );

    let bypass_repair = Command::new(LKIT)
        .env(
            lkit_test_fixture::SYSTEMCTL_CONFIG_ENV,
            &harness.world.systemctl_config,
        )
        .arg("repair")
        .arg("static")
        .arg("--repository")
        .arg(&harness.repository.base_url)
        .arg("--install-dir")
        .arg(&harness.install_root)
        .arg("--test-runtime")
        .arg(&harness.runtime_config)
        .output()
        .unwrap();
    assert!(
        bypass_repair.status.success(),
        "explicit --repository must bypass the corrupted config for repair\nstderr:\n{}",
        String::from_utf8_lossy(&bypass_repair.stderr)
    );
    assert_eq!(
        std::fs::read(&config_path).unwrap(),
        original_bytes,
        "explicit repair must not modify the config file"
    );

    std::fs::remove_file(&config_path).unwrap();
    let fixed = Command::new(LKIT)
        .env(
            lkit_test_fixture::SYSTEMCTL_CONFIG_ENV,
            &harness.world.systemctl_config,
        )
        .arg("reconcile")
        .arg("--install-dir")
        .arg(&harness.install_root)
        .arg("--test-runtime")
        .arg(&harness.runtime_config)
        .output()
        .unwrap();
    assert!(
        fixed.status.success(),
        "command must recover after deleting the corrupted config\nstderr:\n{}",
        String::from_utf8_lossy(&fixed.stderr)
    );
}

#[test]
fn preset_config_drives_first_install_without_writes() {
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("config-preset", "healthy", 10_000);
    let config_path = harness.install_root.join("config.toml");
    let preset = format!(
        "schema_version = 1\n\n[repository]\nkind = \"http\"\nlocation = \"{}\"\n",
        harness.repository.base_url
    );
    std::fs::create_dir_all(&harness.install_root).unwrap();
    std::fs::write(&config_path, &preset).unwrap();

    let output = Command::new(LKIT)
        .env(
            lkit_test_fixture::SYSTEMCTL_CONFIG_ENV,
            &harness.world.systemctl_config,
        )
        .args([
            "install",
            "--non-interactive",
            "--version",
            VERSION,
            "--install-dir",
        ])
        .arg(&harness.install_root)
        .args(["--admin-user", "admin", "--password-file"])
        .arg(&harness.password)
        .args(["--test-runtime"])
        .arg(&harness.runtime_config)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "preset config must drive the first install\nstdout:\n{}\nstderr:\n{}\nservice log:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        harness.service_log()
    );
    assert_eq!(
        std::fs::read(&config_path).unwrap(),
        preset.as_bytes(),
        "first install must leave the preset config untouched"
    );
    let state: serde_json::Value = serde_json::from_slice(
        &std::fs::read(harness.install_root.join("state/install-state.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(state["active_version"], VERSION);
    assert!(
        state.get("repository").is_none(),
        "install-state.json must not record the repository source"
    );
    assert!(
        harness
            .repository
            .request_paths()
            .iter()
            .any(|path| path == "/releases/1.2.3/manifest.json"),
        "install must download from the configured repository"
    );
}

#[test]
fn explicit_repository_bypasses_preset_config_without_modifying_it() {
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("config-bypass", "healthy", 10_000);
    let config_path = harness.install_root.join("config.toml");
    let other = RepositoryServer::start(repository_files_for(VERSION));
    let preset = format!(
        "schema_version = 1\n\n[repository]\nkind = \"http\"\nlocation = \"{}\"\n",
        other.base_url
    );
    std::fs::create_dir_all(&harness.install_root).unwrap();
    std::fs::write(&config_path, &preset).unwrap();

    assert_success(&harness.run());

    // 同版本诊断使用显式来源,完全绕过配置:预设的 other 服务器不应收到任何请求,
    // 配置字节保持不变;诊断基于安装记录核对显式来源的资产身份。
    let reconcile = Command::new(LKIT)
        .env(
            lkit_test_fixture::SYSTEMCTL_CONFIG_ENV,
            &harness.world.systemctl_config,
        )
        .arg("reconcile")
        .arg("--repository")
        .arg(&harness.repository.base_url)
        .arg("--install-dir")
        .arg(&harness.install_root)
        .arg("--test-runtime")
        .arg(&harness.runtime_config)
        .output()
        .unwrap();
    assert!(
        reconcile.status.success(),
        "explicit repository must verify against the configured installation\nstderr:\n{}",
        String::from_utf8_lossy(&reconcile.stderr)
    );
    assert_eq!(
        std::fs::read(&config_path).unwrap(),
        preset.as_bytes(),
        "reconcile must not rewrite the config file"
    );
    assert!(
        other.request_paths().is_empty(),
        "the preset config repository must not be consulted when --repository is explicit: {:?}",
        other.request_paths()
    );

    // 与安装记录资产不一致的显式来源被拒绝,配置仍然不变。
    let drifted = RepositoryServer::start({
        let mut files = repository_files();
        let manifest: serde_json::Value =
            serde_json::from_slice(&files[&"/releases/1.2.3/manifest.json".to_string()]).unwrap();
        let mut manifest = manifest;
        manifest["assets"]["static"]["sha256"] = serde_json::Value::String("f".repeat(64));
        files.insert(
            "/releases/1.2.3/manifest.json".into(),
            serde_json::to_vec(&manifest).unwrap(),
        );
        files
    });
    let refused = Command::new(LKIT)
        .env(
            lkit_test_fixture::SYSTEMCTL_CONFIG_ENV,
            &harness.world.systemctl_config,
        )
        .arg("reconcile")
        .arg("--repository")
        .arg(&drifted.base_url)
        .arg("--install-dir")
        .arg(&harness.install_root)
        .arg("--test-runtime")
        .arg(&harness.runtime_config)
        .output()
        .unwrap();
    assert_eq!(
        refused.status.code(),
        Some(1),
        "a repository with different same-version assets must be refused\nstderr:\n{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert_eq!(
        std::fs::read(&config_path).unwrap(),
        preset.as_bytes(),
        "refused reconcile must not modify the config file"
    );
}

#[test]
fn latest_without_a_stable_channel_fails_instead_of_false_success() {
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("latest-no-stable", "healthy", 10_000);
    assert_success(&harness.run());
    let transactions_before = transaction_count(&harness.install_root);

    // 指向一个没有 stable 渠道的仓库:latest 解析返回 None,必须报错而不是静默成功。
    let mut files = repository_files_for(VERSION);
    files.remove("/channels/stable.json");
    let empty = RepositoryServer::start(files);

    let output = Command::new(LKIT)
        .env(
            lkit_test_fixture::SYSTEMCTL_CONFIG_ENV,
            &harness.world.systemctl_config,
        )
        .arg("switch")
        .arg("--version")
        .arg("latest")
        .arg("--repository")
        .arg(&empty.base_url)
        .arg("--install-dir")
        .arg(&harness.install_root)
        .arg("--test-runtime")
        .arg(&harness.runtime_config)
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(1),
        "switch --version latest must fail when the repository has no stable release\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("no stable version"),
        "switch stderr must report the missing stable release:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains("installed and verified"),
        "switch must not report a false same-version success:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert_eq!(
        transaction_count(&harness.install_root),
        transactions_before,
        "switch must not create a transaction"
    );
}

#[test]
fn cleans_up_after_fixture_health_failure() {
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("health-failure", "health_error", 2_500);
    let output = harness.run();
    assert!(
        !output.status.success(),
        "health failure unexpectedly succeeded\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("health check failed"),
        "unexpected error:\n{}\nservice log:\n{}",
        String::from_utf8_lossy(&output.stderr),
        harness.service_log()
    );
    let active = systemctl(&harness.world, &["is-active", "landscape-router.service"]);
    assert_eq!(active.status.code(), Some(3));
    assert_eq!(String::from_utf8_lossy(&active.stdout).trim(), "inactive");
    assert!(!harness.host.join("units/landscape-router.service").exists());
    assert!(!harness.install_root.join("current").exists());
    assert!(
        !harness
            .install_root
            .join(format!("releases/{VERSION}"))
            .exists()
    );
    assert!(
        !harness
            .install_root
            .join("state/install-state.json")
            .exists()
    );
    assert_eq!(
        std::fs::read(harness.host.join("resolv.conf")).unwrap(),
        b"nameserver 127.0.0.1\n"
    );
}
