//! One peer-level startup domain, dispatched by the host's actual init style.
use super::model::{CheckResult, Status};
use crate::service::manager::{ServiceManagerKind, host_kind};

pub fn run() -> Vec<CheckResult> {
    run_for(host_kind(), crate::deployment::cgroup::boot_enabled)
}

fn run_for(
    kind: Option<ServiceManagerKind>,
    boot_enabled: impl FnOnce() -> bool,
) -> Vec<CheckResult> {
    let result = CheckResult::new("init.backend", crate::tr!(crate::keys::INIT_BACKEND));
    let mut checks = vec![match kind {
        Some(ServiceManagerKind::Systemd) => result.set(
            Status::Pass,
            "systemd",
            crate::tr!(crate::keys::INIT_SYSTEMD),
        ),
        Some(ServiceManagerKind::Openrc) => {
            result.set(Status::Pass, "OpenRC", crate::tr!(crate::keys::INIT_OPENRC))
        }
        Some(ServiceManagerKind::Sysvinit) => result.set(
            Status::Pass,
            "SysVinit",
            crate::tr!(crate::keys::INIT_SYSVINIT),
        ),
        None => result
            .set(
                Status::Unknown,
                "unknown",
                crate::tr!(crate::keys::INIT_UNKNOWN),
            )
            .suggestion(crate::tr!(crate::keys::INIT_UNKNOWN_HINT)),
    }];
    if kind == Some(ServiceManagerKind::Openrc) {
        checks.extend(super::openrc::run(boot_enabled));
    }
    checks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_backends_only_receive_their_own_requirements() {
        for kind in [
            Some(ServiceManagerKind::Systemd),
            Some(ServiceManagerKind::Sysvinit),
            None,
        ] {
            let checks = run_for(kind, || panic!("must not query OpenRC on another backend"));
            assert_eq!(checks.len(), 1);
            assert_eq!(checks[0].id, "init.backend");
            assert_eq!(
                checks[0].status,
                if kind.is_some() {
                    Status::Pass
                } else {
                    Status::Unknown
                }
            );
        }
        let checks = run_for(Some(ServiceManagerKind::Openrc), || false);
        assert_eq!(checks.len(), 2);
        assert_eq!(checks[0].value, "OpenRC");
        assert_eq!(checks[1].id, "openrc.cgroup_boot");
        assert_eq!(checks[1].status, Status::Error);
    }
}
