pub(crate) use super::super::manager::{Availability, ServiceManager, ServiceManagerKind};
use super::super::plan::InstallError;

/// 服务管理模式选择。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ManagerChoice {
    /// 未指定:当前服务管理器可用则使用,明确未检测到任何后端时使用 none,
    /// 看似使用当前后端但环境损坏时失败。
    Auto,
    /// 显式要求 systemd,不可用或环境损坏时失败。
    Systemd,
    /// 显式要求无服务管理器,只管理文件和事务。
    None,
}

/// 根据选择与后端可用性决定实际使用的服务管理器。
pub(crate) fn select_manager(
    choice: ManagerChoice,
    manager: &dyn ServiceManager,
) -> Result<ServiceManagerKind, InstallError> {
    match choice {
        ManagerChoice::None => Ok(ServiceManagerKind::None),
        ManagerChoice::Systemd => match manager.probe() {
            Availability::Available { .. } => Ok(ServiceManagerKind::Systemd),
            availability => Err(InstallError::Systemd(format!(
                "--service-manager systemd requested but systemd is not available: {availability:?}"
            ))),
        },
        ManagerChoice::Auto => match manager.probe() {
            Availability::Available { .. } => Ok(ServiceManagerKind::Systemd),
            Availability::NotDetected => Ok(ServiceManagerKind::None),
            availability => Err(InstallError::Systemd(format!(
                "the host appears to run systemd but it is damaged: {availability:?}"
            ))),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::service::systemd::Systemd;

    #[test]
    fn auto_rejects_damaged_systemd_environment() {
        let dir = std::env::temp_dir().join(format!(
            "lkit-pipeline-test-auto-damaged-systemd-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("run")).unwrap();
        let systemd = Systemd {
            systemctl: dir.join("missing-systemctl"),
            system_unit_dir: dir.join("units"),
            run_systemd_dir: dir.join("run"),
            pid1_is_systemd: true,
            resolv_conf: dir.join("resolv.conf"),
        };

        assert!(matches!(
            select_manager(ManagerChoice::Auto, &systemd),
            Err(InstallError::Systemd(_))
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
