use std::path::Path;

use super::model::{CheckResult, Status};

pub fn run() -> Vec<CheckResult> {
    vec![
        runtime_root(),
        platform_linux(),
        platform_distribution(),
        platform_architecture(),
    ]
}

pub(crate) const OS_RELEASE_PATH: &str = "/etc/os-release";

fn runtime_root() -> CheckResult {
    let result = CheckResult::new("runtime.root", "运行身份");
    let uid = unsafe { libc::geteuid() };
    let value = format!("uid={uid}");
    if uid == 0 {
        result.set(Status::Pass, value, "以 root 身份运行")
    } else {
        result
            .set(Status::Error, value, "必须以 root 身份运行")
            .suggestion("使用 sudo 或切换为 root 后重新执行 lkit check")
    }
}

fn platform_linux() -> CheckResult {
    let mut result = CheckResult::new("platform.linux", "操作系统");
    let os = std::env::consts::OS;
    result.value = os.to_string();
    if os == "linux" {
        result.set(Status::Pass, os, "系统为 Linux")
    } else {
        result
            .set(Status::Error, os, "只支持 Linux 主机")
            .suggestion("在使用 glibc 的 Linux 主机上执行本命令")
    }
}

fn platform_distribution() -> CheckResult {
    platform_distribution_from(Path::new(OS_RELEASE_PATH))
}

pub(crate) fn platform_distribution_from(os_release_path: &Path) -> CheckResult {
    distribution_result(os_release_id(os_release_path))
}

fn distribution_result(id: Option<String>) -> CheckResult {
    let result = CheckResult::new("platform.distribution", "发行版");
    match id {
        Some(id) => result.set(
            Status::Pass,
            id,
            "已识别 Linux 发行版；兼容性由运行能力检查决定",
        ),
        None => result
            .set(
                Status::Warning,
                "无法读取",
                "无法读取 /etc/os-release 中的发行版 ID；不因发行版名称阻断安装",
            )
            .suggestion("确认主机使用 glibc，并根据缺失依赖的检查结果安装相应软件包"),
    }
}

fn platform_architecture() -> CheckResult {
    let mut result = CheckResult::new("platform.architecture", "CPU 架构");
    let arch = std::env::consts::ARCH;
    result.value = arch.to_string();
    match arch {
        "x86_64" | "aarch64" => result.set(Status::Pass, arch, "架构在首版发布产物支持范围内"),
        _ => result
            .set(Status::Warning, arch, "架构不在首版发布产物优先支持范围内")
            .suggestion("安装包或 eBPF 产物可能不可用，请先确认是否有对应架构的发布产物"),
    }
}

fn os_release_id(path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    content.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("ID=")?;
        let id = rest.trim().trim_matches('"');
        (!id.is_empty()).then(|| id.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_debian_distribution_is_informational() {
        let result = distribution_result(Some("fedora".into()));
        assert_eq!(result.id, "platform.distribution");
        assert_eq!(result.status, Status::Pass);
        assert_eq!(result.value, "fedora");
    }

    #[test]
    fn unreadable_distribution_does_not_block_installation() {
        let result = distribution_result(None);
        assert_eq!(result.status, Status::Warning);
        assert!(!result.suggestion.is_empty());
    }
}
