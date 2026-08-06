use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::commands::network::NetworkAction;
use crate::commands::{Commands, ServiceManagerArg};
use crate::deployment::state::StateServiceManager;
use crate::interaction::interactive::SYSTEMD_WORKER_TTY_ENV;
use crate::interaction::presentation::{
    InterruptGuard, OPERATIONS_DIR, PRESENTATION_EVENTS_ENV, WorkerPresentation,
};
use crate::network::config::NetworkPlan;
use crate::service::systemd::{Availability, Systemd};

const WORKER_COMMAND: &str = "__systemd-worker";

#[derive(Debug, Deserialize, Serialize)]
struct WorkerRequest {
    schema_version: u64,
    args: Vec<String>,
    environment: Vec<(String, String)>,
    working_directory: PathBuf,
    result_path: PathBuf,
    unit_path: PathBuf,
    systemctl: PathBuf,
    terminal: Option<PathBuf>,
    presentation_path: PathBuf,
    #[serde(default)]
    credential_path: Option<PathBuf>,
    #[serde(default)]
    network_plan_path: Option<PathBuf>,
}

#[derive(Debug, Deserialize, Serialize)]
struct WorkerResult {
    schema_version: u64,
    exit_code: i32,
}

enum WaitOutcome {
    Completed(ExitCode),
    Interrupted,
}

pub(crate) fn should_delegate(command: &Commands) -> bool {
    if unsafe { libc::geteuid() } != 0 || test_runtime_is_inline(command) {
        return false;
    }
    match command {
        Commands::Check(_) | Commands::Reconcile(_) => false,
        Commands::Network(args) => {
            matches!(args.action, NetworkAction::Rollback { automatic: false })
        }
        Commands::Install(args) => {
            if args.force || args.service_manager == Some(ServiceManagerArg::None) {
                return false;
            }
            if load_manager(args.install_dir.as_deref()).is_some() {
                return false;
            }
            match args.service_manager {
                Some(ServiceManagerArg::Systemd) => true,
                Some(ServiceManagerArg::None) => false,
                None => matches!(Systemd::host().probe(), Availability::Available { .. }),
            }
        }
        Commands::Switch(args) => {
            load_manager(args.install_dir.as_deref()) == Some(StateServiceManager::Systemd)
        }
        Commands::Repair(args) => {
            args.target == crate::commands::repair::RepairTarget::Binary
                && load_manager(args.install_dir.as_deref()) == Some(StateServiceManager::Systemd)
        }
        Commands::ServiceManager(args) => {
            let current = load_manager(args.install_dir.as_deref());
            let target = match args.target {
                ServiceManagerArg::Systemd => StateServiceManager::Systemd,
                ServiceManagerArg::None => StateServiceManager::None,
            };
            current.is_some_and(|manager| manager != target)
        }
    }
}

#[cfg(feature = "test-support")]
fn test_runtime_is_inline(command: &Commands) -> bool {
    let path = match command {
        Commands::Check(_) => return false,
        Commands::Install(args) => args.test_runtime.as_deref(),
        Commands::Switch(args) => args.test_runtime.as_deref(),
        Commands::Repair(args) => args.test_runtime.as_deref(),
        Commands::Reconcile(args) => args.test_runtime.as_deref(),
        Commands::ServiceManager(args) => args.test_runtime.as_deref(),
        Commands::Network(args) => args.test_runtime.as_deref(),
    };
    let Some(path) = path else {
        return false;
    };
    !crate::deployment::runtime::InstallRuntime::test_uses_systemd_worker(path).unwrap_or(false)
}

#[cfg(not(feature = "test-support"))]
fn test_runtime_is_inline(_command: &Commands) -> bool {
    false
}

fn load_manager(install_dir: Option<&Path>) -> Option<StateServiceManager> {
    let selected = crate::deployment::plan::select_install_root(
        install_dir,
        std::env::var("LKIT_INSTALL_DIR").ok().as_deref(),
    )
    .ok()?;
    let root = crate::deployment::root::normalize_install_root(&selected).ok()?;
    Some(
        crate::deployment::state::load_state(&root)
            .ok()??
            .service
            .manager,
    )
}

pub(crate) fn delegate(
    interrupt: &InterruptGuard,
    mut args: Vec<String>,
    interactive_password: Option<String>,
    network_plan: Option<NetworkPlan>,
    full_screen: bool,
) -> Result<ExitCode, String> {
    let systemd = Systemd::host();
    if !matches!(systemd.probe(), Availability::Available { .. }) {
        return Err("the systemd manager is not available".into());
    }
    let operation_id = Uuid::now_v7().to_string();
    let directory = PathBuf::from(OPERATIONS_DIR);
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("create {}: {error}", directory.display()))?;
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("secure {}: {error}", directory.display()))?;

    let request_path = directory.join(format!("{operation_id}.json"));
    let result_path = directory.join(format!("{operation_id}.result.json"));
    let stdout_path = directory.join(format!("{operation_id}.stdout.log"));
    let stderr_path = directory.join(format!("{operation_id}.stderr.log"));
    let presentation_path = directory.join(format!("{operation_id}.presentation.jsonl"));
    let credential_path = directory.join(format!("{operation_id}.credential"));
    let network_plan_path = directory.join(format!("{operation_id}.network.json"));
    let unit_name = format!("lkit-operation-{operation_id}.service");
    let unit_path = systemd.run_systemd_dir.join(&unit_name);
    let executable =
        std::env::current_exe().map_err(|error| format!("resolve current executable: {error}"))?;
    let mut environment = string_environment()?;
    environment.retain(|(key, _)| key != crate::i18n::LANGUAGE_ENV);
    environment.push((
        crate::i18n::LANGUAGE_ENV.to_string(),
        crate::i18n::current().code().to_string(),
    ));
    let working_directory =
        std::env::current_dir().map_err(|error| format!("resolve current directory: {error}"))?;
    let terminal = terminal_path();
    let has_credential = interactive_password.is_some();
    if let Some(password) = interactive_password {
        create_private_secret_file(&credential_path, password.as_bytes())?;
        args.extend([
            "--password-file".into(),
            credential_path.display().to_string(),
        ]);
    }
    let has_network_plan = network_plan.is_some();
    if let Some(network_plan) = network_plan {
        if let Err(error) = write_private_json(&network_plan_path, &network_plan) {
            cleanup_files(&[&credential_path, &network_plan_path]);
            return Err(error);
        }
        args.extend([
            "--network-plan-file".into(),
            network_plan_path.display().to_string(),
        ]);
    }
    let request = WorkerRequest {
        schema_version: 1,
        args,
        environment,
        working_directory,
        result_path: result_path.clone(),
        unit_path: unit_path.clone(),
        systemctl: systemd.systemctl.clone(),
        terminal,
        presentation_path: presentation_path.clone(),
        credential_path: has_credential.then(|| credential_path.clone()),
        network_plan_path: has_network_plan.then(|| network_plan_path.clone()),
    };
    if let Err(error) = write_private_json(&request_path, &request) {
        cleanup_files(&[&credential_path, &network_plan_path]);
        return Err(error);
    }
    let unit = render_unit(&executable, &request_path, &stdout_path, &stderr_path);
    if let Err(error) = write_unit(&unit_path, &unit) {
        cleanup_files(&[&request_path, &credential_path, &network_plan_path]);
        return Err(error);
    }
    if let Err(error) = create_private_file(&presentation_path) {
        cleanup_files(&[
            &request_path,
            &unit_path,
            &credential_path,
            &network_plan_path,
        ]);
        return Err(error);
    }

    if let Err(error) = systemctl(&systemd.systemctl, &["daemon-reload"]) {
        cleanup_files(&[
            &request_path,
            &unit_path,
            &presentation_path,
            &credential_path,
            &network_plan_path,
        ]);
        return Err(error);
    }
    if let Err(error) = systemctl(
        &systemd.systemctl,
        &["start", "--no-block", unit_name.as_str()],
    ) {
        cleanup_files(&[
            &request_path,
            &unit_path,
            &presentation_path,
            &credential_path,
            &network_plan_path,
        ]);
        let _ = systemctl(&systemd.systemctl, &["daemon-reload"]);
        return Err(error);
    }

    if interrupt.requested() {
        return interrupt_worker(
            &systemd.systemctl,
            &unit_name,
            &request_path,
            &result_path,
            &unit_path,
            &stdout_path,
            &stderr_path,
            &presentation_path,
            &credential_path,
            &network_plan_path,
            interrupt,
            full_screen,
        );
    }

    let result = wait_for_result(
        &systemd.systemctl,
        &unit_name,
        &result_path,
        &stdout_path,
        &stderr_path,
        &presentation_path,
        interrupt,
        full_screen,
    );
    if matches!(result, Ok(WaitOutcome::Interrupted)) {
        return interrupt_worker(
            &systemd.systemctl,
            &unit_name,
            &request_path,
            &result_path,
            &unit_path,
            &stdout_path,
            &stderr_path,
            &presentation_path,
            &credential_path,
            &network_plan_path,
            interrupt,
            full_screen,
        );
    }
    cleanup_files(&[
        &request_path,
        &result_path,
        &unit_path,
        &presentation_path,
        &credential_path,
        &network_plan_path,
    ]);
    if result.is_ok() {
        cleanup_files(&[&stdout_path, &stderr_path]);
    }
    let _ = systemctl(&systemd.systemctl, &["daemon-reload"]);
    result.map(|outcome| match outcome {
        WaitOutcome::Completed(code) => code,
        WaitOutcome::Interrupted => unreachable!("interrupted outcome handled above"),
    })
}

#[allow(clippy::too_many_arguments)]
fn interrupt_worker(
    systemctl_path: &Path,
    unit_name: &str,
    request_path: &Path,
    result_path: &Path,
    unit_path: &Path,
    stdout_path: &Path,
    stderr_path: &Path,
    presentation_path: &Path,
    credential_path: &Path,
    network_plan_path: &Path,
    interrupt: &InterruptGuard,
    full_screen: bool,
) -> Result<ExitCode, String> {
    if let Err(error) = systemctl(systemctl_path, &["stop", unit_name]) {
        eprintln!(
            "install: {}",
            crate::trf!(
                ("warning: Ctrl+C restored the terminal, but the delegated operation could not be stopped and may still be running: {error}"),
                ("警告：Ctrl+C 已恢复终端，但无法停止委托操作，该操作可能仍在运行：{error}")
            )
        );
        return Ok(ExitCode::from(130));
    }
    cleanup_files(&[
        request_path,
        result_path,
        unit_path,
        stdout_path,
        stderr_path,
        presentation_path,
        credential_path,
        network_plan_path,
    ]);
    let _ = systemctl(systemctl_path, &["daemon-reload"]);
    if full_screen {
        crate::interaction::presentation::show_cancelled_screen(interrupt)?;
    }
    Ok(ExitCode::from(130))
}

pub(crate) fn run_worker(request_path: &Path) -> ExitCode {
    match run_worker_inner(request_path) {
        // The command's status is transported through WorkerResult. Once that
        // result is durable, the wrapper itself succeeded and the transient
        // unit must not remain in systemd's failed-unit set.
        Ok(_) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("lkit worker: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_worker_inner(request_path: &Path) -> Result<i32, String> {
    validate_request_path(request_path)?;
    let content = std::fs::read(request_path)
        .map_err(|error| format!("read worker request {}: {error}", request_path.display()))?;
    let request: WorkerRequest = serde_json::from_slice(&content)
        .map_err(|error| format!("parse worker request {}: {error}", request_path.display()))?;
    if request.schema_version != 1 {
        return Err(format!(
            "unsupported worker request schema {}",
            request.schema_version
        ));
    }
    let _ = std::fs::remove_file(request_path);
    let _credential = match request.credential_path.as_deref() {
        Some(path) => {
            validate_credential_path(path)?;
            Some(RemoveFile::new(path))
        }
        None => None,
    };
    let _network_plan = match request.network_plan_path.as_deref() {
        Some(path) => {
            validate_network_plan_path(path)?;
            Some(RemoveFile::new(path))
        }
        None => None,
    };

    let executable =
        std::env::current_exe().map_err(|error| format!("resolve worker executable: {error}"))?;
    // The worker owns the operation after systemd starts it. Losing the
    // originating SSH pty must not terminate the transaction process group.
    unsafe {
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
    }
    let mut command = Command::new(executable);
    command
        .arg("--internal-systemd-worker")
        .args(&request.args)
        .env_clear()
        .envs(request.environment)
        .current_dir(&request.working_directory);
    if let Some(terminal) = request.terminal {
        command.env(SYSTEMD_WORKER_TTY_ENV, terminal);
    } else {
        command.env_remove(SYSTEMD_WORKER_TTY_ENV);
    }
    command.env(PRESENTATION_EVENTS_ENV, &request.presentation_path);
    let status = command
        .status()
        .map_err(|error| format!("run delegated lkit command: {error}"))?;
    let exit_code = status.code().unwrap_or(1);
    write_private_json(
        &request.result_path,
        &WorkerResult {
            schema_version: 1,
            exit_code,
        },
    )?;
    let _ = std::fs::remove_file(&request.unit_path);
    let _ = Command::new(&request.systemctl)
        .arg("daemon-reload")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    Ok(exit_code)
}

pub(crate) fn string_args() -> Result<Vec<String>, String> {
    std::env::args_os()
        .skip(1)
        .map(|value| {
            value.into_string().map_err(|_| {
                "command arguments must be valid UTF-8 for systemd delegation".to_string()
            })
        })
        .collect()
}

fn string_environment() -> Result<Vec<(String, String)>, String> {
    std::env::vars_os()
        .map(|(key, value)| {
            let key = key.into_string().map_err(|_| {
                "environment names must be valid UTF-8 for systemd delegation".to_string()
            })?;
            let value = value
                .into_string()
                .map_err(|_| format!("environment value for {key} must be valid UTF-8"))?;
            Ok((key, value))
        })
        .collect()
}

fn render_unit(
    executable: &Path,
    request_path: &Path,
    stdout_path: &Path,
    stderr_path: &Path,
) -> String {
    format!(
        "[Unit]\nDescription=Landscape Kit operation\n\n[Service]\nType=exec\nExecStart={} {} {}\nKillMode=control-group\nTimeoutStartSec=infinity\nStandardInput=null\nStandardOutput=append:{}\nStandardError=append:{}\n",
        unit_quote(&executable.display().to_string()),
        WORKER_COMMAND,
        unit_quote(&request_path.display().to_string()),
        unit_escape(&stdout_path.display().to_string()),
        unit_escape(&stderr_path.display().to_string()),
    )
}

fn unit_quote(value: &str) -> String {
    format!("\"{}\"", unit_escape(value).replace('"', "\\\""))
}

fn unit_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('%', "%%")
}

fn terminal_path() -> Option<PathBuf> {
    let path = std::fs::read_link("/proc/self/fd/0").ok()?;
    (path.starts_with("/dev/") && !path.as_os_str().is_empty()).then_some(path)
}

fn write_unit(path: &Path, content: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("unit path {} has no parent", path.display()))?;
    if !parent.is_dir() {
        return Err(format!(
            "systemd runtime unit directory {} is missing",
            parent.display()
        ));
    }
    let temporary = path.with_extension("service.tmp");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o644)
        .open(&temporary)
        .map_err(|error| format!("create temporary worker unit: {error}"))?;
    file.write_all(content.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("write worker unit: {error}"))?;
    std::fs::rename(&temporary, path)
        .map_err(|error| format!("install worker unit {}: {error}", path.display()))
}

fn write_private_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let temporary = path.with_extension("tmp");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|error| format!("create {}: {error}", temporary.display()))?;
    serde_json::to_writer_pretty(&mut file, value)
        .map_err(|error| format!("serialize {}: {error}", path.display()))?;
    file.write_all(b"\n")
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("write {}: {error}", path.display()))?;
    std::fs::rename(&temporary, path).map_err(|error| format!("commit {}: {error}", path.display()))
}

fn create_private_file(path: &Path) -> Result<(), String> {
    OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map(|_| ())
        .map_err(|error| format!("create {}: {error}", path.display()))
}

fn create_private_secret_file(path: &Path, content: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| format!("create internal credential file: {error}"))?;
    if let Err(error) = file.write_all(content).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(format!("write internal credential file: {error}"));
    }
    Ok(())
}

fn validate_credential_path(path: &Path) -> Result<(), String> {
    if path.parent() != Some(Path::new(OPERATIONS_DIR))
        || !path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().ends_with(".credential"))
    {
        return Err(format!(
            "internal credential path must be under {OPERATIONS_DIR}"
        ));
    }
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("inspect internal credential file: {error}"))?;
    if !metadata.is_file() || metadata.uid() != 0 || metadata.mode() & 0o077 != 0 {
        return Err("internal credential file must be root-only regular file".into());
    }
    Ok(())
}

fn validate_network_plan_path(path: &Path) -> Result<(), String> {
    if path.parent() != Some(Path::new(OPERATIONS_DIR))
        || !path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().ends_with(".network.json"))
    {
        return Err(format!(
            "internal network plan path must be under {OPERATIONS_DIR}"
        ));
    }
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("inspect internal network plan file: {error}"))?;
    if !metadata.is_file() || metadata.uid() != 0 || metadata.mode() & 0o077 != 0 {
        return Err("internal network plan must be a root-only regular file".into());
    }
    Ok(())
}

pub(crate) fn read_network_plan(path: &Path) -> Result<NetworkPlan, String> {
    validate_network_plan_path(path)?;
    let content =
        std::fs::read(path).map_err(|error| format!("read internal network plan: {error}"))?;
    let plan: NetworkPlan = serde_json::from_slice(&content)
        .map_err(|error| format!("parse internal network plan: {error}"))?;
    plan.validate().map_err(|error| error.to_string())?;
    Ok(plan)
}

struct RemoveFile {
    path: PathBuf,
}

impl RemoveFile {
    fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
        }
    }
}

impl Drop for RemoveFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn validate_request_path(path: &Path) -> Result<(), String> {
    if path.parent() != Some(Path::new(OPERATIONS_DIR)) {
        return Err(format!("worker request must be under {OPERATIONS_DIR}"));
    }
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("inspect worker request {}: {error}", path.display()))?;
    if metadata.uid() != 0 || metadata.mode() & 0o077 != 0 {
        return Err("worker request must be root-owned and inaccessible to group/other".into());
    }
    Ok(())
}

fn systemctl(path: &Path, args: &[&str]) -> Result<(), String> {
    let output = Command::new(path)
        .args(args)
        .output()
        .map_err(|error| format!("execute {} {}: {error}", path.display(), args.join(" ")))?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "{} {} failed: {}",
        path.display(),
        args.join(" "),
        String::from_utf8_lossy(&output.stderr).trim()
    ))
}

fn wait_for_result(
    systemctl_path: &Path,
    unit_name: &str,
    result_path: &Path,
    stdout_path: &Path,
    stderr_path: &Path,
    presentation_path: &Path,
    interrupt: &InterruptGuard,
    full_screen: bool,
) -> Result<WaitOutcome, String> {
    let mut stdout = None;
    let mut stderr = None;
    let mut presentation = WorkerPresentation::new(full_screen);
    let mut inactive_polls = 0_u8;
    loop {
        presentation.drain(presentation_path)?;
        if interrupt.requested() {
            if presentation.is_cancellable() {
                presentation.finish();
                return Ok(WaitOutcome::Interrupted);
            }
            interrupt.clear_request();
            presentation.ignore_stop();
        }
        if let Some(action) = presentation.poll_action()? {
            match action {
                crate::interaction::presentation::PresentationAction::Stop => {
                    presentation.finish();
                    return Ok(WaitOutcome::Interrupted);
                }
                crate::interaction::presentation::PresentationAction::Close => unreachable!(),
            }
        }
        drain_log(stdout_path, &mut stdout, false, &mut presentation)?;
        drain_log(stderr_path, &mut stderr, true, &mut presentation)?;
        if result_path.is_file() {
            let content = std::fs::read(result_path)
                .map_err(|error| format!("read worker result: {error}"))?;
            let result: WorkerResult = serde_json::from_slice(&content)
                .map_err(|error| format!("parse worker result: {error}"))?;
            if result.schema_version != 1 {
                return Err(format!(
                    "unsupported worker result schema {}",
                    result.schema_version
                ));
            }
            presentation.drain(presentation_path)?;
            drain_log(stdout_path, &mut stdout, false, &mut presentation)?;
            drain_log(stderr_path, &mut stderr, true, &mut presentation)?;
            let raw_code = result.exit_code.clamp(0, 255) as u8;
            let code = ExitCode::from(raw_code);
            presentation.show_result(code == ExitCode::SUCCESS);
            if full_screen {
                presentation.wait_for_close(interrupt)?;
            }
            announce_completion(raw_code);
            presentation.finish();
            return Ok(WaitOutcome::Completed(code));
        }

        if worker_unit_has_stopped(systemctl_path, unit_name)? {
            inactive_polls = inactive_polls.saturating_add(1);
            if inactive_polls >= 10 {
                return Err(format!(
                    "worker unit {unit_name} stopped without writing a result; inspect {} and {}",
                    stdout_path.display(),
                    stderr_path.display()
                ));
            }
        } else {
            inactive_polls = 0;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn worker_unit_has_stopped(systemctl_path: &Path, unit_name: &str) -> Result<bool, String> {
    let output = Command::new(systemctl_path)
        .args(["show", "--property=ActiveState", "--value", unit_name])
        .output()
        .map_err(|error| format!("inspect worker unit {unit_name}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "inspect worker unit {unit_name}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(matches!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "inactive" | "failed"
    ))
}

fn drain_log(
    path: &Path,
    file: &mut Option<File>,
    to_stderr: bool,
    presentation: &mut WorkerPresentation,
) -> Result<(), String> {
    if file.is_none() {
        *file = File::open(path).ok();
    }
    let Some(file) = file.as_mut() else {
        return Ok(());
    };
    let mut content = String::new();
    file.read_to_string(&mut content)
        .map_err(|error| format!("read worker log {}: {error}", path.display()))?;
    if content.is_empty() {
        return Ok(());
    }
    if presentation.capture_log(&content) {
        return Ok(());
    }
    presentation.before_log();
    if to_stderr {
        eprint!("{content}");
    } else {
        print!("{content}");
        std::io::stdout()
            .flush()
            .map_err(|error| format!("flush delegated stdout: {error}"))?;
    }
    Ok(())
}

fn cleanup_files(paths: &[&Path]) {
    for path in paths {
        let _ = std::fs::remove_file(path);
    }
}

/// 操作结束后在普通终端输出明确的结果提示。全屏安装页关闭、或命令模式
/// 委托安装的流式输出结束（可能被忽略）后，用户都能看到安装是否完成。
fn announce_completion(exit_code: u8) {
    if exit_code == 0 {
        println!("install: {}", completion_message(exit_code));
    } else {
        eprintln!("install: {}", completion_message(exit_code));
    }
}

fn completion_message(exit_code: u8) -> String {
    if exit_code == 0 {
        crate::tr!("installation complete", "安装完成").into()
    } else {
        crate::trf!(
            ("installation failed with exit code {exit_code}"),
            ("安装失败，退出码 {exit_code}")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_credential_file_is_private_and_removed_by_guard() {
        let path = std::env::temp_dir().join(format!(
            "lkit-worker-credential-{}-{}.credential",
            std::process::id(),
            Uuid::now_v7()
        ));
        create_private_secret_file(&path, b"Secret123").unwrap();
        let metadata = std::fs::metadata(&path).unwrap();
        assert_eq!(metadata.mode() & 0o077, 0);
        assert_eq!(std::fs::read(&path).unwrap(), b"Secret123");
        {
            let _credential = RemoveFile::new(&path);
        }
        assert!(!path.exists());
    }

    #[test]
    fn completion_message_announces_success_and_failure() {
        assert_eq!(completion_message(0), "installation complete");
        let failure = completion_message(3);
        assert!(failure.contains("installation failed with exit code 3"));
        assert!(completion_message(0).contains("installation complete"));
    }
}
