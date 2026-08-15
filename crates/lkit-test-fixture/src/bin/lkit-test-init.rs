use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use anyhow::{Context, Result};
use lkit_test_fixture::{INIT_CONFIG_ENV, InitFixtureConfig};

/// 多角色 init 系统测试替身。按 argv[0] 分派:
/// - `rc-service`:解析 `/etc/init.d/<name>` 的 command/command_args 并真实
///   spawn/停止进程,pid 记录在 state dir;
/// - `rc-update`:add/del/show 维护 enabled 标记;
/// - `update-rc.d`:enable/disable 维护 `/etc/rc?.d` 的 S 链接;
/// - `start-stop-daemon`:`--start/--stop` 真实 spawn/杀进程并维护 pidfile。
///
/// 使用方式:在测试环境中把上述名称符号链接到本二进制,并设置
/// `LKIT_TEST_INIT_CONFIG` 指向 fixture 配置。
pub fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("init fixture: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode> {
    let config_path = std::env::var_os(INIT_CONFIG_ENV)
        .map(PathBuf::from)
        .context("LKIT_TEST_INIT_CONFIG is not set")?;
    let config = InitFixtureConfig::read(&config_path)?;
    let role = std::env::args()
        .next()
        .as_deref()
        .and_then(|program| Path::new(program).file_name())
        .and_then(|name| name.to_str())
        .map(PathBuf::from);
    let role = role.ok_or_else(|| anyhow::anyhow!("cannot resolve argv[0]"))?;
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(log) = &config.call_log {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log)
            .context("open call log")?;
        use std::io::Write;
        writeln!(file, "{} {}", role.display(), args.join(" ")).context("append call log")?;
    }
    dispatch(&config, &role, &args)
}

fn dispatch(config: &InitFixtureConfig, role: &Path, args: &[String]) -> Result<ExitCode> {
    let role_name = role.to_string_lossy();
    if role_name.contains("rc-service") {
        return rc_service(config, args);
    }
    if role_name.contains("rc-update") {
        return rc_update(config, args);
    }
    if role_name.contains("update-rc.d") {
        return update_rc_d(config, args);
    }
    if role_name.contains("start-stop-daemon") {
        return start_stop_daemon(args);
    }
    anyhow::bail!("unknown init fixture role {}", role.display())
}

// ---------------------------------------------------------------- rc-service

fn rc_service(config: &InitFixtureConfig, args: &[String]) -> Result<ExitCode> {
    let Some((action, name)) = args.first().zip(args.get(1)) else {
        anyhow::bail!("rc-service requires an action and a service name");
    };
    let script = config.init_d_dir.join(name);
    let pid_path = config.state_dir.join("pids").join(format!("{name}.pid"));
    match action.as_str() {
        "start" => {
            let (command, command_args) = parse_command(&script)?;
            let pid = spawn_background(&command, &command_args)?;
            if let Some(parent) = pid_path.parent() {
                std::fs::create_dir_all(parent).context("create pid dir")?;
            }
            std::fs::write(&pid_path, pid.to_string()).context("write pid")?;
            Ok(ExitCode::SUCCESS)
        }
        "stop" => {
            stop_pid(&pid_path)?;
            Ok(ExitCode::SUCCESS)
        }
        "restart" => {
            stop_pid(&pid_path)?;
            let (command, command_args) = parse_command(&script)?;
            let pid = spawn_background(&command, &command_args)?;
            if let Some(parent) = pid_path.parent() {
                std::fs::create_dir_all(parent).context("create pid dir")?;
            }
            std::fs::write(&pid_path, pid.to_string()).context("write pid")?;
            Ok(ExitCode::SUCCESS)
        }
        "status" => {
            if pid_alive(&pid_path) {
                Ok(ExitCode::SUCCESS)
            } else {
                // OpenRC 约定:未运行返回 3。
                Ok(ExitCode::from(3))
            }
        }
        _ => anyhow::bail!("unsupported rc-service action {action}"),
    }
}

fn parse_command(script: &Path) -> Result<(PathBuf, Vec<String>)> {
    let content = std::fs::read_to_string(script)
        .with_context(|| format!("read init script {}", script.display()))?;
    let command =
        extract_quoted(&content, "command=").context("init script must declare command=")?;
    let command_args = extract_quoted(&content, "command_args=")
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_string)
        .collect();
    Ok((PathBuf::from(command), command_args))
}

fn extract_quoted(content: &str, key: &str) -> Option<String> {
    let line = content
        .lines()
        .find(|line| line.trim_start().starts_with(key))?;
    let value = line.trim_start().trim_start_matches(key);
    let value = value.trim();
    if let Some(inner) = value.strip_prefix('"') {
        inner.strip_suffix('"').map(str::to_string)
    } else {
        Some(value.to_string())
    }
}

fn spawn_background(command: &Path, command_args: &[String]) -> Result<u32> {
    let child = Command::new(command)
        .args(command_args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("spawn {}", command.display()))?;
    Ok(child.id())
}

fn stop_pid(pid_path: &Path) -> Result<()> {
    let Ok(content) = std::fs::read_to_string(pid_path) else {
        return Ok(());
    };
    let pid: i32 = content.trim().parse().context("parse pid")?;
    let _ = unsafe { libc::kill(pid, libc::SIGTERM) };
    std::fs::remove_file(pid_path).ok();
    Ok(())
}

fn pid_alive(pid_path: &Path) -> bool {
    let Ok(content) = std::fs::read_to_string(pid_path) else {
        return false;
    };
    let Ok(pid) = content.trim().parse::<i32>() else {
        return false;
    };
    let result = unsafe { libc::kill(pid, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

// ---------------------------------------------------------------- rc-update

fn rc_update(config: &InitFixtureConfig, args: &[String]) -> Result<ExitCode> {
    let Some(action) = args.first() else {
        anyhow::bail!("rc-update requires an action");
    };
    match action.as_str() {
        "add" => {
            let Some(name) = args.get(1) else {
                anyhow::bail!("rc-update add requires a service name");
            };
            let enabled = config.state_dir.join("enabled").join(name);
            if let Some(parent) = enabled.parent() {
                std::fs::create_dir_all(parent).context("create enabled dir")?;
            }
            std::fs::write(enabled, "default").context("write enabled marker")?;
            Ok(ExitCode::SUCCESS)
        }
        "del" => {
            if let Some(name) = args.get(1) {
                std::fs::remove_file(config.state_dir.join("enabled").join(name)).ok();
            }
            Ok(ExitCode::SUCCESS)
        }
        "show" => {
            let enabled = config.state_dir.join("enabled");
            if let Ok(entries) = std::fs::read_dir(&enabled) {
                let mut names: Vec<String> = entries
                    .flatten()
                    .filter_map(|entry| entry.file_name().into_string().ok())
                    .collect();
                names.sort();
                for name in names {
                    println!(" {name} | default");
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        "--version" | "-V" => {
            println!("OpenRC 0.55 (fixture)");
            Ok(ExitCode::SUCCESS)
        }
        _ => anyhow::bail!("unsupported rc-update action {action}"),
    }
}

// ---------------------------------------------------------------- update-rc.d

fn update_rc_d(config: &InitFixtureConfig, args: &[String]) -> Result<ExitCode> {
    let Some((action, name)) = args.first().zip(args.get(1)) else {
        anyhow::bail!("update-rc.d requires an action and a service name");
    };
    let rc3 = config.rc_d_dir.join("rc3.d");
    match action.as_str() {
        "enable" => {
            std::fs::create_dir_all(&rc3).context("create rc3.d")?;
            let link = rc3.join(format!("S20{name}"));
            std::fs::remove_file(&link).ok();
            std::os::unix::fs::symlink(config.init_d_dir.join(name), &link)
                .with_context(|| format!("create {name} rc link"))?;
            Ok(ExitCode::SUCCESS)
        }
        "disable" => {
            remove_rc_links(config, name)?;
            Ok(ExitCode::SUCCESS)
        }
        _ => anyhow::bail!("unsupported update-rc.d action {action}"),
    }
}

fn remove_rc_links(config: &InitFixtureConfig, name: &str) -> Result<()> {
    let Ok(entries) = std::fs::read_dir(&config.rc_d_dir) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        if let Ok(links) = std::fs::read_dir(&dir) {
            for link in links.flatten() {
                let file_name = link.file_name().to_string_lossy().to_string();
                if file_name.starts_with('S') && file_name.ends_with(name) {
                    std::fs::remove_file(link.path()).ok();
                }
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- start-stop-daemon

fn start_stop_daemon(args: &[String]) -> Result<ExitCode> {
    let Some(action) = args.first() else {
        anyhow::bail!("start-stop-daemon requires --start or --stop");
    };
    match action.as_str() {
        "--start" => {
            let pidfile = flag_value(args, "--pidfile").context("--start requires --pidfile")?;
            let exec = flag_value(args, "--exec").context("--start requires --exec")?;
            let command_args = args
                .iter()
                .skip_while(|arg| arg.as_str() != "--")
                .skip(1)
                .cloned()
                .collect::<Vec<String>>();
            let pid = spawn_background(Path::new(&exec), &command_args)?;
            std::fs::write(&pidfile, pid.to_string()).context("write pidfile")?;
            Ok(ExitCode::SUCCESS)
        }
        "--stop" => {
            let pidfile = flag_value(args, "--pidfile").context("--stop requires --pidfile")?;
            stop_pid(Path::new(&pidfile))?;
            Ok(ExitCode::SUCCESS)
        }
        _ => anyhow::bail!("unsupported start-stop-daemon action {action}"),
    }
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|arg| arg == flag)
        .and_then(|index| args.get(index + 1))
        .cloned()
}
