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
    let result = CheckResult::new(
        "runtime.root",
        crate::tr!(crate::keys::PLATFORM_RUNTIME_IDENTITY),
    );
    let uid = unsafe { libc::geteuid() };
    let value = format!("uid={uid}");
    if uid == 0 {
        result.set(
            Status::Pass,
            value,
            crate::tr!(crate::keys::PLATFORM_RUNNING_AS_ROOT),
        )
    } else {
        result
            .set(
                Status::Error,
                value,
                crate::tr!(crate::keys::PLATFORM_MUST_RUN_AS_ROOT),
            )
            .suggestion(crate::tr!(crate::keys::PLATFORM_USE_SUDO_OR_ROOT))
    }
}

fn platform_linux() -> CheckResult {
    let mut result = CheckResult::new(
        "platform.linux",
        crate::tr!(crate::keys::PLATFORM_OPERATING_SYSTEM),
    );
    let os = std::env::consts::OS;
    result.value = os.to_string();
    if os == "linux" {
        result.set(
            Status::Pass,
            os,
            crate::tr!(crate::keys::PLATFORM_OS_IS_LINUX),
        )
    } else {
        result
            .set(
                Status::Error,
                os,
                crate::tr!(crate::keys::PLATFORM_ONLY_LINUX_HOSTS_SUPPORTED),
            )
            .suggestion(crate::tr!(crate::keys::PLATFORM_RUN_ON_GLIBC_LINUX))
    }
}

fn platform_distribution() -> CheckResult {
    platform_distribution_from(Path::new(OS_RELEASE_PATH))
}

pub(crate) fn platform_distribution_from(os_release_path: &Path) -> CheckResult {
    distribution_result(os_release_id(os_release_path))
}

fn distribution_result(id: Option<String>) -> CheckResult {
    let result = CheckResult::new(
        "platform.distribution",
        crate::tr!(crate::keys::PLATFORM_DISTRIBUTION),
    );
    match id {
        Some(id) => result.set(
            Status::Pass,
            id,
            crate::tr!(crate::keys::PLATFORM_DISTRIBUTION_IDENTIFIED),
        ),
        None => result
            .set(
                Status::Warning,
                crate::tr!(crate::keys::PLATFORM_UNAVAILABLE),
                crate::tr!(crate::keys::PLATFORM_UNABLE_READ_DISTRIBUTION_ID),
            )
            .suggestion(crate::tr!(
                crate::keys::PLATFORM_CONFIRM_GLIBC_AND_INSTALL_PACKAGES
            )),
    }
}

fn platform_architecture() -> CheckResult {
    let mut result = CheckResult::new(
        "platform.architecture",
        crate::tr!(crate::keys::PLATFORM_CPU_ARCHITECTURE),
    );
    let arch = std::env::consts::ARCH;
    result.value = arch.to_string();
    match arch {
        "x86_64" | "aarch64" => result.set(
            Status::Pass,
            arch,
            crate::tr!(crate::keys::PLATFORM_ARCHITECTURE_SUPPORTED_BY_ARTIFACTS),
        ),
        _ => result
            .set(
                Status::Warning,
                arch,
                crate::tr!(crate::keys::PLATFORM_ARCHITECTURE_NOT_PRIMARY_TARGET),
            )
            .suggestion(crate::tr!(crate::keys::PLATFORM_CONFIRM_ARTIFACTS_EXIST)),
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
