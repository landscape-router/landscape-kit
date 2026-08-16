use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;

use serde::Serialize;

use super::super::credentials::Credentials;
use super::super::plan::InstallError;
use super::super::root::InstallRoot;
use crate::deployment::layout;

pub(crate) fn parse_stable_version(
    value: &str,
) -> Result<semver::Version, lkit_repository::ProtocolError> {
    lkit_repository::parse_stable_version(value)
}

pub(crate) fn activate_current(
    root: &InstallRoot,
    version: &semver::Version,
) -> Result<(), InstallError> {
    let current = root.canonical.join("current");
    let tmp_link = layout::territory_run_dir().join(".current.tmp");
    std::fs::create_dir_all(tmp_link.parent().expect("run dir has a parent"))
        .map_err(InstallError::Io)?;
    let _ = std::fs::remove_file(&tmp_link);
    std::os::unix::fs::symlink(format!("releases/{version}"), &tmp_link)
        .map_err(InstallError::Io)?;
    std::fs::rename(&tmp_link, &current).map_err(InstallError::Io)?;
    Ok(())
}

#[derive(Serialize)]
struct InitConfigFile<'a> {
    version: &'a str,
    config: InitAuth<'a>,
}

#[derive(Serialize)]
struct InitAuth<'a> {
    auth: AdminAuth<'a>,
}

#[derive(Serialize)]
struct AdminAuth<'a> {
    admin_user: &'a str,
    admin_pass: &'a str,
}

pub(crate) fn build_init_config(
    version: &semver::Version,
    credentials: &Credentials,
    network: Option<&crate::network::config::NetworkPlan>,
) -> Result<String, InstallError> {
    if let Some(network) = network {
        let config = crate::network::config::LandscapeInit::new(
            version,
            &credentials.admin_user,
            &credentials.password,
            network,
        )?;
        return toml::to_string(&config).map_err(|error| {
            InstallError::ParameterUsage(format!(
                "failed to serialize Landscape network init config: {error}"
            ))
        });
    }
    let config = InitConfigFile {
        version: &version.to_string(),
        config: InitAuth {
            auth: AdminAuth {
                admin_user: &credentials.admin_user,
                admin_pass: &credentials.password,
            },
        },
    };
    toml::to_string(&config).map_err(|error| {
        InstallError::InvalidPassword(format!("failed to serialize init config: {error}"))
    })
}

pub(super) fn write_init_config(root: &InstallRoot, content: &str) -> Result<(), InstallError> {
    let data_dir = root.canonical.join("data");
    std::fs::create_dir_all(&data_dir).map_err(InstallError::Io)?;
    let path = data_dir.join("landscape_init.toml");
    let tmp = data_dir.join(".landscape_init.toml.tmp");
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version() -> semver::Version {
        semver::Version::new(1, 2, 3)
    }

    #[test]
    fn builds_minimal_init_config() {
        let config = build_init_config(
            &version(),
            &Credentials {
                admin_user: "admin".into(),
                password: "Secret123".into(),
            },
            None,
        )
        .unwrap();
        assert_eq!(
            config,
            "version = \"1.2.3\"\n\n[config.auth]\nadmin_user = \"admin\"\nadmin_pass = \"Secret123\"\n"
        );
    }
}
