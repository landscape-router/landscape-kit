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
        None => result.set(Status::Unknown, "无法查询", "systemctl 不可用或查询失败"),
        Some(info) if info.active => result
            .set(
                Status::Error,
                "运行中",
                "NetworkManager 正在运行，可能接管 Landscape 管理的网络接口",
            )
            .suggestion("停止并禁用 NetworkManager，或配置其不管理 Landscape 使用的接口"),
        Some(info) if info.enabled => result
            .set(
                Status::Warning,
                "已启用（未运行）",
                "NetworkManager 已设置开机启用，重启后可能接管网络接口",
            )
            .suggestion("如不需要 NetworkManager，可执行 systemctl disable --now NetworkManager"),
        Some(info) if info.installed => result.set(
            Status::Warning,
            "已安装（未运行）",
            "NetworkManager 已安装但当前未运行，未启用开机启动",
        ),
        Some(_) => result.set(Status::Pass, "未安装", "NetworkManager 未安装"),
    }
}

fn systemd_resolved() -> CheckResult {
    let result = CheckResult::new("service.systemd_resolved", "systemd-resolved");
    match unit_info("systemd-resolved.service") {
        None => result.set(Status::Unknown, "无法查询", "systemctl 不可用或查询失败"),
        Some(info) if info.active || info.enabled => result
            .set(Status::Warning, "运行中或已启用", "systemd-resolved 可能占用或管理 DNS（53）")
            .suggestion(
                "若 53 端口被占用由 port.dns 报告错误；可执行 systemctl stop --now systemd-resolved 释放",
            ),
        Some(_) => result.set(Status::Pass, "未安装或未启用", "systemd-resolved 未运行且未启用"),
    }
}

fn firewalld() -> CheckResult {
    let result = CheckResult::new("service.firewalld", "firewalld");
    match unit_info("firewalld.service") {
        None => result.set(Status::Unknown, "无法查询", "systemctl 不可用或查询失败"),
        Some(info) if info.active => result
            .set(
                Status::Warning,
                "运行中",
                "firewalld 可能阻断 Landscape 的网络规则",
            )
            .suggestion("确认 Landscape 所需端口和流量已加入放行规则"),
        Some(info) if info.enabled => result.set(
            Status::Warning,
            "已启用（未运行）",
            "firewalld 已设置开机启用，重启后可能阻断 Landscape 网络规则",
        ),
        Some(_) => result.set(Status::Pass, "未安装或未启用", "firewalld 未运行且未启用"),
    }
}

fn selinux() -> CheckResult {
    let result = CheckResult::new("security.selinux", "SELinux");
    match std::fs::read_to_string("/sys/fs/selinux/enforce") {
        Ok(raw) => {
            let value = raw.trim();
            if value == "1" {
                result
                    .set(Status::Warning, "enforcing", "SELinux 处于 enforcing 模式")
                    .suggestion("需要额外放行 Landscape 相关权限（或配置 SELinux 策略）")
            } else {
                result.set(Status::Pass, "permissive", "SELinux 未处于 enforcing 模式")
            }
        }
        Err(_) if Path::new("/sys/fs/selinux").exists() => {
            result.set(Status::Unknown, "无法读取", "无法读取 SELinux 状态")
        }
        Err(_) => result.set(Status::Pass, "未启用", "SELinux 未启用"),
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
