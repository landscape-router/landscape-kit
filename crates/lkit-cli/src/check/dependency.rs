use std::fs::Metadata;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;

use super::model::{CheckResult, Status};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PackageManager {
    Apt,
    Dnf,
    Yum,
    Pacman,
    Zypper,
}

#[derive(Clone, Copy)]
enum DependencyPackage {
    Iproute,
    Ppp,
}

pub fn run() -> Vec<CheckResult> {
    vec![iproute2(), tc(), pppd(), container_runtime()]
}

fn iproute2() -> CheckResult {
    let result = CheckResult::new("dependency.iproute2", crate::tr!("ip command", "ip 命令"));
    match find_in_path("ip") {
        Some(path) => result.set(
            Status::Pass,
            path.display().to_string(),
            crate::tr!(
                "The ip command exists and is executable",
                "ip 命令存在且可执行"
            ),
        ),
        None => result
            .set(
                Status::Error,
                crate::tr!("not found", "未找到"),
                crate::tr!(
                    "The ip command was not found (provided by the iproute2 package)",
                    "未找到 ip 命令（属于 iproute2 软件包）"
                ),
            )
            .suggestion(install_suggestion(DependencyPackage::Iproute)),
    }
}

fn tc() -> CheckResult {
    let mut result = CheckResult::new(
        "dependency.tc",
        crate::tr!("tc command and BPF support", "tc 命令与 BPF 支持"),
    );
    let Some(path) = find_in_path("tc") else {
        return result
            .set(
                Status::Error,
                crate::tr!("not found", "未找到"),
                crate::tr!(
                    "The tc command was not found (provided by the iproute2 package)",
                    "未找到 tc 命令（属于 iproute2 软件包）"
                ),
            )
            .suggestion(install_suggestion(DependencyPackage::Iproute));
    };
    let path_display = path.display().to_string();
    result.value = path_display.clone();
    match Command::new(&path).args(["filter", "help"]).output() {
        Ok(output) => {
            let mut text = String::from_utf8_lossy(&output.stdout).to_lowercase();
            text.push_str(&String::from_utf8_lossy(&output.stderr).to_lowercase());
            if output.status.success() && text.contains("bpf") {
                result.set(
                    Status::Pass,
                    path_display,
                    crate::tr!(
                        "tc exists and supports BPF filters",
                        "tc 存在且支持 BPF 过滤器"
                    ),
                )
            } else if !output.status.success() {
                result.set(
                    Status::Unknown,
                    path_display,
                    crate::trf!(
                        (
                            "tc filter help failed (exit code {:?})",
                            output.status.code()
                        ),
                        (
                            "tc filter help 执行失败（退出码 {:?}）",
                            output.status.code()
                        )
                    ),
                )
            } else {
                result
                    .set(
                        Status::Error,
                        path_display,
                        crate::tr!(
                            "tc help does not mention bpf; BPF support is unavailable",
                            "tc 帮助文本中未包含 bpf，BPF 支持不可用"
                        ),
                    )
                    .suggestion(crate::tr!(
                        "Upgrade iproute2 or install a build with BPF support",
                        "升级 iproute2 或安装支持 BPF 的版本"
                    ))
            }
        }
        Err(err) => result.set(
            Status::Unknown,
            path_display,
            crate::trf!(
                ("Unable to run tc filter help: {err}"),
                ("无法执行 tc filter help：{err}")
            ),
        ),
    }
}

fn pppd() -> CheckResult {
    let result = CheckResult::new("dependency.pppd", crate::tr!("pppd command", "pppd 命令"));
    match find_in_path("pppd") {
        Some(path) => result.set(
            Status::Pass,
            path.display().to_string(),
            crate::tr!(
                "The pppd command exists and is executable (used for PPPoE)",
                "pppd 命令存在且可执行（用于 PPPoE 拨号）"
            ),
        ),
        None => result
            .set(
                Status::Error,
                crate::tr!("not found", "未找到"),
                crate::tr!(
                    "The pppd command was not found (required for PPPoE)",
                    "未找到 pppd 命令（用于 PPPoE 拨号）"
                ),
            )
            .suggestion(install_suggestion(DependencyPackage::Ppp)),
    }
}

fn container_runtime() -> CheckResult {
    let result = CheckResult::new(
        "dependency.container_runtime",
        crate::tr!(
            "Container runtime (optional dependency)",
            "容器运行时（软依赖）"
        ),
    );
    let found = ["docker", "podman"]
        .iter()
        .find_map(|name| find_in_path(name));
    match found {
        Some(path) => result.set(
            Status::Pass,
            path.display().to_string(),
            crate::tr!("docker or podman is available", "docker 或 podman 可用"),
        ),
        None => result
            .set(Status::Warning, crate::tr!("not found", "未找到"), crate::tr!("Neither docker nor podman is available", "docker 与 podman 均不可用"))
            .suggestion(
                crate::tr!("A container runtime is not required for basic deployment; install and configure Docker or Podman before routing traffic to containers", "缺少容器运行时不阻断基础部署；需要将流量分流到容器时，必须安装并配置 Docker 或 Podman"),
            ),
    }
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let candidate = dir.join(name);
        std::fs::metadata(&candidate)
            .ok()
            .filter(is_executable)
            .map(|_| candidate)
    })
}

fn is_executable(metadata: &Metadata) -> bool {
    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
}

fn install_suggestion(package: DependencyPackage) -> String {
    install_suggestion_for(package, detect_package_manager())
}

fn detect_package_manager() -> Option<PackageManager> {
    [
        ("apt-get", PackageManager::Apt),
        ("dnf", PackageManager::Dnf),
        ("yum", PackageManager::Yum),
        ("pacman", PackageManager::Pacman),
        ("zypper", PackageManager::Zypper),
    ]
    .into_iter()
    .find_map(|(command, manager)| find_in_path(command).map(|_| manager))
}

fn install_suggestion_for(package: DependencyPackage, manager: Option<PackageManager>) -> String {
    let command = match (manager, package) {
        (Some(PackageManager::Apt), DependencyPackage::Iproute) => "apt install iproute2",
        (Some(PackageManager::Apt), DependencyPackage::Ppp) => "apt install ppp",
        (Some(PackageManager::Dnf), DependencyPackage::Iproute) => "dnf install iproute",
        (Some(PackageManager::Dnf), DependencyPackage::Ppp) => "dnf install ppp",
        (Some(PackageManager::Yum), DependencyPackage::Iproute) => "yum install iproute",
        (Some(PackageManager::Yum), DependencyPackage::Ppp) => "yum install ppp",
        (Some(PackageManager::Pacman), DependencyPackage::Iproute) => "pacman -S iproute2",
        (Some(PackageManager::Pacman), DependencyPackage::Ppp) => "pacman -S ppp",
        (Some(PackageManager::Zypper), DependencyPackage::Iproute) => "zypper install iproute2",
        (Some(PackageManager::Zypper), DependencyPackage::Ppp) => "zypper install ppp",
        (None, DependencyPackage::Iproute) => {
            return crate::tr!("Install the package that provides `ip` and `tc` (usually `iproute2`, or `iproute` on Fedora/RHEL)", "安装提供 `ip` 和 `tc` 命令的软件包（通常名为 `iproute2`，Fedora/RHEL 中名为 `iproute`）").into();
        }
        (None, DependencyPackage::Ppp) => {
            return crate::tr!("Install the `ppp` package that provides `pppd`; the package is usually not named `pppd`", "安装提供 `pppd` 命令的 `ppp` 软件包；软件包名通常不是 `pppd`").into();
        }
    };
    let package_note = match package {
        DependencyPackage::Iproute => "",
        DependencyPackage::Ppp => crate::tr!(
            "; the package is named `ppp`, not `pppd`",
            "；软件包名是 `ppp`，不是 `pppd`"
        ),
    };
    crate::trf!(
        ("Run `{command}` as root (prefix it with `sudo` as a regular user){package_note}"),
        ("以 root 身份运行 `{command}`（普通用户在命令前加 `sudo`）{package_note}")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_apt_package_names() {
        assert!(
            install_suggestion_for(DependencyPackage::Iproute, Some(PackageManager::Apt))
                .contains("apt install iproute2")
        );
        assert!(
            install_suggestion_for(DependencyPackage::Ppp, Some(PackageManager::Apt))
                .contains("apt install ppp")
        );
    }

    #[test]
    fn uses_fedora_iproute_package_name() {
        assert!(
            install_suggestion_for(DependencyPackage::Iproute, Some(PackageManager::Dnf))
                .contains("dnf install iproute")
        );
    }

    #[test]
    fn supports_other_common_package_managers() {
        for (manager, expected) in [
            (PackageManager::Yum, "yum install ppp"),
            (PackageManager::Pacman, "pacman -S ppp"),
            (PackageManager::Zypper, "zypper install ppp"),
        ] {
            assert!(
                install_suggestion_for(DependencyPackage::Ppp, Some(manager)).contains(expected)
            );
        }
    }

    #[test]
    fn unknown_manager_still_explains_the_required_commands() {
        assert!(install_suggestion_for(DependencyPackage::Ppp, None).contains("pppd"));
    }
}
