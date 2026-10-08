use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::process::{Command, ExitCode};

use chrono::{Duration as ChronoDuration, Utc};
use lkit_hostnet::HostNetworkAdapter;
use lkit_hostnet::firewalld::FirewalldAdapter;
use lkit_hostnet::ifupdown::IfupdownAdapter;
use lkit_hostnet::networkd::NetworkdAdapter;
use lkit_hostnet::nm::{NmAdapter, UNMANAGE_CONF};
use lkit_hostnet::{FileSources, Manifest, ToolPaths};

use crate::commands::network::{Network, NetworkAction};
use crate::deployment::layout;
use crate::deployment::plan::InstallError;
use crate::deployment::root::InstallRoot;
use crate::deployment::runtime::InstallRuntime;
use crate::deployment::{lock, state, transaction};
use crate::service::manager::{ManagedService, ServiceManager};
use crate::service::{health, systemd};

use super::config::{NetworkMode, NetworkPlan};

/// 接管对宿主网络组件的两级处理:
/// - 整体 stop/disable/mask 的只有 `systemd-resolved.service`:DNS 是主机全局
///   (stub 监听 :53、resolv.conf 归属),没有按接口摘除的文件语义。
/// - NetworkManager、firewalld 与 systemd-networkd 保持运行:NM 通过 conf.d
///   drop-in、firewalld 通过 zone XML 的接口行、networkd 通过移出 `.network`
///   文件摘除选中接口(见 `unmanage_selected_interfaces`),未选接口继续由宿主
///   管理;回滚/卸载按 `backups/hostnet/<kind>` 的 manifest 逐字恢复并 reload
///   对应守护进程。ifupdown 同理摘除并重启 `networking.service`。
const HOST_SERVICES: [&str; 1] = ["systemd-resolved.service"];
const UNKNOWN_NETWORK_MANAGERS: [&str; 2] = ["wicked.service", "connman.service"];
/// 接管摘除的宿主网络配置备份固定落点(地盘相对):同一主机只有一个
/// Landscape 安装,备份跨事务存活,回滚/卸载后删除。目录内按适配器分子目录
/// (`ifupdown`/`nm`/`firewalld`),各自一份 manifest,任一存在即接管未恢复。
const HOSTNET_BACKUP_REL: &str = "backups/hostnet";
const HOSTNET_KIND_IFUPDOWN: &str = "ifupdown";
const HOSTNET_KIND_NM: &str = "nm";
const HOSTNET_KIND_FIREWALLD: &str = "firewalld";
const HOSTNET_KIND_NETWORKD: &str = "networkd";

pub(crate) fn preflight(runtime: &InstallRuntime) -> Result<(), InstallError> {
    let manager = runtime.service_manager.as_ref();
    if !matches!(
        manager.probe(),
        crate::service::manager::Availability::Available { .. }
    ) {
        return Err(InstallError::UnsupportedPlatform(
            "network takeover requires a reachable systemd system manager".into(),
        ));
    }
    let systemd = systemd::downcast(manager)?;
    if selinux_enabled(&runtime.selinux_fs_path, &runtime.selinux_config_path)? {
        return Err(InstallError::UnsupportedPlatform(
            "network takeover does not support systems where SELinux is loaded or enabled".into(),
        ));
    }
    for unit in UNKNOWN_NETWORK_MANAGERS {
        let before = systemd::inspect_host_service(systemd, unit)?;
        if before.active {
            return Err(InstallError::Preflight(format!(
                "unknown network manager {unit} is active; stop it before network takeover"
            )));
        }
    }
    // 运行中的 NM/firewalld 必须能通过其配置目录摘除,否则接管无法生效。
    for (unit, dir) in [
        ("NetworkManager.service", &runtime.nm_conf_d),
        ("firewalld.service", &runtime.firewalld_zones),
        ("systemd-networkd.service", &runtime.networkd_dir),
    ] {
        if systemd::inspect_host_service(systemd, unit)?.active && !dir.is_dir() {
            return Err(InstallError::Preflight(format!(
                "{unit} is active but {} is missing; create it or stop the service before network takeover",
                dir.display()
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
    let systemd = systemd::downcast(runtime.service_manager.as_ref())?;
    let host_services = HOST_SERVICES
        .iter()
        .map(|unit| systemd::inspect_host_service(systemd, unit))
        .collect::<Result<Vec<_>, _>>()?;
    let stem = format!("lkit-network-{}", transaction_id);
    let timeout = ChronoDuration::from_std(runtime.network_confirm_timeout).map_err(|_| {
        InstallError::ParameterUsage("network confirmation timeout is too large".into())
    })?;
    Ok(transaction::NetworkTakeoverTransaction {
        plan: plan.clone(),
        host_services,
        hostnet_backup: None,
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
        "{} network rollback --automatic{}",
        unit_quote(&recovery),
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
    let systemd = systemd::downcast(runtime.service_manager.as_ref())?;
    write_system_unit(systemd, &network.rollback_service, &rollback)?;
    write_system_unit(systemd, &network.rollback_timer, &timer)?;
    write_system_unit(systemd, &network.boot_rollback_service, &boot)?;
    systemd::daemon_reload(systemd)?;
    systemd::unit_command(systemd, "enable", &network.boot_rollback_service)?;
    systemd::unit_command(systemd, "enable", &network.rollback_timer)?;
    systemd::unit_command(systemd, "start", &network.rollback_timer)
}

pub(crate) fn stop_host_services(
    network: &transaction::NetworkTakeoverTransaction,
    manager: &dyn ServiceManager,
) -> Result<(), InstallError> {
    let systemd = systemd::downcast(manager)?;
    for before in network.host_services.iter().rev() {
        systemd::stop_disable_mask_host_service(systemd, before)?;
    }
    Ok(())
}

/// 接管摘除:把选中接口(WAN + 全部选中 LAN)从宿主网络配置中移出,按宿主
/// 上实际存在的管理工具选择适配器——ifupdown 主配置、NetworkManager 的
/// conf.d、firewalld 的 zone 目录,存在即摘除,可组合。原文件逐字备份到
/// `backups/hostnet/<kind>`;文件改写后让运行中的 NM/firewalld 立即重读配置。
/// 选中接口不由任何适配器管理时是 no-op,返回 None。摘除在停止宿主服务之前
/// 执行,失败(保守解析拒绝、dry-run 校验失败、reload 失败)时事务按失败
/// 清理恢复文件。
pub(crate) fn unmanage_selected_interfaces(
    plan: &NetworkPlan,
    runtime: &InstallRuntime,
) -> Result<Option<String>, InstallError> {
    let selected: Vec<String> = plan.selected_macs.iter().map(|s| s.name.clone()).collect();
    if selected.is_empty() {
        return Ok(None);
    }
    let mut applied = false;
    if unmanage_ifupdown_files(
        &selected,
        &runtime.interfaces_file,
        runtime.ifup_command.as_deref(),
    )? {
        applied = true;
    }
    if runtime.nm_conf_d.is_dir() && unmanage_nm_files(&selected, &runtime.nm_conf_d)? {
        applied = true;
    }
    if runtime.firewalld_zones.is_dir()
        && unmanage_firewalld_files(&selected, &runtime.firewalld_zones)?
    {
        applied = true;
    }
    if runtime.networkd_dir.is_dir() && unmanage_networkd_files(&selected, &runtime.networkd_dir)? {
        applied = true;
    }
    if applied {
        reload_running_host_daemons(runtime)?;
        Ok(Some(HOSTNET_BACKUP_REL.to_string()))
    } else {
        Ok(None)
    }
}

/// 摘除的核心(纯文件操作,单测直接驱动):返回是否创建了备份。备份已存在时
/// 视为选中接口已处于摘除态(如 reinit 重放接管),保留原备份不重复改写。
fn unmanage_ifupdown_files(
    selected: &[String],
    interfaces_file: &Path,
    ifup: Option<&Path>,
) -> Result<bool, InstallError> {
    // 无 ifupdown 配置:没有可摘除的内容。
    if !interfaces_file.is_file() {
        return Ok(false);
    }
    let backup_dir = hostnet_backup_dir(HOSTNET_KIND_IFUPDOWN);
    if hostnet_kind_backup_stands(HOSTNET_KIND_IFUPDOWN) {
        return Ok(true);
    }
    let sources = FileSources::new(interfaces_file.to_path_buf());
    let tools = ToolPaths {
        ifup: ifup.map(Path::to_path_buf),
        ..Default::default()
    };
    let adapter = IfupdownAdapter::new();
    match adapter.execute_unmanage(&sources, selected, &backup_dir, &tools) {
        Ok(outcome) => Ok(outcome.manifest.is_some()),
        Err(error) => Err(InstallError::Preflight(format!(
            "cannot unmanage host network interfaces [{}] from ifupdown: {error}",
            selected.join(", ")
        ))),
    }
}

/// NM 摘除:conf.d 存在即写入 drop-in(NM 未运行也写,覆盖未来启动)。备份
/// 已存在且 drop-in 仍在时视为已摘除;备份存在但 drop-in 缺失是现场漂移
/// (人工删除),明确拒绝而不是带着失效的接管继续。
fn unmanage_nm_files(selected: &[String], conf_d: &Path) -> Result<bool, InstallError> {
    let backup_dir = hostnet_backup_dir(HOSTNET_KIND_NM);
    if hostnet_kind_backup_stands(HOSTNET_KIND_NM) {
        if conf_d.join(UNMANAGE_CONF).is_file() {
            return Ok(true);
        }
        return Err(InstallError::Preflight(format!(
            "the hostnet NM backup exists but {} is missing; restore it or remove {} before takeover",
            conf_d.join(UNMANAGE_CONF).display(),
            backup_dir.display()
        )));
    }
    let sources = FileSources {
        nm_conf_d: Some(conf_d.to_path_buf()),
        ..Default::default()
    };
    match NmAdapter::new().execute_unmanage(&sources, selected, &backup_dir, &ToolPaths::default())
    {
        Ok(outcome) => Ok(outcome.manifest.is_some()),
        Err(error) => Err(InstallError::Preflight(format!(
            "cannot unmanage host network interfaces [{}] from NetworkManager: {error}",
            selected.join(", ")
        ))),
    }
}

/// firewalld 摘除:zone 目录存在即从 zone XML 的接口行中移除选中接口。
/// 备份已存在时视为已摘除。
fn unmanage_firewalld_files(selected: &[String], zones: &Path) -> Result<bool, InstallError> {
    let backup_dir = hostnet_backup_dir(HOSTNET_KIND_FIREWALLD);
    if hostnet_kind_backup_stands(HOSTNET_KIND_FIREWALLD) {
        return Ok(true);
    }
    let sources = FileSources {
        firewalld_zones: Some(zones.to_path_buf()),
        ..Default::default()
    };
    match FirewalldAdapter::new().execute_unmanage(
        &sources,
        selected,
        &backup_dir,
        &ToolPaths::default(),
    ) {
        Ok(outcome) => Ok(outcome.manifest.is_some()),
        Err(error) => Err(InstallError::Preflight(format!(
            "cannot unmanage host network interfaces [{}] from firewalld zones: {error}",
            selected.join(", ")
        ))),
    }
}

/// networkd 摘除:配置目录存在即移出 `[Match]` 只引用选中接口的 `.network`
/// 文件。备份仍在但所有被移出文件已被人工重建时是现场漂移,明确拒绝。
fn unmanage_networkd_files(selected: &[String], dir: &Path) -> Result<bool, InstallError> {
    let backup_dir = hostnet_backup_dir(HOSTNET_KIND_NETWORKD);
    if let Some(manifest) = read_hostnet_manifest(HOSTNET_KIND_NETWORKD)? {
        let recreated = manifest.files.iter().any(|file| file.original.exists());
        if recreated {
            return Err(InstallError::Preflight(format!(
                "the hostnet networkd backup exists but removed .network files were recreated; restore them or remove {} before takeover",
                backup_dir.display()
            )));
        }
        return Ok(true);
    }
    let sources = FileSources {
        networkd_dir: Some(dir.to_path_buf()),
        ..Default::default()
    };
    match NetworkdAdapter::new().execute_unmanage(
        &sources,
        selected,
        &backup_dir,
        &ToolPaths::default(),
    ) {
        Ok(outcome) => Ok(outcome.manifest.is_some()),
        Err(error) => Err(InstallError::Preflight(format!(
            "cannot unmanage host network interfaces [{}] from systemd-networkd: {error}",
            selected.join(", ")
        ))),
    }
}

/// 文件摘除后让运行中的宿主守护进程立即重读配置。NM/firewalld/networkd 未
/// 运行时跳过(文件改写在下次启动时生效);运行中但工具缺失或 reload 失败时报错,
/// 运行时未生效的摘除不是有效接管,事务按失败清理恢复文件。
fn reload_running_host_daemons(runtime: &InstallRuntime) -> Result<(), InstallError> {
    let manager = runtime.service_manager.as_ref();
    if host_service_active(manager, "systemd-networkd.service") {
        let networkctl = runtime.networkctl.as_deref().ok_or_else(|| {
            InstallError::Preflight(
                "systemd-networkd is active but networkctl was not found; install it or stop systemd-networkd before network takeover"
                    .into(),
            )
        })?;
        run_host_daemon_reload(networkctl, &["reload"], "systemd-networkd")?;
    }
    if host_service_active(manager, "NetworkManager.service") {
        let nmcli = runtime.nmcli.as_deref().ok_or_else(|| {
            InstallError::Preflight(
                "NetworkManager is active but nmcli was not found; install it or stop NetworkManager before network takeover"
                    .into(),
            )
        })?;
        run_host_daemon_reload(nmcli, &["general", "reload"], "NetworkManager")?;
    }
    if host_service_active(manager, "firewalld.service") {
        let firewall_cmd = runtime.firewall_cmd.as_deref().ok_or_else(|| {
            InstallError::Preflight(
                "firewalld is active but firewall-cmd was not found; install it or stop firewalld before network takeover"
                    .into(),
            )
        })?;
        run_host_daemon_reload(firewall_cmd, &["--reload"], "firewalld")?;
    }
    Ok(())
}

fn host_service_active(manager: &dyn ServiceManager, unit: &str) -> bool {
    systemd::downcast(manager)
        .ok()
        .and_then(|systemd| systemd::inspect_host_service(systemd, unit).ok())
        .map(|before| before.active)
        .unwrap_or(false)
}

fn run_host_daemon_reload(tool: &Path, args: &[&str], unit: &str) -> Result<(), InstallError> {
    let output = Command::new(tool)
        .args(args)
        .output()
        .map_err(InstallError::Io)?;
    if !output.status.success() {
        return Err(InstallError::Preflight(format!(
            "cannot apply the network takeover to the running {unit}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

/// 接管特征:地盘 `backups/hostnet` 下任一适配器的 manifest 仍在,即摘除尚未
/// 恢复。ifupdown 主机上 `networking.service` 保持运行、NM/firewalld 也不再
/// 整体停止,仅凭服务状态探测不到接管,reinit 与卸载的接管检测以此为准。
pub(crate) fn hostnet_backup_stands() -> bool {
    [
        HOSTNET_KIND_IFUPDOWN,
        HOSTNET_KIND_NM,
        HOSTNET_KIND_FIREWALLD,
        HOSTNET_KIND_NETWORKD,
    ]
    .iter()
    .any(|kind| hostnet_kind_backup_stands(kind))
}

fn hostnet_kind_backup_stands(kind: &str) -> bool {
    hostnet_backup_dir(kind).join("manifest.json").is_file()
}

fn hostnet_backup_dir(kind: &str) -> std::path::PathBuf {
    layout::territory_relative(HOSTNET_BACKUP_REL).join(kind)
}

/// 读取地盘 `backups/hostnet/<kind>` 的 manifest;无备份时返回 None。
fn read_hostnet_manifest(kind: &str) -> Result<Option<Manifest>, InstallError> {
    let manifest_path = hostnet_backup_dir(kind).join("manifest.json");
    let bytes = match std::fs::read(&manifest_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(InstallError::Io(error)),
    };
    serde_json::from_slice(&bytes).map(Some).map_err(|error| {
        InstallError::CorruptedState(format!("hostnet backup manifest is invalid: {error}"))
    })
}

/// 按 `backups/hostnet/<kind>` 的 manifest 逐字恢复每个适配器改写的宿主文件并
/// 删除备份。无备份(接管未改写任何文件)时返回 false。文件恢复失败向上报错;
/// 恢复后的运行时重载(`networking.service` 重启、NM/firewalld/networkd
/// reload)由调用方按各守护进程实况状态决定。
pub(crate) fn restore_hostnet_backup() -> Result<bool, InstallError> {
    let backup_root = layout::territory_relative(HOSTNET_BACKUP_REL);
    let mut restored = false;
    for kind in [
        HOSTNET_KIND_IFUPDOWN,
        HOSTNET_KIND_NM,
        HOSTNET_KIND_FIREWALLD,
        HOSTNET_KIND_NETWORKD,
    ] {
        let Some(manifest) = read_hostnet_manifest(kind)? else {
            continue;
        };
        let adapter_restore = |error: lkit_hostnet::HostNetError| {
            InstallError::Preflight(format!("cannot restore host network config: {error}"))
        };
        match kind {
            HOSTNET_KIND_IFUPDOWN => IfupdownAdapter::new()
                .restore(&manifest)
                .map_err(adapter_restore)?,
            HOSTNET_KIND_NM => NmAdapter::new()
                .restore(&manifest)
                .map_err(adapter_restore)?,
            HOSTNET_KIND_FIREWALLD => FirewalldAdapter::new()
                .restore(&manifest)
                .map_err(adapter_restore)?,
            _ => NetworkdAdapter::new()
                .restore(&manifest)
                .map_err(adapter_restore)?,
        }
        restored = true;
    }
    if restored {
        std::fs::remove_dir_all(&backup_root).map_err(InstallError::Io)?;
    }
    Ok(restored)
}

/// reinit 前置校验:地盘 hostnet 备份仍在时,新的接口选择必须与摘除现场一致。
/// 现场集合取各适配器反查结果的并集——ifupdown 的 manual stanza、NM drop-in 的
/// `unmanaged-devices`、firewalld 快照与现场的差集、networkd 被移出文件的
/// `Name=` 精确集。换选接口的重放需要完整的
/// 谱系回滚,不在当前范围;不一致时明确拒绝,引导用户走 uninstall + 重新接管。
/// 无任何备份不设限。
pub(crate) fn ensure_reinit_selection_matches_takeover(
    selected: &[String],
    runtime: &InstallRuntime,
) -> Result<(), InstallError> {
    ensure_reinit_selection_matches_files(selected, &runtime.interfaces_file, &runtime.nm_conf_d)
}

fn ensure_reinit_selection_matches_files(
    selected: &[String],
    interfaces_file: &Path,
    nm_conf_d: &Path,
) -> Result<(), InstallError> {
    let mut unmanaged: Vec<String> = Vec::new();
    if let Some(manifest) = read_hostnet_manifest(HOSTNET_KIND_IFUPDOWN)?
        && interfaces_file.is_file()
    {
        let sources = FileSources::new(interfaces_file.to_path_buf());
        let standing = IfupdownAdapter::new()
            .unmanaged_interfaces(&sources, &manifest)
            .map_err(|error| {
                InstallError::Preflight(format!(
                    "cannot inspect the host ifupdown takeover state: {error}"
                ))
            })?;
        unmanaged.extend(standing);
    }
    if read_hostnet_manifest(HOSTNET_KIND_NM)?.is_some() {
        let sources = FileSources {
            nm_conf_d: Some(nm_conf_d.to_path_buf()),
            ..Default::default()
        };
        unmanaged.extend(NmAdapter::unmanaged_interfaces(&sources));
    }
    if let Some(manifest) = read_hostnet_manifest(HOSTNET_KIND_FIREWALLD)? {
        unmanaged.extend(FirewalldAdapter::unmanaged_interfaces(&manifest));
    }
    if let Some(manifest) = read_hostnet_manifest(HOSTNET_KIND_NETWORKD)? {
        unmanaged.extend(NetworkdAdapter::unmanaged_interfaces(&manifest));
    }
    if unmanaged.is_empty() {
        return Ok(());
    }
    unmanaged.sort();
    unmanaged.dedup();
    let unchanged =
        selected.len() == unmanaged.len() && selected.iter().all(|name| unmanaged.contains(name));
    if !unchanged {
        return Err(InstallError::ParameterUsage(crate::tr!(
            crate::keys::REINIT_REQUIRES_SAME_INTERFACES,
            unmanaged = unmanaged.join(", ")
        )));
    }
    Ok(())
}

/// 恢复宿主网络文件后的运行时重放:重启 active 的 `networking.service` 让
/// `ifup -a` 重新套用原配置,reload 运行中的 NM/firewalld/networkd 重读配置。
/// 全部尽力而为——文件已是原样,失败只提示,守护进程下次重启自然收敛。
pub(crate) fn reapply_host_network_after_restore(
    manager: &dyn ServiceManager,
    nmcli: Option<&Path>,
    firewall_cmd: Option<&Path>,
    networkctl: Option<&Path>,
) {
    restart_networking_if_active(manager);
    if host_service_active(manager, "NetworkManager.service")
        && let Some(nmcli) = nmcli
    {
        report_reload_failure(
            Command::new(nmcli)
                .args(["general", "reload"])
                .output()
                .map(|output| output.status.success()),
            nmcli,
            crate::keys::TAKEOVER_NM_RELOAD_FAILED,
        );
    }
    if host_service_active(manager, "firewalld.service")
        && let Some(firewall_cmd) = firewall_cmd
    {
        report_reload_failure(
            Command::new(firewall_cmd)
                .args(["--reload"])
                .output()
                .map(|output| output.status.success()),
            firewall_cmd,
            crate::keys::TAKEOVER_FIREWALLD_RELOAD_FAILED,
        );
    }
    if host_service_active(manager, "systemd-networkd.service")
        && let Some(networkctl) = networkctl
    {
        report_reload_failure(
            Command::new(networkctl)
                .args(["reload"])
                .output()
                .map(|output| output.status.success()),
            networkctl,
            crate::keys::TAKEOVER_NETWORKD_RELOAD_FAILED,
        );
    }
}

fn report_reload_failure(result: Result<bool, std::io::Error>, tool: &Path, key: &'static str) {
    if !result.unwrap_or(false) {
        eprintln!(
            "network: {}",
            crate::tr!(
                key,
                error = format!("{} did not complete successfully", tool.display())
            )
        );
    }
}

/// 恢复 ifupdown 原文件后重启 `networking.service`,让 `ifup -a` 重新套用
/// 原配置、归还选中接口的地址。仅在服务当前 active 时重启(接管不再停止它,
/// 实况即接管前的状态);重启失败不阻断其余恢复,只提示。NM/firewalld 的
/// 对应重载见 `reapply_host_network_after_restore`。
pub(crate) fn restart_networking_if_active(manager: &dyn ServiceManager) {
    let Ok(systemd) = systemd::downcast(manager) else {
        return;
    };
    let active = systemd::inspect_host_service(systemd, "networking.service")
        .map(|before| before.active)
        .unwrap_or(false);
    if !active {
        return;
    }
    if let Err(error) = systemd::unit_command(systemd, "restart", "networking.service") {
        eprintln!(
            "network: {}",
            crate::tr!(
                crate::keys::TAKEOVER_NETWORKING_RESTART_FAILED,
                error = error
            )
        );
    }
}

pub(crate) fn cleanup_failed_takeover(
    root: &InstallRoot,
    network: &transaction::NetworkTakeoverTransaction,
    manager: &dyn ServiceManager,
) -> Result<(), InstallError> {
    let systemd = systemd::downcast(manager)?;
    for before in &network.host_services {
        systemd::restore_host_service(systemd, before)?;
    }
    if restore_hostnet_backup()? {
        // 中断恢复路径拿不到完整 runtime(工具路径),只重放 networking;
        // NM/firewalld 的文件已恢复,守护进程下次重启自然收敛。
        restart_networking_if_active(manager);
    }
    remove_recovery_units(root, network, systemd, false)
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
        InstallError::CorruptedTransaction(
            "pending transaction has no network takeover state".into(),
        )
    })?;
    let systemd = systemd::downcast(runtime.service_manager.as_ref())?;
    if pending.phase == transaction::Phase::AwaitingNetworkConfirmation {
        if Utc::now() > network.confirmation_deadline {
            return Err(InstallError::ParameterUsage(
                "network confirmation deadline has expired; wait for automatic rollback or run `lkit network rollback`"
                    .into(),
            ));
        }
        verify_interfaces(&network.plan, runtime)?;
        super::discovery::verify_live(&network.plan, &runtime.ip_command)?;
        let pid = systemd.main_pid(ManagedService::LandscapeRouter)?;
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
            unit_state: Some(&(|| systemd.active_state(ManagedService::LandscapeRouter).ok())),
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
    remove_recovery_units(root, &network, systemd, false)?;
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

async fn rollback(
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
                let systemd = systemd::downcast(runtime.service_manager.as_ref())?;
                for before in &network.host_services {
                    systemd::restore_host_service(systemd, before)?;
                }
                if restore_hostnet_backup()? {
                    reapply_host_network_after_restore(
                        runtime.service_manager.as_ref(),
                        runtime.nmcli.as_deref(),
                        runtime.firewall_cmd.as_deref(),
                        runtime.networkctl.as_deref(),
                    );
                }
                remove_recovery_units(root, &network, systemd, automatic)?;
            }
            transaction::Operation::Reinit => {
                crate::workflows::reinit::rollback_reinit_inner(
                    root,
                    &pending,
                    runtime.service_manager.as_ref(),
                    &health,
                )
                .await?;
                let systemd = systemd::downcast(runtime.service_manager.as_ref())?;
                remove_recovery_units(root, &network, systemd, automatic)?;
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
    if let Err(error) = result {
        let _ = transaction::mark_phase(root, &pending, transaction::Phase::Failed);
        return Err(error);
    }
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
    fn recovery_unit_command_keeps_test_runtime() {
        let path = Path::new("/tmp/runtime file.json");
        assert_eq!(unit_quote(path), "\"/tmp/runtime file.json\"");
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

    /// 临时地盘 + 临时宿主目录(interfaces 文件、NM conf.d、firewalld zones、
    /// networkd 配置目录),返回 (地盘目录, interfaces 路径, conf.d 路径,
    /// zones 路径, networkd 路径)。
    #[allow(clippy::type_complexity)]
    fn hostnet_fixture(
        tag: &str,
        interfaces: &str,
    ) -> (
        std::path::PathBuf,
        std::path::PathBuf,
        std::path::PathBuf,
        std::path::PathBuf,
        std::path::PathBuf,
    ) {
        let dir = std::env::temp_dir().join(format!(
            "lkit-hostnet-takeover-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let territory = dir.join("territory");
        std::fs::create_dir_all(&territory).unwrap();
        let interfaces_path = dir.join("interfaces");
        std::fs::write(&interfaces_path, interfaces).unwrap();
        let conf_d = dir.join("nm/conf.d");
        std::fs::create_dir_all(&conf_d).unwrap();
        let zones = dir.join("firewalld/zones");
        std::fs::create_dir_all(&zones).unwrap();
        let networkd = dir.join("systemd/network");
        std::fs::create_dir_all(&networkd).unwrap();
        (territory, interfaces_path, conf_d, zones, networkd)
    }

    const INTERFACES: &str = "\
auto ens3 ens4 ens5
iface ens3 inet static
    address 192.0.2.10/24
    gateway 192.0.2.1

iface ens4 inet dhcp

iface ens5 inet static
    address 198.51.100.10/24
";

    #[test]
    fn unmanage_rewrites_selected_and_backs_up_original() {
        let (territory, interfaces, _, _, _) = hostnet_fixture("unmanage", INTERFACES);
        let _guard = layout::test_territory(&territory);

        assert!(
            unmanage_ifupdown_files(&["ens3".into(), "ens4".into()], &interfaces, None).unwrap()
        );

        let rewritten = std::fs::read_to_string(&interfaces).unwrap();
        assert!(rewritten.contains("iface ens3 inet manual"));
        assert!(rewritten.contains("iface ens4 inet manual"));
        // 未选接口保持原样。
        assert!(rewritten.contains("address 198.51.100.10/24"));
        // auto 行中的选中接口被摘除,未选接口保留。
        assert!(rewritten.contains("auto ens5"));
        assert!(!rewritten.contains("auto ens3"));

        let manifest = territory
            .join(HOSTNET_BACKUP_REL)
            .join("ifupdown/manifest.json");
        assert!(manifest.is_file());
        // 幂等:备份已存在时再次摘除不再改写。
        assert!(unmanage_ifupdown_files(&["ens3".into()], &interfaces, None).unwrap());
        assert_eq!(std::fs::read_to_string(&interfaces).unwrap(), rewritten);

        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    #[test]
    fn unmanage_without_ifupdown_or_stanzas_is_a_noop() {
        let (territory, interfaces, _, _, _) = hostnet_fixture("noop", INTERFACES);
        let _guard = layout::test_territory(&territory);

        // 选中接口不在 ifupdown 配置中:不改任何文件、不建备份。
        assert!(!unmanage_ifupdown_files(&["ens9".into()], &interfaces, None).unwrap());
        assert_eq!(std::fs::read_to_string(&interfaces).unwrap(), INTERFACES);
        assert!(
            !territory
                .join(HOSTNET_BACKUP_REL)
                .join("ifupdown/manifest.json")
                .is_file()
        );

        // 无 ifupdown 配置(NetworkManager 主机)同理。
        assert!(
            !unmanage_ifupdown_files(&["ens3".into()], &territory.join("missing"), None).unwrap()
        );
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    /// 四个适配器同时摘除:各建各的备份,恢复一次调用全部逐字还原。
    #[test]
    fn multi_adapter_unmanage_and_restore_round_trip() {
        let (territory, interfaces, conf_d, zones, networkd) = hostnet_fixture("multi", INTERFACES);
        let _guard = layout::test_territory(&territory);
        let zone = zones.join("public.xml");
        let zone_original = "<?xml version=\"1.0\"?>\n<zone>\n  <short>Public</short>\n  <interface name=\"ens3\"/>\n  <interface name=\"ens9\"/>\n</zone>\n";
        std::fs::write(&zone, zone_original).unwrap();
        let wan_network = networkd.join("10-wan.network");
        let wan_original = "[Match]\nName=ens3\n\n[Network]\nDHCP=yes\n";
        std::fs::write(&wan_network, wan_original).unwrap();
        let selected = vec!["ens3".to_string(), "ens4".to_string()];

        assert!(unmanage_ifupdown_files(&selected, &interfaces, None).unwrap());
        assert!(unmanage_nm_files(&selected, &conf_d).unwrap());
        assert!(unmanage_firewalld_files(&selected, &zones).unwrap());
        assert!(unmanage_networkd_files(&selected, &networkd).unwrap());
        assert!(hostnet_backup_stands());

        let drop_in = std::fs::read_to_string(conf_d.join(UNMANAGE_CONF)).unwrap();
        assert!(drop_in.contains("interface-name:ens3;"));
        assert!(drop_in.contains("interface-name:ens4"));
        let zone_current = std::fs::read_to_string(&zone).unwrap();
        assert!(!zone_current.contains("ens3"));
        assert!(zone_current.contains("ens9"));
        assert!(
            !wan_network.exists(),
            "the selected .network file must be removed"
        );

        assert!(restore_hostnet_backup().unwrap());
        assert_eq!(std::fs::read_to_string(&interfaces).unwrap(), INTERFACES);
        assert!(!conf_d.join(UNMANAGE_CONF).exists());
        assert_eq!(std::fs::read_to_string(&zone).unwrap(), zone_original);
        assert_eq!(std::fs::read_to_string(&wan_network).unwrap(), wan_original);
        assert!(
            !territory.join(HOSTNET_BACKUP_REL).exists(),
            "the hostnet backup root must be removed after a full restore"
        );
        // 备份目录已删除:再次恢复是 no-op。
        assert!(!restore_hostnet_backup().unwrap());

        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    /// NM 备份仍在但 drop-in 被人工删除:现场漂移,明确拒绝而不是带着失效的
    /// 接管继续。
    #[test]
    fn stale_nm_backup_without_drop_in_is_rejected() {
        let (territory, _, conf_d, _, _) = hostnet_fixture("stale-nm", INTERFACES);
        let _guard = layout::test_territory(&territory);
        assert!(
            unmanage_nm_files(&["ens3".into()], &conf_d).unwrap(),
            "first unmanage applies the drop-in"
        );
        std::fs::remove_file(conf_d.join(UNMANAGE_CONF)).unwrap();
        let error = unmanage_nm_files(&["ens3".into()], &conf_d)
            .expect_err("a missing drop-in with a standing backup must be rejected");
        assert!(
            matches!(error, InstallError::Preflight(ref message) if message.contains("is missing")),
            "unexpected error: {error:?}"
        );

        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    #[test]
    fn reinit_gate_matches_the_standing_unmanaged_set() {
        let (territory, interfaces, conf_d, zones, networkd) =
            hostnet_fixture("reinit-gate", INTERFACES);
        let _guard = layout::test_territory(&territory);
        let zone = zones.join("public.xml");
        std::fs::write(&zone, "<zone>\n  <interface name=\"ens3\"/>\n</zone>\n").unwrap();
        std::fs::write(
            networkd.join("10-wan.network"),
            "[Match]\nName=ens3\n\n[Network]\nDHCP=yes\n",
        )
        .unwrap();
        let gate = |selected: &[String]| {
            ensure_reinit_selection_matches_files(selected, &interfaces, &conf_d)
        };

        // 无备份不设限。
        gate(&["ens3".into()]).unwrap();

        assert!(
            unmanage_ifupdown_files(&["ens3".into(), "ens4".into()], &interfaces, None).unwrap()
        );
        // 集合一致(顺序无关)通过。
        gate(&["ens4".into(), "ens3".into()]).unwrap();
        // 换选接口、多选、少选都拒绝。
        for selected in [
            vec!["ens3".to_string()],
            vec!["ens3".to_string(), "ens5".to_string()],
            vec!["ens9".to_string()],
        ] {
            let error = gate(&selected).expect_err("a changed selection must be rejected");
            assert!(
                matches!(error, InstallError::ParameterUsage(_)),
                "unexpected error: {error:?}"
            );
        }

        // firewalld 备份的现场集合并入并集:仅恢复 ifupdown 备份后,firewalld
        // 侧的 ens3 仍要求选中集合包含它。
        assert!(unmanage_firewalld_files(&["ens3".into()], &zones).unwrap());
        std::fs::remove_dir_all(territory.join(HOSTNET_BACKUP_REL).join("ifupdown")).unwrap();
        gate(&["ens4".into()]).expect_err("the firewalld standing set still pins ens3");
        gate(&["ens3".into()]).unwrap();

        // NM drop-in 的现场集合同样并入:firewalld 仍钉住 ens3、drop-in 声明
        // ens4,单独任一被拒,全集通过。
        assert!(unmanage_nm_files(&["ens4".into()], &conf_d).unwrap());
        gate(&["ens3".to_string()]).expect_err("the NM drop-in pins ens4");
        gate(&["ens4".to_string()]).expect_err("the firewalld backup still pins ens3");
        gate(&["ens3".to_string(), "ens4".to_string()]).unwrap();

        // networkd 被移出文件的 Name= 精确集并入:先模拟 firewalld 备份恢复,
        // 现场只剩 NM{ens4};networkd 移出 ens3 后全集重新变回 {ens3, ens4}。
        std::fs::remove_dir_all(territory.join(HOSTNET_BACKUP_REL).join("firewalld")).unwrap();
        gate(&["ens4".to_string()]).unwrap();
        assert!(unmanage_networkd_files(&["ens3".into()], &networkd).unwrap());
        gate(&["ens4".to_string()]).expect_err("the removed .network file still pins ens3");
        gate(&["ens3".to_string(), "ens4".to_string()]).unwrap();

        // 备份仍在而文件被人工重建:再次摘除被明确拒绝,反查不再计入被重建
        // 文件的名字。
        std::fs::write(
            networkd.join("10-wan.network"),
            "[Match]\nName=ens3\n\n[Network]\nDHCP=yes\n",
        )
        .unwrap();
        let error = unmanage_networkd_files(&["ens3".into()], &networkd)
            .expect_err("a recreated .network file with a standing backup must be rejected");
        assert!(
            matches!(error, InstallError::Preflight(ref message) if message.contains("recreated")),
            "unexpected error: {error:?}"
        );
        gate(&["ens4".to_string()]).unwrap();
        gate(&["ens3".to_string()]).expect_err("the NM drop-in still pins ens4");

        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }
}
