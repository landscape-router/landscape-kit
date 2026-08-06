use std::collections::HashSet;
use std::net::Ipv4Addr;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::deployment::plan::InstallError;

pub(crate) const MANAGEMENT_BRIDGE: &str = "br_lan";
pub(crate) const DEFAULT_MANAGEMENT_CIDR: &str = "192.168.10.1/24";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Ipv4Cidr {
    pub address: Ipv4Addr,
    pub prefix: u8,
}

impl Ipv4Cidr {
    pub(crate) fn network(self) -> u32 {
        u32::from(self.address) & prefix_mask(self.prefix)
    }

    pub(crate) fn broadcast(self) -> u32 {
        self.network() | !prefix_mask(self.prefix)
    }

    pub(crate) fn contains_usable(self, address: Ipv4Addr) -> bool {
        let value = u32::from(address);
        value > self.network() && value < self.broadcast()
    }

    pub(crate) fn default_pool(self) -> Result<(Ipv4Addr, Ipv4Addr), InstallError> {
        let first = self.network() + 1;
        let last = self.broadcast().saturating_sub(1);
        let preferred = self.network().saturating_add(100);
        let start = if preferred <= last {
            preferred.max(first)
        } else {
            first
        };
        let server = u32::from(self.address);
        if !(start..=last).contains(&server) {
            return Ok((Ipv4Addr::from(start), Ipv4Addr::from(last)));
        }
        let left_size = server.saturating_sub(start);
        let right_size = last.saturating_sub(server);
        if right_size >= left_size && right_size > 0 {
            Ok((Ipv4Addr::from(server + 1), Ipv4Addr::from(last)))
        } else if left_size > 0 {
            Ok((Ipv4Addr::from(start), Ipv4Addr::from(server - 1)))
        } else {
            Err(network_error(
                "management subnet has no DHCP client address",
            ))
        }
    }
}

impl std::fmt::Display for Ipv4Cidr {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}/{}", self.address, self.prefix)
    }
}

impl FromStr for Ipv4Cidr {
    type Err = InstallError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (address, prefix) = value
            .trim()
            .split_once('/')
            .ok_or_else(|| network_error("IPv4 address must use address/prefix notation"))?;
        let address = address
            .parse::<Ipv4Addr>()
            .map_err(|_| network_error("invalid IPv4 address"))?;
        let prefix = prefix
            .parse::<u8>()
            .map_err(|_| network_error("invalid IPv4 prefix"))?;
        if !(1..=30).contains(&prefix) {
            return Err(network_error("IPv4 prefix must be between 1 and 30"));
        }
        let cidr = Self { address, prefix };
        if !cidr.contains_usable(address) {
            return Err(network_error(
                "IPv4 address must be a usable host address in its subnet",
            ));
        }
        Ok(cidr)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "mode")]
pub(crate) enum NetworkMode {
    WanOnly {
        wan: String,
        address: Ipv4Cidr,
        gateway: Ipv4Addr,
    },
    WanDhcp {
        wan: String,
    },
    RoutedLan {
        wan: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wan_ipv4: Option<WanIpv4Config>,
        lan: Vec<String>,
        management: Ipv4Cidr,
        dhcp_start: Ipv4Addr,
        dhcp_end: Ipv4Addr,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "mode")]
pub(crate) enum WanIpv4Config {
    Static {
        address: Ipv4Cidr,
        gateway: Ipv4Addr,
    },
    Dhcp,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct NetworkPlan {
    pub mode: NetworkMode,
    pub selected_macs: Vec<SelectedInterface>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct SelectedInterface {
    pub name: String,
    pub mac: String,
}

impl NetworkPlan {
    pub(crate) fn wan(&self) -> &str {
        match &self.mode {
            NetworkMode::WanOnly { wan, .. }
            | NetworkMode::WanDhcp { wan }
            | NetworkMode::RoutedLan { wan, .. } => wan,
        }
    }

    pub(crate) fn management_address(&self) -> Option<Ipv4Cidr> {
        match self.mode {
            NetworkMode::WanOnly { address, .. } => Some(address),
            NetworkMode::WanDhcp { .. } => None,
            NetworkMode::RoutedLan { management, .. } => Some(management),
        }
    }

    pub(crate) fn validate(&self) -> Result<(), InstallError> {
        validate_iface_name(self.wan())?;
        let mut expected_interfaces = vec![self.wan().as_bytes()];
        match &self.mode {
            NetworkMode::WanOnly {
                address, gateway, ..
            } => {
                validate_static_wan(*address, *gateway)?;
            }
            NetworkMode::WanDhcp { .. } => {}
            NetworkMode::RoutedLan {
                wan,
                wan_ipv4,
                lan,
                management,
                dhcp_start,
                dhcp_end,
            } => {
                if let Some(WanIpv4Config::Static { address, gateway }) = wan_ipv4 {
                    validate_static_wan(*address, *gateway)?;
                }
                if lan.is_empty() {
                    return Err(network_error("at least one LAN interface is required"));
                }
                let mut unique = HashSet::new();
                for iface in lan {
                    validate_iface_name(iface)?;
                    if iface == wan {
                        return Err(network_error("WAN and LAN interfaces must not overlap"));
                    }
                    if !unique.insert(iface) {
                        return Err(network_error("LAN interfaces must not contain duplicates"));
                    }
                    expected_interfaces.push(iface.as_bytes());
                }
                if !management.contains_usable(*dhcp_start)
                    || !management.contains_usable(*dhcp_end)
                {
                    return Err(network_error(
                        "DHCP range must contain usable addresses in the management subnet",
                    ));
                }
                if u32::from(*dhcp_start) > u32::from(*dhcp_end) {
                    return Err(network_error("DHCP range start must not exceed its end"));
                }
                let server = u32::from(management.address);
                if (u32::from(*dhcp_start)..=u32::from(*dhcp_end)).contains(&server) {
                    return Err(network_error(
                        "DHCP range must not contain the management address",
                    ));
                }
            }
        }
        if self.selected_macs.len() != expected_interfaces.len() {
            return Err(network_error(
                "selected interface MAC records must exactly match the network interfaces",
            ));
        }
        let mut selected_names = HashSet::new();
        for selected in &self.selected_macs {
            validate_iface_name(&selected.name)?;
            if !expected_interfaces
                .iter()
                .any(|expected| *expected == selected.name.as_bytes())
                || !selected_names.insert(selected.name.as_str())
            {
                return Err(network_error(
                    "selected interface MAC records must exactly match the network interfaces",
                ));
            }
            validate_mac(&selected.mac)?;
        }
        Ok(())
    }
}

#[derive(Serialize)]
pub(crate) struct LandscapeInit<'a> {
    version: String,
    config: LandscapeConfig<'a>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    ifaces: Vec<IfaceConfig<'a>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    ipconfigs: Vec<IpService<'a>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    firewalls: Vec<InterfaceService<'a>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    dhcpv4_services: Vec<DhcpService<'a>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    route_lans: Vec<LanRoute<'a>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    route_wans: Vec<InterfaceService<'a>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    static_nat_mappings_v4: Vec<StaticNatV4<'a>>,
}

#[derive(Serialize)]
struct LandscapeConfig<'a> {
    auth: Auth<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    web: Option<WebConfig>,
}

#[derive(Serialize)]
struct Auth<'a> {
    admin_user: &'a str,
    admin_pass: &'a str,
}

#[derive(Serialize)]
struct WebConfig {}

#[derive(Serialize)]
struct IfaceConfig<'a> {
    name: &'a str,
    create_dev_type: &'static str,
    controller_name: Option<&'a str>,
    zone_type: &'static str,
    enable_in_boot: bool,
    wifi_mode: &'static str,
    update_at: f64,
}

#[derive(Serialize)]
struct IpService<'a> {
    iface_name: &'a str,
    enable: bool,
    ip_model: StaticIpModel,
    update_at: f64,
}

#[derive(Serialize)]
#[serde(tag = "t", rename_all = "lowercase")]
enum StaticIpModel {
    Static {
        default_router_ip: Ipv4Addr,
        default_router: bool,
        ipv4: Ipv4Addr,
        ipv4_mask: u8,
        ipv6: Option<String>,
    },
    #[serde(rename = "dhcpclient")]
    DhcpClient {
        default_router: bool,
        hostname: Option<String>,
        custome_opts: Vec<serde_json::Value>,
    },
}

#[derive(Serialize)]
struct InterfaceService<'a> {
    iface_name: &'a str,
    enable: bool,
    update_at: f64,
}

#[derive(Serialize)]
struct LanRoute<'a> {
    iface_name: &'a str,
    enable: bool,
    static_routes: Option<Vec<String>>,
    update_at: f64,
}

#[derive(Serialize)]
struct DhcpService<'a> {
    iface_name: &'a str,
    enable: bool,
    config: DhcpConfig,
    update_at: f64,
}

#[derive(Serialize)]
struct DhcpConfig {
    ip_range_start: Ipv4Addr,
    ip_range_end: Ipv4Addr,
    server_ip_addr: Ipv4Addr,
    network_mask: u8,
    address_lease_time: u32,
    custom_options: Vec<String>,
}

#[derive(Serialize)]
struct StaticNatV4<'a> {
    id: String,
    enable: bool,
    remark: &'static str,
    wan_iface_name: &'a str,
    mapping_pair_ports: Vec<PortPair>,
    lan_target: StaticNatTarget,
    l4_protocols: Vec<u8>,
    update_at: f64,
}

#[derive(Serialize)]
struct PortPair {
    wan_port: u16,
    lan_port: u16,
}

#[derive(Serialize)]
#[serde(tag = "t", rename_all = "snake_case")]
enum StaticNatTarget {
    Local,
}

impl<'a> LandscapeInit<'a> {
    pub(crate) fn new(
        version: &semver::Version,
        admin_user: &'a str,
        admin_pass: &'a str,
        network: &'a NetworkPlan,
    ) -> Result<Self, InstallError> {
        network.validate()?;
        let mut ifaces = Vec::new();
        let mut ipconfigs = Vec::new();
        let mut dhcpv4_services = Vec::new();
        let mut route_lans = Vec::new();
        let mut static_nat_mappings_v4 = Vec::new();
        match &network.mode {
            NetworkMode::WanOnly {
                wan,
                address,
                gateway,
            } => {
                ifaces.push(physical_iface(wan, None, "wan"));
                ipconfigs.push(IpService {
                    iface_name: wan,
                    enable: true,
                    ip_model: StaticIpModel::Static {
                        default_router_ip: *gateway,
                        default_router: true,
                        ipv4: address.address,
                        ipv4_mask: address.prefix,
                        ipv6: None,
                    },
                    update_at: 0.0,
                });
                static_nat_mappings_v4.push(StaticNatV4 {
                    id: Uuid::now_v7().to_string(),
                    enable: true,
                    remark: "lkit WAN management",
                    wan_iface_name: wan,
                    mapping_pair_ports: vec![
                        PortPair {
                            wan_port: 22,
                            lan_port: 22,
                        },
                        PortPair {
                            wan_port: 6443,
                            lan_port: 6443,
                        },
                    ],
                    lan_target: StaticNatTarget::Local,
                    l4_protocols: vec![6],
                    update_at: 0.0,
                });
            }
            NetworkMode::WanDhcp { wan } => {
                ifaces.push(physical_iface(wan, None, "wan"));
                ipconfigs.push(dhcp_client(wan));
            }
            NetworkMode::RoutedLan {
                wan,
                wan_ipv4,
                lan,
                management,
                dhcp_start,
                dhcp_end,
            } => {
                ifaces.push(physical_iface(wan, None, "wan"));
                match wan_ipv4 {
                    Some(WanIpv4Config::Static { address, gateway }) => {
                        ipconfigs.push(static_ip(wan, *address, *gateway));
                    }
                    Some(WanIpv4Config::Dhcp) => ipconfigs.push(dhcp_client(wan)),
                    None => {}
                }
                ifaces.push(IfaceConfig {
                    name: MANAGEMENT_BRIDGE,
                    create_dev_type: "bridge",
                    controller_name: None,
                    zone_type: "lan",
                    enable_in_boot: true,
                    wifi_mode: "undefined",
                    update_at: 0.0,
                });
                for iface in lan {
                    ifaces.push(physical_iface(iface, Some(MANAGEMENT_BRIDGE), "lan"));
                }
                dhcpv4_services.push(DhcpService {
                    iface_name: MANAGEMENT_BRIDGE,
                    enable: true,
                    config: DhcpConfig {
                        ip_range_start: *dhcp_start,
                        ip_range_end: *dhcp_end,
                        server_ip_addr: management.address,
                        network_mask: management.prefix,
                        address_lease_time: 43_200,
                        custom_options: Vec::new(),
                    },
                    update_at: 0.0,
                });
                route_lans.push(LanRoute {
                    iface_name: MANAGEMENT_BRIDGE,
                    enable: true,
                    static_routes: None,
                    update_at: 0.0,
                });
            }
        }
        Ok(Self {
            version: version.to_string(),
            config: LandscapeConfig {
                auth: Auth {
                    admin_user,
                    admin_pass,
                },
                web: None,
            },
            ifaces,
            ipconfigs,
            firewalls: vec![InterfaceService {
                iface_name: network.wan(),
                enable: true,
                update_at: 0.0,
            }],
            dhcpv4_services,
            route_lans,
            route_wans: vec![InterfaceService {
                iface_name: network.wan(),
                enable: true,
                update_at: 0.0,
            }],
            static_nat_mappings_v4,
        })
    }
}

fn static_ip(iface_name: &str, address: Ipv4Cidr, gateway: Ipv4Addr) -> IpService<'_> {
    IpService {
        iface_name,
        enable: true,
        ip_model: StaticIpModel::Static {
            default_router_ip: gateway,
            default_router: true,
            ipv4: address.address,
            ipv4_mask: address.prefix,
            ipv6: None,
        },
        update_at: 0.0,
    }
}

fn dhcp_client(iface_name: &str) -> IpService<'_> {
    IpService {
        iface_name,
        enable: true,
        ip_model: StaticIpModel::DhcpClient {
            default_router: true,
            hostname: None,
            custome_opts: Vec::new(),
        },
        update_at: 0.0,
    }
}

fn physical_iface<'a>(
    name: &'a str,
    controller_name: Option<&'a str>,
    zone_type: &'static str,
) -> IfaceConfig<'a> {
    IfaceConfig {
        name,
        create_dev_type: "no_need_to_create",
        controller_name,
        zone_type,
        enable_in_boot: true,
        wifi_mode: "undefined",
        update_at: 0.0,
    }
}

fn validate_iface_name(value: &str) -> Result<(), InstallError> {
    if value.is_empty()
        || value.len() > 15
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err(network_error(format!("invalid interface name {value:?}")));
    }
    Ok(())
}

fn validate_mac(value: &str) -> Result<(), InstallError> {
    let parts: Vec<&str> = value.split(':').collect();
    if parts.len() != 6
        || parts
            .iter()
            .any(|part| part.len() != 2 || !part.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err(network_error(format!(
            "invalid interface MAC address {value:?}"
        )));
    }
    Ok(())
}

fn validate_static_wan(address: Ipv4Cidr, gateway: Ipv4Addr) -> Result<(), InstallError> {
    if !address.contains_usable(gateway) {
        return Err(network_error("default gateway must be in the WAN subnet"));
    }
    if address.address == gateway {
        return Err(network_error("WAN address and gateway must differ"));
    }
    Ok(())
}

fn prefix_mask(prefix: u8) -> u32 {
    u32::MAX << (32 - prefix)
}

fn network_error(reason: impl Into<String>) -> InstallError {
    InstallError::ParameterUsage(format!(
        "invalid network takeover configuration: {}",
        reason.into()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_ipv4_and_default_pool() {
        let cidr: Ipv4Cidr = "192.168.10.1/24".parse().unwrap();
        assert_eq!(
            cidr.default_pool().unwrap(),
            (
                Ipv4Addr::new(192, 168, 10, 100),
                Ipv4Addr::new(192, 168, 10, 254)
            )
        );
        assert!("192.168.10.0/24".parse::<Ipv4Cidr>().is_err());
        assert!("192.168.10.1/31".parse::<Ipv4Cidr>().is_err());
        let middle: Ipv4Cidr = "192.168.10.150/24".parse().unwrap();
        assert_eq!(
            middle.default_pool().unwrap(),
            (
                Ipv4Addr::new(192, 168, 10, 151),
                Ipv4Addr::new(192, 168, 10, 254)
            )
        );
        let last: Ipv4Cidr = "192.168.10.254/24".parse().unwrap();
        assert_eq!(
            last.default_pool().unwrap(),
            (
                Ipv4Addr::new(192, 168, 10, 100),
                Ipv4Addr::new(192, 168, 10, 253)
            )
        );
        let small: Ipv4Cidr = "10.0.0.2/30".parse().unwrap();
        assert_eq!(
            small.default_pool().unwrap(),
            (Ipv4Addr::new(10, 0, 0, 1), Ipv4Addr::new(10, 0, 0, 1))
        );
    }

    #[test]
    fn renders_single_interface_landscape_config() {
        let plan = NetworkPlan {
            mode: NetworkMode::WanOnly {
                wan: "ens3".into(),
                address: "198.51.100.10/24".parse().unwrap(),
                gateway: Ipv4Addr::new(198, 51, 100, 1),
            },
            selected_macs: vec![SelectedInterface {
                name: "ens3".into(),
                mac: "02:00:00:00:00:03".into(),
            }],
        };
        let config =
            LandscapeInit::new(&semver::Version::new(1, 2, 3), "admin", "Secret123", &plan)
                .unwrap();
        let value = toml::Value::try_from(config).unwrap();
        assert_eq!(value["ipconfigs"][0]["iface_name"].as_str(), Some("ens3"));
        assert_eq!(value["route_wans"][0]["iface_name"].as_str(), Some("ens3"));
        assert_eq!(value["firewalls"][0]["iface_name"].as_str(), Some("ens3"));
        assert_eq!(
            value["static_nat_mappings_v4"][0]["mapping_pair_ports"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            value["static_nat_mappings_v4"][0]["mapping_pair_ports"][0]["wan_port"].as_integer(),
            Some(22)
        );
        assert_eq!(
            value["static_nat_mappings_v4"][0]["mapping_pair_ports"][1]["wan_port"].as_integer(),
            Some(6443)
        );
        assert_eq!(
            value["static_nat_mappings_v4"][0]["lan_target"]["t"].as_str(),
            Some("local")
        );
        assert_eq!(
            value["static_nat_mappings_v4"][0]["l4_protocols"][0].as_integer(),
            Some(6)
        );
        assert!(value.get("dhcpv4_services").is_none());
    }

    #[test]
    fn renders_multi_interface_without_wan_ip_or_nat() {
        let plan = NetworkPlan {
            mode: NetworkMode::RoutedLan {
                wan: "ens3".into(),
                wan_ipv4: None,
                lan: vec!["ens4".into(), "ens5".into()],
                management: DEFAULT_MANAGEMENT_CIDR.parse().unwrap(),
                dhcp_start: Ipv4Addr::new(192, 168, 10, 100),
                dhcp_end: Ipv4Addr::new(192, 168, 10, 254),
            },
            selected_macs: vec![
                SelectedInterface {
                    name: "ens3".into(),
                    mac: "02:00:00:00:00:03".into(),
                },
                SelectedInterface {
                    name: "ens4".into(),
                    mac: "02:00:00:00:00:04".into(),
                },
                SelectedInterface {
                    name: "ens5".into(),
                    mac: "02:00:00:00:00:05".into(),
                },
            ],
        };
        let config =
            LandscapeInit::new(&semver::Version::new(1, 2, 3), "admin", "Secret123", &plan)
                .unwrap();
        let value = toml::Value::try_from(config).unwrap();
        assert!(value.get("ipconfigs").is_none());
        assert!(value.get("static_nat_mappings_v4").is_none());
        assert_eq!(value["ifaces"].as_array().unwrap().len(), 4);
        assert_eq!(
            value["dhcpv4_services"][0]["config"]["server_ip_addr"].as_str(),
            Some("192.168.10.1")
        );
    }

    #[test]
    fn renders_dhcp_wan_with_upstream_model_tag() {
        let plan = NetworkPlan {
            mode: NetworkMode::WanDhcp { wan: "ens3".into() },
            selected_macs: vec![SelectedInterface {
                name: "ens3".into(),
                mac: "02:00:00:00:00:03".into(),
            }],
        };
        let config =
            LandscapeInit::new(&semver::Version::new(1, 2, 3), "admin", "Secret123", &plan)
                .unwrap();
        let value = toml::Value::try_from(config).unwrap();
        assert_eq!(
            value["ipconfigs"][0]["ip_model"]["t"].as_str(),
            Some("dhcpclient")
        );
        assert!(value.get("dhcpv4_services").is_none());
    }

    #[test]
    fn rejects_duplicate_lan_interfaces() {
        let plan = NetworkPlan {
            mode: NetworkMode::RoutedLan {
                wan: "ens3".into(),
                wan_ipv4: None,
                lan: vec!["ens4".into(), "ens4".into()],
                management: DEFAULT_MANAGEMENT_CIDR.parse().unwrap(),
                dhcp_start: Ipv4Addr::new(192, 168, 10, 100),
                dhcp_end: Ipv4Addr::new(192, 168, 10, 254),
            },
            selected_macs: vec![
                SelectedInterface {
                    name: "ens3".into(),
                    mac: "02:00:00:00:00:03".into(),
                },
                SelectedInterface {
                    name: "ens4".into(),
                    mac: "02:00:00:00:00:04".into(),
                },
                SelectedInterface {
                    name: "ens4".into(),
                    mac: "02:00:00:00:00:04".into(),
                },
            ],
        };
        assert!(plan.validate().is_err());
    }
}
