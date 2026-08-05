use std::path::Path;
use std::process::Command;

use super::model::{CheckResult, Status};

const UNIT_DIRS: [&str; 3] = [
    "/etc/systemd/system/",
    "/usr/lib/systemd/system/",
    "/lib/systemd/system/",
];

struct UnitInfo {
    installed: bool,
    active: bool,
    enabled: bool,
}

pub fn run() -> Vec<CheckResult> {
    vec![
        network_manager(),
        systemd_resolved(),
        firewalld(),
        selinux(),
    ]
}

fn network_manager() -> CheckResult {
    let result = CheckResult::new("service.network_manager", "NetworkManager");
    match unit_info("NetworkManager.service") {
        None => result.set(Status::Unknown, crate::tr!("query failed", "无法查询"), crate::tr!("systemctl is unavailable or the query failed", "systemctl 不可用或查询失败")),
        Some(info) if info.active => result
            .set(
                Status::Error,
                crate::tr!("running", "运行中"),
                crate::tr!("NetworkManager is running and may take over network interfaces managed by Landscape", "NetworkManager 正在运行，可能接管 Landscape 管理的网络接口"),
            )
            .suggestion(crate::tr!("Stop and disable NetworkManager, or configure it not to manage interfaces used by Landscape", "停止并禁用 NetworkManager，或配置其不管理 Landscape 使用的接口")),
        Some(info) if info.enabled => result
            .set(
                Status::Warning,
                crate::tr!("enabled (not running)", "已启用（未运行）"),
                crate::tr!("NetworkManager is enabled and may take over network interfaces after a restart", "NetworkManager 已设置开机启用，重启后可能接管网络接口"),
            )
            .suggestion(crate::tr!("If NetworkManager is not needed, run systemctl disable --now NetworkManager", "如不需要 NetworkManager，可执行 systemctl disable --now NetworkManager")),
        Some(info) if info.installed => result.set(
            Status::Warning,
            crate::tr!("installed (not running)", "已安装（未运行）"),
            crate::tr!("NetworkManager is installed but is not running or enabled", "NetworkManager 已安装但当前未运行，未启用开机启动"),
        ),
        Some(_) => result.set(Status::Pass, crate::tr!("not installed", "未安装"), crate::tr!("NetworkManager is not installed", "NetworkManager 未安装")),
    }
}

fn systemd_resolved() -> CheckResult {
    let result = CheckResult::new("service.systemd_resolved", "systemd-resolved");
    match unit_info("systemd-resolved.service") {
        None => result.set(Status::Unknown, crate::tr!("query failed", "无法查询"), crate::tr!("systemctl is unavailable or the query failed", "systemctl 不可用或查询失败")),
        Some(info) if info.active || info.enabled => result
            .set(Status::Warning, crate::tr!("running or enabled", "运行中或已启用"), crate::tr!("systemd-resolved may occupy or manage DNS port 53", "systemd-resolved 可能占用或管理 DNS（53）"))
            .suggestion(
                crate::tr!("If port 53 is occupied, port.dns reports an error; run systemctl stop --now systemd-resolved to release it", "若 53 端口被占用由 port.dns 报告错误；可执行 systemctl stop --now systemd-resolved 释放"),
            ),
        Some(_) => result.set(Status::Pass, crate::tr!("not installed or enabled", "未安装或未启用"), crate::tr!("systemd-resolved is not running or enabled", "systemd-resolved 未运行且未启用")),
    }
}

fn firewalld() -> CheckResult {
    let result = CheckResult::new("service.firewalld", "firewalld");
    match unit_info("firewalld.service") {
        None => result.set(
            Status::Unknown,
            crate::tr!("query failed", "无法查询"),
            crate::tr!(
                "systemctl is unavailable or the query failed",
                "systemctl 不可用或查询失败"
            ),
        ),
        Some(info) if info.active => result
            .set(
                Status::Warning,
                crate::tr!("running", "运行中"),
                crate::tr!(
                    "firewalld may block Landscape network rules",
                    "firewalld 可能阻断 Landscape 的网络规则"
                ),
            )
            .suggestion(crate::tr!(
                "Confirm that ports and traffic required by Landscape are allowed",
                "确认 Landscape 所需端口和流量已加入放行规则"
            )),
        Some(info) if info.enabled => result.set(
            Status::Warning,
            crate::tr!("enabled (not running)", "已启用（未运行）"),
            crate::tr!(
                "firewalld is enabled and may block Landscape network rules after a restart",
                "firewalld 已设置开机启用，重启后可能阻断 Landscape 网络规则"
            ),
        ),
        Some(_) => result.set(
            Status::Pass,
            crate::tr!("not installed or enabled", "未安装或未启用"),
            crate::tr!(
                "firewalld is not running or enabled",
                "firewalld 未运行且未启用"
            ),
        ),
    }
}

fn selinux() -> CheckResult {
    let result = CheckResult::new("security.selinux", "SELinux");
    match std::fs::read_to_string("/sys/fs/selinux/enforce") {
        Ok(raw) => {
            let value = raw.trim();
            if value == "1" {
                result
                    .set(
                        Status::Warning,
                        "enforcing",
                        crate::tr!(
                            "SELinux is in enforcing mode",
                            "SELinux 处于 enforcing 模式"
                        ),
                    )
                    .suggestion(crate::tr!(
                        "Additional Landscape permissions or an SELinux policy are required",
                        "需要额外放行 Landscape 相关权限（或配置 SELinux 策略）"
                    ))
            } else {
                result.set(
                    Status::Pass,
                    "permissive",
                    crate::tr!(
                        "SELinux is not in enforcing mode",
                        "SELinux 未处于 enforcing 模式"
                    ),
                )
            }
        }
        Err(_) if Path::new("/sys/fs/selinux").exists() => result.set(
            Status::Unknown,
            crate::tr!("unavailable", "无法读取"),
            crate::tr!("Unable to read SELinux status", "无法读取 SELinux 状态"),
        ),
        Err(_) => result.set(
            Status::Pass,
            crate::tr!("disabled", "未启用"),
            crate::tr!("SELinux is disabled", "SELinux 未启用"),
        ),
    }
}

fn unit_info(unit: &str) -> Option<UnitInfo> {
    let installed = UNIT_DIRS
        .iter()
        .any(|dir| Path::new(dir).join(unit).exists());
    let is_active = systemctl_equals(unit, "is-active", "active");
    let is_enabled = systemctl_equals(unit, "is-enabled", "enabled");
    Some(UnitInfo {
        installed,
        active: is_active?,
        enabled: is_enabled?,
    })
}

fn systemctl_equals(unit: &str, verb: &str, expected: &str) -> Option<bool> {
    let output = Command::new("systemctl").args([verb, unit]).output().ok()?;
    let status = output.status;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !text.is_empty() {
        Some(text == expected)
    } else if status.success() {
        Some(true)
    } else if verb == "is-enabled" {
        // systemd 252 对不存在的 unit 把错误写入 stderr 且以非零退出,
        // stdout 为空;此时判定为未启用(管理器是否可达由 is-active 检查),
        // 与新版本 systemd 在 stdout 输出 "not-found" 的语义一致。
        Some(false)
    } else {
        None
    }
}
