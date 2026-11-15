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
    let host_services = host_network::names(manager)
        .iter()
        .map(|unit| host_network::inspect(manager, unit))
        .collect::<Result<Vec<_>, _>>()?;
    let stem = format!("lkit-network-{}", transaction_id);
    let timeout = ChronoDuration::from_std(runtime.network_confirm_timeout).map_err(|_| {
        InstallError::ParameterUsage("network confirmation timeout is too large".into())
    })?;
    Ok(transaction::NetworkTakeoverTransaction {
        boot_id: Some(current_boot_id()?),
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
    let capability =
        std::fs::read_to_string(layout::territory_run_dir().join("network-recovery-v1.pid"))
            .unwrap_or_default();
    let pid = manager.main_pid(ManagedService::LkitDaemon)?;
    if capability.trim() != pid.to_string() || pid == 0 {
        return Err(InstallError::Preflight(
            "restart lkit with lkit self install to enable daemon network recovery".into(),
        ));
    }
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
    for before in &network.host_services {
        host_network::restore(manager, before)?;
    }
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
        if Utc::now() >= network.confirmation_deadline
            || network
                .boot_id
                .as_deref()
                .is_some_and(|saved| current_boot_id().map_or(true, |boot| saved != boot))
        {
            return Err(InstallError::ParameterUsage(
                "network confirmation deadline has expired; wait for automatic rollback or run `lkit network rollback`"
                    .into(),
            ));
        }
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
        pending.phase = transaction::Phase::Finalizing;
        pending.updated_at = Utc::now();
        transaction::persist(root, &pending)?;
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
    ) || !matches!(
        pending.phase,
        transaction::Phase::AwaitingNetworkConfirmation
            | transaction::Phase::Finalizing
            | transaction::Phase::RollingBack
    ) {
        return Err(InstallError::BlockedByTransaction(format!(
            "transaction {} is {}; network rollback only handles an uncommitted confirmation phase",
            pending.transaction_id,
            pending.phase.key()
        )));
    }
    let health = runtime.health_options()?;
    transaction::mark_phase(root, &pending, transaction::Phase::RollingBack)?;
    let result: Result<(), InstallError> = async {
        match pending.operation {
            transaction::Operation::Install => {
                transaction::restore_uncommitted_network_systemd(
                    root,
                    &pending,
                    runtime.service_manager.as_ref(),
                )?;
                for before in &network.host_services {
                    host_network::restore(runtime.service_manager.as_ref(), before)?;
                }
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
        if pending.operation == transaction::Operation::Install {
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

fn resolve_runtime(_args: &Network) -> Result<InstallRuntime, InstallError> {
    #[cfg(feature = "test-support")]
    if let Some(path) = _args.test_runtime.as_deref() {
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
