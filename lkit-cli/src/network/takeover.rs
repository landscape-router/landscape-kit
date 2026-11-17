use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::process::{Command, ExitCode};

use chrono::{Duration as ChronoDuration, Utc};

use crate::commands::network::{Network, NetworkAction};
use crate::deployment::layout;
use crate::deployment::plan::InstallError;
use crate::deployment::root::InstallRoot;
use crate::deployment::runtime::InstallRuntime;
use crate::deployment::{lock, state, transaction};
use crate::service::manager::{ManagedService, ServiceManager};
use crate::service::{health, host_network, systemd};

use super::config::{NetworkMode, NetworkPlan};

const UNKNOWN_NETWORK_MANAGERS: [&str; 4] = [
    "systemd-networkd.service",
    "wicked.service",
    "connman.service",
    "dhcpcd.service",
];

pub(crate) fn preflight(runtime: &InstallRuntime) -> Result<(), InstallError> {
    let manager = runtime.service_manager.as_ref();
    if !matches!(
        manager.probe(),
        crate::service::manager::Availability::Available { .. }
    ) {
        return Err(InstallError::UnsupportedPlatform(
            "network takeover requires a reachable service manager".into(),
        ));
    }
    require_recovery_daemon(runtime)?;
    if selinux_enabled(&runtime.selinux_fs_path, &runtime.selinux_config_path)? {
        return Err(InstallError::UnsupportedPlatform(
            "network takeover does not support systems where SELinux is loaded or enabled".into(),
        ));
    }
    for unit in UNKNOWN_NETWORK_MANAGERS {
        if unit == "systemd-networkd.service"
            && manager.kind() == crate::service::manager::ServiceManagerKind::Systemd
        {
            continue;
        }
        let name = if manager.kind() == crate::service::manager::ServiceManagerKind::Systemd {
            unit
        } else {
            unit.trim_end_matches(".service")
        };
        let before = host_network::inspect(manager, name)?;
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
    let manager = runtime.service_manager.as_ref();
    let host_services = host_network::inspect_all(manager)?;

    let stem = format!("lkit-network-{}", transaction_id);
    let timeout = ChronoDuration::from_std(runtime.network_confirm_timeout).map_err(|_| {
        InstallError::ParameterUsage("network confirmation timeout is too large".into())
    })?;
    Ok(transaction::NetworkTakeoverTransaction {
        boot_id: Some(current_boot_id()?),
        plan: plan.clone(),
        network_manager: Some(super::network_manager::capture(runtime, &host_services)?),
        preserve_startup_links: true,
        host_services,
        confirmation_deadline: Utc::now() + timeout,
        rollback_service: format!("{stem}-rollback.service"),
        rollback_timer: format!("{stem}-rollback.timer"),
        boot_rollback_service: format!("{stem}-boot-rollback.service"),
        recovery_binary: "service/lkit-network-recovery".into(),
        pending_state: format!("transactions/{transaction_id}/pending-install-state.json"),
    })
}

pub(crate) fn require_existing_takeover(
    root: &InstallRoot,
    installed: &state::InstallState,
    manager: &dyn ServiceManager,
) -> Result<(), InstallError> {
    require_takeover_identity(root, installed, manager.kind())?;
    host_network::require_taken_over(manager, &host_network::inspect_all(manager)?)
}

/// The console can check durable eligibility without querying host services on every frame.
/// Submission and daemon execution must also call `require_existing_takeover`.
pub(crate) fn require_takeover_identity(
    root: &InstallRoot,
    installed: &state::InstallState,
    manager: crate::service::manager::ServiceManagerKind,
) -> Result<(), InstallError> {
    transaction::latest_committed_network_takeover(root)?.ok_or_else(|| {
        InstallError::ParameterUsage(crate::tr!(crate::keys::REINIT_REQUIRES_NETWORK_TAKEOVER))
    })?;
    if installed.service.manager != manager {
        return Err(InstallError::Preflight(
            "network takeover service backend does not match the current backend".into(),
        ));
    }
    Ok(())
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
    let _ = (root, network);
    require_recovery_daemon(runtime)
}

pub(crate) fn current_boot_id() -> Result<String, InstallError> {
    let value =
        std::fs::read_to_string("/proc/sys/kernel/random/boot_id").map_err(InstallError::Io)?;
    let value = value.trim();
    uuid::Uuid::parse_str(value)
        .map_err(|_| InstallError::Preflight("invalid host boot id".into()))?;
    Ok(value.into())
}

fn require_recovery_daemon(runtime: &InstallRuntime) -> Result<(), InstallError> {
    if runtime.test_runtime_path.is_some() {
        return Ok(());
    }
    let manager = runtime.service_manager.as_ref();
    crate::daemon::require_capability("network-recovery-v1.pid")?;
    if !manager.is_enabled(ManagedService::LkitDaemon)?
        || !manager.is_active(ManagedService::LkitDaemon)?
    {
        return Err(InstallError::Preflight(
            "network takeover requires an enabled, running lkit daemon; run lkit self install"
                .into(),
        ));
    }
    Ok(())
}

pub(crate) fn stop_host_services(
    network: &transaction::NetworkTakeoverTransaction,
    manager: &dyn ServiceManager,
) -> Result<(), InstallError> {
    if manager.kind() == crate::service::manager::ServiceManagerKind::Systemd {
        return crate::service::host_systemd::stop(
            crate::service::systemd::downcast(manager)?,
            &network.host_services,
        );
    }
    for before in network.host_services.iter().rev() {
        host_network::stop(manager, before)?;
    }
    Ok(())
}

pub(crate) fn cleanup_failed_takeover(
    root: &InstallRoot,
    network: &transaction::NetworkTakeoverTransaction,
    manager: &dyn ServiceManager,
) -> Result<(), InstallError> {
    host_network::restore_all(
        manager,
        &network.host_services,
        network.preserve_startup_links,
    )?;
    super::network_manager::restore(network)?;
    cleanup_recovery(root, network, manager, false)
}

pub(crate) fn write_pending_state(
    _root: &InstallRoot,
    network: &transaction::NetworkTakeoverTransaction,
    state: &state::InstallState,
) -> Result<(), InstallError> {
    let path = layout::territory_relative(&network.pending_state);
    let parent = path.parent().ok_or_else(|| {
        InstallError::CorruptedTransaction("pending state path has no parent".into())
    })?;
    std::fs::create_dir_all(parent).map_err(InstallError::Io)?;
    write_private_json(&path, state)
}

pub(crate) async fn run_command(args: &Network) -> ExitCode {
    let native = matches!(
        args.action,
        NetworkAction::Confirm | NetworkAction::Rollback { automatic: false }
    );
    #[cfg(feature = "test-support")]
    let native = native
        && args
            .test_runtime
            .as_deref()
            .is_none_or(|path| InstallRuntime::test_uses_daemon(path).unwrap_or(false));
    if native {
        match submit_native(args).await {
            Ok(code) => return ExitCode::from(code),
            Err(error) => {
                eprintln!("network: {error}");
                return ExitCode::from(2);
            }
        }
    }
    match run_command_inner(args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("network: {error}");
            match error {
                InstallError::ParameterUsage(_) => ExitCode::from(2),
                _ if matches!(&args.action, NetworkAction::Rollback { .. }) => ExitCode::from(6),
                _ => ExitCode::FAILURE,
            }
        }
    }
}

async fn run_command_inner(args: &Network) -> Result<(), InstallError> {
    if matches!(args.action, NetworkAction::StartupCheck) {
        return super::recovery::startup_check();
    }
    let runtime = resolve_runtime(args)?;
    if !runtime.allow_non_root && unsafe { libc::geteuid() } != 0 {
        return Err(InstallError::UnsupportedPlatform(crate::tr!(
            crate::keys::TAKEOVER_NETWORK_COMMANDS_REQUIRE_ROOT
        )));
    }
    // 待确认的接管安装还没有提交状态:状态缺失时从未完成事务发现根。
    let normalized = match state::discover_landscape_root()? {
        Some(root) => root,
        None => match state::discover_landscape_root_from_unfinished_transaction()? {
            Some(root) => root,
            None => {
                return Err(InstallError::ParameterUsage(
                    "no installed landscape found; run `lkit install` first".into(),
                ));
            }
        },
    };
    let root = normalized;
    let _lock = lock::acquire_install_lock()?;
    match &args.action {
        NetworkAction::StartupCheck => unreachable!(),
        NetworkAction::Status => status(&root),
        NetworkAction::Confirm => confirm(&root, &runtime).await,
        NetworkAction::Rollback { automatic } => rollback(&root, &runtime, *automatic).await,
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
        println!(
            "network: {} {}",
            crate::tr!(crate::keys::TAKEOVER_TRANSACTION),
            transaction.transaction_id
        );
        println!(
            "network: {} {}",
            crate::tr!(crate::keys::TAKEOVER_PHASE),
            transaction.phase.key()
        );
        println!(
            "network: {} {}",
            crate::tr!(crate::keys::TAKEOVER_MANAGEMENT_ADDRESS),
            network
                .plan
                .management_address()
                .map(|address| address.to_string())
                .unwrap_or_else(|| crate::tr!(crate::keys::TAKEOVER_DHCP_LEASE))
        );
        println!(
            "network: {} {}",
            crate::tr!(crate::keys::TAKEOVER_CONFIRMATION_DEADLINE),
            network.confirmation_deadline.to_rfc3339()
        );
    } else if state::load_state(root)?.is_some() {
        println!(
            "network: {}",
            crate::tr!(crate::keys::TAKEOVER_NO_TAKEOVER_AWAITING_CONFIRMATION)
        );
    } else {
        return Err(InstallError::ParameterUsage(
            "no Landscape installation or pending network takeover exists".into(),
        ));
    }
    Ok(())
}

pub(super) fn check_confirmation_window(
    network: &transaction::NetworkTakeoverTransaction,
    now: chrono::DateTime<Utc>,
    boot: Result<String, InstallError>,
) -> Result<(), InstallError> {
    let boot = boot?;
    if now >= network.confirmation_deadline
        || network.boot_id.as_ref().is_some_and(|saved| saved != &boot)
    {
        return Err(InstallError::ParameterUsage("network confirmation expired or host restarted; wait for automatic rollback or run `lkit network rollback`".into()));
    }
    Ok(())
}

pub(crate) async fn confirm(
    root: &InstallRoot,
    runtime: &InstallRuntime,
) -> Result<(), InstallError> {
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
        InstallError::CorruptedTransaction(
            "pending transaction has no network takeover state".into(),
        )
    })?;
    let manager = runtime.service_manager.as_ref();
    if pending.phase == transaction::Phase::AwaitingNetworkConfirmation {
        check_confirmation_window(&network, Utc::now(), current_boot_id())?;
        verify_interfaces(&network.plan, runtime)?;
        super::discovery::verify_live(&network.plan, &runtime.ip_command)?;
        let pid = manager.main_pid(ManagedService::LandscapeRouter)?;
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
            unit_state: Some(&(|| manager.active_state(ManagedService::LandscapeRouter).ok())),
            init_required: true,
            data_dir: &root.canonical.join("data"),
            startup_timeout: health_options.startup_timeout,
            stable_duration: health_options.stable_duration,
        };
        health::wait_for_startup(&options).await?;
        clear_inherited_wan_ipv4(&network.plan, &runtime.ip_command)?;
        check_confirmation_window(&network, Utc::now(), current_boot_id())?;
        pending.phase = transaction::Phase::Finalizing;
        pending.updated_at = Utc::now();
        transaction::persist(root, &pending)?;
        #[cfg(feature = "test-support")]
        crate::workflows::operation_control::checkpoint("network-finalizing")?;
    }
    cleanup_recovery(root, &network, manager, false)?;
    let bytes = std::fs::read(layout::territory_relative(&network.pending_state))
        .map_err(InstallError::Io)?;
    let mut install_state: state::InstallState =
        serde_json::from_slice(&bytes).map_err(|error| {
            InstallError::CorruptedState(format!("pending install state is invalid: {error}"))
        })?;
    state::validate_state(&install_state)?;
    install_state.last_transaction_id = Some(pending.transaction_id.clone());
    install_state.committed_at = Some(Utc::now());
    state::write_state(root, &install_state)?;
    transaction::mark_phase(root, &pending, transaction::Phase::Committed)?;
    let _ = std::fs::remove_file(layout::territory_relative(&network.pending_state));
    println!(
        "network: {}",
        crate::tr!(crate::keys::TAKEOVER_CONFIRMED_LANDSCAPE_TAKEOVER)
    );
    Ok(())
}

fn clear_inherited_wan_ipv4(plan: &NetworkPlan, ip_command: &Path) -> Result<(), InstallError> {
    let NetworkMode::RoutedLan {
        wan,
        wan_ipv4: None,
        ..
    } = &plan.mode
    else {
        return Ok(());
    };
    let output = Command::new(ip_command)
        .args(["-4", "address", "flush", "dev", wan])
        .output()
        .map_err(InstallError::Io)?;
    if !output.status.success() {
        return Err(InstallError::HealthCheck(format!(
            "cannot remove the inherited IPv4 addresses from WAN {wan}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

pub(crate) fn clear_selected_lan_addresses(
    plan: &NetworkPlan,
    ip_command: &Path,
) -> Result<(), InstallError> {
    let NetworkMode::RoutedLan { lan, .. } = &plan.mode else {
        return Ok(());
    };
    for iface in lan {
        for family in ["-4", "-6"] {
            let output = Command::new(ip_command)
                .args([family, "address", "flush", "dev", iface])
                .output()
                .map_err(InstallError::Io)?;
            if !output.status.success() {
                return Err(InstallError::HealthCheck(format!(
                    "cannot remove inherited {family} addresses from LAN {iface}: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                )));
            }
        }
    }
    Ok(())
}

fn rollback_allowed(phase: transaction::Phase) -> bool {
    matches!(
        phase,
        transaction::Phase::AwaitingNetworkConfirmation | transaction::Phase::RollingBack
    )
}

pub(crate) async fn rollback(
    root: &InstallRoot,
    runtime: &InstallRuntime,
    automatic: bool,
) -> Result<(), InstallError> {
    let pending = transaction::find_unfinished(root)?.ok_or_else(|| {
        InstallError::ParameterUsage("no network takeover is available to roll back".into())
    })?;
    let network = pending.network_takeover.clone().ok_or_else(|| {
        InstallError::CorruptedTransaction(
            "unfinished transaction has no network takeover state".into(),
        )
    })?;
    if !matches!(
        pending.operation,
        transaction::Operation::Install | transaction::Operation::Reinit
    ) || !rollback_allowed(pending.phase)
    {
        return Err(InstallError::BlockedByTransaction(format!(
            "transaction {} is {}; network rollback only handles an uncommitted confirmation phase",
            pending.transaction_id,
            pending.phase.key()
        )));
    }
    let health = runtime.health_options()?;
    transaction::mark_phase(root, &pending, transaction::Phase::RollingBack)?;
    #[cfg(feature = "test-support")]
    crate::workflows::operation_control::checkpoint("network-rolling-back")?;
    let result: Result<(), InstallError> = async {
        match pending.operation {
            transaction::Operation::Install if pending.schema_version == 12 => {
                crate::workflows::install::native::recover(
                    root,
                    &pending,
                    runtime.service_manager.as_ref(),
                )?;
            }
            transaction::Operation::Install => {
                transaction::restore_uncommitted_network_systemd(
                    root,
                    &pending,
                    runtime.service_manager.as_ref(),
                )?;
                host_network::restore_all(
                    runtime.service_manager.as_ref(),
                    &network.host_services,
                    network.preserve_startup_links,
                )?;
                super::network_manager::restore(&network)?;
                cleanup_recovery(root, &network, runtime.service_manager.as_ref(), automatic)?;
            }
            transaction::Operation::Reinit => {
                crate::workflows::reinit::rollback_reinit_inner(
                    root,
                    &pending,
                    runtime.service_manager.as_ref(),
                    &health,
                )
                .await?;
                cleanup_recovery(root, &network, runtime.service_manager.as_ref(), automatic)?;
            }
            other => {
                return Err(InstallError::BlockedByTransaction(format!(
                    "network rollback does not handle {} transactions",
                    other.key()
                )));
            }
        }
        if pending.operation == transaction::Operation::Install && pending.schema_version != 12 {
            transaction::cleanup_uncommitted_network_install(root, &pending)?;
        }
        Ok(())
    }
    .await;
    result?;
    transaction::mark_phase(root, &pending, transaction::Phase::RolledBack)?;
    println!(
        "network: {}",
        crate::tr!(crate::keys::TAKEOVER_RESTORED_HOST_NETWORK_SERVICES)
    );
    Ok(())
}

pub(crate) fn verify_interfaces(
    plan: &NetworkPlan,
    runtime: &InstallRuntime,
) -> Result<(), InstallError> {
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

fn cleanup_recovery(
    root: &InstallRoot,
    network: &transaction::NetworkTakeoverTransaction,
    manager: &dyn ServiceManager,
    automatic: bool,
) -> Result<(), InstallError> {
    if network.boot_id.is_none() {
        remove_recovery_units(root, network, systemd::downcast(manager)?, automatic)?;
    }
    Ok(())
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

fn selinux_enabled(fs_path: &Path, config_path: &Path) -> Result<bool, InstallError> {
    // An empty mount point can exist even when SELinux is not loaded.
    match std::fs::metadata(fs_path.join("enforce")) {
        Ok(_) => return Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(InstallError::Io(error)),
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

fn resolve_runtime(_args: &Network) -> Result<InstallRuntime, InstallError> {
    #[cfg(feature = "test-support")]
    if let Some(path) = _args.test_runtime.as_deref() {
        return InstallRuntime::from_test_file(path);
    }
    Ok(InstallRuntime::production())
}

async fn submit_native(args: &Network) -> Result<u8, InstallError> {
    let runtime = resolve_runtime(args)?;
    if !runtime.allow_non_root && unsafe { libc::geteuid() } != 0 {
        return Err(InstallError::UnsupportedPlatform(
            "network commands require root".into(),
        ));
    }
    let root = match state::discover_landscape_root()? {
        Some(root) => root,
        None => state::discover_landscape_root_from_unfinished_transaction()?
            .ok_or_else(|| InstallError::ParameterUsage("no pending network transaction".into()))?,
    };
    let tx = transaction::find_unfinished(&root)?
        .ok_or_else(|| InstallError::ParameterUsage("no pending network transaction".into()))?;
    let confirm = matches!(args.action, NetworkAction::Confirm);
    validate_action(&tx, confirm)?;
    let kind = if confirm {
        crate::daemon_operations::Kind::NetworkConfirm
    } else {
        crate::daemon_operations::Kind::NetworkRollback
    };
    crate::daemon_operations::submit_network_and_wait(
        root.canonical.to_string_lossy().into_owned(),
        kind,
        tx.transaction_id,
    )
    .await
}

pub(crate) fn validate_action(
    tx: &transaction::TransactionFile,
    confirm: bool,
) -> Result<(), InstallError> {
    use transaction::{Operation, Phase};
    if !matches!(tx.operation, Operation::Install | Operation::Reinit) {
        return Err(InstallError::ParameterUsage(
            "not a network takeover transaction".into(),
        ));
    }
    let network = tx.network_takeover.as_ref().ok_or_else(|| {
        InstallError::ParameterUsage("transaction has no network takeover".into())
    })?;
    if confirm {
        if !matches!(
            tx.phase,
            Phase::AwaitingNetworkConfirmation | Phase::Finalizing
        ) {
            return Err(InstallError::ParameterUsage(
                "transaction is not awaiting network confirmation".into(),
            ));
        }
        if tx.phase == Phase::AwaitingNetworkConfirmation {
            check_confirmation_window(network, Utc::now(), current_boot_id())?;
        }
    } else if !rollback_allowed(tx.phase) {
        return Err(InstallError::ParameterUsage(
            "accepted network confirmation cannot be rolled back".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepted_confirmation_cannot_be_changed_to_rollback() {
        use transaction::Phase;
        assert!(rollback_allowed(Phase::AwaitingNetworkConfirmation));
        assert!(rollback_allowed(Phase::RollingBack));
        for phase in [
            Phase::Finalizing,
            Phase::Committed,
            Phase::RolledBack,
            Phase::Preparing,
        ] {
            assert!(!rollback_allowed(phase), "{phase:?}");
        }
    }

    #[test]
    fn selinux_mount_point_without_kernel_interface_is_not_loaded() {
        let dir = std::env::temp_dir().join(format!("lkit-selinux-empty-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("config");
        assert!(!selinux_enabled(&dir, &config).unwrap());
        for enforcing in ["0", "1"] {
            std::fs::write(dir.join("enforce"), enforcing).unwrap();
            assert!(selinux_enabled(&dir, &config).unwrap());
        }
        std::fs::remove_file(dir.join("enforce")).unwrap();
        std::fs::write(&config, "SELINUX=permissive\n").unwrap();
        assert!(selinux_enabled(&dir, &config).unwrap());
        std::fs::remove_dir_all(dir).unwrap();
    }

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
    fn only_routed_lan_confirmation_flushes_wan_ipv4() {
        let mut plan = NetworkPlan {
            mode: NetworkMode::RoutedLan {
                wan: "ens3".into(),
                wan_ipv4: None,
                lan: vec!["ens4".into()],
                management: "192.168.10.1/24".parse().unwrap(),
                dhcp_start: "192.168.10.100".parse().unwrap(),
                dhcp_end: "192.168.10.254".parse().unwrap(),
            },
            selected_macs: Vec::new(),
        };
        assert!(matches!(
            clear_inherited_wan_ipv4(&plan, Path::new("/bin/false")),
            Err(InstallError::HealthCheck(_))
        ));

        plan.mode = NetworkMode::WanOnly {
            wan: "ens3".into(),
            address: "198.51.100.20/24".parse().unwrap(),
            gateway: "198.51.100.1".parse().unwrap(),
        };
        assert!(clear_inherited_wan_ipv4(&plan, Path::new("/bin/false")).is_ok());
    }

    #[test]
    fn selected_lan_cleanup_does_not_touch_wan_only_plans() {
        let plan = NetworkPlan {
            mode: NetworkMode::WanOnly {
                wan: "ens3".into(),
                address: "198.51.100.20/24".parse().unwrap(),
                gateway: "198.51.100.1".parse().unwrap(),
            },
            selected_macs: Vec::new(),
        };
        assert!(clear_selected_lan_addresses(&plan, Path::new("/bin/false")).is_ok());
    }
}

#[cfg(test)]
mod eligibility_tests {
    use super::*;
    use crate::service::manager::ServiceManagerKind;

    fn setup() -> (layout::TerritoryOverride, std::path::PathBuf, InstallRoot) {
        let temp =
            std::env::temp_dir().join(format!("lkit-takeover-proof-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(temp.join("landscape")).unwrap();
        let guard = layout::test_territory(&temp.join("territory"));
        std::fs::create_dir_all(layout::territory_transactions_dir()).unwrap();
        let root = InstallRoot {
            install_root: temp.join("landscape"),
            canonical: temp.join("landscape"),
        };
        (guard, temp, root)
    }

    fn install(root: &InstallRoot, takeover: bool, age: i64) -> transaction::TransactionFile {
        let mut tx =
            transaction::TransactionFile::new_install(root, &semver::Version::new(1, 0, 0))
                .unwrap();
        tx.started_at = Utc::now() - ChronoDuration::days(age);
        tx.updated_at = tx.started_at;
        tx.phase = transaction::Phase::Committed;
        if takeover {
            tx.network_takeover = Some(transaction::NetworkTakeoverTransaction {
                boot_id: Some(uuid::Uuid::now_v7().to_string()),
                plan: NetworkPlan {
                    mode: NetworkMode::WanDhcp { wan: "eth0".into() },
                    selected_macs: vec![super::super::config::SelectedInterface {
                        name: "eth0".into(),
                        mac: "02:00:00:00:00:01".into(),
                    }],
                },
                host_services: Vec::new(),
                network_manager: Some(Vec::new()),
                preserve_startup_links: true,
                confirmation_deadline: Utc::now(),
                rollback_service: "test-rollback.service".into(),
                rollback_timer: "test-rollback.timer".into(),
                boot_rollback_service: "test-boot.service".into(),
                recovery_binary: "service/lkit-network-recovery".into(),
                pending_state: format!(
                    "transactions/{}/pending-install-state.json",
                    tx.transaction_id
                ),
            });
        }
        tx
    }

    fn save(tx: &transaction::TransactionFile) {
        transaction::validate_transaction(tx).unwrap();
        std::fs::write(
            layout::territory_transactions_dir().join(format!("{}.json", tx.transaction_id)),
            serde_json::to_vec(tx).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn takeover_requires_committed_provenance_for_the_current_installation() {
        let (_guard, temp, root) = setup();
        assert!(
            transaction::latest_committed_network_takeover(&root)
                .unwrap()
                .is_none()
        );
        let old = install(&root, true, 3);
        save(&old);
        assert_eq!(
            transaction::latest_committed_network_takeover(&root)
                .unwrap()
                .unwrap()
                .transaction_id,
            old.transaction_id
        );
        let mut failed = install(&root, false, 2);
        failed.phase = transaction::Phase::Failed;
        save(&failed);
        assert!(
            transaction::latest_committed_network_takeover(&root)
                .unwrap()
                .is_some()
        );
        save(&install(&root, false, 1));
        assert!(
            transaction::latest_committed_network_takeover(&root)
                .unwrap()
                .is_none()
        );
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn uninstall_and_migration_invalidate_prior_takeover_but_asset_changes_do_not() {
        let (_guard, temp, root) = setup();
        save(&install(&root, true, 5));
        let version = semver::Version::new(1, 0, 0);
        let mut repair = transaction::TransactionFile::new_repair_static(&root).unwrap();
        repair.phase = transaction::Phase::Committed;
        save(&repair);
        assert!(
            transaction::latest_committed_network_takeover(&root)
                .unwrap()
                .is_some()
        );
        let mut uninstall = transaction::TransactionFile::new_uninstall(&root, &version).unwrap();
        uninstall.phase = transaction::Phase::Committed;
        uninstall.no_backup = true;
        save(&uninstall);
        assert!(
            transaction::latest_committed_network_takeover(&root)
                .unwrap()
                .is_none()
        );
        let mut fresh = install(&root, true, 0);
        fresh.started_at = uninstall.started_at + ChronoDuration::seconds(1);
        fresh.updated_at = fresh.started_at;
        save(&fresh);
        let mut migration = transaction::TransactionFile::new_migrate(&root, &version).unwrap();
        migration.phase = transaction::Phase::Committed;
        migration.started_at = fresh.started_at + ChronoDuration::seconds(1);
        migration.updated_at = migration.started_at;
        migration.backup = Some(transaction::BackupRef {
            backup_id: uuid::Uuid::now_v7().to_string(),
            path: "backups/migration.lkb".into(),
            sha256: "a".repeat(64),
        });
        save(&migration);
        assert!(
            transaction::latest_committed_network_takeover(&root)
                .unwrap()
                .is_none()
        );
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn reinit_provenance_ignores_foreign_history_and_rejects_corruption() {
        let (_guard, temp, root) = setup();
        let mut reinit = install(&root, true, 2);
        reinit.operation = transaction::Operation::Reinit;
        reinit.from_version = reinit.target_version.take();
        reinit.target_release = None;
        reinit.no_backup = true;
        save(&reinit);
        let foreign_root = InstallRoot {
            install_root: temp.join("other"),
            canonical: temp.join("other"),
        };
        save(&install(&foreign_root, false, 0));
        assert_eq!(
            transaction::latest_committed_network_takeover(&root)
                .unwrap()
                .unwrap()
                .transaction_id,
            reinit.transaction_id
        );
        std::fs::write(
            layout::territory_transactions_dir().join("broken.json"),
            "{",
        )
        .unwrap();
        assert!(transaction::latest_committed_network_takeover(&root).is_err());
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn proven_takeover_without_host_services_is_eligible_and_backend_is_bound() {
        let (_guard, temp, root) = setup();
        save(&install(&root, true, 1));
        let init_dir = temp.join("etc/init.d");
        std::fs::create_dir_all(&init_dir).unwrap();
        let manager = crate::service::openrc::Openrc {
            init_d_dir: init_dir,
            rc_service: temp.join("must-not-run"),
            rc_update: temp.join("must-not-run"),
            resolv_conf: temp.join("resolv.conf"),
        };
        let mut installed: state::InstallState = serde_json::from_value(serde_json::json!({
            "schema_version": 1, "layout_version": 2,
            "install_root": root.install_root, "canonical_install_root": root.canonical,
            "active_version": "1.0.0",
            "assets": {
                "webserver": {"architecture": "x86_64", "sha256": "a".repeat(64), "size": 1},
                "static_archive": {"sha256": "b".repeat(64), "size": 1}
            },
            "initialization": {"status": "complete", "lock_present": true, "initialized_at": Utc::now()},
            "service": {"manager": "openrc", "registered": true, "enabled": true, "verified": true,
                "definition_path": "service/landscape-router.service", "definition_sha256": "c".repeat(64)},
            "last_transaction_id": null, "committed_at": Utc::now()
        })).unwrap();
        assert!(require_existing_takeover(&root, &installed, &manager).is_ok());
        installed.service.manager = ServiceManagerKind::Sysvinit;
        assert!(require_existing_takeover(&root, &installed, &manager).is_err());
        std::fs::remove_dir_all(temp).unwrap();
    }
}
