use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;

use super::super::artifacts::hash_str;
use super::super::manager::{ManagedService, ServiceManager};
use super::super::plan::InstallError;
use super::super::root::InstallRoot;

/// 写入受管服务定义原件(0600,原子替换),返回其 SHA-256。
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
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(InstallError::Io)?;
    file.write_all(content.as_bytes())
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
