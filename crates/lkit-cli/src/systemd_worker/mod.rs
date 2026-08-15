mod interrupt;
mod protocol;
mod unit;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use uuid::Uuid;

use crate::commands::network::NetworkAction;
use crate::commands::{Commands, ServiceManagerArg};
use crate::deployment::state::StateServiceManager;
use crate::interaction::interactive::SYSTEMD_WORKER_TTY_ENV;
use crate::interaction::presentation::{
    InterruptGuard, OPERATIONS_DIR, PRESENTATION_EVENTS_ENV, operation_screen,
};
use crate::network::config::NetworkPlan;
use crate::service::systemd::{Availability, Systemd};

use self::interrupt::{interrupt_worker, wait_for_result};
use self::protocol::{
    RemoveFile, WaitOutcome, WorkerRequest, WorkerResult, string_environment,
    validate_credential_path, validate_network_plan_path, validate_request_path,
};
pub(crate) use self::protocol::{read_network_plan, string_args};
use self::unit::{
    create_private_file, create_private_secret_file, render_unit, terminal_path,
    write_private_json, write_unit,
};

pub(crate) const WORKER_COMMAND: &str = "__systemd-worker";

pub(crate) fn should_delegate(command: &Commands) -> bool {
    if unsafe { libc::geteuid() } != 0 || test_runtime_is_inline(command) {
        return false;
    }
    match command {
        Commands::Check(_) | Commands::Reconcile(_) | Commands::SetMirror(_) => false,
        Commands::Software(_) => false,
        Commands::Backup(_) => false,
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
        Commands::Migrate(args) => match args.service_manager {
            Some(ServiceManagerArg::Systemd) => true,
            Some(ServiceManagerArg::None) => false,
            None => matches!(Systemd::host().probe(), Availability::Available { .. }),
        },
        Commands::Switch(args) => {
            load_manager(args.install_dir.as_deref()) == Some(StateServiceManager::Systemd)
        }
        Commands::Update(args) => {
            load_manager(args.install_dir.as_deref()) == Some(StateServiceManager::Systemd)
        }
        Commands::Repair(args) => {
            args.target == crate::commands::repair::RepairTarget::Binary
                && load_manager(args.install_dir.as_deref()) == Some(StateServiceManager::Systemd)
        }
        Commands::Restore(args) => {
            load_manager(args.install_dir.as_deref()) == Some(StateServiceManager::Systemd)
        }
        Commands::Reinit(args) => {
            load_manager(args.install_dir.as_deref()) == Some(StateServiceManager::Systemd)
        }
        Commands::Uninstall(args) => {
            load_manager(args.install_dir.as_deref()) == Some(StateServiceManager::Systemd)
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
        Commands::SetMirror(_) => None,
        Commands::Software(_) => None,
        Commands::Install(args) => args.test_runtime.as_deref(),
        Commands::Migrate(args) => args.test_runtime.as_deref(),
        Commands::Switch(args) => args.test_runtime.as_deref(),
        Commands::Update(args) => args.test_runtime.as_deref(),
        Commands::Repair(args) => args.test_runtime.as_deref(),
        Commands::Restore(args) => args.test_runtime.as_deref(),
        Commands::Reinit(args) => args.test_runtime.as_deref(),
        Commands::Uninstall(args) => args.test_runtime.as_deref(),
        Commands::Backup(args) => match &args.action {
            crate::commands::backup::BackupAction::Create(args) => args.test_runtime.as_deref(),
            crate::commands::backup::BackupAction::List(args) => args.test_runtime.as_deref(),
            crate::commands::backup::BackupAction::Show(args) => args.test_runtime.as_deref(),
            crate::commands::backup::BackupAction::Verify(args) => args.test_runtime.as_deref(),
            crate::commands::backup::BackupAction::Delete(_) => None,
        },
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
    let operation = operation_screen(&args);
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
        operation,
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

fn cleanup_files(paths: &[&Path]) {
    for path in paths {
        let _ = std::fs::remove_file(path);
    }
}
