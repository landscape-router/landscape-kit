use super::super::manager::{Availability, ServiceManager, ServiceManagerKind};
use super::super::plan::InstallError;

/// 安装必须托管到可用的服务管理器。lkit 明确依赖发行版自启服务,
/// 不再支持 `none`(不托管运行态)部署;探测不到可用后端时明确失败。
pub(crate) fn require_manager(
    manager: &dyn ServiceManager,
) -> Result<ServiceManagerKind, InstallError> {
    match manager.probe() {
        Availability::Available { .. } => Ok(ServiceManagerKind::Systemd),
        availability => Err(InstallError::UnsupportedPlatform(format!(
            "no supported service manager is available on this host: {availability:?}; lkit requires the distro init system to supervise the service"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::service::systemd::Systemd;

    #[test]
    fn rejects_unavailable_systemd_environment() {
        let dir = std::env::temp_dir().join(format!(
            "lkit-pipeline-test-unavailable-systemd-{}",
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
            require_manager(&systemd),
            Err(InstallError::UnsupportedPlatform(_))
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
