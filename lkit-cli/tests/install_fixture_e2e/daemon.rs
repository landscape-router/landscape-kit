use std::os::fd::AsRawFd;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::support::{E2E_LOCK, InstallHarness, LKIT, VERSION, e2e_enabled, write_valid_state_at};

/// daemon 周期恢复:CLI 进程消失后遗留的未完成事务由 daemon 自动按
/// `recover_interrupted` 语义恢复(SSH 断开、崩溃等场景的续跑/回滚)。
/// 本测试直接构造一个 `activating` 阶段的 install 事务(模拟 CLI 死于
/// 激活中途),断言 daemon 完成清理并标记 `failed`。daemon 固定读取 lkit
/// 地盘(docs/commands/self.md):pidfile 写地盘 `run/lkit.pid`,恢复目标从
/// 地盘的状态与事务发现 landscape 根。
#[test]
fn daemon_recovers_an_interrupted_install_transaction() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("daemon-recover", "healthy", 30_000);
    let canonical = prepare_interrupted_install(&harness);

    let mut daemon = spawn_daemon(&harness);
    let transaction_file = harness.transactions_dir().join("t.json");
    wait_for(
        || {
            let Ok(content) = std::fs::read_to_string(&transaction_file) else {
                return false;
            };
            content.contains("\"phase\": \"failed\"")
                && !canonical.join("releases").join(VERSION).exists()
                && !canonical.join("current").exists()
                && !canonical.join("data/landscape_init.toml").exists()
        },
        "daemon must recover the interrupted install transaction",
    );
    assert!(
        harness.run_dir().join("lkit.pid").is_file(),
        "daemon must keep running after recovery"
    );

    terminate(&mut daemon);
    assert!(
        !harness.run_dir().join("lkit.pid").exists(),
        "daemon must remove its pidfile on shutdown"
    );
}

/// daemon 尊重安装锁:CLI 命令持有锁期间,daemon 不触碰事务;
/// 锁释放后下一个周期才执行恢复。
#[test]
fn daemon_defers_while_the_install_lock_is_held() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("daemon-lock", "healthy", 30_000);
    prepare_interrupted_install(&harness);

    std::fs::create_dir_all(harness.run_dir()).unwrap();
    let lock_file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(harness.run_dir().join("install.lock"))
        .unwrap();
    let locked = unsafe { libc::flock(lock_file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    assert_eq!(locked, 0, "test must hold the install lock");

    let mut daemon = spawn_daemon(&harness);
    let transaction_file = harness.transactions_dir().join("t.json");
    std::thread::sleep(Duration::from_secs(7));
    let content = std::fs::read_to_string(&transaction_file).unwrap();
    assert!(
        content.contains("\"phase\": \"activating\""),
        "daemon must not recover while the lock is held"
    );

    drop(lock_file);
    wait_for(
        || {
            std::fs::read_to_string(&transaction_file)
                .map(|content| content.contains("\"phase\": \"failed\""))
                .unwrap_or(false)
        },
        "daemon must recover after the lock is released",
    );

    terminate(&mut daemon);
}

/// DAE-03:网络接管待确认阶段保持人工处理——daemon 周期跳过
/// `awaiting_network_confirmation` 事务,不代替用户确认、不触碰现场。
#[test]
fn daemon_never_acts_on_pending_network_confirmation() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("daemon-pending-takeover", "healthy", 30_000);
    let root = prepare_interrupted_install(&harness);

    let transaction_file = harness.transactions_dir().join("t.json");
    let mut transaction: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&transaction_file).unwrap()).unwrap();
    transaction["phase"] = "awaiting_network_confirmation".into();
    transaction["network_takeover"] = serde_json::json!({
        "plan": {
            "mode": "wan_dhcp",
            "wan": "ens3",
            "selected_macs": [{"name": "ens3", "mac": "52:54:00:12:34:01"}],
        },
        "host_services": [],
        "confirmation_deadline": chrono::Utc::now().to_rfc3339(),
        "rollback_service": "lkit-network-tx-rollback.service",
        "rollback_timer": "lkit-network-tx-rollback.timer",
        "boot_rollback_service": "lkit-network-tx-boot-rollback.service",
        "recovery_binary": "service/lkit-network-recovery",
        "pending_state": format!(
            "transactions/{}/pending-install-state.json",
            transaction["transaction_id"].as_str().unwrap()
        ),
    });
    std::fs::write(
        &transaction_file,
        serde_json::to_vec_pretty(&transaction).unwrap(),
    )
    .unwrap();
    assert!(
        root.join("current").exists(),
        "the interrupted install scene must stay intact"
    );

    let mut daemon = spawn_daemon(&harness);
    std::thread::sleep(Duration::from_secs(7));
    let transaction: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&transaction_file).unwrap()).unwrap();
    assert_eq!(
        transaction["phase"], "awaiting_network_confirmation",
        "daemon must never touch a transaction awaiting network confirmation"
    );
    assert!(
        process_alive(daemon.id()),
        "daemon must stay alive while deferring"
    );
    terminate(&mut daemon);
}

/// 构造中断现场:landscape 根(install_root)有暂存的 release/current/data;
/// lkit 地盘有有效安装状态(active_version 与目标不同,确保恢复走回滚而不是
/// 判定已完成)、进行中事务与事务日志。
fn prepare_interrupted_install(harness: &InstallHarness) -> std::path::PathBuf {
    let root = &harness.install_root;
    std::fs::create_dir_all(root).unwrap();
    let canonical = std::fs::canonicalize(root).unwrap();

    std::fs::create_dir_all(canonical.join("releases").join(VERSION)).unwrap();
    std::fs::write(
        canonical
            .join("releases")
            .join(VERSION)
            .join("landscape-webserver"),
        b"staged backend",
    )
    .unwrap();
    std::os::unix::fs::symlink(format!("releases/{VERSION}"), canonical.join("current")).unwrap();
    std::fs::create_dir_all(canonical.join("data")).unwrap();
    std::fs::write(
        canonical.join("data/landscape_init.toml"),
        b"version = \"x\"\n",
    )
    .unwrap();

    write_valid_state_at(&harness.state_path(), &canonical, "1.2.4");
    std::fs::create_dir_all(harness.logs_dir()).unwrap();
    std::fs::write(harness.logs_dir().join("t.log"), b"phase: activating\n").unwrap();
    std::fs::create_dir_all(harness.transactions_dir()).unwrap();
    let now = chrono::Utc::now().to_rfc3339();
    let transaction = serde_json::json!({
        "schema_version": 4,
        "transaction_id": "t",
        "operation": "install",
        "phase": "activating",
        "install_root": canonical.display().to_string(),
        "canonical_install_root": canonical.display().to_string(),
        "from_version": null,
        "target_version": VERSION,
        "from_service_manager": null,
        "target_service_manager": null,
        "previous_current": null,
        "target_release": format!("releases/{VERSION}"),
        "backup": null,
        "restore_backup": null,
        "no_backup": false,
        "static_backup": null,
        "systemd_before": null,
        "resolv_conf_backup": null,
        "network_takeover": null,
        "legacy_unit": null,
        "log_path": "logs/t.log",
        "started_at": now,
        "updated_at": now,
    });
    std::fs::write(
        harness.transactions_dir().join("t.json"),
        serde_json::to_vec_pretty(&transaction).unwrap(),
    )
    .unwrap();
    canonical
}

fn spawn_daemon(harness: &InstallHarness) -> std::process::Child {
    Command::new(LKIT)
        .env("LKIT_TERRITORY", &harness.territory)
        .arg("daemon")
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap()
}

fn wait_for(mut condition: impl FnMut() -> bool, message: &str) {
    let started = Instant::now();
    while !condition() {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "timeout: {message}"
        );
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn terminate(daemon: &mut std::process::Child) {
    unsafe {
        libc::kill(daemon.id() as libc::pid_t, libc::SIGTERM);
    }
    let status = daemon.wait().unwrap();
    assert!(status.success(), "daemon must exit cleanly on SIGTERM");
}

fn process_alive(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}
