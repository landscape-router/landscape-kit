use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::process::ExitCode;

use chrono::{Duration as ChronoDuration, Utc};

use crate::commands::network::{Network, NetworkAction};
use crate::deployment::plan::InstallError;
use crate::deployment::root::InstallRoot;
use crate::deployment::runtime::InstallRuntime;
use crate::deployment::{lock, plan, root, state, transaction};
use crate::service::{health, systemd};

use super::config::NetworkPlan;

const HOST_SERVICES: [&str; 3] = [
    "NetworkManager.service",
    "firewalld.service",
    "systemd-resolved.service",
];
const UNKNOWN_NETWORK_MANAGERS: [&str; 4] = [
    "systemd-networkd.service",
    "networking.service",
    "wicked.service",
    "connman.service",
];

pub(crate) fn preflight(runtime: &InstallRuntime) -> Result<(), InstallError> {
    if !matches!(
        runtime.systemd.probe(),
        systemd::Availability::Available { .. }
    ) {
        return Err(InstallError::UnsupportedPlatform(
            "network takeover requires a reachable systemd system manager".into(),
        ));
    }
    if selinux_enabled(&runtime.selinux_fs_path, &runtime.selinux_config_path)? {
        return Err(InstallError::UnsupportedPlatform(
            "network takeover does not support systems where SELinux is loaded or enabled".into(),
        ));
    }
    for unit in UNKNOWN_NETWORK_MANAGERS {
        let before = systemd::inspect_host_service(&runtime.systemd, unit)?;
        if before.active {
            return Err(InstallError::Preflight(format!(
                "unknown network manager {unit} is active; stop it before network takeover"
            )));
        }
    }
    Ok(())
}

pub(crate) fn prepare_transaction(
    transaction_id: &str,
    plan: &NetworkPlan,
    runtime: &InstallRuntime,
) -> Result<transaction::NetworkTakeoverTransaction, InstallError> {
    let host_services = HOST_SERVICES
        .iter()
        .map(|unit| systemd::inspect_host_service(&runtime.systemd, unit))
        .collect::<Result<Vec<_>, _>>()?;
    let stem = format!("lkit-network-{}", transaction_id);
    let timeout = ChronoDuration::from_std(runtime.network_confirm_timeout).map_err(|_| {
        InstallError::ParameterUsage("network confirmation timeout is too large".into())
    })?;
    Ok(transaction::NetworkTakeoverTransaction {
        plan: plan.clone(),
        host_services,
        confirmation_deadline: Utc::now() + timeout,
        rollback_service: format!("{stem}-rollback.service"),
        rollback_timer: format!("{stem}-rollback.timer"),
        boot_rollback_service: format!("{stem}-boot-rollback.service"),
        recovery_binary: "service/lkit-network-recovery".into(),
        pending_state: format!("transactions/{transaction_id}/pending-install-state.json"),
    })
}

pub(crate) fn refresh_confirmation_deadline(
    network: &mut transaction::NetworkTakeoverTransaction,
    runtime: &InstallRuntime,
) -> Result<(), InstallError> {
    let timeout = ChronoDuration::from_std(runtime.network_confirm_timeout).map_err(|_| {
        InstallError::ParameterUsage("network confirmation timeout is too large".into())
    })?;
    network.confirmation_deadline = Utc::now() + timeout;
    Ok(())
}

pub(crate) fn arm_recovery(
    root: &InstallRoot,
    network: &transaction::NetworkTakeoverTransaction,
    runtime: &InstallRuntime,
) -> Result<(), InstallError> {
    let recovery = root.canonical.join(&network.recovery_binary);
    if let Some(parent) = recovery.parent() {
        std::fs::create_dir_all(parent).map_err(InstallError::Io)?;
    }
    std::fs::copy(
        std::env::current_exe().map_err(InstallError::Io)?,
        &recovery,
    )
    .map_err(InstallError::Io)?;
    std::fs::set_permissions(&recovery, std::fs::Permissions::from_mode(0o700))
        .map_err(InstallError::Io)?;

    let runtime_arg = runtime
        .test_runtime_path
        .as_ref()
        .map(|path| format!(" --test-runtime={}", unit_quote(path)))
        .unwrap_or_default();
    let rollback_command = format!(
        "{} network rollback --automatic --install-dir={}{}",
        unit_quote(&recovery),
        unit_quote(&root.canonical),
        runtime_arg
    );
    let rollback = format!(
        "[Unit]\nDescription=Rollback unconfirmed Landscape network takeover\nAfter=local-fs.target\n\n[Service]\nType=oneshot\nExecStart={rollback_command}\nRestart=on-failure\nRestartSec=10s\n"
    );
    let timer_seconds = runtime.network_confirm_timeout.as_secs().max(1);
    let timer = format!(
        "[Unit]\nDescription=Network takeover confirmation deadline\n\n[Timer]\nOnActiveSec={timer_seconds}s\nPersistent=true\nAccuracySec=1s\nUnit={}\n\n[Install]\nWantedBy=timers.target\n",
        network.rollback_service
    );
    let boot = format!(
        "[Unit]\nDescription=Rollback network takeover after unconfirmed reboot\nDefaultDependencies=no\nAfter=local-fs.target\nBefore=landscape-router.service network-online.target\n\n[Service]\nType=oneshot\nExecStart={rollback_command}\n\n[Install]\nWantedBy=multi-user.target\n"
    );
    write_system_unit(&runtime.systemd, &network.rollback_service, &rollback)?;
    write_system_unit(&runtime.systemd, &network.rollback_timer, &timer)?;
    write_system_unit(&runtime.systemd, &network.boot_rollback_service, &boot)?;
    systemd::daemon_reload(&runtime.systemd)?;
    systemd::unit_command(&runtime.systemd, "enable", &network.boot_rollback_service)?;
    systemd::unit_command(&runtime.systemd, "enable", &network.rollback_timer)?;
    systemd::unit_command(&runtime.systemd, "start", &network.rollback_timer)
}

pub(crate) fn stop_host_services(
    network: &transaction::NetworkTakeoverTransaction,
    systemd: &systemd::Systemd,
) -> Result<(), InstallError> {
    for before in network.host_services.iter().rev() {
        systemd::stop_disable_mask_host_service(systemd, before)?;
    }
    Ok(())
}

pub(crate) fn cleanup_failed_takeover(
    root: &InstallRoot,
    network: &transaction::NetworkTakeoverTransaction,
    systemd: &systemd::Systemd,
) -> Result<(), InstallError> {
    for before in &network.host_services {
        systemd::restore_host_service(systemd, before)?;
    }
    remove_recovery_units(root, network, systemd, false)
}

pub(crate) fn write_pending_state(
    root: &InstallRoot,
    network: &transaction::NetworkTakeoverTransaction,
    state: &state::InstallState,
) -> Result<(), InstallError> {
    let path = root.canonical.join(&network.pending_state);
    let parent = path.parent().ok_or_else(|| {
        InstallError::CorruptedTransaction("pending state path has no parent".into())
    })?;
    std::fs::create_dir_all(parent).map_err(InstallError::Io)?;
    write_private_json(&path, state)
}

pub(crate) async fn run_command(args: &Network) -> ExitCode {
    match run_command_inner(args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("network: {error}");
            match error {
                InstallError::ParameterUsage(_) => ExitCode::from(2),
                _ => ExitCode::FAILURE,
            }
        }
    }
}

async fn run_command_inner(args: &Network) -> Result<(), InstallError> {
    let runtime = resolve_runtime(args)?;
    if !runtime.allow_non_root && unsafe { libc::geteuid() } != 0 {
        return Err(InstallError::UnsupportedPlatform(
            "network commands must run as root (uid 0)".into(),
        ));
    }
    let selected = plan::select_install_root(
        args.install_dir.as_deref(),
        std::env::var("LKIT_INSTALL_DIR").ok().as_deref(),
    )?;
    let root = root::normalize_install_root(&selected)?;
    let _lock = lock::acquire_install_lock(&root)?;
    match &args.action {
        NetworkAction::Status => status(&root),
        NetworkAction::Confirm => confirm(&root, &runtime).await,
        NetworkAction::Rollback { automatic } => rollback(&root, &runtime, *automatic),
    }
}

fn status(root: &InstallRoot) -> Result<(), InstallError> {
    if let Some(transaction) = transaction::find_unfinished(root)? {
        let network = transaction.network_takeover.as_ref().ok_or_else(|| {
            InstallError::BlockedByTransaction(format!(
                "unfinished {} transaction is not a network takeover",
                transaction.operation.key()
            ))
        })?;
        println!("network: transaction {}", transaction.transaction_id);
        println!("network: phase {}", transaction.phase.key());
        println!(
            "network: management address {}",
            network.plan.management_address()
        );
        println!(
            "network: confirmation deadline {}",
            network.confirmation_deadline.to_rfc3339()
        );
    } else if state::load_state(root)?.is_some() {
        println!("network: no takeover is awaiting confirmation");
    } else {
        return Err(InstallError::ParameterUsage(
            "no Landscape installation or pending network takeover exists".into(),
        ));
    }
    Ok(())
}

async fn confirm(root: &InstallRoot, runtime: &InstallRuntime) -> Result<(), InstallError> {
    let mut pending = transaction::find_unfinished(root)?.ok_or_else(|| {
        InstallError::ParameterUsage("no network takeover is awaiting confirmation".into())
    })?;
    if pending.phase != transaction::Phase::AwaitingNetworkConfirmation
        && pending.phase != transaction::Phase::Finalizing
    {
        return Err(InstallError::BlockedByTransaction(format!(
            "transaction {} is {}, not awaiting network confirmation",
            pending.transaction_id,
            pending.phase.key()
        )));
    }
    let network = pending.network_takeover.clone().ok_or_else(|| {
        InstallError::CorruptedTransaction("pending install has no network takeover state".into())
    })?;
    if pending.phase == transaction::Phase::AwaitingNetworkConfirmation {
        if Utc::now() > network.confirmation_deadline {
            return Err(InstallError::ParameterUsage(
                "network confirmation deadline has expired; wait for automatic rollback or run `lkit network rollback`"
                    .into(),
            ));
        }
        verify_confirmation_session(&network.plan)?;
        verify_interfaces(&network.plan, runtime)?;
        super::discovery::verify_live(&network.plan, &runtime.ip_command)?;
        let pid = systemd::main_pid(&runtime.systemd)?;
        if pid == 0 {
            return Err(InstallError::HealthCheck(
                "Landscape has no running MainPID".into(),
            ));
        }
        let health_options = runtime.health_options()?;
        let options = health::StartupOptions {
            ports: &health_options.ports,
            expected_pid: pid,
            docs: &health_options.docs,
            unit_state: Some(&(|| systemd::active_state(&runtime.systemd).ok())),
            init_required: true,
            data_dir: &root.canonical.join("data"),
            startup_timeout: health_options.startup_timeout,
            stable_duration: health_options.stable_duration,
        };
        health::wait_for_startup(&options).await?;
        pending.phase = transaction::Phase::Finalizing;
        pending.updated_at = Utc::now();
        transaction::persist(root, &pending)?;
    }
    remove_recovery_units(root, &network, &runtime.systemd, false)?;
    let bytes =
        std::fs::read(root.canonical.join(&network.pending_state)).map_err(InstallError::Io)?;
    let mut install_state: state::InstallState =
        serde_json::from_slice(&bytes).map_err(|error| {
            InstallError::CorruptedState(format!("pending install state is invalid: {error}"))
        })?;
    state::validate_state(&install_state)?;
    install_state.last_transaction_id = Some(pending.transaction_id.clone());
    install_state.committed_at = Some(Utc::now());
    state::write_state(root, &install_state)?;
    transaction::mark_phase(root, &pending, transaction::Phase::Committed)?;
    let _ = std::fs::remove_file(root.canonical.join(&network.pending_state));
    println!("network: confirmed Landscape network takeover");
    Ok(())
}

fn rollback(
    root: &InstallRoot,
    runtime: &InstallRuntime,
    automatic: bool,
) -> Result<(), InstallError> {
    let pending = transaction::find_unfinished(root)?.ok_or_else(|| {
        InstallError::ParameterUsage("no network takeover is available to roll back".into())
    })?;
    let network = pending.network_takeover.as_ref().ok_or_else(|| {
        InstallError::CorruptedTransaction(
            "unfinished install has no network takeover state".into(),
        )
    })?;
    transaction::mark_phase(root, &pending, transaction::Phase::RollingBack)?;
    transaction::cleanup_failed_first_install(root, &pending, &runtime.systemd)?;
    for before in &network.host_services {
        systemd::restore_host_service(&runtime.systemd, before)?;
    }
    remove_recovery_units(root, network, &runtime.systemd, automatic)?;
    transaction::mark_phase(root, &pending, transaction::Phase::RolledBack)?;
    println!("network: restored the pre-install host network services");
    Ok(())
}

fn verify_confirmation_session(plan: &NetworkPlan) -> Result<(), InstallError> {
    let current =
        super::discovery::ssh_server_address(std::env::var("SSH_CONNECTION").ok().as_deref())?
            .ok_or_else(|| {
                InstallError::ParameterUsage(
                    "network confirmation must be run from the reconnected SSH session".into(),
                )
            })?;
    if current != plan.management_address().address {
        return Err(InstallError::ParameterUsage(format!(
            "current SSH session targets {current}, expected {}",
            plan.management_address().address
        )));
    }
    Ok(())
}

fn verify_interfaces(plan: &NetworkPlan, runtime: &InstallRuntime) -> Result<(), InstallError> {
    let (interfaces, _) = super::discovery::discover(&runtime.sys_class_net, &runtime.ip_command)?;
    for selected in &plan.selected_macs {
        let current = interfaces
            .iter()
            .find(|iface| iface.name == selected.name)
            .ok_or_else(|| {
                InstallError::Preflight(format!("selected interface {} disappeared", selected.name))
            })?;
        if !current.mac.eq_ignore_ascii_case(&selected.mac) {
            return Err(InstallError::Preflight(format!(
                "selected interface {} changed MAC from {} to {}",
                selected.name, selected.mac, current.mac
            )));
        }
    }
    Ok(())
}

fn remove_recovery_units(
    root: &InstallRoot,
    network: &transaction::NetworkTakeoverTransaction,
    systemd: &systemd::Systemd,
    running_rollback: bool,
) -> Result<(), InstallError> {
    for unit in [&network.rollback_timer, &network.boot_rollback_service] {
        if !running_rollback {
            let _ = systemd::unit_command(systemd, "stop", unit);
        }
        let _ = systemd::unit_command(systemd, "disable", unit);
    }
    if !running_rollback {
        let _ = systemd::unit_command(systemd, "stop", &network.rollback_service);
    }
    for unit in [
        &network.rollback_timer,
        &network.rollback_service,
        &network.boot_rollback_service,
    ] {
        let _ = std::fs::remove_file(systemd.system_unit_dir.join(unit));
    }
    systemd::daemon_reload(systemd)?;
    let _ = std::fs::remove_file(root.canonical.join(&network.recovery_binary));
    Ok(())
}

fn write_system_unit(
    systemd: &systemd::Systemd,
    name: &str,
    content: &str,
) -> Result<(), InstallError> {
    let path = systemd.system_unit_dir.join(name);
    if path.exists() {
        return Err(InstallError::Systemd(format!(
            "refusing to overwrite foreign recovery unit {}",
            path.display()
        )));
    }
    let tmp = systemd.system_unit_dir.join(format!(".{name}.tmp"));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .open(&tmp)
        .map_err(InstallError::Io)?;
    file.write_all(content.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(InstallError::Io)?;
    std::fs::rename(&tmp, &path).map_err(|error| {
        let _ = std::fs::remove_file(&tmp);
        InstallError::Io(error)
    })
}

fn write_private_json(path: &Path, value: &impl serde::Serialize) -> Result<(), InstallError> {
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(value).map_err(InstallError::StateWrite)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(InstallError::Io)?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(InstallError::Io)?;
    std::fs::rename(&tmp, path).map_err(InstallError::Io)
}

fn unit_quote(path: &Path) -> String {
    format!(
        "\"{}\"",
        path.display()
            .to_string()
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
    )
}

fn selinux_enabled(fs_path: &Path, config_path: &Path) -> Result<bool, InstallError> {
    if fs_path.exists() {
        return Ok(true);
    }
    let content = match std::fs::read_to_string(config_path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(InstallError::Io(error)),
    };
    Ok(content.lines().any(|line| {
        let line = line.trim();
        !line.starts_with('#')
            && line
                .split_once('=')
                .is_some_and(|(key, value)| key.trim() == "SELINUX" && value.trim() != "disabled")
    }))
}

fn resolve_runtime(args: &Network) -> Result<InstallRuntime, InstallError> {
    #[cfg(feature = "test-support")]
    if let Some(path) = args.test_runtime.as_deref() {
        return InstallRuntime::from_test_file(path);
    }
    Ok(InstallRuntime::production())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_selinux_config_even_when_not_mounted() {
        let dir = std::env::temp_dir().join(format!("lkit-selinux-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("config");
        std::fs::write(&config, b"SELINUX=permissive\n").unwrap();
        assert!(selinux_enabled(&dir.join("missing"), &config).unwrap());
        std::fs::write(&config, b"SELINUX=disabled\n").unwrap();
        assert!(!selinux_enabled(&dir.join("missing"), &config).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recovery_unit_command_keeps_test_runtime() {
        let path = Path::new("/tmp/runtime file.json");
        assert_eq!(unit_quote(path), "\"/tmp/runtime file.json\"");
    }
}
