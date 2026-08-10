use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;

use super::super::artifacts::hash_str;
use super::super::plan::InstallError;
use super::super::root::InstallRoot;
use super::super::systemd;

/// 写入受管 unit 原件(0600,原子替换),返回其 SHA-256。
pub(crate) fn write_unit_origin(root: &InstallRoot, content: &str) -> Result<String, InstallError> {
    systemd::validate_unit(content, &root.canonical)?;
    let service_dir = root.canonical.join("service");
    std::fs::create_dir_all(&service_dir).map_err(InstallError::Io)?;
    let path = service_dir.join("landscape-router.service");
    let tmp = service_dir.join(".landscape-router.service.tmp");
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
