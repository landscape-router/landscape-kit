use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use sha2::Digest;

use super::support::*;

#[test]
fn installs_and_verifies_fixture_through_full_cli() {
    if !e2e_enabled() {
        return;
    }
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

    let state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(harness.state_path()).unwrap()).unwrap();
    assert_eq!(state["active_version"], VERSION);
    assert_eq!(state["initialization"]["status"], "complete");
    assert_eq!(state["service"]["manager"], "systemd");
    assert_eq!(state["service"]["verified"], true);
    assert!(
        state.get("repository").is_none(),
        "install-state.json must not record the repository source"
    );
    let config = std::fs::read_to_string(harness.config_path()).unwrap();
    assert!(
        config.contains("[flare]"),
        "install must write the [flare] section into config.toml: {config}"
    );
    assert!(
        config.contains("fixture-flare-recovery-secret"),
        "install must persist the provided flare psk: {config}"
    );
    let metadata = std::fs::metadata(harness.config_path()).unwrap();
    assert_eq!(
        metadata.mode() & 0o077,
        0,
        "config.toml holding the flare psk must be root-only"
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

/// 单实例约束(docs/deployment/layout-and-state.md):lkit 地盘已有有效安装状态时,
/// 再次 `install` 返回参数错误 `2` 并提示先卸载,不创建第二套安装、不改动现场。
#[test]
fn install_rejects_an_existing_installation() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("second-install", "healthy", 10_000);
    assert_success(&harness.run());
    let state_before = std::fs::read(harness.state_path()).unwrap();
    let transactions_before = transaction_count(&harness.territory);

    let output = harness.run();
    assert_eq!(
        output.status.code(),
        Some(2),
        "a second install must be rejected as a usage error\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("uninstall"),
        "the rejection must hint at `lkit uninstall`:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read(harness.state_path()).unwrap(),
        state_before,
        "the second install must not touch the recorded state"
    );
    assert_eq!(
        transaction_count(&harness.territory),
        transactions_before,
        "the second install must not create a transaction"
    );
    let active = systemctl(&harness.world, &["is-active", "landscape-router.service"]);
    assert_eq!(String::from_utf8_lossy(&active.stdout).trim(), "active");
}

#[test]
fn corrupted_config_blocks_repository_commands_but_not_plain_reconcile() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("config-corrupt", "healthy", 10_000);
    assert_success(&harness.run());
    let config_path = harness.config_path();
    assert!(
        config_path.is_file(),
        "first install must create config.toml with the [flare] section"
    );
    assert!(
        std::fs::read_to_string(&config_path)
            .unwrap()
            .contains("[flare]"),
        "the created config must contain the [flare] section"
    );

    std::fs::write(&config_path, b"not valid toml [[[").unwrap();
    let original_bytes = std::fs::read(&config_path).unwrap();

    // 普通 reconcile 不读取配置,损坏配置不影响它。
    let reconcile = harness
        .command()
        .arg("reconcile")
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
        let blocked = harness
            .command()
            .args(&args)
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
    let blocked = harness
        .command()
        .env("LKIT_INTERNAL_DAEMON_TTY", &update_tty.slave_path)
        .arg("update")
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
    let bypass = harness
        .command()
        .arg("reconcile")
        .arg("--repository")
        .arg(&harness.repository.base_url)
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

    let bypass_repair = harness
        .command()
        .arg("repair")
        .arg("static")
        .arg("--repository")
        .arg(&harness.repository.base_url)
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
    let fixed = harness
        .command()
        .arg("reconcile")
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
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("config-preset", "healthy", 10_000);
    let config_path = harness.config_path();
    let preset = format!(
        "schema_version = 1\n\n[repository]\nkind = \"http\"\nlocation = \"{}\"\n",
        harness.repository.base_url
    );
    std::fs::write(&config_path, &preset).unwrap();

    let output = harness
        .command()
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
    let state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(harness.state_path()).unwrap()).unwrap();
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
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("config-bypass", "healthy", 10_000);
    let config_path = harness.config_path();
    let other = RepositoryServer::start(repository_files_for(VERSION));
    let preset = format!(
        "schema_version = 1\n\n[repository]\nkind = \"http\"\nlocation = \"{}\"\n",
        other.base_url
    );
    std::fs::write(&config_path, &preset).unwrap();

    assert_success(&harness.run());

    // 同版本诊断使用显式来源,完全绕过配置:预设的 other 服务器不应收到任何请求,
    // 配置字节保持不变;诊断基于安装记录核对显式来源的资产身份。
    let reconcile = harness
        .command()
        .arg("reconcile")
        .arg("--repository")
        .arg(&harness.repository.base_url)
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
    let refused = harness
        .command()
        .arg("reconcile")
        .arg("--repository")
        .arg(&drifted.base_url)
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
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("latest-no-stable", "healthy", 10_000);
    assert_success(&harness.run());
    let transactions_before = transaction_count(&harness.territory);

    // 指向一个没有 stable 渠道的仓库:latest 解析返回 None,必须报错而不是静默成功。
    let mut files = repository_files_for(VERSION);
    files.remove("/channels/stable.json");
    let empty = RepositoryServer::start(files);

    let output = harness
        .command()
        .arg("switch")
        .arg("--version")
        .arg("latest")
        .arg("--repository")
        .arg(&empty.base_url)
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
        transaction_count(&harness.territory),
        transactions_before,
        "switch must not create a transaction"
    );
}

#[test]
fn cleans_up_after_fixture_health_failure() {
    if !e2e_enabled() {
        return;
    }
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
        !harness.state_path().exists(),
        "a failed first install must not leave install-state.json"
    );
    assert_eq!(
        std::fs::read(harness.host.join("resolv.conf")).unwrap(),
        b"nameserver 127.0.0.1\n"
    );
}

/// REC-02:受管 unit 原件内容变化后,reconcile 检测到差异;
/// `--accept-service-change` 接受修改并更新状态记录,服务保持运行。
#[test]
fn reconcile_accepts_a_modified_service_unit() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("reconcile-modified-unit", "healthy", 10_000);
    assert_success(&harness.run());
    let origin = harness
        .install_root
        .join("service/landscape-router.service");
    let mut content = std::fs::read_to_string(&origin).unwrap();
    content.push_str("# operator modification\n");
    std::fs::write(&origin, content).unwrap();

    let output = harness
        .command()
        .args(["reconcile", "--accept-service-change", "--test-runtime"])
        .arg(&harness.runtime_config)
        .output()
        .unwrap();
    assert_success(&output);
    assert!(
        std::fs::read_to_string(&origin)
            .unwrap()
            .ends_with("# operator modification\n"),
        "the accepted modification must be kept"
    );
    let state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(harness.state_path()).unwrap()).unwrap();
    let recorded = state["service"]["definition_sha256"].as_str().unwrap();
    let mut hasher = sha2::Sha256::new();
    hasher.update(std::fs::read_to_string(&origin).unwrap().as_bytes());
    let expected = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    assert_eq!(
        recorded, expected,
        "reconcile must record the accepted unit definition hash"
    );
    let active = systemctl(&harness.world, &["is-active", "landscape-router.service"]);
    assert_eq!(String::from_utf8_lossy(&active.stdout).trim(), "active");
}
