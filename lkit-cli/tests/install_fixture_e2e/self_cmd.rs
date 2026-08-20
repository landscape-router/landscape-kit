use std::time::{Duration, Instant};

use super::support::{
    E2E_LOCK, InstallHarness, LKIT, SelfUpgradeFixture, assert_success, e2e_enabled, systemctl,
};

/// `lkit self install` 把 lkit 注册为全局常驻服务(docs/commands/self.md):
/// unit 原件渲染到 `/usr/local/lib/lkit/lkit.service`
/// (`ExecStart=/usr/local/bin/lkit daemon`),注册链接指向全局原件,启用并启动;
/// daemon 写 pidfile 到 lkit 地盘 `run/lkit.pid`。`lkit self remove` 停止、
/// 注销并删除原件,幂等可重复。`self` 命令不接收 `--install-dir`。
#[test]
fn self_installs_and_removes_the_lkit_service() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("self", "healthy", 30_000);
    harness.seed_global_lkit_binary();

    let installed = harness
        .command()
        .args(["self", "install", "--test-runtime"])
        .arg(&harness.runtime_config)
        .output()
        .unwrap();
    assert_success(&installed);

    let origin = harness.host.join("usr/local/lib/lkit/lkit.service");
    let unit = std::fs::read_to_string(&origin).unwrap();
    let expected_exec = harness.host.join("usr/local/bin/lkit");
    assert!(
        unit.contains(&format!("ExecStart={} daemon", expected_exec.display())),
        "unit ExecStart must point at the global lkit binary: {unit}"
    );
    assert!(unit.contains("User=root"));
    assert!(unit.contains("Restart=always"));
    assert!(unit.contains("WantedBy=multi-user.target"));

    // 注册链接:/etc/systemd/system/lkit.service → 全局原件(fixture 世界映射到
    // host/units/),启用并启动。
    let link = harness.host.join("units/lkit.service");
    assert!(
        link.is_symlink(),
        "the registration link must point at the global unit origin"
    );
    let active = systemctl(&harness.world, &["is-active", "lkit.service"]);
    assert_eq!(String::from_utf8_lossy(&active.stdout).trim(), "active");
    let enabled = systemctl(&harness.world, &["is-enabled", "lkit.service"]);
    assert_eq!(String::from_utf8_lossy(&enabled.stdout).trim(), "enabled");

    // daemon 写 pidfile 到 lkit 地盘 run/ 且进程真实存活。
    let pidfile = harness.run_dir().join("lkit.pid");
    let started = Instant::now();
    let pid: u32 = loop {
        if let Ok(content) = std::fs::read_to_string(&pidfile)
            && let Ok(pid) = content.trim().parse::<u32>()
        {
            break pid;
        }
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "lkit daemon must write its pidfile"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(
        process_alive(pid),
        "lkit daemon must be a real running process"
    );

    let calls = std::fs::read_to_string(harness.world.path("systemctl-calls.jsonl")).unwrap();
    assert!(calls.contains("\"enable\",\"lkit.service\""), "{calls}");
    assert!(calls.contains("\"start\",\"lkit.service\""), "{calls}");

    let removed = harness
        .command()
        .args(["self", "remove", "--test-runtime"])
        .arg(&harness.runtime_config)
        .output()
        .unwrap();
    assert_success(&removed);

    assert!(!process_alive(pid), "lkit daemon must exit after removal");
    assert!(!origin.exists(), "unit origin must be removed");
    assert!(!link.exists(), "registration link must be removed");
    let inactive = systemctl(&harness.world, &["is-active", "lkit.service"]);
    assert_eq!(String::from_utf8_lossy(&inactive.stdout).trim(), "inactive");
}

/// `self upgrade` 下载、校验、自检并原子替换全局二进制;运行中的 daemon
/// 必须由 fake systemd 重启并实际从新文件启动。
#[test]
fn self_upgrade_replaces_binary_and_restarts_active_daemon() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("self-upgrade", "healthy", 30_000);
    harness.seed_global_lkit_binary();
    let installed = harness
        .command()
        .args(["self", "install", "--test-runtime"])
        .arg(&harness.runtime_config)
        .output()
        .unwrap();
    assert_success(&installed);
    let old_pid = wait_for_pid(&harness, None);

    let tag = "v9.9.9";
    let marker = harness.world.path("upgrade-daemon-marker");
    let fixture =
        SelfUpgradeFixture::start(tag, upgrade_asset_name(), upgrade_asset(tag, &marker), None);
    let upgraded = run_self_upgrade(&harness, &fixture, tag);
    assert_success(&upgraded);

    let new_pid = wait_for_pid(&harness, Some(old_pid));
    assert!(!process_alive(old_pid), "the old daemon must be stopped");
    assert!(process_alive(new_pid), "the restarted daemon must be alive");
    assert!(
        marker.exists(),
        "the replacement binary must start the daemon"
    );

    let binary = harness.host.join("usr/local/bin/lkit");
    assert!(
        !binary.is_symlink(),
        "upgrade must replace the seeded symlink"
    );
    assert!(
        is_executable(&binary),
        "the upgraded binary must be executable"
    );
    let version = std::process::Command::new(&binary)
        .arg("--version")
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&version.stdout).trim(),
        "lkit 9.9.9"
    );

    let calls = std::fs::read_to_string(harness.world.path("systemctl-calls.jsonl")).unwrap();
    assert!(calls.contains("\"restart\",\"lkit.service\""), "{calls}");
    assert_eq!(
        fixture.api.request_paths(),
        vec![format!(
            "/repos/landscape-router/landscape-kit/releases/tags/{tag}"
        )]
    );
    assert_eq!(
        fixture.downloads.request_paths(),
        vec![
            format!("/{tag}/SHA256SUMS"),
            format!("/{tag}/{}", upgrade_asset_name())
        ]
    );
    assert!(!has_upgrade_temp_file(&harness));

    let removed = harness
        .command()
        .args(["self", "remove", "--test-runtime"])
        .arg(&harness.runtime_config)
        .output()
        .unwrap();
    assert_success(&removed);
}

/// 同版本升级只读取 Release 元数据,不下载资产、不替换二进制,也不重启 daemon。
#[test]
fn self_upgrade_same_version_is_a_noop() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("self-upgrade-same", "healthy", 30_000);
    harness.seed_global_lkit_binary();
    let tag = format!("v{}", env!("CARGO_PKG_VERSION"));
    let fixture = SelfUpgradeFixture::start(&tag, upgrade_asset_name(), b"unused".to_vec(), None);
    let before = std::fs::read_link(harness.host.join("usr/local/bin/lkit")).unwrap();
    let output = run_self_upgrade(&harness, &fixture, &tag);
    assert_success(&output);
    assert!(String::from_utf8_lossy(&output.stdout).contains(env!("CARGO_PKG_VERSION")));
    assert_eq!(
        std::fs::read_link(harness.host.join("usr/local/bin/lkit")).unwrap(),
        before
    );
    assert_eq!(fixture.downloads.request_paths(), Vec::<String>::new());
    assert!(!has_upgrade_temp_file(&harness));
}

/// checksum 错误时保留原二进制并清理不完整暂存文件。
#[test]
fn self_upgrade_checksum_failure_preserves_binary() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("self-upgrade-checksum", "healthy", 30_000);
    harness.seed_global_lkit_binary();
    let tag = "v9.9.8";
    let invalid_checksum = "0".repeat(64);
    let fixture = SelfUpgradeFixture::start(
        tag,
        upgrade_asset_name(),
        upgrade_asset(tag, &harness.world.path("unused-marker")),
        Some(&invalid_checksum),
    );
    let output = run_self_upgrade(&harness, &fixture, tag);
    assert!(!output.status.success());
    assert!(
        harness.host.join("usr/local/bin/lkit").is_symlink(),
        "checksum failure must preserve the original binary"
    );
    assert!(!has_upgrade_temp_file(&harness));
}

/// 下载成功但版本自检失败时保留原二进制并清理暂存文件。
#[test]
fn self_upgrade_self_check_failure_preserves_binary() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("self-upgrade-self-check", "healthy", 30_000);
    harness.seed_global_lkit_binary();
    let tag = "v9.9.7";
    let fixture = SelfUpgradeFixture::start(
        tag,
        upgrade_asset_name(),
        b"#!/bin/sh\nprintf 'not lkit\\n'\n".to_vec(),
        None,
    );
    let output = run_self_upgrade(&harness, &fixture, tag);
    assert!(!output.status.success());
    assert!(
        harness.host.join("usr/local/bin/lkit").is_symlink(),
        "self-check failure must preserve the original binary"
    );
    assert!(!has_upgrade_temp_file(&harness));
}

/// daemon 未注册时仅更新 CLI 并输出安装提示,不尝试 restart。
#[test]
fn self_upgrade_without_registered_daemon_updates_cli_only() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("self-upgrade-unregistered", "healthy", 30_000);
    harness.seed_global_lkit_binary();
    let tag = "v9.9.6";
    let fixture = SelfUpgradeFixture::start(
        tag,
        upgrade_asset_name(),
        upgrade_asset(tag, &harness.world.path("unregistered-marker")),
        None,
    );
    let output = run_self_upgrade(&harness, &fixture, tag);
    assert_success(&output);
    assert!(String::from_utf8_lossy(&output.stdout).contains("self install"));
    let calls = std::fs::read_to_string(harness.world.path("systemctl-calls.jsonl")).unwrap();
    assert!(!calls.contains("\"restart\",\"lkit.service\""), "{calls}");
    let binary = harness.host.join("usr/local/bin/lkit");
    assert!(!binary.is_symlink());
    assert_eq!(
        String::from_utf8_lossy(
            &std::process::Command::new(&binary)
                .arg("--version")
                .output()
                .unwrap()
                .stdout
        )
        .trim(),
        "lkit 9.9.6"
    );
}

fn run_self_upgrade(
    harness: &InstallHarness,
    fixture: &SelfUpgradeFixture,
    tag: &str,
) -> std::process::Output {
    harness
        .command()
        .args([
            "self",
            "upgrade",
            "--version",
            tag,
            "--test-release-api-root",
            fixture.api.base_url.as_str(),
            "--test-release-download-root",
            fixture.downloads.base_url.as_str(),
            "--test-runtime",
        ])
        .arg(&harness.runtime_config)
        .output()
        .unwrap()
}

fn upgrade_asset_name() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "lkit-x86_64",
        "aarch64" => "lkit-aarch64",
        architecture => panic!("unsupported test architecture {architecture}"),
    }
}

fn upgrade_asset(version: &str, marker: &std::path::Path) -> Vec<u8> {
    let version = version.strip_prefix('v').unwrap_or(version);
    format!(
        "#!/bin/sh\ncase \"$1\" in\n  --version) printf 'lkit {version}\\n'; exit 0 ;;\n  *) printf '%s\\n' \"$$\" >> {}; exec {} \"$@\" ;;\nesac\n",
        shell_quote(&marker.display().to_string()),
        shell_quote(LKIT),
    )
    .into_bytes()
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn wait_for_pid(harness: &InstallHarness, previous: Option<u32>) -> u32 {
    let started = Instant::now();
    loop {
        if let Ok(content) = std::fs::read_to_string(harness.run_dir().join("lkit.pid"))
            && let Ok(pid) = content.trim().parse::<u32>()
            && previous.is_none_or(|old| old != pid)
            && process_alive(pid)
        {
            return pid;
        }
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "lkit daemon did not reach the expected pid state"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn has_upgrade_temp_file(harness: &InstallHarness) -> bool {
    std::fs::read_dir(harness.host.join("usr/local/bin"))
        .unwrap()
        .filter_map(Result::ok)
        .any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".lkit.upgrade.")
        })
}

fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    std::fs::metadata(path)
        .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// 参数错误路径:systemd 不可用时返回退出码 2,不遗留任何现场
/// (unit 原件与 daemon pidfile 均不得存在)。
#[test]
fn self_install_rejects_unavailable_systemd() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("self-reject", "healthy", 30_000);
    harness.seed_global_lkit_binary();

    // systemd 不可用:改写测试运行时的 systemctl 路径指向不存在文件。
    let mut runtime: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&harness.runtime_config).unwrap()).unwrap();
    runtime["systemd"]["systemctl"] = serde_json::Value::String(
        harness
            .world
            .path("missing-systemctl")
            .display()
            .to_string(),
    );
    let broken_runtime = harness.world.path("broken-runtime.json");
    std::fs::write(
        &broken_runtime,
        serde_json::to_vec_pretty(&runtime).unwrap(),
    )
    .unwrap();

    let unavailable = harness
        .command()
        .args(["self", "install", "--test-runtime"])
        .arg(&broken_runtime)
        .output()
        .unwrap();
    assert_eq!(
        unavailable.status.code(),
        Some(2),
        "unavailable systemd must be a usage error: {}",
        String::from_utf8_lossy(&unavailable.stderr)
    );
    assert!(
        !harness
            .host
            .join("usr/local/lib/lkit/lkit.service")
            .exists(),
        "the unit origin must not be left behind"
    );
    assert!(
        !harness.run_dir().join("lkit.pid").exists(),
        "no daemon may be started"
    );
}

fn process_alive(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

/// `lkit self install --flare-psk-file` 在 daemon 启动前把 flare psk 写入地盘
/// `config.toml` 的 `[flare]` 段(0600),daemon 首启即用该 psk 托管 flare 服务。
#[test]
fn self_install_provisions_the_flare_psk_before_starting_the_daemon() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("self-flare", "healthy", 30_000);
    harness.seed_global_lkit_binary();
    let psk_file = harness.world.path("flare-psk");
    std::fs::write(&psk_file, b"fixture-recovery-secret\n").unwrap();
    std::fs::set_permissions(&psk_file, std::fs::Permissions::from_mode(0o600)).unwrap();

    let installed = harness
        .command()
        .args([
            "self",
            "install",
            "--flare-psk-file",
            psk_file.to_str().unwrap(),
            "--test-runtime",
        ])
        .arg(&harness.runtime_config)
        .output()
        .unwrap();
    assert_success(&installed);

    let config = std::fs::read_to_string(harness.config_path()).unwrap();
    assert!(
        config.contains("[flare]"),
        "self install must write the [flare] section: {config}"
    );
    assert!(
        config.contains("fixture-recovery-secret"),
        "self install must persist the provided flare psk: {config}"
    );
    let metadata = std::fs::metadata(harness.config_path()).unwrap();
    assert_eq!(
        metadata.mode() & 0o077,
        0,
        "config.toml holding the flare psk must be root-only"
    );

    // 既有 psk 不被覆盖:再跑一次无 flag 的 self install,配置保持不变。
    let before = std::fs::read(harness.config_path()).unwrap();
    let again = harness
        .command()
        .args(["self", "install", "--test-runtime"])
        .arg(&harness.runtime_config)
        .output()
        .unwrap();
    assert_success(&again);
    assert_eq!(
        std::fs::read(harness.config_path()).unwrap(),
        before,
        "a repeated self install without a psk must not touch the config"
    );

    let removed = harness
        .command()
        .args(["self", "remove", "--test-runtime"])
        .arg(&harness.runtime_config)
        .output()
        .unwrap();
    assert_success(&removed);
}
