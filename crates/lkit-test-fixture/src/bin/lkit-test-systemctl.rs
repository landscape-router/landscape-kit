use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use lkit_test_fixture::{FIXTURE_CONFIG_ENV, SYSTEMCTL_CONFIG_ENV, SystemctlFixtureConfig};

const UNIT_NAME: &str = "landscape-router.service";
const ENABLED_FILE: &str = "enabled";
const PID_FILE: &str = "main.pid";
const ACTIVE_FILE: &str = "active";
const MASKED_FILE: &str = "masked";

pub fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("systemctl fixture: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<ExitCode> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let [command, root, destination] = args.as_slice()
        && command == "--collect-diagnostics"
    {
        for entry in std::fs::read_dir(root)? {
            let entry = entry?;
            if entry.file_type()?.is_dir()
                && entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("lkit-cli-fixture-")
                && let Err(error) = lkit_test_fixture::diagnostics::export(
                    &entry.path(),
                    &Path::new(destination).join(entry.file_name()),
                )
            {
                eprintln!(
                    "cannot collect remaining fixture {}: {error:#}",
                    entry.path().display()
                );
            }
        }
        return Ok(ExitCode::SUCCESS);
    }
    let config_path = std::env::var_os(SYSTEMCTL_CONFIG_ENV)
        .map(PathBuf::from)
        .context("LKIT_TEST_SYSTEMCTL_CONFIG is not set")?;
    let config = SystemctlFixtureConfig::read(&config_path)?;
    std::fs::create_dir_all(&config.state_dir)
        .with_context(|| format!("create systemctl state dir {}", config.state_dir.display()))?;
    append_call_log(&config, &args)?;
    stage_host_vendor_definitions(&config)?;
    dispatch(&config, &args)
}

fn dispatch(config: &SystemctlFixtureConfig, args: &[String]) -> Result<ExitCode> {
    let mut exit_code = ExitCode::SUCCESS;
    match args {
        [command, full, legend, unit]
            if command == "list-unit-files" && full == "--full" && legend == "--no-legend" =>
        {
            if load_state(config, unit) != "not-found" {
                let state = if masked_path(config, unit).is_file() {
                    std::fs::read_to_string(masked_path(config, unit))?
                        .trim()
                        .to_string()
                } else if unit_is_enabled(config, unit) {
                    enable_state(config, unit).into()
                } else {
                    "disabled".into()
                };
                println!("{unit} {state} disabled");
            }
        }
        [command, all, full, plain, legend, pattern]
            if command == "list-units"
                && all == "--all"
                && full == "--full"
                && plain == "--plain"
                && legend == "--no-legend"
                && pattern == "systemd-networkd-wait-online@*.service" =>
        {
            let mut names = std::collections::BTreeSet::new();
            for directory in [&config.unit_dir, &config.state_dir.join("units")] {
                if !directory.exists() {
                    continue;
                }
                for entry in std::fs::read_dir(directory)? {
                    names.insert(entry?.file_name().to_string_lossy().into_owned());
                }
            }
            for name in names {
                if name.starts_with("systemd-networkd-wait-online@")
                    && name.ends_with(".service")
                    && !name.ends_with("@.service")
                {
                    println!("{name} loaded inactive dead fixture");
                }
            }
        }
        [show, property] if show == "show" && property == "--property=Version" => {
            println!("Version={}", config.systemd_version);
        }
        [show, property, value, unit]
            if show == "show" && property == "--property=ActiveState" && value == "--value" =>
        {
            println!(
                "{}",
                if unit_is_active(config, unit) {
                    "active"
                } else {
                    "inactive"
                }
            );
        }
        [show, property, value, unit]
            if show == "show" && property == "--property=MainPID" && value == "--value" =>
        {
            println!("{}", active_pid(config, unit).unwrap_or(0));
        }
        [show, property, value, unit]
            if show == "show" && property == "--property=LoadState" && value == "--value" =>
        {
            println!("{}", load_state(config, unit));
        }
        [show, property, value, unit]
            if show == "show" && property == "--property=FragmentPath" && value == "--value" =>
        {
            println!("{}", config.unit_dir.join(unit).display());
        }
        [command, unit] if command == "is-enabled" => {
            if masked_path(config, unit).is_file() {
                println!(
                    "{}",
                    std::fs::read_to_string(masked_path(config, unit))?.trim()
                );
                exit_code = ExitCode::from(1);
            } else if unit_is_enabled(config, unit) {
                println!("{}", enable_state(config, unit));
            } else {
                println!("disabled");
                exit_code = ExitCode::from(1);
            }
        }
        [command, quiet, unit] if command == "is-enabled" && quiet == "--quiet" => {
            if !unit_is_enabled(config, unit) || masked_path(config, unit).is_file() {
                exit_code = ExitCode::from(1);
            }
        }
        [command, unit] if command == "is-active" => {
            if unit_is_active(config, unit) {
                println!("active");
            } else {
                println!("inactive");
                exit_code = ExitCode::from(3);
            }
        }
        [command, quiet, unit] if command == "is-active" && quiet == "--quiet" => {
            if !unit_is_active(config, unit) {
                exit_code = ExitCode::from(3);
            }
        }
        [command, unit] if command == "enable" => {
            ensure_installed(config, unit)?;
            enable_unit(config, unit, false)?;
        }
        [command, runtime, unit] if command == "enable" && runtime == "--runtime" => {
            ensure_installed(config, unit)?;
            enable_unit(config, unit, true)?;
        }
        [command, unit] if command == "disable" => {
            disable_unit(config, unit, false)?;
        }
        [command, runtime, unit] if command == "disable" && runtime == "--runtime" => {
            disable_unit(config, unit, true)?;
        }
        [command, unit] if command == "mask" => {
            ensure_installed(config, unit)?;
            if is_host_service(unit) {
                host_mask(config, unit, false)?;
            }
            write_marker(&masked_path(config, unit), b"masked\n")?;
        }
        [command, unit] if command == "unmask" => {
            if is_host_service(unit) {
                remove_if_exists(&config.unit_dir.join(unit))?;
            }
            remove_if_exists(&masked_path(config, unit))?;
        }
        [command, runtime, unit] if command == "mask" && runtime == "--runtime" => {
            ensure_installed(config, unit)?;
            if is_host_service(unit) {
                host_mask(config, unit, true)?;
            }
            write_marker(&masked_path(config, unit), b"masked-runtime\n")?;
        }
        [command, runtime, unit] if command == "unmask" && runtime == "--runtime" => {
            if is_host_service(unit) {
                remove_if_exists(&host_runtime_dir(config).join(unit))?;
            }
            remove_if_exists(&masked_path(config, unit))?;
        }
        [command, unit] if command == "start" => {
            ensure_installed(config, unit)?;
            start(config, unit)?;
        }
        [command, unit] if command == "stop" => {
            stop(config, unit)?;
        }
        [command, unit] if command == "restart" => {
            ensure_installed(config, unit)?;
            stop(config, unit)?;
            start(config, unit)?;
        }
        [command] if command == "daemon-reload" => sync_host_masks(config)?,
        _ => anyhow::bail!("unsupported systemctl arguments: {args:?}"),
    }
    Ok(exit_code)
}

// Host service tests model mask files as well as manager state so production can
// remove a specific mask without invoking unmask's recursive link cleanup.
fn is_host_service(unit: &str) -> bool {
    unit.starts_with("systemd-networkd")
        || matches!(
            unit,
            "NetworkManager.service"
                | "networking.service"
                | "firewalld.service"
                | "systemd-resolved.service"
        )
}

fn stage_host_vendor_definitions(config: &SystemctlFixtureConfig) -> Result<()> {
    for entry in std::fs::read_dir(&config.unit_dir)? {
        let entry = entry?;
        let unit = entry.file_name().to_string_lossy().into_owned();
        if is_host_service(&unit) && entry.file_type()?.is_file() {
            let vendor = unit_state_dir(config, &unit).join("vendor-definition");
            std::fs::create_dir_all(vendor.parent().unwrap())?;
            std::fs::rename(entry.path(), vendor)?;
            if let Ok(state) = std::fs::read_to_string(masked_path(config, &unit)) {
                host_mask(config, &unit, state.trim() == "masked-runtime")?;
            }
        }
    }
    Ok(())
}
fn host_runtime_dir(config: &SystemctlFixtureConfig) -> PathBuf {
    config.unit_dir.parent().unwrap().join("run/systemd/system")
}
fn host_mask(config: &SystemctlFixtureConfig, unit: &str, runtime: bool) -> Result<()> {
    let vendor = unit_state_dir(config, unit).join("vendor-definition");
    let definition = config.unit_dir.join(unit);
    if !vendor.exists() {
        std::fs::copy(&definition, &vendor)?;
    }
    let directory = if runtime {
        host_runtime_dir(config)
    } else {
        config.unit_dir.clone()
    };
    std::fs::create_dir_all(&directory)?;
    let path = directory.join(unit);
    remove_if_exists(&path)?;
    std::os::unix::fs::symlink("/dev/null", path)?;
    Ok(())
}
fn sync_host_masks(config: &SystemctlFixtureConfig) -> Result<()> {
    let directory = config.state_dir.join("units");
    if !directory.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let vendor = entry.path().join("vendor-definition");
        if !vendor.exists() {
            continue;
        }
        let unit = entry.file_name().to_string_lossy().into_owned();
        let persistent = config.unit_dir.join(&unit);
        let runtime = host_runtime_dir(config).join(&unit);
        let is_mask = |path: &Path| {
            std::fs::read_link(path).is_ok_and(|target| target == Path::new("/dev/null"))
        };
        if is_mask(&persistent) {
            write_marker(&masked_path(config, &unit), b"masked\n")?;
        } else {
            if is_mask(&runtime) {
                write_marker(&masked_path(config, &unit), b"masked-runtime\n")?;
            } else {
                remove_if_exists(&masked_path(config, &unit))?;
            }
        }
    }
    Ok(())
}

fn ensure_installed(config: &SystemctlFixtureConfig, unit: &str) -> Result<()> {
    anyhow::ensure!(
        load_state(config, unit) != "not-found",
        "unit {unit:?} is not installed"
    );
    Ok(())
}

fn start(config: &SystemctlFixtureConfig, unit: &str) -> Result<()> {
    if !should_spawn(config, unit) {
        return write_marker(&active_path(config, unit), b"active\n");
    }
    if active_pid(config, unit).is_some() {
        return Ok(());
    }
    let state = unit_state_dir(config, unit);
    std::fs::create_dir_all(&state)
        .with_context(|| format!("create unit state dir {}", state.display()))?;
    remove_if_exists(&pid_path(config, unit))?;
    let command = exec_start(&config.unit_dir.join(unit))?;
    let executable = command
        .first()
        .context("unit ExecStart does not contain an executable")?;
    let log = open_log(&config.log_path)?;
    let error_log = log.try_clone().context("clone fixture log file")?;
    let mut child_command = Command::new(executable);
    child_command.args(&command[1..]);
    if unit == UNIT_NAME {
        if let Some(path) = &config.landscape_config {
            child_command.env(FIXTURE_CONFIG_ENV, path);
        } else {
            child_command.env_remove(FIXTURE_CONFIG_ENV);
        }
    }
    // A real systemd service is outside the invoking SSH/PTY session. Keep the
    // fixture process alive when the command terminal disappears as well.
    unsafe {
        child_command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = child_command
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(error_log))
        .spawn()
        .with_context(|| format!("start fixture service with {command:?}"))?;
    write_pid(config, unit, child.id())
}

fn stop(config: &SystemctlFixtureConfig, unit: &str) -> Result<()> {
    let state = unit_state_dir(config, unit);
    let pid = std::fs::read_to_string(state.join(PID_FILE))
        .ok()
        .and_then(|content| content.trim().parse::<u32>().ok())
        .filter(|pid| process_alive(*pid));
    if let Some(pid) = pid {
        signal(pid, libc::SIGTERM)?;
        let started = Instant::now();
        while process_alive(pid) && started.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(50));
        }
        if process_alive(pid) {
            signal(pid, libc::SIGKILL)?;
        }
    }
    remove_if_exists(&pid_path(config, unit))?;
    remove_if_exists(&active_path(config, unit))
}

fn exec_start(unit_path: &Path) -> Result<Vec<String>> {
    let content = std::fs::read_to_string(unit_path)
        .with_context(|| format!("read unit {}", unit_path.display()))?;
    let value = content
        .lines()
        .find_map(|line| line.trim().strip_prefix("ExecStart="))
        .context("unit does not contain ExecStart")?;
    let command = shell_words::split(value).context("parse unit ExecStart")?;
    anyhow::ensure!(!command.is_empty(), "unit ExecStart is empty");
    Ok(command)
}

fn open_log(path: &Path) -> Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create log directory {}", parent.display()))?;
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open fixture log {}", path.display()))
}

fn append_call_log(config: &SystemctlFixtureConfig, args: &[String]) -> Result<()> {
    let Some(path) = &config.call_log else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create call log directory {}", parent.display()))?;
    }
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open systemctl call log {}", path.display()))?;
    serde_json::to_writer(&mut log, args).context("serialize systemctl call")?;
    writeln!(log).context("append systemctl call")
}

fn write_pid(config: &SystemctlFixtureConfig, unit: &str, pid: u32) -> Result<()> {
    let path = pid_path(config, unit);
    let tmp = path.with_extension("tmp");
    let mut file = std::fs::File::create(&tmp)
        .with_context(|| format!("create temporary PID file {}", tmp.display()))?;
    writeln!(file, "{pid}").context("write fixture PID")?;
    file.sync_all().context("sync fixture PID")?;
    std::fs::rename(&tmp, &path)
        .with_context(|| format!("commit fixture PID file {}", path.display()))
}

fn active_pid(config: &SystemctlFixtureConfig, unit: &str) -> Option<u32> {
    let pid = read_pid(config, unit)?;
    process_alive(pid).then_some(pid)
}

fn read_pid(config: &SystemctlFixtureConfig, unit: &str) -> Option<u32> {
    std::fs::read_to_string(pid_path(config, unit))
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn process_alive(pid: u32) -> bool {
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn signal(pid: u32, signal: i32) -> Result<()> {
    let result = unsafe { libc::kill(pid as libc::pid_t, signal) };
    if result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
        return Ok(());
    }
    Err(std::io::Error::last_os_error()).with_context(|| format!("signal process {pid}"))
}

fn load_state(config: &SystemctlFixtureConfig, unit: &str) -> &'static str {
    if masked_path(config, unit).is_file() {
        "masked"
    } else if config.unit_dir.join(unit).exists()
        || unit_state_dir(config, unit)
            .join("vendor-definition")
            .is_file()
    {
        "loaded"
    } else {
        "not-found"
    }
}

fn unit_is_active(config: &SystemctlFixtureConfig, unit: &str) -> bool {
    if should_spawn(config, unit) {
        active_pid(config, unit).is_some()
    } else {
        active_path(config, unit).is_file()
    }
}

fn unit_is_enabled(config: &SystemctlFixtureConfig, unit: &str) -> bool {
    matches!(enable_state(config, unit), "enabled" | "enabled-runtime")
}

fn is_managed(unit: &str) -> bool {
    matches!(unit, UNIT_NAME | "lkit.service")
}

fn memberships(directory: &Path, unit: &str) -> Vec<PathBuf> {
    std::fs::read_dir(directory)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.ends_with(".wants") || name.ends_with(".requires")
        })
        .map(|entry| entry.path().join(unit))
        .filter(|path| std::fs::symlink_metadata(path).is_ok())
        .collect()
}

fn enable_state(config: &SystemctlFixtureConfig, unit: &str) -> &'static str {
    if is_managed(unit) {
        // Managed-unit recovery edits actual links. A separate boolean marker
        // would conceal runtime/persistent restoration errors from the fixture.
        if !memberships(&config.unit_dir, unit).is_empty() {
            "enabled"
        } else if !memberships(&host_runtime_dir(config), unit).is_empty() {
            "enabled-runtime"
        } else {
            "disabled"
        }
    } else {
        match std::fs::read_to_string(enabled_path(config, unit)) {
            Ok(state) if state.trim() == "enabled-runtime" => "enabled-runtime",
            Ok(_) => "enabled",
            Err(_) => "disabled",
        }
    }
}

fn enable_unit(config: &SystemctlFixtureConfig, unit: &str, runtime: bool) -> Result<()> {
    if !is_managed(unit) {
        return write_marker(
            &enabled_path(config, unit),
            if runtime {
                b"enabled-runtime\n"
            } else {
                b"enabled\n"
            },
        );
    }
    let directory = if runtime {
        host_runtime_dir(config)
    } else {
        config.unit_dir.clone()
    };
    let path = directory.join("multi-user.target.wants").join(unit);
    std::fs::create_dir_all(path.parent().unwrap())?;
    if std::fs::symlink_metadata(&path).is_err() {
        std::os::unix::fs::symlink(config.unit_dir.join(unit), path)?;
    }
    Ok(())
}

fn disable_unit(config: &SystemctlFixtureConfig, unit: &str, runtime: bool) -> Result<()> {
    if !is_managed(unit) {
        return remove_if_exists(&enabled_path(config, unit));
    }
    let mut paths = memberships(&host_runtime_dir(config), unit);
    if !runtime {
        paths.extend(memberships(&config.unit_dir, unit));
    }
    for path in paths {
        anyhow::ensure!(
            std::fs::symlink_metadata(&path)?.file_type().is_symlink(),
            "startup membership is not a symlink"
        );
        std::fs::remove_file(path)?;
    }
    Ok(())
}

fn unit_state_dir(config: &SystemctlFixtureConfig, unit: &str) -> PathBuf {
    if unit == UNIT_NAME {
        config.state_dir.clone()
    } else {
        config.state_dir.join("units").join(unit)
    }
}

fn enabled_path(config: &SystemctlFixtureConfig, unit: &str) -> PathBuf {
    unit_state_dir(config, unit).join(ENABLED_FILE)
}

fn active_path(config: &SystemctlFixtureConfig, unit: &str) -> PathBuf {
    unit_state_dir(config, unit).join(ACTIVE_FILE)
}

fn masked_path(config: &SystemctlFixtureConfig, unit: &str) -> PathBuf {
    unit_state_dir(config, unit).join(MASKED_FILE)
}

fn pid_path(config: &SystemctlFixtureConfig, unit: &str) -> PathBuf {
    unit_state_dir(config, unit).join(PID_FILE)
}

fn should_spawn(config: &SystemctlFixtureConfig, unit: &str) -> bool {
    unit == UNIT_NAME || config.spawn_units.iter().any(|name| name == unit)
}

fn write_marker(path: &Path, content: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create marker directory {}", parent.display()))?;
    }
    std::fs::write(path, content).with_context(|| format!("write marker {}", path.display()))
}

fn remove_if_exists(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "lkit-systemctl-fixture-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn parses_exec_start() {
        let dir = temp_dir("exec");
        let unit = dir.join(UNIT_NAME);
        std::fs::write(
            &unit,
            "[Service]\nExecStart='/tmp/fixture server' --config-dir '/tmp/data dir' --web /tmp/web\n",
        )
        .unwrap();
        assert_eq!(
            exec_start(&unit).unwrap(),
            vec![
                "/tmp/fixture server",
                "--config-dir",
                "/tmp/data dir",
                "--web",
                "/tmp/web"
            ]
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    fn config_in(root: &Path) -> SystemctlFixtureConfig {
        let config = SystemctlFixtureConfig {
            schema_version: 1,
            unit_dir: root.join("system"),
            state_dir: root.join("state"),
            landscape_config: None,
            log_path: root.join("fixture.log"),
            call_log: None,
            systemd_version: "252.fixture".into(),
            spawn_units: Vec::new(),
        };
        std::fs::create_dir_all(&config.unit_dir).unwrap();
        std::fs::create_dir_all(&config.state_dir).unwrap();
        for unit in [UNIT_NAME, "lkit.service"] {
            std::fs::write(
                config.unit_dir.join(unit),
                "[Install]\nWantedBy=multi-user.target\n",
            )
            .unwrap();
        }
        config
    }

    #[test]
    fn managed_enablement_returns_to_runtime_when_persistent_link_is_removed() {
        let dir = temp_dir("mixed-enablements");
        let config = config_in(&dir);
        for unit in [UNIT_NAME, "lkit.service"] {
            enable_unit(&config, unit, true).unwrap();
            assert_eq!(enable_state(&config, unit), "enabled-runtime");

            enable_unit(&config, unit, false).unwrap();
            assert_eq!(enable_state(&config, unit), "enabled");
            assert_eq!(memberships(&host_runtime_dir(&config), unit).len(), 1);
            let persistent = memberships(&config.unit_dir, unit);
            assert_eq!(persistent.len(), 1);

            std::fs::remove_file(&persistent[0]).unwrap();
            assert_eq!(enable_state(&config, unit), "enabled-runtime");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn runtime_disable_preserves_persistent_wants_and_requires() {
        let dir = temp_dir("runtime-disable");
        let config = config_in(&dir);
        enable_unit(&config, UNIT_NAME, false).unwrap();
        enable_unit(&config, UNIT_NAME, true).unwrap();
        for base in [&config.unit_dir, &host_runtime_dir(&config)] {
            let requires = base.join("custom.target.requires");
            std::fs::create_dir_all(&requires).unwrap();
            std::os::unix::fs::symlink(config.unit_dir.join(UNIT_NAME), requires.join(UNIT_NAME))
                .unwrap();
        }

        disable_unit(&config, UNIT_NAME, true).unwrap();
        assert_eq!(enable_state(&config, UNIT_NAME), "enabled");
        assert_eq!(memberships(&config.unit_dir, UNIT_NAME).len(), 2);
        assert!(memberships(&host_runtime_dir(&config), UNIT_NAME).is_empty());

        disable_unit(&config, UNIT_NAME, false).unwrap();
        assert_eq!(enable_state(&config, UNIT_NAME), "disabled");
        assert!(memberships(&config.unit_dir, UNIT_NAME).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn managed_queries_follow_links_while_host_queries_retain_markers() {
        let dir = temp_dir("link-queries");
        let config = config_in(&dir);
        write_marker(&enabled_path(&config, UNIT_NAME), b"enabled\n").unwrap();
        assert_eq!(enable_state(&config, UNIT_NAME), "disabled");

        let requires = host_runtime_dir(&config).join("custom.target.requires");
        std::fs::create_dir_all(&requires).unwrap();
        let membership = requires.join(UNIT_NAME);
        std::os::unix::fs::symlink(config.unit_dir.join(UNIT_NAME), &membership).unwrap();
        assert_eq!(enable_state(&config, UNIT_NAME), "enabled-runtime");
        std::fs::remove_file(membership).unwrap();
        assert_eq!(enable_state(&config, UNIT_NAME), "disabled");

        let host = "NetworkManager.service";
        enable_unit(&config, host, true).unwrap();
        assert_eq!(enable_state(&config, host), "enabled-runtime");
        assert!(memberships(&host_runtime_dir(&config), host).is_empty());
        disable_unit(&config, host, true).unwrap();
        assert_eq!(enable_state(&config, host), "disabled");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
