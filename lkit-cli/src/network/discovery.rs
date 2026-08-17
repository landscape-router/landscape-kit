use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use super::config::{
    DEFAULT_MANAGEMENT_CIDR, Ipv4Cidr, MANAGEMENT_BRIDGE, NetworkMode, NetworkPlan,
    SelectedInterface, WanIpv4Config,
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

pub(crate) fn prompt_plan(
    interfaces: &[Interface],
    routes: &[DefaultRoute],
    tty: &mut Tty,
) -> Result<NetworkPlan, InstallError> {
    let options: Vec<String> = interfaces.iter().map(format_interface).collect();
    let wan_index = tty.select_one(
        &crate::tr!(crate::keys::DISCOVERY_SELECT_WAN_INTERFACE),
        &options,
        None,
    )?;
    let wan = &interfaces[wan_index];
    let plan = if interfaces.len() == 1 {
        if !tty.confirm(&crate::tr!(crate::keys::DISCOVERY_SINGLE_ARM_CONFIRM))? {
            return Err(InstallError::UserRefused(
                "single-interface WAN-only network takeover was not authorized".into(),
            ));
        }
        wan_only_plan(wan, routes)
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
            &crate::tr!(crate::keys::DISCOVERY_SELECT_LAN_INTERFACES),
            &lan_options,
        )?;
        if selected_lan.is_empty() {
            wan_only_plan(wan, routes)
        } else {
            let management: Ipv4Cidr = tty
                .input_default(
                    &crate::tr!(crate::keys::DISCOVERY_MANAGEMENT_IPV4_ADDRESS),
                    DEFAULT_MANAGEMENT_CIDR,
                )?
                .parse()?;
            let (default_start, default_end) = management.default_pool()?;
            let dhcp_start = tty
                .input_default(
                    &crate::tr!(crate::keys::DISCOVERY_LAN_DHCP_RANGE_START),
                    &default_start.to_string(),
                )?
                .parse::<Ipv4Addr>()
                .map_err(|_| network_error("invalid LAN DHCP range start"))?;
            let dhcp_end = tty
                .input_default(
                    &crate::tr!(crate::keys::DISCOVERY_LAN_DHCP_RANGE_END),
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
                    wan_ipv4: Some(discovered_wan_ipv4(wan, routes)),
                    lan,
                    management,
                    dhcp_start,
                    dhcp_end,
                },
                selected_macs,
            }
        }
    };
    plan.validate()?;
    Ok(plan)
}

fn wan_only_plan(wan: &Interface, routes: &[DefaultRoute]) -> NetworkPlan {
    let mode = match discovered_static(wan, routes) {
        Some((address, gateway)) => NetworkMode::WanOnly {
            wan: wan.name.clone(),
            address,
            gateway,
        },
        None => NetworkMode::WanDhcp {
            wan: wan.name.clone(),
        },
    };
    NetworkPlan {
        mode,
        selected_macs: vec![selected(wan)],
    }
}

fn discovered_wan_ipv4(wan: &Interface, routes: &[DefaultRoute]) -> WanIpv4Config {
    match discovered_static(wan, routes) {
        Some((address, gateway)) => WanIpv4Config::Static { address, gateway },
        None => WanIpv4Config::Dhcp,
    }
}

/// 按发现顺序取该接口的第一个 IPv4 和该接口的第一个默认网关；
/// 两项都存在时才返回完整静态对，供 CLI 与控制台向导共用。
pub(crate) fn discovered_static(
    wan: &Interface,
    routes: &[DefaultRoute],
) -> Option<(Ipv4Cidr, Ipv4Addr)> {
    let address = wan.addresses.first().copied()?;
    let gateway = routes
        .iter()
        .find(|route| route.iface == wan.name)
        .map(|route| route.gateway)?;
    Some((address, gateway))
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

pub(crate) fn is_physical_ethernet(name: &str, path: &Path) -> Result<bool, InstallError> {
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

pub(crate) fn verify_live(plan: &NetworkPlan, ip_command: &Path) -> Result<(), InstallError> {
    let (iface, expected) = match &plan.mode {
        NetworkMode::WanOnly { wan, address, .. } => (wan.as_str(), Some(*address)),
        NetworkMode::WanDhcp { wan } => (wan.as_str(), None),
        NetworkMode::RoutedLan { management, .. } => (MANAGEMENT_BRIDGE, Some(*management)),
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
            expected.is_none_or(|expected| {
                info.local.parse::<Ipv4Addr>().ok() == Some(expected.address)
                    && info.prefixlen == expected.prefix
            })
        });
    if !present {
        let expected = expected
            .map(|expected| expected.to_string())
            .unwrap_or_else(|| "an IPv4 DHCP lease".into());
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
    fn builds_wan_only_plan_for_a_multi_interface_host() {
        let wan = Interface {
            name: "ens3".into(),
            mac: "02:00:00:00:00:03".into(),
            operstate: "up".into(),
            addresses: vec!["198.51.100.20/24".parse().unwrap()],
        };
        let plan = wan_only_plan(
            &wan,
            &[DefaultRoute {
                iface: "ens3".into(),
                gateway: "198.51.100.1".parse().unwrap(),
            }],
        );
        assert!(matches!(plan.mode, NetworkMode::WanOnly { .. }));
        assert_eq!(plan.selected_macs.len(), 1);
        assert!(plan.validate().is_ok());
    }

    #[test]
    fn wan_only_uses_first_existing_static_address_or_dhcp() {
        let mut wan = Interface {
            name: "ens3".into(),
            mac: "02:00:00:00:00:03".into(),
            operstate: "up".into(),
            addresses: vec![
                "198.51.100.20/24".parse().unwrap(),
                "198.51.100.21/24".parse().unwrap(),
            ],
        };
        let routes = [DefaultRoute {
            iface: "ens3".into(),
            gateway: "198.51.100.1".parse().unwrap(),
        }];
        let plan = wan_only_plan(&wan, &routes);
        assert!(matches!(
            plan.mode,
            NetworkMode::WanOnly { address, .. }
                if address.address == Ipv4Addr::new(198, 51, 100, 20)
        ));

        wan.addresses.clear();
        assert!(matches!(
            wan_only_plan(&wan, &routes).mode,
            NetworkMode::WanDhcp { .. }
        ));
    }

    #[test]
    fn discovered_static_requires_address_and_gateway_of_the_same_interface() {
        let wan = Interface {
            name: "ens3".into(),
            mac: "02:00:00:00:00:03".into(),
            operstate: "up".into(),
            addresses: vec!["198.51.100.20/24".parse().unwrap()],
        };
        let routes = [DefaultRoute {
            iface: "ens3".into(),
            gateway: "198.51.100.1".parse().unwrap(),
        }];
        assert_eq!(
            discovered_static(&wan, &routes),
            Some((
                "198.51.100.20/24".parse().unwrap(),
                Ipv4Addr::new(198, 51, 100, 1)
            ))
        );
        let other_route = [DefaultRoute {
            iface: "ens4".into(),
            gateway: "198.51.100.1".parse().unwrap(),
        }];
        assert_eq!(discovered_static(&wan, &other_route), None);
        let mut addressless = wan.clone();
        addressless.addresses.clear();
        assert_eq!(discovered_static(&addressless, &routes), None);
    }
}
