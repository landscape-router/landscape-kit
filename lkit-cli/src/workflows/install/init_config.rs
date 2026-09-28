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
    root: &InstallRoot,
    version: &semver::Version,
    credentials: &Credentials,
    network: Option<&crate::network::config::NetworkPlan>,
) -> Result<String, InstallError> {
    if let Some(network) = network {
        // >= 0.25.1 的 release 由目标二进制的 `config` 子命令生成初始化配置:
        // 生成的文件内嵌目标二进制版本且只能被同版本导入,必须用目标 release
        // 目录下的 webserver 生成(此时尚未激活 `current` 链接)。
        if crate::network::config_cli::config_cli_available(version) {
            let binary = root
                .canonical
                .join("releases")
                .join(version.to_string())
                .join(crate::release::artifacts::WEBSERVER_BINARY);
            let args = crate::network::config_cli::config_subcommand_args(credentials, network)?;
            return crate::network::config_cli::generate_init_config_via_cli(&binary, &args);
        }
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
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn version() -> semver::Version {
        semver::Version::new(1, 2, 3)
    }

    fn credentials() -> Credentials {
        Credentials {
            admin_user: "admin".into(),
            password: "Secret123".into(),
        }
    }

    fn temp_root(name: &str) -> InstallRoot {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "lkit-init-config-{name}-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        InstallRoot {
            install_root: dir.clone(),
            canonical: dir,
        }
    }

    #[test]
    fn builds_minimal_init_config_without_a_network_plan() {
        let root = temp_root("minimal");
        let config = build_init_config(&root, &version(), &credentials(), None).unwrap();
        assert_eq!(
            config,
            "version = \"1.2.3\"\n\n[config.auth]\nadmin_user = \"admin\"\nadmin_pass = \"Secret123\"\n"
        );
        std::fs::remove_dir_all(root.canonical).unwrap();
    }

    fn routed_lan_plan() -> crate::network::config::NetworkPlan {
        crate::network::config::NetworkPlan {
            mode: crate::network::config::NetworkMode::RoutedLan {
                wan: "ens3".into(),
                wan_ipv4: Some(crate::network::config::WanIpv4Config::Dhcp),
                lan: vec!["ens4".into()],
                management: "192.168.10.1/24".parse().unwrap(),
                dhcp_start: "192.168.10.100".parse().unwrap(),
                dhcp_end: "192.168.10.254".parse().unwrap(),
            },
            selected_macs: vec![
                crate::network::config::SelectedInterface {
                    name: "ens3".into(),
                    mac: "52:54:00:00:00:01".into(),
                },
                crate::network::config::SelectedInterface {
                    name: "ens4".into(),
                    mac: "52:54:00:00:00:02".into(),
                },
            ],
        }
    }

    #[test]
    fn releases_before_the_config_cli_threshold_keep_the_hand_assembled_path() {
        let root = temp_root("legacy");
        let config = build_init_config(
            &root,
            &semver::Version::new(0, 25, 0),
            &credentials(),
            Some(&routed_lan_plan()),
        )
        .unwrap();
        assert_eq!(config.split('"').nth(1), Some("0.25.0"));
        assert!(config.contains("[[firewalls]]"));
        assert!(config.contains("[[route_wans]]"));
        std::fs::remove_dir_all(root.canonical).unwrap();
    }

    #[test]
    fn supported_releases_generate_the_init_config_via_the_target_binary() {
        let root = temp_root("config-cli");
        let release = root.canonical.join("releases/1.2.3");
        std::fs::create_dir_all(&release).unwrap();
        std::fs::write(
            release.join(crate::release::artifacts::WEBSERVER_BINARY),
            "#!/bin/sh\nprintf 'version = \"1.2.3\"\\n'\n",
        )
        .unwrap();
        let binary = release.join(crate::release::artifacts::WEBSERVER_BINARY);
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        let config =
            build_init_config(&root, &version(), &credentials(), Some(&routed_lan_plan())).unwrap();
        assert_eq!(config, "version = \"1.2.3\"\n");
        std::fs::remove_dir_all(root.canonical).unwrap();
    }

    #[test]
    fn missing_target_binary_surfaces_a_parameter_usage_error() {
        let root = temp_root("missing-binary");
        let error = build_init_config(&root, &version(), &credentials(), Some(&routed_lan_plan()))
            .unwrap_err();
        assert!(error.to_string().contains("cannot run"), "{error}");
        std::fs::remove_dir_all(root.canonical).unwrap();
    }
}
