//! Persist connection identities before takeover and verify their recovery.
use std::path::Path;
use std::process::Command;

use crate::deployment::{
    plan::InstallError, runtime::InstallRuntime, transaction::HostServiceBefore,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Connection {
    pub uuid: String,
    pub interface: String,
    pub physical_mac: Option<String>,
    pub ipv4: bool,
    pub ipv6: bool,
    pub gateway4: bool,
    pub gateway6: bool,
}

fn error(message: impl Into<String>) -> InstallError {
    InstallError::Preflight(format!("NetworkManager recovery: {}", message.into()))
}

pub(crate) fn validate(connections: &[Connection]) -> Result<(), InstallError> {
    let mut identities = std::collections::HashSet::new();
    for c in connections {
        if uuid::Uuid::parse_str(&c.uuid).is_err()
            || !identities.insert((&c.uuid, &c.interface))
            || c.physical_mac.as_ref().is_some_and(|mac| {
                mac.len() != 17
                    || !mac
                        .split(':')
                        .all(|part| part.len() == 2 && part.bytes().all(|b| b.is_ascii_hexdigit()))
            })
        {
            return Err(error("invalid or duplicate saved connection"));
        }
        super::config::validate_iface_name(&c.interface)?;
    }
    Ok(())
}

trait Manager {
    fn active(&mut self) -> Result<Vec<(String, String)>, InstallError>;
    fn physical_mac(&mut self, interface: &str) -> Result<Option<String>, InstallError>;
    fn connectivity(&mut self, interface: &str) -> Result<[bool; 4], InstallError>;
    fn activate(&mut self, uuid: &str, interface: &str) -> Result<(), InstallError>;
}

struct Nmcli<'a> {
    sysfs: &'a Path,
    ip_command: &'a Path,
}
fn command(args: &[&str]) -> Result<String, InstallError> {
    let output = Command::new("nmcli")
        .args(["--wait", "20", "--terse", "--escape", "no"])
        .args(args)
        .env("LC_ALL", "C")
        .output()
        .map_err(|e| error(format!("nmcli is required: {e}")))?;
    if !output.status.success() {
        return Err(error(format!(
            "nmcli {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    String::from_utf8(output.stdout).map_err(|_| error("invalid nmcli output"))
}

fn parse_active(text: &str) -> Result<Vec<(String, String)>, InstallError> {
    let mut active = Vec::new();
    for line in text.lines().filter(|s| !s.is_empty()) {
        let parts: Vec<_> = line.splitn(3, ':').collect();
        if parts.len() != 3 || uuid::Uuid::parse_str(parts[0]).is_err() {
            return Err(error("invalid active connection identity"));
        }
        if parts[1] == "loopback" {
            continue;
        }
        super::config::validate_iface_name(parts[2])?;
        if parts[2] == "--" {
            return Err(error("active connection has no restorable interface"));
        }
        active.push((parts[0].into(), parts[2].into()));
    }
    Ok(active)
}

fn has_address(text: &str, ipv6: bool) -> bool {
    text.lines().any(|s| {
        let address = s.split('/').next().unwrap_or("").trim();
        match address.parse::<std::net::IpAddr>() {
            Ok(std::net::IpAddr::V4(ip)) => !ipv6 && !ip.is_unspecified() && !ip.is_loopback(),
            Ok(std::net::IpAddr::V6(ip)) => {
                ipv6 && !ip.is_unspecified() && !ip.is_loopback() && !ip.is_unicast_link_local()
            }
            Err(_) => false,
        }
    })
}

impl Manager for Nmcli<'_> {
    fn active(&mut self) -> Result<Vec<(String, String)>, InstallError> {
        parse_active(&command(&[
            "--fields",
            "UUID,TYPE,DEVICE",
            "connection",
            "show",
            "--active",
        ])?)
    }
    fn physical_mac(&mut self, interface: &str) -> Result<Option<String>, InstallError> {
        let path = self.sysfs.join(interface);
        if !path.join("device").exists() {
            return Ok(None);
        }
        let output = Command::new(self.ip_command)
            .args(["-j", "link", "show", "dev", interface])
            .output()
            .map_err(InstallError::Io)?;
        if !output.status.success() {
            return Err(error("cannot read permanent interface identity"));
        }
        let links: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout)
            .map_err(|_| error("invalid interface identity output"))?;
        let link = links
            .first()
            .filter(|_| links.len() == 1)
            .ok_or_else(|| error("ambiguous interface identity"))?;
        let permanent = link.get("permaddr").and_then(|v| v.as_str());
        if permanent.is_none()
            && std::fs::read_to_string(path.join("addr_assign_type"))
                .map_err(InstallError::Io)?
                .trim()
                != "0"
        {
            return Err(error(format!(
                "cannot identify permanent MAC for {interface}"
            )));
        }
        Ok(Some(
            permanent
                .or_else(|| link.get("address").and_then(|v| v.as_str()))
                .ok_or_else(|| error("missing interface MAC"))?
                .to_ascii_lowercase(),
        ))
    }
    fn connectivity(&mut self, interface: &str) -> Result<[bool; 4], InstallError> {
        let mut fields = Vec::new();
        for field in ["IP4.ADDRESS", "IP6.ADDRESS", "IP4.GATEWAY", "IP6.GATEWAY"] {
            fields.push(command(&[
                "--get-values",
                field,
                "device",
                "show",
                interface,
            ])?);
        }
        Ok([
            has_address(&fields[0], false),
            has_address(&fields[1], true),
            fields[2]
                .trim()
                .parse::<std::net::Ipv4Addr>()
                .is_ok_and(|ip| !ip.is_unspecified()),
            fields[3]
                .trim()
                .parse::<std::net::Ipv6Addr>()
                .is_ok_and(|ip| !ip.is_unspecified()),
        ])
    }
    fn activate(&mut self, uuid: &str, interface: &str) -> Result<(), InstallError> {
        command(&["connection", "up", "uuid", uuid, "ifname", interface]).map(|_| ())
    }
}

fn capture_with(manager: &mut impl Manager) -> Result<Vec<Connection>, InstallError> {
    let active = manager.active()?;
    let mut result = Vec::new();
    for (uuid, interface) in &active {
        let [ipv4, ipv6, gateway4, gateway6] = manager.connectivity(interface)?;
        result.push(Connection {
            uuid: uuid.clone(),
            interface: interface.clone(),
            physical_mac: manager.physical_mac(interface)?,
            ipv4,
            ipv6,
            gateway4,
            gateway6,
        });
    }
    validate(&result)?;
    // Never bind a snapshot to a profile which changed during capture.
    let mut after = manager.active()?;
    let mut before = active;
    before.sort();
    after.sort();
    if before != after {
        return Err(error(
            "connections changed during capture; retry installation",
        ));
    }
    Ok(result)
}

fn restore_with(
    connections: &[Connection],
    manager: &mut impl Manager,
) -> Result<(), InstallError> {
    validate(connections)?;
    // Validate all physical identities before activating any connection.
    for c in connections {
        if let Some(mac) = &c.physical_mac
            && manager.physical_mac(&c.interface)?.as_deref() != Some(mac.as_str())
        {
            return Err(error(format!("physical interface {} changed", c.interface)));
        }
    }
    let mut activation_error = None;
    for c in connections {
        if !manager
            .active()?
            .iter()
            .any(|(uuid, iface)| uuid == &c.uuid && iface == &c.interface)
        {
            // A missing profile must not prevent restoration of another management link.
            if let Err(error) = manager.activate(&c.uuid, &c.interface) {
                activation_error.get_or_insert(error);
            }
        }
    }
    if let Some(error) = activation_error {
        return Err(error);
    }
    let active = manager.active()?;
    for c in connections {
        let actual = manager.connectivity(&c.interface)?;
        let expected = [c.ipv4, c.ipv6, c.gateway4, c.gateway6];
        if !active
            .iter()
            .any(|(uuid, iface)| uuid == &c.uuid && iface == &c.interface)
            || expected
                .iter()
                .zip(actual)
                .any(|(required, present)| *required && !present)
        {
            return Err(error(format!(
                "connection {} on {} has not restored its addresses/routes; will retry",
                c.uuid, c.interface
            )));
        }
    }
    Ok(())
}

pub(crate) fn capture(
    runtime: &InstallRuntime,
    services: &[HostServiceBefore],
) -> Result<Vec<Connection>, InstallError> {
    // Explicit fixture runtimes have fake services; real NM semantics are tested in QEMU.
    if runtime.test_runtime_path.is_some() || !has_active_network_manager(services) {
        return Ok(Vec::new());
    }
    capture_with(&mut Nmcli {
        sysfs: &runtime.sys_class_net,
        ip_command: &runtime.ip_command,
    })
}

pub(crate) fn restore(
    network: &crate::deployment::transaction::NetworkTakeoverTransaction,
) -> Result<(), InstallError> {
    let Some(connections) = &network.network_manager else {
        if has_active_network_manager(&network.host_services) {
            eprintln!(
                "lkit: legacy transaction has no NetworkManager connection snapshot; verify the original connections and IP connectivity manually"
            );
        }
        return Ok(());
    };
    if connections.is_empty() {
        return Ok(());
    }
    restore_with(
        connections,
        &mut Nmcli {
            sysfs: Path::new("/sys/class/net"),
            ip_command: Path::new(super::discovery::HOST_IP_COMMAND),
        },
    )
}

fn has_active_network_manager(services: &[HostServiceBefore]) -> bool {
    services.iter().any(|service| {
        service.active && crate::service::host_network::is_network_manager(&service.unit)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_network_manager_service_aliases_require_connection_snapshots() {
        for name in ["networkmanager", "NetworkManager", "NetworkManager.service"] {
            let mut service = HostServiceBefore {
                unit: name.into(),
                installed: true,
                active: true,
                enable_state: String::new(),
            };
            assert!(has_active_network_manager(&[service.clone()]));
            service.active = false;
            assert!(!has_active_network_manager(&[service]));
        }
        assert!(!has_active_network_manager(&[HostServiceBefore {
            unit: "networking".into(),
            installed: true,
            active: true,
            enable_state: String::new()
        }]));
    }

    const UUID: &str = "1c776a18-1caf-4ac8-8f46-5573fb8db501";
    struct Fake {
        active: Vec<(String, String)>,
        mac: Option<String>,
        connectivity: [bool; 4],
        activations: usize,
        fail_activation: bool,
        fail_interface: Option<String>,
        active_reads: std::collections::VecDeque<Vec<(String, String)>>,
    }
    impl Default for Fake {
        fn default() -> Self {
            Self {
                active: vec![(UUID.into(), "eth0".into())],
                mac: Some("52:54:00:12:34:01".into()),
                connectivity: [true, false, true, false],
                activations: 0,
                fail_activation: false,
                fail_interface: None,
                active_reads: Default::default(),
            }
        }
    }
    impl Manager for Fake {
        fn active(&mut self) -> Result<Vec<(String, String)>, InstallError> {
            Ok(self
                .active_reads
                .pop_front()
                .unwrap_or_else(|| self.active.clone()))
        }
        fn physical_mac(&mut self, _: &str) -> Result<Option<String>, InstallError> {
            Ok(self.mac.clone())
        }
        fn connectivity(&mut self, _: &str) -> Result<[bool; 4], InstallError> {
            Ok(self.connectivity)
        }
        fn activate(&mut self, uuid: &str, interface: &str) -> Result<(), InstallError> {
            self.activations += 1;
            if self.fail_activation || self.fail_interface.as_deref() == Some(interface) {
                return Err(error("profile deleted"));
            }
            self.active.retain(|(_, iface)| iface != interface);
            self.active.push((uuid.into(), interface.into()));
            Ok(())
        }
    }
    #[test]
    fn external_ipv6_does_not_count_as_restored_dhcp_and_pending_dhcp_is_not_restarted() {
        let mut manager = Fake::default();
        let before = capture_with(&mut manager).unwrap();
        manager.active = vec![("299df81c-247d-4f7a-a796-26638d3f205c".into(), "eth0".into())];
        manager.connectivity = [false, true, false, true];
        assert!(restore_with(&before, &mut manager).is_err());
        assert_eq!(manager.activations, 1);
        assert_eq!(manager.active[0].0, UUID);
        assert!(restore_with(&before, &mut manager).is_err());
        assert_eq!(
            manager.activations, 1,
            "do not reset a pending DHCP acquisition on every retry"
        );
        manager.connectivity = [true, false, true, false];
        restore_with(&before, &mut manager).unwrap();
        restore_with(&before, &mut manager).unwrap();
        assert_eq!(manager.activations, 1);
    }
    #[test]
    fn changed_hardware_and_deleted_profiles_cannot_finish_recovery() {
        let mut manager = Fake::default();
        let before = capture_with(&mut manager).unwrap();
        manager.mac = Some("52:54:00:12:34:99".into());
        assert!(restore_with(&before, &mut manager).is_err());
        assert_eq!(manager.activations, 0);
        manager.mac = before[0].physical_mac.clone();
        manager.active.clear();
        manager.fail_activation = true;
        assert!(restore_with(&before, &mut manager).is_err());
        manager.fail_activation = false;
        restore_with(&before, &mut manager).unwrap();
    }
    #[test]
    fn ipv6_only_and_no_default_route_connections_do_not_require_ipv4() {
        let mut manager = Fake {
            connectivity: [false, true, false, false],
            ..Fake::default()
        };
        let before = capture_with(&mut manager).unwrap();
        restore_with(&before, &mut manager).unwrap();
        manager.connectivity = [false; 4];
        assert!(restore_with(&before, &mut manager).is_err());
        assert!(!has_address("fe80::1234/64\n::1/128", true));
        assert!(has_address("fec0::1234/64", true));
        assert!(has_address("10.0.2.99/24", false));
    }
    #[test]
    fn saved_connections_reject_duplicate_identities_and_unsafe_interfaces() {
        let mut before = capture_with(&mut Fake::default()).unwrap();
        before.push(before[0].clone());
        assert!(validate(&before).is_err());
        before.pop();
        before[0].interface = "../outside".into();
        assert!(validate(&before).is_err());
        before[0].interface = "eth0".into();
        before[0].uuid = "--help".into();
        assert!(validate(&before).is_err());
    }
    #[test]
    fn active_output_preserves_uuid_and_interface_and_ignores_loopback() {
        let rows =
            parse_active(&format!("{UUID}:802-3-ethernet:eth0\n{UUID}:loopback:lo\n")).unwrap();
        assert_eq!(rows, vec![(UUID.into(), "eth0".into())]);
        assert!(parse_active(&format!("{UUID}:vpn:--")).is_err());
        assert!(parse_active("unexpected").is_err());
    }
    #[test]
    fn failed_profile_does_not_prevent_other_connections_from_recovering() {
        let second = "299df81c-247d-4f7a-a796-26638d3f205c";
        let mut manager = Fake::default();
        manager.active.push((second.into(), "eth1".into()));
        let before = capture_with(&mut manager).unwrap();
        manager.active.clear();
        manager.fail_interface = Some("eth0".into());
        assert!(restore_with(&before, &mut manager).is_err());
        assert_eq!(manager.active, vec![(second.into(), "eth1".into())]);
        assert_eq!(manager.activations, 2);
        assert!(restore_with(&before, &mut manager).is_err());
        assert_eq!(
            manager.activations, 3,
            "healthy connection must not restart"
        );
        manager.fail_interface = None;
        restore_with(&before, &mut manager).unwrap();
        assert_eq!(manager.activations, 4);
        restore_with(&before, &mut manager).unwrap();
        assert_eq!(manager.activations, 4);
    }

    #[test]
    fn capture_rejects_connection_changes_but_accepts_reordered_results() {
        let mut manager = Fake::default();
        manager.active_reads = [manager.active.clone(), vec![]].into();
        assert!(capture_with(&mut manager).is_err());
        assert_eq!(manager.activations, 0);
        manager
            .active
            .push(("299df81c-247d-4f7a-a796-26638d3f205c".into(), "eth1".into()));
        let mut reversed = manager.active.clone();
        reversed.reverse();
        manager.active_reads = [manager.active.clone(), reversed].into();
        assert_eq!(capture_with(&mut manager).unwrap().len(), 2);
    }

    #[test]
    fn disappearing_connection_after_activation_cannot_complete_recovery() {
        let mut manager = Fake::default();
        let before = capture_with(&mut manager).unwrap();
        manager.active.clear();
        manager.active_reads = [vec![], vec![]].into();
        assert!(restore_with(&before, &mut manager).is_err());
        assert_eq!(manager.activations, 1);
        restore_with(&before, &mut manager).unwrap();
        assert_eq!(manager.activations, 1);
    }

    #[test]
    fn legacy_network_snapshots_remain_readable() {
        let value = serde_json::json!({
            "plan": {"mode": {"mode": "wan_dhcp", "wan": "eth0"}, "selected_macs": []},
            "host_services": [], "confirmation_deadline": "2026-01-01T00:00:00Z",
            "rollback_service":"old.service", "rollback_timer":"old.timer",
            "boot_rollback_service":"old-boot.service", "recovery_binary":"service/recovery",
            "pending_state":"transactions/state.json"
        });
        let decoded: crate::deployment::transaction::NetworkTakeoverTransaction =
            serde_json::from_value(value).unwrap();
        assert!(decoded.network_manager.is_none());
        assert!(!decoded.preserve_startup_links);
        assert!(!decoded.management_bridge_was_absent);
    }
}
