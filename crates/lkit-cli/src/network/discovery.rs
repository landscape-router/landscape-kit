use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use super::config::{
    DEFAULT_MANAGEMENT_CIDR, Ipv4Cidr, MANAGEMENT_BRIDGE, NetworkMode, NetworkPlan,
    SelectedInterface,
};
use crate::deployment::plan::InstallError;
use crate::interaction::interactive::Tty;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Interface {
    pub name: String,
    pub mac: String,
    pub operstate: String,
    pub addresses: Vec<Ipv4Cidr>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DefaultRoute {
    pub iface: String,
    pub gateway: Ipv4Addr,
}

#[derive(Debug, Deserialize)]
struct IpAddressRecord {
    ifname: String,
    #[serde(default)]
    addr_info: Vec<IpAddressInfo>,
}

#[derive(Debug, Deserialize)]
struct IpAddressInfo {
    family: String,
    local: String,
    prefixlen: u8,
    #[serde(default)]
    scope: String,
}

#[derive(Debug, Deserialize)]
struct IpRouteRecord {
    #[serde(default)]
    dev: String,
    gateway: Option<String>,
}

pub(crate) fn discover(
    sys_class_net: &Path,
    ip_command: &Path,
) -> Result<(Vec<Interface>, Vec<DefaultRoute>), InstallError> {
    let addresses = read_addresses(ip_command)?;
    let routes = read_default_routes(ip_command)?;
    let entries = std::fs::read_dir(sys_class_net).map_err(InstallError::Io)?;
    let mut interfaces = Vec::new();
    for entry in entries {
        let entry = entry.map_err(InstallError::Io)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        if !is_physical_ethernet(&name, &path)? {
            continue;
        }
        interfaces.push(Interface {
            name: name.clone(),
            mac: read_trimmed(path.join("address"))?,
            operstate: read_trimmed(path.join("operstate")).unwrap_or_else(|_| "unknown".into()),
            addresses: addresses.get(&name).cloned().unwrap_or_default(),
        });
    }
    interfaces.sort_by(|left, right| left.name.cmp(&right.name));
    if interfaces.is_empty() {
        return Err(network_error(
            "no wired physical Ethernet interfaces were found",
        ));
    }
    Ok((interfaces, routes))
}

pub(crate) fn ensure_management_bridge_absent(sys_class_net: &Path) -> Result<(), InstallError> {
    if sys_class_net.join(MANAGEMENT_BRIDGE).exists() {
        return Err(network_error(format!(
            "{MANAGEMENT_BRIDGE} already exists; remove the existing bridge before network takeover"
        )));
    }
    Ok(())
}

pub(crate) fn prompt_plan(
    interfaces: &[Interface],
    routes: &[DefaultRoute],
    ssh_connection: Option<&str>,
    tty: &mut Tty,
) -> Result<NetworkPlan, InstallError> {
    let options: Vec<String> = interfaces.iter().map(format_interface).collect();
    let wan_index = tty.select_one(
        crate::tr!("Select the WAN interface:", "选择 WAN 网卡："),
        &options,
    )?;
    let wan = &interfaces[wan_index];
    let plan = if interfaces.len() == 1 {
        if !tty.confirm(
            crate::tr!("Landscape does not support single-arm WAN/LAN routing. Continue with WAN-only management mode? Type `yes`: ", "Landscape 不支持单臂 WAN/LAN 路由。是否继续使用仅 WAN 管理模式？请输入 `yes`："),
        )? {
            return Err(InstallError::UserRefused(
                "single-interface WAN-only network takeover was not authorized".into(),
            ));
        }
        let endpoint = ssh_server_address(ssh_connection)?;
        let address = select_wan_address(wan, endpoint)?;
        let gateway = routes
            .iter()
            .find(|route| route.iface == wan.name)
            .map(|route| route.gateway)
            .ok_or_else(|| {
                network_error(format!(
                    "{} has no unambiguous IPv4 default gateway",
                    wan.name
                ))
            })?;
        NetworkPlan {
            mode: NetworkMode::WanOnly {
                wan: wan.name.clone(),
                address,
                gateway,
            },
            selected_macs: vec![selected(wan)],
        }
    } else {
        let lan_candidates: Vec<&Interface> = interfaces
            .iter()
            .enumerate()
            .filter_map(|(index, iface)| (index != wan_index).then_some(iface))
            .collect();
        let lan_options: Vec<String> = lan_candidates
            .iter()
            .map(|iface| format_interface(iface))
            .collect();
        let selected_lan = tty.select_many(
            crate::tr!("Select the LAN interfaces:", "选择 LAN 网卡："),
            &lan_options,
        )?;
        let management: Ipv4Cidr = tty
            .input_default(
                crate::tr!("Management IPv4 address", "管理 IPv4 地址"),
                DEFAULT_MANAGEMENT_CIDR,
            )?
            .parse()?;
        let (default_start, default_end) = management.default_pool()?;
        let dhcp_start = tty
            .input_default(
                crate::tr!("LAN DHCP range start", "LAN DHCP 地址池起始地址"),
                &default_start.to_string(),
            )?
            .parse::<Ipv4Addr>()
            .map_err(|_| network_error("invalid LAN DHCP range start"))?;
        let dhcp_end = tty
            .input_default(
                crate::tr!("LAN DHCP range end", "LAN DHCP 地址池结束地址"),
                &default_end.to_string(),
            )?
            .parse::<Ipv4Addr>()
            .map_err(|_| network_error("invalid LAN DHCP range end"))?;
        let lan: Vec<String> = selected_lan
            .iter()
            .map(|index| lan_candidates[*index].name.clone())
            .collect();
        let mut selected_macs = vec![selected(wan)];
        selected_macs.extend(
            selected_lan
                .iter()
                .map(|index| selected(lan_candidates[*index])),
        );
        NetworkPlan {
            mode: NetworkMode::RoutedLan {
                wan: wan.name.clone(),
                lan,
                management,
                dhcp_start,
                dhcp_end,
            },
            selected_macs,
        }
    };
    plan.validate()?;
    Ok(plan)
}

fn read_addresses(ip_command: &Path) -> Result<HashMap<String, Vec<Ipv4Cidr>>, InstallError> {
    let output = run_ip(ip_command, &["-j", "-4", "addr", "show"])?;
    let records: Vec<IpAddressRecord> = serde_json::from_slice(&output)
        .map_err(|error| network_error(format!("cannot parse `ip -j -4 addr show`: {error}")))?;
    let mut addresses = HashMap::new();
    for record in records {
        let values = record
            .addr_info
            .into_iter()
            .filter(|info| info.family == "inet" && info.scope != "host")
            .filter_map(|info| {
                info.local.parse::<Ipv4Addr>().ok().map(|address| Ipv4Cidr {
                    address,
                    prefix: info.prefixlen,
                })
            })
            .collect();
        addresses.insert(record.ifname, values);
    }
    Ok(addresses)
}

fn read_default_routes(ip_command: &Path) -> Result<Vec<DefaultRoute>, InstallError> {
    let output = run_ip(ip_command, &["-j", "-4", "route", "show", "default"])?;
    let records: Vec<IpRouteRecord> = serde_json::from_slice(&output).map_err(|error| {
        network_error(format!(
            "cannot parse `ip -j -4 route show default`: {error}"
        ))
    })?;
    let mut routes = Vec::new();
    for record in records {
        let Some(gateway) = record.gateway.and_then(|value| value.parse().ok()) else {
            continue;
        };
        if !record.dev.is_empty() {
            routes.push(DefaultRoute {
                iface: record.dev,
                gateway,
            });
        }
    }
    Ok(routes)
}

fn run_ip(ip_command: &Path, args: &[&str]) -> Result<Vec<u8>, InstallError> {
    let output = Command::new(ip_command)
        .args(args)
        .output()
        .map_err(InstallError::Io)?;
    if !output.status.success() {
        return Err(network_error(format!(
            "{} {} failed: {}",
            ip_command.display(),
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(output.stdout)
}

fn is_physical_ethernet(name: &str, path: &Path) -> Result<bool, InstallError> {
    if name == "lo" || path.join("wireless").exists() || path.join("phy80211").exists() {
        return Ok(false);
    }
    let iface_type = read_trimmed(path.join("type"))?;
    if iface_type != "1" {
        return Ok(false);
    }
    let canonical = path.canonicalize().map_err(InstallError::Io)?;
    Ok(!canonical
        .to_string_lossy()
        .contains("/devices/virtual/net/"))
}

fn read_trimmed(path: PathBuf) -> Result<String, InstallError> {
    Ok(std::fs::read_to_string(path)
        .map_err(InstallError::Io)?
        .trim()
        .to_string())
}

pub(crate) fn ssh_server_address(value: Option<&str>) -> Result<Option<Ipv4Addr>, InstallError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let fields: Vec<&str> = value.split_whitespace().collect();
    if fields.len() != 4 {
        return Err(network_error("SSH_CONNECTION has an invalid shape"));
    }
    let server = fields[2]
        .parse::<std::net::IpAddr>()
        .map_err(|_| network_error("SSH_CONNECTION contains an invalid server address"))?;
    Ok(match server {
        std::net::IpAddr::V4(address) => Some(address),
        std::net::IpAddr::V6(_) => {
            return Err(network_error(
                "single-interface network takeover requires an IPv4 SSH connection",
            ));
        }
    })
}

pub(crate) fn verify_live(plan: &NetworkPlan, ip_command: &Path) -> Result<(), InstallError> {
    let (iface, expected) = match &plan.mode {
        NetworkMode::WanOnly { wan, address, .. } => (wan.as_str(), *address),
        NetworkMode::RoutedLan { management, .. } => (MANAGEMENT_BRIDGE, *management),
    };
    let output = run_ip(ip_command, &["-j", "-4", "addr", "show", "dev", iface])?;
    let records: Vec<IpAddressRecord> = serde_json::from_slice(&output).map_err(|error| {
        network_error(format!(
            "cannot parse live address state for {iface}: {error}"
        ))
    })?;
    let present = records
        .iter()
        .flat_map(|record| &record.addr_info)
        .any(|info| {
            info.local.parse::<Ipv4Addr>().ok() == Some(expected.address)
                && info.prefixlen == expected.prefix
        });
    if !present {
        return Err(network_error(format!(
            "{iface} does not have expected address {expected}"
        )));
    }
    if let NetworkMode::RoutedLan { lan, .. } = &plan.mode {
        let output = run_ip(
            ip_command,
            &["-j", "link", "show", "master", MANAGEMENT_BRIDGE],
        )?;
        let records: Vec<serde_json::Value> = serde_json::from_slice(&output).map_err(|error| {
            network_error(format!(
                "cannot parse {MANAGEMENT_BRIDGE} member state: {error}"
            ))
        })?;
        let members: Vec<&str> = records
            .iter()
            .filter_map(|record| record.get("ifname").and_then(|value| value.as_str()))
            .collect();
        for expected in lan {
            if !members.contains(&expected.as_str()) {
                return Err(network_error(format!(
                    "{expected} is not attached to {MANAGEMENT_BRIDGE}"
                )));
            }
        }
    }
    Ok(())
}

fn select_wan_address(
    interface: &Interface,
    ssh_server: Option<Ipv4Addr>,
) -> Result<Ipv4Cidr, InstallError> {
    if let Some(server) = ssh_server {
        return interface
            .addresses
            .iter()
            .copied()
            .find(|cidr| cidr.address == server)
            .ok_or_else(|| {
                network_error(format!(
                    "the SSH server address {server} does not belong to selected WAN {}",
                    interface.name
                ))
            });
    }
    match interface.addresses.as_slice() {
        [address] => Ok(*address),
        _ => Err(network_error(format!(
            "{} must have exactly one usable IPv4 address for local WAN-only takeover",
            interface.name
        ))),
    }
}

fn format_interface(interface: &Interface) -> String {
    let addresses = if interface.addresses.is_empty() {
        "no IPv4".to_string()
    } else {
        interface
            .addresses
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        "{}  MAC {}  link {}  {}",
        interface.name, interface.mac, interface.operstate, addresses
    )
}

fn selected(interface: &Interface) -> SelectedInterface {
    SelectedInterface {
        name: interface.name.clone(),
        mac: interface.mac.clone(),
    }
}

fn network_error(reason: impl Into<String>) -> InstallError {
    InstallError::Preflight(format!("network takeover: {}", reason.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_ssh_endpoint_only_from_selected_wan() {
        let interface = Interface {
            name: "ens3".into(),
            mac: "02:00:00:00:00:03".into(),
            operstate: "up".into(),
            addresses: vec![
                "198.51.100.10/24".parse().unwrap(),
                "198.51.100.11/24".parse().unwrap(),
            ],
        };
        assert_eq!(
            select_wan_address(&interface, Some(Ipv4Addr::new(198, 51, 100, 11)))
                .unwrap()
                .address,
            Ipv4Addr::new(198, 51, 100, 11)
        );
        assert!(select_wan_address(&interface, None).is_err());
    }

    #[test]
    fn parses_ipv4_ssh_connection() {
        assert_eq!(
            ssh_server_address(Some("203.0.113.5 51111 198.51.100.10 22")).unwrap(),
            Some(Ipv4Addr::new(198, 51, 100, 10))
        );
        assert!(ssh_server_address(Some("bad input")).is_err());
    }

    #[test]
    fn refuses_an_existing_management_bridge() {
        let root =
            std::env::temp_dir().join(format!("lkit-existing-bridge-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(MANAGEMENT_BRIDGE)).unwrap();
        assert!(ensure_management_bridge_absent(&root).is_err());
        let _ = std::fs::remove_dir_all(root);
    }
}
