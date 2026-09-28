use std::path::Path;
use std::process::{Command, Stdio};

use crate::deployment::plan::InstallError;
use crate::interaction::credentials::Credentials;
use crate::network::config::{
    Ipv4Cidr, MANAGEMENT_BRIDGE, NetworkMode, NetworkPlan, WanIpv4Config, network_error,
};

/// `landscape config` 子命令自该版本起提供 lkit 依赖的完整 flag 面
/// (admin 凭据、可选 LAN、静态 NAT);更早的 release 不具备,继续走 lkit 手拼路径。
/// 按完整 semver 语义比较:0.25.1 的预发布版本(如 0.25.1-rc.1)不算达标。
const MIN_CONFIG_CLI_VERSION: semver::Version = semver::Version::new(0, 25, 1);

/// WAN 管理端口映射(WAN-only 模式下管理通道依赖这两条 TCP 静态映射)。
const WAN_MANAGEMENT_NAT_PORTS: [(u16, u16); 2] = [(22, 22), (6443, 6443)];

/// lkit 网络计划固定的 LAN DHCP 租期(秒),与旧手拼路径保持一致。
const LAN_DHCP_LEASE_SECONDS: u32 = 43_200;

pub(crate) fn config_cli_available(version: &semver::Version) -> bool {
    *version >= MIN_CONFIG_CLI_VERSION
}

/// 把网络计划与凭据翻译为 `landscape-webserver config` 的参数。
///
/// 生成的文件内嵌目标二进制的版本,只能被同版本导入,因此调用方必须传入目标
/// release 目录下的 webserver 二进制路径,而不是 lkit 进程内拼装。
pub(crate) fn config_subcommand_args(
    credentials: &Credentials,
    network: &NetworkPlan,
) -> Result<Vec<String>, InstallError> {
    let mut args = vec![
        "--admin-user".to_string(),
        credentials.admin_user.clone(),
        "--admin-pass".to_string(),
        credentials.password.clone(),
        // lkit 的服务集与子命令默认值不同:WAN 上启用 firewall,不启用 nat。
        "--enable".to_string(),
        "firewall".to_string(),
        "--disable".to_string(),
        "nat".to_string(),
    ];
    match &network.mode {
        NetworkMode::WanOnly {
            wan,
            address,
            gateway,
        } => {
            push_static_wan(&mut args, wan, address, *gateway);
            for (wan_port, lan_port) in WAN_MANAGEMENT_NAT_PORTS {
                args.push("--static-nat".to_string());
                args.push(format!("{wan_port}:{lan_port}"));
            }
        }
        NetworkMode::WanDhcp { wan } => {
            push_wan_iface(&mut args, wan);
            args.extend(["--wan-mode".to_string(), "dhcp".to_string()]);
        }
        NetworkMode::RoutedLan {
            wan,
            wan_ipv4,
            lan,
            management,
            dhcp_start,
            dhcp_end,
        } => {
            match wan_ipv4 {
                Some(WanIpv4Config::Static { address, gateway }) => {
                    push_static_wan(&mut args, wan, address, *gateway);
                }
                Some(WanIpv4Config::Dhcp) => {
                    push_wan_iface(&mut args, wan);
                    args.extend(["--wan-mode".to_string(), "dhcp".to_string()]);
                }
                // 向导与接管发现都恒产生 Some;None 计划无法用子命令表达。
                None => {
                    return Err(network_error(
                        "a RoutedLan plan without a WAN IPv4 mode cannot be expressed via \
                         `landscape config`",
                    ));
                }
            }
            args.push("--lan-iface".to_string());
            args.push(MANAGEMENT_BRIDGE.to_string());
            args.push("--lan-ip".to_string());
            args.push(management.to_string());
            for member in lan {
                args.push("--lan-member".to_string());
                args.push(member.clone());
            }
            args.push("--lan-dhcp-range".to_string());
            args.push(format!("{dhcp_start}-{dhcp_end}"));
            args.push("--lan-dhcp-lease".to_string());
            args.push(LAN_DHCP_LEASE_SECONDS.to_string());
        }
    }
    args.push("--stdout".to_string());
    Ok(args)
}

fn push_wan_iface(args: &mut Vec<String>, wan: &str) {
    args.push("--wan-iface".to_string());
    args.push(wan.to_string());
}

fn push_static_wan(
    args: &mut Vec<String>,
    wan: &str,
    address: &Ipv4Cidr,
    gateway: std::net::Ipv4Addr,
) {
    push_wan_iface(args, wan);
    args.extend(["--wan-mode".to_string(), "static".to_string()]);
    args.push("--wan-ip".to_string());
    args.push(address.to_string());
    args.push("--wan-gateway".to_string());
    args.push(gateway.to_string());
}

/// 运行目标 release 的 `landscape-webserver config --stdout` 并返回生成的 TOML。
pub(crate) fn generate_init_config_via_cli(
    binary: &Path,
    args: &[String],
) -> Result<String, InstallError> {
    let output = Command::new(binary)
        .arg("config")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| {
            InstallError::ParameterUsage(format!("cannot run {} config: {error}", binary.display()))
        })?;
    if !output.status.success() {
        return Err(InstallError::ParameterUsage(format!(
            "`landscape config` failed with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.trim().is_empty() {
        return Err(InstallError::ParameterUsage(
            "`landscape config` produced no output".to_string(),
        ));
    }
    Ok(stdout.into_owned())
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::network::config::{Ipv4Cidr, SelectedInterface};

    fn credentials() -> Credentials {
        Credentials {
            admin_user: "admin".into(),
            password: "Secret123".into(),
        }
    }

    fn cidr(raw: &str) -> Ipv4Cidr {
        raw.parse().unwrap()
    }

    fn selected(names: &[&str]) -> Vec<SelectedInterface> {
        names
            .iter()
            .map(|name| SelectedInterface {
                name: (*name).to_string(),
                mac: "52:54:00:00:00:01".to_string(),
            })
            .collect()
    }

    #[test]
    fn version_gate_requires_0_25_1() {
        assert!(!config_cli_available(&semver::Version::new(0, 25, 0)));
        assert!(!config_cli_available(&semver::Version::new(0, 24, 9)));
        assert!(config_cli_available(&semver::Version::new(0, 25, 1)));
        assert!(config_cli_available(&semver::Version::new(1, 2, 3)));
        // 预发布版本不算稳定 0.25.1。
        assert!(!config_cli_available(
            &semver::Version::parse("0.25.1-rc.1").unwrap()
        ));
    }

    #[test]
    fn wan_only_maps_to_static_wan_with_management_nat() {
        let plan = NetworkPlan {
            mode: NetworkMode::WanOnly {
                wan: "ens3".into(),
                address: cidr("198.51.100.20/24"),
                gateway: Ipv4Addr::new(198, 51, 100, 1),
            },
            selected_macs: selected(&["ens3"]),
        };
        assert_eq!(
            config_subcommand_args(&credentials(), &plan).unwrap(),
            vec![
                "--admin-user",
                "admin",
                "--admin-pass",
                "Secret123",
                "--enable",
                "firewall",
                "--disable",
                "nat",
                "--wan-iface",
                "ens3",
                "--wan-mode",
                "static",
                "--wan-ip",
                "198.51.100.20/24",
                "--wan-gateway",
                "198.51.100.1",
                "--static-nat",
                "22:22",
                "--static-nat",
                "6443:6443",
                "--stdout",
            ]
        );
    }

    #[test]
    fn wan_dhcp_maps_to_dhcp_wan_without_lan() {
        let plan = NetworkPlan {
            mode: NetworkMode::WanDhcp { wan: "ens3".into() },
            selected_macs: selected(&["ens3"]),
        };
        assert_eq!(
            config_subcommand_args(&credentials(), &plan).unwrap(),
            vec![
                "--admin-user",
                "admin",
                "--admin-pass",
                "Secret123",
                "--enable",
                "firewall",
                "--disable",
                "nat",
                "--wan-iface",
                "ens3",
                "--wan-mode",
                "dhcp",
                "--stdout",
            ]
        );
    }

    #[test]
    fn routed_lan_static_maps_wan_and_lan() {
        let plan = NetworkPlan {
            mode: NetworkMode::RoutedLan {
                wan: "ens3".into(),
                wan_ipv4: Some(WanIpv4Config::Static {
                    address: cidr("198.51.100.20/24"),
                    gateway: Ipv4Addr::new(198, 51, 100, 1),
                }),
                lan: vec!["ens4".into(), "ens5".into()],
                management: cidr("192.168.10.1/24"),
                dhcp_start: "192.168.10.100".parse().unwrap(),
                dhcp_end: "192.168.10.254".parse().unwrap(),
            },
            selected_macs: selected(&["ens3", "ens4", "ens5"]),
        };
        assert_eq!(
            config_subcommand_args(&credentials(), &plan).unwrap(),
            vec![
                "--admin-user",
                "admin",
                "--admin-pass",
                "Secret123",
                "--enable",
                "firewall",
                "--disable",
                "nat",
                "--wan-iface",
                "ens3",
                "--wan-mode",
                "static",
                "--wan-ip",
                "198.51.100.20/24",
                "--wan-gateway",
                "198.51.100.1",
                "--lan-iface",
                "br_lan",
                "--lan-ip",
                "192.168.10.1/24",
                "--lan-member",
                "ens4",
                "--lan-member",
                "ens5",
                "--lan-dhcp-range",
                "192.168.10.100-192.168.10.254",
                "--lan-dhcp-lease",
                "43200",
                "--stdout",
            ]
        );
    }

    #[test]
    fn routed_lan_dhcp_wan_omits_static_flags() {
        let plan = NetworkPlan {
            mode: NetworkMode::RoutedLan {
                wan: "ens3".into(),
                wan_ipv4: Some(WanIpv4Config::Dhcp),
                lan: vec!["ens4".into()],
                management: cidr("192.168.10.1/24"),
                dhcp_start: "192.168.10.100".parse().unwrap(),
                dhcp_end: "192.168.10.254".parse().unwrap(),
            },
            selected_macs: selected(&["ens3", "ens4"]),
        };
        let args = config_subcommand_args(&credentials(), &plan).unwrap();
        assert!(args.contains(&"--wan-mode".to_string()));
        assert!(args.contains(&"dhcp".to_string()));
        assert!(!args.contains(&"--wan-ip".to_string()));
        assert!(!args.contains(&"--static-nat".to_string()));
    }

    #[test]
    fn routed_lan_without_wan_ipv4_mode_is_rejected() {
        let plan = NetworkPlan {
            mode: NetworkMode::RoutedLan {
                wan: "ens3".into(),
                wan_ipv4: None,
                lan: vec!["ens4".into()],
                management: cidr("192.168.10.1/24"),
                dhcp_start: "192.168.10.100".parse().unwrap(),
                dhcp_end: "192.168.10.254".parse().unwrap(),
            },
            selected_macs: selected(&["ens3", "ens4"]),
        };
        assert!(config_subcommand_args(&credentials(), &plan).is_err());
    }

    fn write_script(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "lkit-config-cli-{name}-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn generates_config_via_fake_binary() {
        let dir = temp_dir("fake-binary");
        let script = write_script(
            &dir,
            "landscape-webserver",
            "#!/bin/sh\nprintf 'version = \"1.2.3\"\\n'\n",
        );
        let output = generate_init_config_via_cli(&script, &["--stdout".to_string()]).unwrap();
        assert_eq!(output, "version = \"1.2.3\"\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn failing_binary_maps_to_parameter_usage_error() {
        let dir = temp_dir("failing-binary");
        let script = write_script(
            &dir,
            "landscape-webserver",
            "#!/bin/sh\necho 'boom' >&2\nexit 3\n",
        );
        let error = generate_init_config_via_cli(&script, &[]).unwrap_err();
        assert!(error.to_string().contains("boom"), "{error}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_binary_maps_to_error() {
        let error = generate_init_config_via_cli(
            std::path::Path::new("/nonexistent/lkit-test/landscape-webserver"),
            &[],
        )
        .unwrap_err();
        assert!(error.to_string().contains("cannot run"), "{error}");
    }
}
