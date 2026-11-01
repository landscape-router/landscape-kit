use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use super::super::artifacts::hash_str;
use super::super::manager::{ManagedService, ServiceManager, ServiceManagerKind};
use super::super::plan::InstallError;
use super::super::root::InstallRoot;

/// 原子写入服务原件：systemd 0600，init 脚本 0755；返回 SHA-256。
/// 写入前用当前后端校验定义仍满足安全不变量。
pub(crate) fn write_unit_origin(
    root: &InstallRoot,
    manager: &dyn ServiceManager,
    service: ManagedService,
    content: &str,
) -> Result<String, InstallError> {
    manager.validate_definition(service, content, &root.canonical)?;
    let service_dir = root.canonical.join("service");
    std::fs::create_dir_all(&service_dir).map_err(InstallError::Io)?;
    let name = manager.service_name(service);
    let path = service_dir.join(name);
    let tmp = service_dir.join(format!(".{name}.tmp"));
    let mode = if manager.kind() == ServiceManagerKind::Systemd {
        0o600
    } else {
        0o755
    };
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(&tmp)
        .map_err(InstallError::Io)?;
    file.write_all(content.as_bytes())
        .map_err(InstallError::Io)?;
    file.set_permissions(std::fs::Permissions::from_mode(mode))
        .map_err(InstallError::Io)?;
    file.sync_all().map_err(InstallError::Io)?;
    std::fs::rename(&tmp, &path).map_err(InstallError::Io)?;
    Ok(hash_str(content))
}

pub(crate) fn reference_command(root: &InstallRoot) -> String {
    format!(
        "{} --config-dir {} --web {}",
        shell_escape(
            &root
                .canonical
                .join("current/landscape-webserver")
                .display()
                .to_string()
        ),
        shell_escape(&root.canonical.join("data").display().to_string()),
        shell_escape(&root.canonical.join("current/static").display().to_string()),
    )
}

fn shell_escape(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openrc_service_origins_are_executable() {
        let dir = std::env::temp_dir().join(format!("lkit-openrc-origin-{}", uuid::Uuid::now_v7()));
        let root = InstallRoot {
            install_root: dir.clone(),
            canonical: dir.clone(),
        };
        let manager = crate::service::openrc::Openrc {
            rc_service: dir.join("rc-service"),
            rc_update: dir.join("rc-update"),
            init_d_dir: dir.join("init.d"),
            resolv_conf: dir.join("resolv.conf"),
        };
        let content = manager
            .render_definition(ManagedService::LandscapeRouter, &dir)
            .unwrap();
        write_unit_origin(&root, &manager, ManagedService::LandscapeRouter, &content).unwrap();
        let path = dir.join("service/landscape-router.service");
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o755
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn builds_reference_command() {
        let root = InstallRoot {
            install_root: "/root/.lkit/landscape".into(),
            canonical: "/root/.lkit/landscape".into(),
        };
        assert_eq!(
            reference_command(&root),
            "'/root/.lkit/landscape/current/landscape-webserver' --config-dir '/root/.lkit/landscape/data' --web '/root/.lkit/landscape/current/static'"
        );
    }
}
