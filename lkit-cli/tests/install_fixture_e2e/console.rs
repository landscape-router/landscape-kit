use std::io::Write;
use std::process::Command;
use std::time::Duration;

use super::support::*;

#[test]
fn update_interaction_handles_defaults_cancellation_and_non_interactive_mode() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("update-interaction", "healthy", 10_000);
    assert_success(&harness.run());

    // 预置有效配置后,update 渠道选择的首个默认选项是配置中的"当前来源"。
    let config_path = harness.config_path();
    let preset = format!(
        "schema_version = 1\n\n[repository]\nkind = \"http\"\nlocation = \"{}\"\n",
        harness.repository.base_url
    );
    std::fs::write(&config_path, &preset).unwrap();

    let state_path = harness.state_path();
    let original_state = std::fs::read(&state_path).unwrap();
    let original_current = std::fs::read_link(harness.install_root.join("current")).unwrap();
    let original_transaction_count = transaction_count(&harness.territory);

    let mut default_tty = Pty::open();
    default_tty.master.write_all(b"\n").unwrap();
    let default_output = harness
        .update_command()
        .env("LKIT_INTERNAL_DAEMON_TTY", &default_tty.slave_path)
        .output()
        .unwrap();
    assert_success(&default_output);
    let default_stderr = String::from_utf8_lossy(&default_output.stderr);
    assert!(default_stderr.contains("Select the repository source for the update"));
    assert!(default_stderr.contains("Current source (http:"));
    assert!(default_stderr.contains("Official GitHub repository"));
    assert!(default_stderr.contains("Default HTTP mirror"));
    assert!(default_stderr.contains("Custom HTTP repository"));
    assert!(default_stderr.contains("Select an option [1]:"));
    assert!(!default_stderr.contains("interface"));
    assert!(String::from_utf8_lossy(&default_output.stdout).contains("already up to date"));
    assert_eq!(
        std::fs::read(&config_path).unwrap(),
        preset.as_bytes(),
        "update must not modify the config file"
    );

    let explicit_tty = Pty::open();
    let explicit_output = harness
        .update_command()
        .env("LKIT_INTERNAL_DAEMON_TTY", &explicit_tty.slave_path)
        .arg("--repository")
        .arg(&harness.repository.base_url)
        .output()
        .unwrap();
    assert_success(&explicit_output);
    assert!(
        !String::from_utf8_lossy(&explicit_output.stderr)
            .contains("Select the repository source for the update")
    );

    let newer_repository = RepositoryServer::start(repository_files_for("1.2.4"));
    let mut cancel_tty = Pty::open();
    cancel_tty.master.write_all(b"no\n").unwrap();
    let cancel_output = harness
        .update_command()
        .env("LKIT_INTERNAL_DAEMON_TTY", &cancel_tty.slave_path)
        .arg("--repository")
        .arg(&newer_repository.base_url)
        .output()
        .unwrap();
    assert_eq!(cancel_output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&cancel_output.stderr).contains("1.2.3 -> 1.2.4"));
    assert!(String::from_utf8_lossy(&cancel_output.stdout).contains("update cancelled"));
    assert_eq!(std::fs::read(&state_path).unwrap(), original_state);
    assert_eq!(
        std::fs::read_link(harness.install_root.join("current")).unwrap(),
        original_current
    );
    assert_eq!(
        transaction_count(&harness.territory),
        original_transaction_count
    );
    let requests = newer_repository.request_paths();
    assert!(
        requests
            .iter()
            .any(|path| path == "/releases/1.2.4/manifest.json")
    );
    assert!(
        !requests
            .iter()
            .any(|path| { path.ends_with(".zst") || path.ends_with("static.zip") }),
        "update downloaded assets before confirmation: {requests:?}"
    );

    let non_interactive_output = harness
        .update_command()
        .arg("--non-interactive")
        .output()
        .unwrap();
    assert_eq!(non_interactive_output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&non_interactive_output.stderr)
            .contains("use `lkit switch --version <VERSION>")
    );

    // 控制台分发路径:--console-confirmed 跳过 /dev/tty,解析与比较照常进行。
    let console_output = harness
        .update_command()
        .arg("--console-confirmed")
        .arg("--repository")
        .arg(&harness.repository.base_url)
        .output()
        .unwrap();
    assert_success(&console_output);
    let console_stderr = String::from_utf8_lossy(&console_output.stderr);
    assert!(
        !console_stderr.contains("Select the repository source for the update"),
        "console-confirmed update must not open the repository prompt: {console_stderr}"
    );
    assert!(String::from_utf8_lossy(&console_output.stdout).contains("already up to date"));
    assert_eq!(
        std::fs::read(&config_path).unwrap(),
        preset.as_bytes(),
        "console-confirmed update must not modify the config file"
    );
}

#[test]
fn ctrl_c_during_password_restores_terminal_echo() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("password-sigint", "healthy", 10_000);
    let mut pty = Pty::open();
    assert!(pty.echo_enabled());
    let mut child = harness.password_prompt_command(&pty).spawn().unwrap();
    let output = pty.read_until("Enter admin password: ", Duration::from_secs(10));
    assert!(!pty.echo_enabled(), "password input did not disable echo");
    assert_eq!(
        unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGINT) },
        0
    );
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(130), "pty output:\n{output}");
    assert!(pty.echo_enabled(), "Ctrl+C did not restore terminal echo");
}

#[test]
fn explicit_non_interactive_mode_ignores_available_tty() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let harness = InstallHarness::new("explicit-non-interactive", "healthy", 10_000);
    let mut pty = Pty::open();
    let mut command = harness.password_prompt_command(&pty);
    command.arg("--non-interactive");
    let mut child = command.spawn().unwrap();
    let output = pty.read_until(
        "--password-file is required in non-interactive mode",
        Duration::from_secs(10),
    );
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(2), "pty output:\n{output}");
    assert!(!output.contains("Enter admin password"));
    assert!(pty.echo_enabled());
}

fn bare_console_command() -> (Command, TestWorld) {
    let world = TestWorld::new("bare-console");
    let mut command = Command::new(LKIT);
    command.env("LKIT_TERRITORY", world.path("territory"));
    (command, world)
}

#[test]
fn bare_lkit_console_restores_terminal_on_exit() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let mut pty = Pty::open();
    let (mut command, _world) = bare_console_command();
    attach_pty(&mut command, &pty);
    let mut child = command.spawn().unwrap();
    let entered = pty.read_until("Landscape Kit", Duration::from_secs(5));
    assert!(
        entered.contains("\x1b[?1049h"),
        "console did not enter alternate screen: {entered:?}"
    );
    pty.master.write_all(b"\x1b").unwrap();
    let armed = pty.read_until("Exit armed", Duration::from_secs(5));
    assert!(
        child.try_wait().unwrap().is_none(),
        "console exited after one Esc: {armed:?}"
    );
    assert!(!armed.contains("Confirm exit"));
    pty.master.write_all(b"\x1b").unwrap();
    let confirmation = pty.read_until("Confirm exit", Duration::from_secs(5));
    assert!(
        child.try_wait().unwrap().is_none(),
        "console exited while showing confirmation: {confirmation:?}"
    );
    pty.master.write_all(b"\r").unwrap();
    let exited = pty.read_until("\x1b[?1049l", Duration::from_secs(5));
    let status = child.wait().unwrap();
    assert!(status.success(), "console exit failed: {exited:?}");
    assert!(
        pty.echo_enabled(),
        "console exit did not restore terminal echo"
    );
}

#[test]
fn ctrl_c_leaves_bare_lkit_console_and_restores_terminal() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let mut pty = Pty::open();
    let (mut command, _world) = bare_console_command();
    attach_pty(&mut command, &pty);
    let mut child = command.spawn().unwrap();
    let entered = pty.read_until("Landscape Kit", Duration::from_secs(5));
    assert!(
        entered.contains("\x1b[?1049h"),
        "console did not enter alternate screen: {entered:?}"
    );
    assert_eq!(
        unsafe { libc::kill(child.id() as libc::pid_t, libc::SIGINT) },
        0
    );
    let exited = pty.read_until("\x1b[?1049l", Duration::from_secs(5));
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(130), "console output: {exited:?}");
    assert!(
        pty.echo_enabled(),
        "console Ctrl+C did not restore terminal echo"
    );
}

#[test]
fn language_toggle_persists_across_console_sessions() {
    if !e2e_enabled() {
        return;
    }
    let _guard = E2E_LOCK.lock().unwrap_or_else(|error| error.into_inner());
    let (mut command, world) = bare_console_command();
    let config_path = world.path("territory").join("config.toml");
    assert!(
        !config_path.exists(),
        "the world must start without a config"
    );

    let mut pty = Pty::open();
    command.env("LKIT_LANG", "en");
    attach_pty(&mut command, &pty);
    let mut child = command.spawn().unwrap();
    // 底栏显示目标语言;只锚定纯 ASCII 前缀,全角字符在 ratatui 增量 diff 的
    // pty 字节流中可能不连续(见下方 "English" 锚点说明)。
    pty.read_until("[L] Switch to", Duration::from_secs(10));
    pty.master.write_all(b"l").unwrap();
    // 切换后的中文底栏是 "[L] 切换到 English (en)";目标语言名是切换后唯一
    // 新出现的连续 ASCII 锚点(中文 UI 不再包含 "zh" 字样,全角文本在增量
    // diff 字节流中不连续)。
    pty.read_until("English", Duration::from_secs(5));
    let config = std::fs::read_to_string(&config_path).unwrap();
    assert!(config.contains("[ui]"), "config: {config}");
    assert!(config.contains("language = \"zh\""), "config: {config}");
    // Esc 需要独立送达:连续写入的 ESC ESC 只触发一次 armed(与
    // bare_lkit_console_restores_terminal_on_exit 相同的输入时序约束)。
    // 本会话已切换为中文,武装提示是本地化文本且不含纯 ASCII 的独特锚点
    // (唯一的 ASCII 片段 "Esc" 在武装前的导航提示里也出现),改用固定间隔
    // 送达第一次 Esc,并以确认层的 "Enter"(按 Enter 退出,中文导航提示
    // 不含该词)确认第二段武装生效。
    pty.master.write_all(b"\x1b").unwrap();
    std::thread::sleep(Duration::from_millis(300));
    pty.master.write_all(b"\x1b").unwrap();
    pty.read_until("Enter", Duration::from_secs(5));
    std::thread::sleep(Duration::from_millis(200));
    pty.master.write_all(b"\r").unwrap();
    let exited = pty.read_until("\x1b[?1049l", Duration::from_secs(5));
    let status = child.wait().unwrap();
    assert!(status.success(), "console exit failed: {exited:?}");

    // 第二个会话没有 --lang/LKIT_LANG 与中文 locale,配置预设决定语言。
    let mut command = Command::new(LKIT);
    command.env("LKIT_TERRITORY", world.path("territory"));
    command.env_remove("LKIT_LANG");
    command.env("LC_ALL", "C");
    let mut pty = Pty::open();
    attach_pty(&mut command, &pty);
    let mut child = command.spawn().unwrap();
    // 中文底栏 "[L] 切换到 English (en)" 含连续 ASCII 的目标语言名;
    // 英文底栏不含 "English",可区分两种语言态。
    pty.read_until("English", Duration::from_secs(10));
    pty.master.write_all(b"l").unwrap();
    // 英文态底栏 "[L] Switch to 中文 (zh)" 不含稳定的 ASCII 锚点(全角文本
    // 在增量 diff 字节流中不连续),这里以 config 写回为切换完成的稳定信号
    // (第一会话已用 "English" 验证过切换生效的 UI 反馈)。
    for _ in 0..50 {
        if std::fs::read_to_string(&config_path)
            .map(|config| config.contains("language = \"en\""))
            .unwrap_or(false)
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let config = std::fs::read_to_string(&config_path).unwrap();
    assert!(config.contains("language = \"en\""), "config: {config}");
    pty.master.write_all(b"\x1b").unwrap();
    pty.read_until("Exit armed", Duration::from_secs(5));
    pty.master.write_all(b"\x1b").unwrap();
    std::thread::sleep(Duration::from_millis(200));
    pty.master.write_all(b"\r").unwrap();
    let exited = pty.read_until("\x1b[?1049l", Duration::from_secs(5));
    let status = child.wait().unwrap();
    assert!(status.success(), "console exit failed: {exited:?}");
}
