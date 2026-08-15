use super::super::plan::InstallError;
use super::super::systemd::{self, Availability, Systemd};
use super::super::transaction::{Registration, RegistrationKind, SystemdBefore};

/// 服务管理模式选择。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ManagerChoice {
    /// 未指定:systemd 可用则使用,明确不是 systemd init 时使用 none,
    /// 看似 systemd 但环境损坏时失败。
    Auto,
    /// 显式要求 systemd,不可用或环境损坏时失败。
    Systemd,
    /// 显式要求无 systemd,只管理文件和事务。
    None,
}

/// 实际选择的服务管理模式。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ServiceManager {
    Systemd,
    None,
}

pub(crate) fn select_manager(
    choice: ManagerChoice,
    systemd: &Systemd,
) -> Result<ServiceManager, InstallError> {
    match choice {
        ManagerChoice::None => Ok(ServiceManager::None),
        ManagerChoice::Systemd => match systemd.probe() {
            Availability::Available { .. } => Ok(ServiceManager::Systemd),
            availability => Err(InstallError::Systemd(format!(
                "--service-manager systemd requested but systemd is not available: {availability:?}"
            ))),
        },
        ManagerChoice::Auto => match systemd.probe() {
            Availability::Available { .. } => Ok(ServiceManager::Systemd),
            Availability::NotSystemdInit => Ok(ServiceManager::None),
            availability => Err(InstallError::Systemd(format!(
                "the host appears to run systemd but it is damaged: {availability:?}"
            ))),
        },
    }
}

pub(crate) fn capture_systemd_before(systemd: &Systemd) -> Result<SystemdBefore, InstallError> {
    let (kind, target) = match systemd::query_registration(systemd)? {
        systemd::Registration::Missing => (RegistrationKind::Missing, None),
        systemd::Registration::Symlink { target } => (
            RegistrationKind::Symlink,
            Some(target.display().to_string()),
        ),
        systemd::Registration::Conflict { file_type } => {
            return Err(InstallError::Systemd(format!(
                "cannot take over {}: {file_type} ownership conflict",
                systemd::UNIT_NAME
            )));
        }
    };
    Ok(SystemdBefore {
        registration: Registration { kind, target },
        enabled: systemd::is_enabled(systemd)?,
        active: systemd::is_active(systemd)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
