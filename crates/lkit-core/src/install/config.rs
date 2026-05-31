//! Data models for the install wizard.
//!
//! These are pure data types collected by the wizard — no system side effects.

use std::net::Ipv4Addr;
use std::path::PathBuf;

/// Complete installation configuration collected by the wizard.
///
/// Contains network setup, Landscape service settings, source selection,
/// and the target Landscape binary version.
#[derive(Debug, Clone)]
pub struct InstallConfig {
    /// Network configuration (WAN + optional LAN).
    pub network: NetworkSetup,
    /// Landscape web service and admin settings.
    pub landscape: LandscapeServiceConfig,
    /// Installation source and version selection.
    pub source: SourceSelection,
    /// Landscape binary version string (from ReleaseManifest.tag, v prefix stripped).
    /// Written to TOML `version` field. Landscape validates this matches its VERSION constant.
    pub landscape_version: String,
    /// Landscape HOME directory path (e.g., /root/.landscape-router).
    pub home: PathBuf,
}

/// Network setup with WAN and optional LAN configuration.
///
/// When `lan` is `None`, only the WAN interface is configured (single-NIC mode).
#[derive(Debug, Clone)]
pub struct NetworkSetup {
    /// WAN interface configuration.
    pub wan: WanSetup,
    /// LAN bridge configuration. `None` in single-NIC mode.
    pub lan: Option<LanSetup>,
}

/// WAN interface setup.
#[derive(Debug, Clone)]
pub struct WanSetup {
    /// Physical NIC name selected as WAN.
    pub iface_name: String,
    /// WAN IP mode (DHCP, static, or nothing).
    pub mode: WanMode,
}

/// WAN IP configuration mode.
///
/// `Nothing` declares a WAN interface without configuring IP — the user
/// configures it later in the Web UI.
#[derive(Debug, Clone)]
pub enum WanMode {
    /// Declare WAN interface but do not configure IP.
    Nothing,
    /// Obtain IP via DHCP.
    Dhcp,
    /// Use a static IP address.
    Static {
        /// IPv4 address.
        ipv4: Ipv4Addr,
        /// Subnet mask in CIDR notation (e.g., 24).
        mask: u8,
        /// Default gateway address.
        gateway: Ipv4Addr,
    },
}

/// LAN bridge configuration.
///
/// Physical NICs are combined into a `br_lan` bridge managed by Landscape.
#[derive(Debug, Clone)]
pub struct LanSetup {
    /// Physical NIC names to join into the bridge.
    pub member_nics: Vec<String>,
    /// Bridge gateway IP address (e.g., 192.168.5.1).
    pub gateway: Ipv4Addr,
    /// Subnet mask in CIDR notation (e.g., 24).
    pub mask: u8,
}

/// Landscape web service and admin account configuration.
#[derive(Debug, Clone)]
pub struct LandscapeServiceConfig {
    /// Web UI listen port.
    pub web_port: u16,
    /// HTTPS listen port.
    pub https_port: u16,
    /// Admin username.
    pub admin_user: String,
    /// Admin password.
    pub admin_pass: String,
}

/// Installation source and version selection.
///
/// Both fields are `None` when using auto-detection defaults.
#[derive(Debug, Clone)]
pub struct SourceSelection {
    /// Source name (e.g., "r2-official"). `None` = auto-detect.
    pub source_name: Option<String>,
    /// Version tag (e.g., "v0.19.2"). `None` = latest.
    pub version: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify all three WanMode variants can be constructed and matched.
    #[test]
    fn test_wan_mode_variants() {
        let nothing = WanMode::Nothing;
        assert!(matches!(nothing, WanMode::Nothing));

        let dhcp = WanMode::Dhcp;
        assert!(matches!(dhcp, WanMode::Dhcp));

        let s = WanMode::Static {
            ipv4: Ipv4Addr::new(10, 0, 0, 1),
            mask: 24,
            gateway: Ipv4Addr::new(10, 0, 0, 254),
        };
        if let WanMode::Static { ipv4, mask, gateway } = s {
            assert_eq!(ipv4, Ipv4Addr::new(10, 0, 0, 1));
            assert_eq!(mask, 24);
            assert_eq!(gateway, Ipv4Addr::new(10, 0, 0, 254));
        } else {
            panic!("expected Static variant");
        }
    }

    /// When LAN is configured, `lan` field is Some.
    #[test]
    fn test_network_setup_with_lan() {
        let setup = NetworkSetup {
            wan: WanSetup {
                iface_name: "eth0".to_string(),
                mode: WanMode::Dhcp,
            },
            lan: Some(LanSetup {
                member_nics: vec!["eth1".to_string()],
                gateway: Ipv4Addr::new(192, 168, 5, 1),
                mask: 24,
            }),
        };
        assert!(setup.lan.is_some());
        let lan = setup.lan.as_ref();
        assert!(lan.is_some());
        assert_eq!(lan.map(|l| l.member_nics.len()), Some(1));
    }

    /// Single-NIC mode: `lan` is None.
    #[test]
    fn test_network_setup_without_lan() {
        let setup = NetworkSetup {
            wan: WanSetup {
                iface_name: "eth0".to_string(),
                mode: WanMode::Dhcp,
            },
            lan: None,
        };
        assert!(setup.lan.is_none());
    }

    /// Verify gateway is within the subnet defined by mask.
    #[test]
    fn test_lan_setup_gateway_in_range() {
        let lan = LanSetup {
            member_nics: vec!["eth1".to_string()],
            gateway: Ipv4Addr::new(192, 168, 5, 1),
            mask: 24,
        };
        let gw = u32::from(lan.gateway);
        let mask_bits = !0u32 << (32 - lan.mask);
        let network = gw & mask_bits;
        let broadcast = network | !mask_bits;
        assert_ne!(gw, network, "gateway must not be network address");
        assert_ne!(gw, broadcast, "gateway must not be broadcast address");
    }
}
