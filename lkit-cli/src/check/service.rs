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
        None => result.set(
            Status::Unknown,
            crate::tr!(crate::keys::SERVICE_QUERY_FAILED),
            crate::tr!(crate::keys::SERVICE_SYSTEMCTL_UNAVAILABLE_OR_QUERY_FAILED),
        ),
        Some(info) if info.active => result
            .set(
                Status::Error,
                crate::tr!(crate::keys::SERVICE_RUNNING),
                crate::tr!(crate::keys::SERVICE_NETWORK_MANAGER_RUNNING_TAKEOVER),
            )
            .suggestion(crate::tr!(
                crate::keys::SERVICE_STOP_AND_DISABLE_NETWORK_MANAGER
            )),
        Some(info) if info.enabled => result
            .set(
                Status::Warning,
                crate::tr!(crate::keys::SERVICE_ENABLED_NOT_RUNNING),
                crate::tr!(crate::keys::SERVICE_NETWORK_MANAGER_ENABLED_TAKEOVER),
            )
            .suggestion(crate::tr!(crate::keys::SERVICE_DISABLE_NETWORK_MANAGER_NOW)),
        Some(info) if info.installed => result.set(
            Status::Warning,
            crate::tr!(crate::keys::SERVICE_INSTALLED_NOT_RUNNING),
            crate::tr!(crate::keys::SERVICE_NETWORK_MANAGER_INSTALLED_NOT_RUNNING),
        ),
        Some(_) => result.set(
            Status::Pass,
            crate::tr!(crate::keys::SERVICE_NOT_INSTALLED),
            crate::tr!(crate::keys::SERVICE_NETWORK_MANAGER_NOT_INSTALLED),
        ),
    }
}

fn systemd_resolved() -> CheckResult {
    let result = CheckResult::new("service.systemd_resolved", "systemd-resolved");
    match unit_info("systemd-resolved.service") {
        None => result.set(
            Status::Unknown,
            crate::tr!(crate::keys::SERVICE_QUERY_FAILED),
            crate::tr!(crate::keys::SERVICE_SYSTEMCTL_UNAVAILABLE_OR_QUERY_FAILED),
        ),
        Some(info) if info.active || info.enabled => result
            .set(
                Status::Warning,
                crate::tr!(crate::keys::SERVICE_RUNNING_OR_ENABLED),
                crate::tr!(crate::keys::SERVICE_SYSTEMD_RESOLVED_MAY_OCCUPY_DNS),
            )
            .suggestion(crate::tr!(crate::keys::SERVICE_RELEASE_DNS_PORT_53)),
        Some(_) => result.set(
            Status::Pass,
            crate::tr!(crate::keys::SERVICE_NOT_INSTALLED_OR_ENABLED),
            crate::tr!(crate::keys::SERVICE_SYSTEMD_RESOLVED_NOT_RUNNING_OR_ENABLED),
        ),
    }
}

fn firewalld() -> CheckResult {
    let result = CheckResult::new("service.firewalld", "firewalld");
    match unit_info("firewalld.service") {
        None => result.set(
            Status::Unknown,
            crate::tr!(crate::keys::SERVICE_QUERY_FAILED),
            crate::tr!(crate::keys::SERVICE_SYSTEMCTL_UNAVAILABLE_OR_QUERY_FAILED),
        ),
        Some(info) if info.active => result
            .set(
                Status::Error,
                crate::tr!(crate::keys::SERVICE_RUNNING),
                crate::tr!(crate::keys::SERVICE_FIREWALLD_MAY_BLOCK_RULES),
            )
            .suggestion(crate::tr!(
                crate::keys::SERVICE_CONFIRM_LANDSCAPE_PORTS_ALLOWED
            )),
        Some(info) if info.enabled => result.set(
            Status::Error,
            crate::tr!(crate::keys::SERVICE_ENABLED_NOT_RUNNING),
            crate::tr!(crate::keys::SERVICE_FIREWALLD_ENABLED_MAY_BLOCK_AFTER_RESTART),
        ),
        Some(_) => result.set(
            Status::Pass,
            crate::tr!(crate::keys::SERVICE_NOT_INSTALLED_OR_ENABLED),
            crate::tr!(crate::keys::SERVICE_FIREWALLD_NOT_RUNNING_OR_ENABLED),
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
                        crate::tr!(crate::keys::SERVICE_SELINUX_ENFORCING_MODE),
                    )
                    .suggestion(crate::tr!(crate::keys::SERVICE_REQUIRE_SELINUX_PERMISSIONS))
            } else {
                result.set(
                    Status::Pass,
                    "permissive",
                    crate::tr!(crate::keys::SERVICE_SELINUX_NOT_ENFORCING),
                )
            }
        }
        Err(_) if Path::new("/sys/fs/selinux").exists() => result.set(
            Status::Unknown,
            crate::tr!(crate::keys::SERVICE_UNAVAILABLE),
            crate::tr!(crate::keys::SERVICE_UNABLE_READ_SELINUX_STATUS),
        ),
        Err(_) => result.set(
            Status::Pass,
            crate::tr!(crate::keys::SERVICE_DISABLED),
            crate::tr!(crate::keys::SERVICE_SELINUX_DISABLED),
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
