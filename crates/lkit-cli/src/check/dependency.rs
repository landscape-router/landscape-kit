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
    let result = CheckResult::new(
        "dependency.iproute2",
        crate::tr!(crate::keys::DEPENDENCY_IP_COMMAND),
    );
    match find_in_path("ip") {
        Some(path) => result.set(
            Status::Pass,
            path.display().to_string(),
            crate::tr!(crate::keys::DEPENDENCY_IP_COMMAND_EXISTS_EXECUTABLE),
        ),
        None => result
            .set(
                Status::Error,
                crate::tr!(crate::keys::DEPENDENCY_NOT_FOUND),
                crate::tr!(crate::keys::DEPENDENCY_IP_COMMAND_NOT_FOUND),
            )
            .suggestion(install_suggestion(DependencyPackage::Iproute)),
    }
}

fn tc() -> CheckResult {
    let mut result = CheckResult::new(
        "dependency.tc",
        crate::tr!(crate::keys::DEPENDENCY_TC_COMMAND_AND_BPF),
    );
    let Some(path) = find_in_path("tc") else {
        return result
            .set(
                Status::Error,
                crate::tr!(crate::keys::DEPENDENCY_NOT_FOUND),
                crate::tr!(crate::keys::DEPENDENCY_IP_COMMAND_NOT_FOUND),
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
                    crate::tr!(crate::keys::DEPENDENCY_TC_EXISTS_SUPPORTS_BPF),
                )
            } else if !output.status.success() {
                result.set(
                    Status::Unknown,
                    path_display,
                    crate::tr!(
                        crate::keys::DEPENDENCY_TC_FILTER_HELP_FAILED,
                        exit_code = format!("{:?}", output.status.code())
                    ),
                )
            } else {
                result
                    .set(
                        Status::Error,
                        path_display,
                        crate::tr!(crate::keys::DEPENDENCY_TC_HELP_MENTIONS_BPF),
                    )
                    .suggestion(crate::tr!(crate::keys::DEPENDENCY_UPGRADE_IPROUTE2))
            }
        }
        Err(err) => result.set(
            Status::Unknown,
            path_display,
            crate::tr!(crate::keys::DEPENDENCY_UNABLE_RUN_TC_FILTER_HELP, err = err),
        ),
    }
}

fn pppd() -> CheckResult {
    let result = CheckResult::new(
        "dependency.pppd",
        crate::tr!(crate::keys::DEPENDENCY_PPPD_COMMAND),
    );
    match find_in_path("pppd") {
        Some(path) => result.set(
            Status::Pass,
            path.display().to_string(),
            crate::tr!(crate::keys::DEPENDENCY_PPPD_COMMAND_EXISTS),
        ),
        None => result
            .set(
                Status::Error,
                crate::tr!(crate::keys::DEPENDENCY_NOT_FOUND),
                crate::tr!(crate::keys::DEPENDENCY_PPPD_NOT_FOUND),
            )
            .suggestion(install_suggestion(DependencyPackage::Ppp)),
    }
}

fn container_runtime() -> CheckResult {
    let result = CheckResult::new(
        "dependency.container_runtime",
        crate::tr!(crate::keys::DEPENDENCY_CONTAINER_RUNTIME),
    );
    let found = ["docker", "podman"]
        .iter()
        .find_map(|name| find_in_path(name));
    match found {
        Some(path) => result.set(
            Status::Pass,
            path.display().to_string(),
            crate::tr!(crate::keys::DEPENDENCY_DOCKER_OR_PODMAN_AVAILABLE),
        ),
        None => result
            .set(
                Status::Warning,
                crate::tr!(crate::keys::DEPENDENCY_NOT_FOUND),
                crate::tr!(crate::keys::DEPENDENCY_NO_CONTAINER_RUNTIME),
            )
            .suggestion(crate::tr!(
                crate::keys::DEPENDENCY_CONTAINER_RUNTIME_NOT_REQUIRED
            )),
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
            return crate::tr!(crate::keys::DEPENDENCY_INSTALL_PROVIDING_IP_AND_TC).into();
        }
        (None, DependencyPackage::Ppp) => {
            return crate::tr!(crate::keys::DEPENDENCY_INSTALL_PPP_PACKAGE).into();
        }
    };
    let package_note = match package {
        DependencyPackage::Iproute => String::new(),
        DependencyPackage::Ppp => crate::tr!(crate::keys::DEPENDENCY_PACKAGE_NAMED_PPP),
    };
    crate::tr!(
        crate::keys::DEPENDENCY_RUN_INSTALL_COMMAND,
        command = command,
        package_note = package_note
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
