//! Generates `landscape_init.toml` from an [`InstallConfig`].
//!
//! Covers 6 scenarios: 3 WAN modes (Dhcp / Static / Nothing) x 2 LAN modes (with / without).

use std::net::Ipv4Addr;

use lkit_core::{InstallConfig, WanMode};

use crate::error::AppError;

/// Generate a `landscape_init.toml` string from the collected install configuration.
///
/// Returns `Err(AppError::ConfigGeneration(...))` when DHCP pool derivation fails
/// (e.g., /30 subnet).
pub fn generate_init_toml(config: &InstallConfig) -> Result<String, AppError> {
    let mut doc = toml_edit::DocumentMut::new();

    // version
    doc["version"] = toml_edit::value(&config.landscape_version);

    // [config.auth] — use explicit Table to avoid inline format
    let mut auth = toml_edit::Table::new();
    auth["admin_user"] = toml_edit::value(&config.landscape.admin_user);
    auth["admin_pass"] = toml_edit::value(&config.landscape.admin_pass);

    // [config.web] — use explicit Table to avoid inline format
    let mut web = toml_edit::Table::new();
    web["port"] = toml_edit::value(i64::from(config.landscape.web_port));
    web["https_port"] = toml_edit::value(i64::from(config.landscape.https_port));
    web["web_root"] = toml_edit::value(format!("{}/static", config.home.display()));

    let mut cfg = toml_edit::Table::new();
    cfg["auth"] = toml_edit::Item::Table(auth);
    cfg["web"] = toml_edit::Item::Table(web);
    doc["config"] = toml_edit::Item::Table(cfg);

    // ── ifaces ──────────────────────────────────────────────────

    let mut ifaces = toml_edit::ArrayOfTables::new();

    // WAN interface
    let mut wan_iface = toml_edit::Table::new();
    wan_iface["name"] = toml_edit::value(&config.network.wan.iface_name);
    wan_iface["create_dev_type"] = toml_edit::value("no_need_to_create");
    wan_iface["zone_type"] = toml_edit::value("wan");
    wan_iface["enable_in_boot"] = toml_edit::value(true);
    wan_iface["wifi_mode"] = toml_edit::value("undefined");
    ifaces.push(wan_iface);

    // LAN bridge members + bridge itself
    if let Some(ref lan) = config.network.lan {
        for nic in &lan.member_nics {
            let mut member = toml_edit::Table::new();
            member["name"] = toml_edit::value(nic);
            member["create_dev_type"] = toml_edit::value("no_need_to_create");
            member["controller_name"] = toml_edit::value("br_lan");
            member["zone_type"] = toml_edit::value("undefined");
            member["enable_in_boot"] = toml_edit::value(true);
            member["wifi_mode"] = toml_edit::value("undefined");
            ifaces.push(member);
        }

        let mut br = toml_edit::Table::new();
        br["name"] = toml_edit::value("br_lan");
        br["create_dev_type"] = toml_edit::value("bridge");
        br["zone_type"] = toml_edit::value("lan");
        br["enable_in_boot"] = toml_edit::value(true);
        br["wifi_mode"] = toml_edit::value("undefined");
        ifaces.push(br);
    }

    doc.insert("ifaces", toml_edit::Item::ArrayOfTables(ifaces));

    // ── ipconfigs ───────────────────────────────────────────────

    let mut ipconfigs = toml_edit::ArrayOfTables::new();

    // WAN ipconfig
    let mut wan_ip = toml_edit::Table::new();
    wan_ip["iface_name"] = toml_edit::value(&config.network.wan.iface_name);
    wan_ip["enable"] = toml_edit::value(true);
    {
        let mut ip_model = toml_edit::InlineTable::new();
        match &config.network.wan.mode {
            WanMode::Dhcp => {
                ip_model.insert("t", "dhcpclient".into());
                ip_model.insert("default_router", true.into());
            }
            WanMode::Static {
                ipv4,
                mask,
                gateway,
            } => {
                ip_model.insert("t", "static".into());
                ip_model.insert("default_router", true.into());
                ip_model.insert("default_router_ip", gateway.to_string().into());
                ip_model.insert("ipv4", ipv4.to_string().into());
                ip_model.insert("ipv4_mask", i64::from(*mask).into());
            }
            WanMode::Nothing => {
                ip_model.insert("t", "nothing".into());
            }
        }
        wan_ip["ip_model"] = toml_edit::value(ip_model);
    }
    ipconfigs.push(wan_ip);

    // LAN bridge ipconfig
    if let Some(ref lan) = config.network.lan {
        let mut lan_ip = toml_edit::Table::new();
        lan_ip["iface_name"] = toml_edit::value("br_lan");
        lan_ip["enable"] = toml_edit::value(true);
        {
            let mut ip_model = toml_edit::InlineTable::new();
            ip_model.insert("t", "static".into());
            ip_model.insert("default_router", false.into());
            ip_model.insert("ipv4", lan.gateway.to_string().into());
            ip_model.insert("ipv4_mask", i64::from(lan.mask).into());
            lan_ip["ip_model"] = toml_edit::value(ip_model);
        }
        ipconfigs.push(lan_ip);
    }

    doc.insert("ipconfigs", toml_edit::Item::ArrayOfTables(ipconfigs));

    // ── route_wans ──────────────────────────────────────────────

    let mut route_wans = toml_edit::ArrayOfTables::new();
    let mut rw = toml_edit::Table::new();
    rw["iface_name"] = toml_edit::value(&config.network.wan.iface_name);
    rw["enable"] = toml_edit::value(true);
    route_wans.push(rw);
    doc.insert("route_wans", toml_edit::Item::ArrayOfTables(route_wans));

    // ── route_lans + dhcpv4_services (only with LAN) ────────────

    if let Some(ref lan) = config.network.lan {
        let mut route_lans = toml_edit::ArrayOfTables::new();
        let mut rl = toml_edit::Table::new();
        rl["iface_name"] = toml_edit::value("br_lan");
        rl["enable"] = toml_edit::value(true);
        route_lans.push(rl);
        doc.insert("route_lans", toml_edit::Item::ArrayOfTables(route_lans));

        // DHCP server
        let mut dhcp_services = toml_edit::ArrayOfTables::new();
        let mut dhcp = toml_edit::Table::new();
        dhcp["iface_name"] = toml_edit::value("br_lan");
        dhcp["enable"] = toml_edit::value(true);

        let (range_start, _range_end) = derive_dhcp_pool(lan.gateway, lan.mask)?;
        let mut cfg = toml_edit::Table::new();
        cfg["ip_range_start"] = toml_edit::value(range_start.to_string());
        cfg["server_ip_addr"] = toml_edit::value(lan.gateway.to_string());
        cfg["network_mask"] = toml_edit::value(i64::from(lan.mask));
        // 12h lease (LANDSCAPE_DHCP_DEFAULT_ADDRESS_LEASE_TIME in release builds)
        cfg["address_lease_time"] = toml_edit::value(43_200i64);
        dhcp.insert("config", toml_edit::Item::Table(cfg));

        dhcp_services.push(dhcp);
        doc.insert(
            "dhcpv4_services",
            toml_edit::Item::ArrayOfTables(dhcp_services),
        );
    }

    Ok(doc.to_string())
}

/// Derive DHCP range start and end addresses from gateway and mask.
///
/// Returns `Err` when mask == 30 (only 2 usable addresses, unsuitable for auto-DHCP).
/// For mask >= 25, uses gateway + 1 as range start (instead of .100 which may be out of subnet).
fn derive_dhcp_pool(gateway: Ipv4Addr, mask: u8) -> Result<(Ipv4Addr, Ipv4Addr), AppError> {
    if mask >= 30 {
        return Err(AppError::ConfigGeneration(format!(
            "mask /{mask} only has 2 usable addresses — configure DHCP manually in Web UI"
        )));
    }

    let gw = u32::from(gateway);
    let mask_bits = !0u32 << (32 - mask);
    let network = gw & mask_bits;
    let broadcast = network | !mask_bits;

    let range_start = if mask >= 25 {
        // Small subnet: start at gateway + 1
        Ipv4Addr::from(gw + 1)
    } else {
        // Normal subnet: start at .100 within the network
        let base = network;
        let candidate = base + 100;
        if candidate >= broadcast {
            // .100 is beyond broadcast — fallback to gateway + 1
            Ipv4Addr::from(gw + 1)
        } else {
            Ipv4Addr::from(candidate)
        }
    };

    let range_end = Ipv4Addr::from(broadcast - 1);

    Ok((range_start, range_end))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lkit_core::{
        InstallConfig, LanSetup, LandscapeServiceConfig, NetworkSetup, SourceSelection, WanSetup,
    };

    fn base_config() -> InstallConfig {
        InstallConfig {
            network: NetworkSetup {
                wan: WanSetup {
                    iface_name: "eth0".to_string(),
                    mode: WanMode::Dhcp,
                },
                lan: None,
            },
            landscape: LandscapeServiceConfig {
                web_port: 6300,
                https_port: 6443,
                admin_user: "root".to_string(),
                admin_pass: "secret".to_string(),
            },
            source: SourceSelection {
                source_name: None,
                version: None,
            },
            landscape_version: "0.19.2".to_string(),
            home: std::path::PathBuf::from("/tmp/test-landscape"),
        }
    }

    fn lan_setup() -> LanSetup {
        LanSetup {
            member_nics: vec!["eth1".to_string(), "eth2".to_string()],
            gateway: Ipv4Addr::new(192, 168, 5, 1),
            mask: 24,
        }
    }

    /// WAN DHCP + LAN bridge with DHCP server.
    #[test]
    fn test_wan_dhcp_with_lan() -> Result<(), Box<dyn std::error::Error>> {
        let mut cfg = base_config();
        cfg.network.lan = Some(lan_setup());

        let toml_str = generate_init_toml(&cfg)?;
        let parsed: toml::Value = toml::from_str(&toml_str)?;

        // version
        assert_eq!(parsed["version"].as_str(), Some("0.19.2"));

        // WAN ip_model tag
        let wan_ip = &parsed["ipconfigs"][0];
        assert_eq!(wan_ip["ip_model"]["t"].as_str(), Some("dhcpclient"));
        assert_eq!(wan_ip["ip_model"]["default_router"].as_bool(), Some(true));

        // bridge member has controller_name
        let br_member = &parsed["ifaces"][1];
        assert_eq!(br_member["name"].as_str(), Some("eth1"));
        assert_eq!(br_member["controller_name"].as_str(), Some("br_lan"));
        assert_eq!(br_member["zone_type"].as_str(), Some("undefined"));

        // bridge iface
        let br = &parsed["ifaces"][3];
        assert_eq!(br["name"].as_str(), Some("br_lan"));
        assert_eq!(br["create_dev_type"].as_str(), Some("bridge"));

        // DHCP service exists
        assert_eq!(
            parsed["dhcpv4_services"][0]["iface_name"].as_str(),
            Some("br_lan")
        );
        assert_eq!(
            parsed["dhcpv4_services"][0]["config"]["ip_range_start"].as_str(),
            Some("192.168.5.100")
        );

        // route_lans exists
        assert_eq!(
            parsed["route_lans"][0]["iface_name"].as_str(),
            Some("br_lan")
        );

        Ok(())
    }

    /// WAN static IP + LAN bridge.
    #[test]
    fn test_wan_static_with_lan() -> Result<(), Box<dyn std::error::Error>> {
        let mut cfg = base_config();
        cfg.network.wan.mode = WanMode::Static {
            ipv4: Ipv4Addr::new(10, 0, 0, 100),
            mask: 24,
            gateway: Ipv4Addr::new(10, 0, 0, 1),
        };
        cfg.network.lan = Some(lan_setup());

        let toml_str = generate_init_toml(&cfg)?;
        let parsed: toml::Value = toml::from_str(&toml_str)?;

        let wan_ip = &parsed["ipconfigs"][0];
        assert_eq!(wan_ip["ip_model"]["t"].as_str(), Some("static"));
        assert_eq!(wan_ip["ip_model"]["ipv4"].as_str(), Some("10.0.0.100"));
        assert_eq!(wan_ip["ip_model"]["ipv4_mask"].as_integer(), Some(24));
        assert_eq!(
            wan_ip["ip_model"]["default_router_ip"].as_str(),
            Some("10.0.0.1")
        );

        Ok(())
    }

    /// WAN Nothing + LAN bridge.
    #[test]
    fn test_wan_nothing_with_lan() -> Result<(), Box<dyn std::error::Error>> {
        let mut cfg = base_config();
        cfg.network.wan.mode = WanMode::Nothing;
        cfg.network.lan = Some(lan_setup());

        let toml_str = generate_init_toml(&cfg)?;
        let parsed: toml::Value = toml::from_str(&toml_str)?;

        let wan_ip = &parsed["ipconfigs"][0];
        assert_eq!(wan_ip["ip_model"]["t"].as_str(), Some("nothing"));

        Ok(())
    }

    /// Single-NIC mode: no bridge, no route_lans, no dhcpv4_services.
    #[test]
    fn test_single_nic_wan_only() -> Result<(), Box<dyn std::error::Error>> {
        let cfg = base_config(); // lan = None

        let toml_str = generate_init_toml(&cfg)?;
        let parsed: toml::Value = toml::from_str(&toml_str)?;

        // Only 1 iface (WAN), no bridge
        assert_eq!(parsed["ifaces"].as_array().map(|a| a.len()), Some(1));
        assert_eq!(parsed["ifaces"][0]["name"].as_str(), Some("eth0"));

        // Only 1 ipconfig (WAN)
        assert_eq!(parsed["ipconfigs"].as_array().map(|a| a.len()), Some(1));

        // No route_lans or dhcpv4_services
        assert!(parsed.get("route_lans").is_none());
        assert!(parsed.get("dhcpv4_services").is_none());

        Ok(())
    }

    /// Small subnet (/28): DHCP range starts at gateway+1, not .100.
    #[test]
    fn test_dhcp_pool_small_subnet() -> Result<(), Box<dyn std::error::Error>> {
        let mut cfg = base_config();
        cfg.network.lan = Some(LanSetup {
            member_nics: vec!["eth1".to_string()],
            gateway: Ipv4Addr::new(192, 168, 5, 1),
            mask: 28,
        });

        let toml_str = generate_init_toml(&cfg)?;
        let parsed: toml::Value = toml::from_str(&toml_str)?;

        // /28: range start should be gateway+1 = 192.168.5.2
        assert_eq!(
            parsed["dhcpv4_services"][0]["config"]["ip_range_start"].as_str(),
            Some("192.168.5.2")
        );

        Ok(())
    }

    /// mask=30 returns ConfigGeneration error.
    #[test]
    fn test_dhcp_pool_mask_30() {
        let mut cfg = base_config();
        cfg.network.lan = Some(LanSetup {
            member_nics: vec!["eth1".to_string()],
            gateway: Ipv4Addr::new(192, 168, 5, 1),
            mask: 30,
        });

        let result = generate_init_toml(&cfg);
        match result {
            Err(AppError::ConfigGeneration(msg)) => {
                assert!(
                    msg.contains("/30"),
                    "message should mention /30, got: {msg}"
                );
            }
            Err(other) => panic!("expected ConfigGeneration, got: {other}"),
            Ok(_) => panic!("expected error for mask=30, got Ok"),
        }
    }
}
