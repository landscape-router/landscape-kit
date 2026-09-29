//! Fixture 实现的 `landscape-webserver config` 子命令。
//!
//! 镜像真实子命令(lkit 依赖的 flag 子集)把部署参数翻译成 `landscape_init.toml`。
//! 序列化结构与 landscape 的 `InitConfig` 保持一致:实体数组为空时整个键省略,
//! `Option` 字段为 `None` 时省略。版本号由调用方传入——真实二进制内嵌自身版本,
//! fixture 从 release 目录名推导(见 bin 侧 `release_version_from_exe`)。

use std::net::Ipv4Addr;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Args, ValueEnum};
use serde::Serialize;

/// 与真实子命令一致的基础服务集。
const BASE_ENABLED_SERVICES: [&str; 3] = ["nat", "route-wan", "route-lan"];
const KNOWN_SERVICES: [&str; 5] = ["nat", "firewall", "mss-clamp", "route-wan", "route-lan"];
const WAN_SERVICES: [&str; 4] = ["nat", "firewall", "mss-clamp", "route-wan"];
const LAN_SERVICES: [&str; 1] = ["route-lan"];
const TCP_L4_PROTOCOL: u8 = 6;
/// 与真实子命令相同的默认 LAN 网段。
const DEFAULT_LAN_IP: &str = "192.168.5.1/24";

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum WanMode {
    Dhcp,
    Static,
    /// 注册 WAN 接口但不配置地址。
    None,
}

#[derive(Debug, Args)]
pub struct ConfigCliArgs {
    /// Print the generated TOML to stdout instead of writing a file
    #[arg(long)]
    pub stdout: bool,

    /// Write landscape_init.toml into this directory
    #[arg(long, value_name = "DIR")]
    pub dir: Option<PathBuf>,

    /// Overwrite an existing landscape_init.toml
    #[arg(short, long)]
    pub force: bool,

    /// Admin username written to [config.auth]
    #[arg(long, value_name = "USER")]
    pub admin_user: Option<String>,

    /// Admin password written to [config.auth]
    #[arg(long, value_name = "PASS")]
    pub admin_pass: Option<String>,

    /// WAN physical interface name
    #[arg(long, value_name = "NAME")]
    pub wan_iface: Option<String>,

    /// WAN address acquisition mode
    #[arg(long, value_enum, default_value_t = WanMode::Dhcp)]
    pub wan_mode: WanMode,

    /// Static WAN address, e.g. 203.0.113.2/24
    #[arg(long, value_name = "CIDR")]
    pub wan_ip: Option<String>,

    /// Static WAN default gateway
    #[arg(long, value_name = "IP")]
    pub wan_gateway: Option<Ipv4Addr>,

    /// Static NAT port mapping (TCP only, repeatable): <wan_port>:<lan_port>
    #[arg(long = "static-nat", value_name = "WAN_PORT:LAN_PORT")]
    pub static_nat: Vec<String>,

    /// LAN bridge interface name (omit for a WAN-only deployment)
    #[arg(long, value_name = "NAME")]
    pub lan_iface: Option<String>,

    /// LAN bridge address, e.g. 192.168.5.1/24
    #[arg(long, value_name = "CIDR", default_value = DEFAULT_LAN_IP)]
    pub lan_ip: String,

    /// Physical interface to attach to the LAN bridge (repeatable)
    #[arg(long = "lan-member", value_name = "NAME")]
    pub lan_member: Vec<String>,

    /// Disable the LAN DHCPv4 server
    #[arg(long)]
    pub no_lan_dhcp: bool,

    /// DHCPv4 pool range: <start> or <start>-<end>
    #[arg(long, value_name = "RANGE")]
    pub lan_dhcp_range: Option<String>,

    /// DHCPv4 address lease time in seconds
    #[arg(long, value_name = "SECONDS")]
    pub lan_dhcp_lease: Option<u32>,

    /// Services to enable (comma separated)
    #[arg(long, value_delimiter = ',', value_name = "LIST")]
    pub enable: Vec<String>,

    /// Services to disable (comma separated)
    #[arg(long, value_delimiter = ',', value_name = "LIST")]
    pub disable: Vec<String>,
}

#[derive(Serialize)]
struct InitToml {
    version: String,
    config: TomlConfig,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    ifaces: Vec<Iface>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    ipconfigs: Vec<IpService>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    nats: Vec<IfaceService>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    firewalls: Vec<IfaceService>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    dhcpv4_services: Vec<DhcpService>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    route_lans: Vec<LanRoute>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    route_wans: Vec<IfaceService>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    static_nat_mappings_v4: Vec<StaticNat>,
}

#[derive(Serialize)]
struct TomlConfig {
    auth: TomlAuth,
}

#[derive(Serialize)]
struct TomlAuth {
    #[serde(skip_serializing_if = "Option::is_none")]
    admin_user: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    admin_pass: Option<String>,
}

#[derive(Serialize)]
struct Iface {
    name: String,
    create_dev_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    controller_name: Option<String>,
    zone_type: String,
    enable_in_boot: bool,
    wifi_mode: String,
    update_at: f64,
}

#[derive(Serialize)]
struct IpService {
    iface_name: String,
    enable: bool,
    ip_model: IpModel,
    update_at: f64,
}

#[derive(Serialize)]
#[serde(tag = "t")]
enum IpModel {
    #[serde(rename = "static")]
    Static {
        default_router_ip: String,
        default_router: bool,
        ipv4: String,
        ipv4_mask: u8,
        #[serde(skip_serializing_if = "Option::is_none")]
        ipv6: Option<String>,
    },
    #[serde(rename = "dhcpclient")]
    DhcpClient {
        default_router: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        hostname: Option<String>,
        custome_opts: Vec<String>,
    },
}

#[derive(Serialize)]
struct IfaceService {
    iface_name: String,
    enable: bool,
    update_at: f64,
}

#[derive(Serialize)]
struct LanRoute {
    iface_name: String,
    enable: bool,
    static_routes: Option<Vec<String>>,
    update_at: f64,
}

#[derive(Serialize)]
struct DhcpService {
    iface_name: String,
    enable: bool,
    config: DhcpConfig,
    update_at: f64,
}

#[derive(Serialize)]
struct DhcpConfig {
    ip_range_start: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    ip_range_end: Option<String>,
    server_ip_addr: String,
    network_mask: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    address_lease_time: Option<u32>,
    custom_options: Vec<String>,
}

#[derive(Serialize)]
struct StaticNat {
    id: String,
    enable: bool,
    remark: String,
    wan_iface_name: String,
    mapping_pair_ports: Vec<PortPair>,
    lan_target: NatTarget,
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
enum NatTarget {
    Local,
}

/// 生成 `landscape_init.toml` 内容。
pub fn build_init_toml(args: &ConfigCliArgs, version: &str) -> Result<String> {
    let init = build_init(args, version)?;
    toml::to_string(&init).context("serialize the generated init config")
}

fn build_init(args: &ConfigCliArgs, version: &str) -> Result<InitToml> {
    let now = now_f64();
    // 拓扑仅由接口名是否出现决定;缺失一侧的 flag 静默忽略(与真实子命令一致)。
    let wan = args.wan_iface.clone();
    let lan = args.lan_iface.clone();

    let enabled = resolve_enabled_services(args, lan.is_some())?;

    let mut ifaces = Vec::new();
    if let Some(wan) = &wan {
        ifaces.push(Iface {
            name: wan.clone(),
            create_dev_type: "no_need_to_create".into(),
            controller_name: None,
            zone_type: "wan".into(),
            enable_in_boot: true,
            wifi_mode: "undefined".into(),
            update_at: now,
        });
    }
    if let Some(lan) = &lan {
        ifaces.push(Iface {
            name: lan.clone(),
            create_dev_type: "bridge".into(),
            controller_name: None,
            zone_type: "lan".into(),
            enable_in_boot: true,
            wifi_mode: "undefined".into(),
            update_at: now,
        });
        for member in &args.lan_member {
            if wan.as_deref() == Some(member.as_str())
                || Some(lan.as_str()) == Some(member.as_str())
            {
                bail!("invalid LAN member {member}: must differ from the WAN and LAN interfaces");
            }
            ifaces.push(Iface {
                name: member.clone(),
                create_dev_type: "no_need_to_create".into(),
                controller_name: Some(lan.clone()),
                zone_type: "undefined".into(),
                enable_in_boot: true,
                wifi_mode: "undefined".into(),
                update_at: now,
            });
        }
    }

    let mut ipconfigs = Vec::new();
    if let Some(wan) = &wan {
        match args.wan_mode {
            WanMode::Dhcp => ipconfigs.push(IpService {
                iface_name: wan.clone(),
                enable: true,
                ip_model: IpModel::DhcpClient {
                    default_router: true,
                    hostname: None,
                    custome_opts: Vec::new(),
                },
                update_at: now,
            }),
            WanMode::Static => {
                let (ipv4, mask) = parse_cidr(
                    args.wan_ip
                        .as_deref()
                        .context("--wan-mode static requires --wan-ip")?,
                )?;
                let gateway = args
                    .wan_gateway
                    .context("--wan-mode static requires --wan-gateway")?;
                ipconfigs.push(IpService {
                    iface_name: wan.clone(),
                    enable: true,
                    ip_model: IpModel::Static {
                        default_router_ip: gateway.to_string(),
                        default_router: true,
                        ipv4: ipv4.to_string(),
                        ipv4_mask: mask,
                        ipv6: None,
                    },
                    update_at: now,
                });
            }
            WanMode::None => {}
        }
    }

    let mut dhcpv4_services = Vec::new();
    if let Some(lan) = &lan
        && !args.no_lan_dhcp
    {
        let (server, mask) = parse_cidr(&args.lan_ip)?;
        let (start, end) = match args.lan_dhcp_range.as_deref() {
            Some(range) => parse_range(range)?,
            None => (default_range_start(server, mask), None),
        };
        dhcpv4_services.push(DhcpService {
            iface_name: lan.clone(),
            enable: true,
            config: DhcpConfig {
                ip_range_start: start.to_string(),
                ip_range_end: end.map(|end| end.to_string()),
                server_ip_addr: server.to_string(),
                network_mask: mask,
                address_lease_time: args.lan_dhcp_lease,
                custom_options: Vec::new(),
            },
            update_at: now,
        });
    }

    let mut nats = Vec::new();
    let mut firewalls = Vec::new();
    let mut route_wans = Vec::new();
    if let Some(wan) = &wan {
        for service in &enabled {
            match *service {
                "nat" => nats.push(IfaceService {
                    iface_name: wan.clone(),
                    enable: true,
                    update_at: now,
                }),
                "firewall" => firewalls.push(IfaceService {
                    iface_name: wan.clone(),
                    enable: true,
                    update_at: now,
                }),
                "route-wan" => route_wans.push(IfaceService {
                    iface_name: wan.clone(),
                    enable: true,
                    update_at: now,
                }),
                _ => {}
            }
        }
    }
    let mut route_lans = Vec::new();
    if let Some(lan) = lan.as_deref()
        && enabled.contains(&"route-lan")
    {
        route_lans.push(LanRoute {
            iface_name: lan.to_string(),
            enable: true,
            static_routes: None,
            update_at: now,
        });
    }

    let mut static_nat_mappings_v4 = Vec::new();
    if let Some(wan) = wan.as_deref()
        && !args.static_nat.is_empty()
    {
        let mut pairs: Vec<PortPair> = Vec::new();
        for raw in &args.static_nat {
            let (wan_port, lan_port) = raw.split_once(':').with_context(|| {
                format!("invalid --static-nat {raw}: expected <wan_port>:<lan_port>")
            })?;
            let pair = PortPair {
                wan_port: wan_port
                    .trim()
                    .parse()
                    .with_context(|| format!("invalid --static-nat {raw}: bad WAN port"))?,
                lan_port: lan_port
                    .trim()
                    .parse()
                    .with_context(|| format!("invalid --static-nat {raw}: bad LAN port"))?,
            };
            if pair.wan_port == 0 || pair.lan_port == 0 {
                bail!("invalid --static-nat {raw}: ports must not be zero");
            }
            if pairs
                .iter()
                .any(|existing| existing.wan_port == pair.wan_port)
            {
                bail!(
                    "invalid --static-nat {raw}: duplicate WAN port would conflict in DNAT rules"
                );
            }
            pairs.push(pair);
        }
        static_nat_mappings_v4.push(StaticNat {
            id: "123e4567-e89b-12d3-a456-426614174000".into(),
            enable: true,
            remark: "generated by the lkit fixture".into(),
            wan_iface_name: wan.to_string(),
            mapping_pair_ports: pairs,
            lan_target: NatTarget::Local,
            l4_protocols: vec![TCP_L4_PROTOCOL],
            update_at: now,
        });
    }

    Ok(InitToml {
        version: version.to_string(),
        config: TomlConfig {
            auth: TomlAuth {
                admin_user: args.admin_user.clone(),
                admin_pass: args.admin_pass.clone(),
            },
        },
        ifaces,
        ipconfigs,
        nats,
        firewalls,
        dhcpv4_services,
        route_lans,
        route_wans,
        static_nat_mappings_v4,
    })
}

fn resolve_enabled_services(args: &ConfigCliArgs, has_lan: bool) -> Result<Vec<&'static str>> {
    for name in args.enable.iter().chain(args.disable.iter()) {
        if !KNOWN_SERVICES.contains(&name.as_str()) {
            bail!("unknown service {name}");
        }
    }
    for name in &args.enable {
        if args.disable.contains(name) {
            bail!("service {name} is both enabled and disabled");
        }
    }
    let mut enabled: Vec<&'static str> = BASE_ENABLED_SERVICES.to_vec();
    for name in &args.enable {
        let known = KNOWN_SERVICES
            .iter()
            .find(|known| **known == name.as_str())
            .unwrap();
        if !enabled.contains(known) {
            enabled.push(known);
        }
    }
    enabled.retain(|name| !args.disable.iter().any(|d| d.as_str() == *name));
    enabled.retain(|name| args.wan_iface.is_some() || !WAN_SERVICES.contains(name));
    enabled.retain(|name| has_lan || !LAN_SERVICES.contains(name));
    Ok(enabled)
}

fn parse_cidr(raw: &str) -> Result<(Ipv4Addr, u8)> {
    let (ip, prefix) = raw
        .split_once('/')
        .with_context(|| format!("invalid CIDR {raw}: expected address/prefix"))?;
    let ip = ip
        .trim()
        .parse()
        .with_context(|| format!("invalid CIDR {raw}: bad address"))?;
    let prefix = prefix
        .trim()
        .parse::<u8>()
        .with_context(|| format!("invalid CIDR {raw}: bad prefix"))?;
    if prefix > 32 {
        bail!("invalid CIDR {raw}: prefix exceeds 32");
    }
    Ok((ip, prefix))
}

fn parse_range(raw: &str) -> Result<(Ipv4Addr, Option<Ipv4Addr>)> {
    let (start, end) = match raw.split_once('-') {
        Some((start, end)) => (start, Some(end)),
        None => (raw, None),
    };
    let start = start
        .trim()
        .parse()
        .with_context(|| format!("invalid DHCP range {raw}: bad start"))?;
    let end = match end {
        Some(end) => Some(
            end.trim()
                .parse()
                .with_context(|| format!("invalid DHCP range {raw}: bad end"))?,
        ),
        None => None,
    };
    Ok((start, end))
}

fn default_range_start(server: Ipv4Addr, mask: u8) -> Ipv4Addr {
    let mask_bits = if mask == 0 {
        0
    } else {
        u32::MAX << (32 - mask)
    };
    let network = u32::from(server) & mask_bits;
    Ipv4Addr::from(network.saturating_add(100))
}

fn now_f64() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_args() -> ConfigCliArgs {
        ConfigCliArgs {
            stdout: false,
            dir: None,
            force: false,
            admin_user: Some("admin".into()),
            admin_pass: Some("Secret123".into()),
            wan_iface: Some("ens3".into()),
            wan_mode: WanMode::Dhcp,
            wan_ip: None,
            wan_gateway: None,
            static_nat: Vec::new(),
            lan_iface: Some("br_lan".into()),
            lan_ip: "192.168.10.1/24".into(),
            lan_member: vec!["ens4".into()],
            no_lan_dhcp: false,
            lan_dhcp_range: Some("192.168.10.100-192.168.10.254".into()),
            lan_dhcp_lease: Some(43_200),
            enable: vec!["firewall".into()],
            disable: vec!["nat".into()],
        }
    }

    #[test]
    fn routed_lan_static_wan_matches_the_real_shape() {
        let mut args = base_args();
        args.wan_mode = WanMode::Static;
        args.wan_ip = Some("198.51.100.20/24".into());
        args.wan_gateway = Some("198.51.100.1".parse().unwrap());

        let toml =
            toml::from_str::<toml::Value>(&build_init_toml(&args, "1.2.3").unwrap()).unwrap();
        assert_eq!(toml["version"].as_str(), Some("1.2.3"));
        assert_eq!(
            toml["config"]["auth"]["admin_pass"].as_str(),
            Some("Secret123")
        );
        assert_eq!(toml["ipconfigs"][0]["iface_name"].as_str(), Some("ens3"));
        assert_eq!(
            toml["ipconfigs"][0]["ip_model"]["t"].as_str(),
            Some("static")
        );
        assert_eq!(
            toml["ipconfigs"][0]["ip_model"]["ipv4"].as_str(),
            Some("198.51.100.20")
        );
        assert_eq!(
            toml["ipconfigs"][0]["ip_model"]["default_router_ip"].as_str(),
            Some("198.51.100.1")
        );
        assert_eq!(toml["route_wans"][0]["iface_name"].as_str(), Some("ens3"));
        assert_eq!(toml["route_lans"][0]["iface_name"].as_str(), Some("br_lan"));
        assert_eq!(
            toml["dhcpv4_services"][0]["config"]["server_ip_addr"].as_str(),
            Some("192.168.10.1")
        );
        assert_eq!(
            toml["dhcpv4_services"][0]["config"]["ip_range_start"].as_str(),
            Some("192.168.10.100")
        );
        assert_eq!(
            toml["dhcpv4_services"][0]["config"]["ip_range_end"].as_str(),
            Some("192.168.10.254")
        );
        assert_eq!(
            toml["dhcpv4_services"][0]["config"]["address_lease_time"].as_integer(),
            Some(43_200)
        );
        // --disable nat --enable firewall:nats 与静态 NAT 键都必须省略,
        // firewalls 挂在 WAN 上。
        assert!(toml.get("nats").is_none());
        assert!(toml.get("static_nat_mappings_v4").is_none());
        assert_eq!(toml["firewalls"][0]["iface_name"].as_str(), Some("ens3"));
        assert_eq!(toml["ifaces"].as_array().map(Vec::len), Some(3));
    }

    #[test]
    fn wan_only_maps_static_nat_pairs_to_local_tcp() {
        let mut args = base_args();
        args.lan_iface = None;
        args.lan_member = Vec::new();
        args.lan_dhcp_range = None;
        args.lan_dhcp_lease = None;
        args.wan_mode = WanMode::Static;
        args.wan_ip = Some("198.51.100.20/24".into());
        args.wan_gateway = Some("198.51.100.1".parse().unwrap());
        args.static_nat = vec!["22:22".into(), "6443:6443".into()];

        let toml =
            toml::from_str::<toml::Value>(&build_init_toml(&args, "1.2.3").unwrap()).unwrap();
        assert!(toml.get("dhcpv4_services").is_none());
        assert!(toml.get("route_lans").is_none());
        let mapping = &toml["static_nat_mappings_v4"][0];
        assert_eq!(mapping["wan_iface_name"].as_str(), Some("ens3"));
        assert_eq!(
            mapping["mapping_pair_ports"].as_array().map(Vec::len),
            Some(2)
        );
        assert_eq!(
            mapping["mapping_pair_ports"][0]["wan_port"].as_integer(),
            Some(22)
        );
        assert_eq!(
            mapping["mapping_pair_ports"][1]["lan_port"].as_integer(),
            Some(6443)
        );
        assert_eq!(mapping["lan_target"]["t"].as_str(), Some("local"));
        assert_eq!(mapping["l4_protocols"].as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn dhcp_wan_uses_the_dhcpclient_model() {
        let args = base_args();
        let toml =
            toml::from_str::<toml::Value>(&build_init_toml(&args, "0.25.1").unwrap()).unwrap();
        assert_eq!(
            toml["ipconfigs"][0]["ip_model"]["t"].as_str(),
            Some("dhcpclient")
        );
    }

    #[test]
    fn wan_mode_none_with_iface_registers_the_wan_without_an_address() {
        let mut args = base_args();
        args.wan_mode = WanMode::None;
        args.wan_ip = None;
        args.wan_gateway = None;

        let toml =
            toml::from_str::<toml::Value>(&build_init_toml(&args, "1.2.3").unwrap()).unwrap();
        assert!(toml.get("ipconfigs").is_none());
        // WAN 接口照常注册,WAN 服务照常挂载(--disable nat,故只剩 firewall 与 route-wan)。
        let ifaces = toml["ifaces"].as_array().unwrap();
        assert!(
            ifaces
                .iter()
                .any(|iface| iface["name"].as_str() == Some("ens3"))
        );
        assert_eq!(toml["firewalls"][0]["iface_name"].as_str(), Some("ens3"));
        assert_eq!(toml["route_wans"][0]["iface_name"].as_str(), Some("ens3"));
        assert_eq!(toml["route_lans"][0]["iface_name"].as_str(), Some("br_lan"));
    }

    #[test]
    fn lan_flags_without_lan_iface_are_ignored() {
        let mut args = base_args();
        args.lan_iface = None;
        args.lan_member = vec!["ens4".into()];
        args.lan_dhcp_range = Some("192.168.10.100-192.168.10.254".into());

        let toml =
            toml::from_str::<toml::Value>(&build_init_toml(&args, "1.2.3").unwrap()).unwrap();
        assert!(toml.get("dhcpv4_services").is_none());
        assert!(toml.get("route_lans").is_none());
        assert_eq!(toml["ifaces"].as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn static_nat_without_wan_iface_is_ignored() {
        let mut args = base_args();
        args.wan_iface = None;
        args.static_nat = vec!["22:22".into()];

        let toml =
            toml::from_str::<toml::Value>(&build_init_toml(&args, "1.2.3").unwrap()).unwrap();
        assert!(toml.get("static_nat_mappings_v4").is_none());
    }

    #[test]
    fn duplicate_static_nat_wan_ports_are_rejected() {
        let mut args = base_args();
        args.lan_iface = None;
        args.lan_member = Vec::new();
        args.lan_dhcp_range = None;
        args.lan_dhcp_lease = None;
        args.static_nat = vec!["22:22".into(), "22:8080".into()];
        assert!(build_init_toml(&args, "1.2.3").is_err());
    }

    #[test]
    fn unknown_service_is_rejected() {
        let mut args = base_args();
        args.enable = vec!["dns".into()];
        assert!(build_init_toml(&args, "1.2.3").is_err());
    }
}
