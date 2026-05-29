//! NIC scanner — discovers physical network interfaces from sysfs.

use std::path::{Path, PathBuf};

/// Information about a discovered network interface.
#[derive(Debug, Clone)]
pub struct NicInfo {
    /// Interface name (e.g., "eth0").
    pub name: String,
    /// MAC address.
    pub mac: String,
    /// Current IP address if available.
    pub current_ip: Option<String>,
    /// Whether the interface is operationally up.
    pub is_up: bool,
}

/// Prefixes of virtual/excluded interfaces.
const EXCLUDED_PREFIXES: &[&str] = &[
    "lo", "docker", "veth", "br-", "virbr", "tun", "tap", "wg",
];

/// Scan `/sys/class/net/` for physical network interfaces.
pub fn scan_nics() -> Vec<NicInfo> {
    scan_nics_at(Path::new("/sys/class/net"))
}

/// Scan a sysfs net directory for physical network interfaces.
///
/// Excludes virtual interfaces (lo, docker, veth, br-, virbr, tun, tap, wg).
/// Returns interfaces sorted by name for stable ordering.
pub fn scan_nics_at(base: &Path) -> Vec<NicInfo> {
    let entries = match std::fs::read_dir(base) {
        Ok(e) => e,
        Err(_) => return vec![],
    };

    let mut nics: Vec<NicInfo> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name().to_string_lossy().to_string();

            if EXCLUDED_PREFIXES.iter().any(|p| name.starts_with(p)) {
                return None;
            }

            let iface_dir = entry.path();
            let mac = read_trimmed(iface_dir.join("address")).unwrap_or_default();
            let operstate = read_trimmed(iface_dir.join("operstate")).unwrap_or_default();
            let is_up = operstate == "up";

            Some(NicInfo {
                name,
                mac,
                current_ip: None,
                is_up,
            })
        })
        .collect();

    nics.sort_by(|a, b| a.name.cmp(&b.name));
    nics
}

/// Read a file and trim whitespace.
fn read_trimmed(path: PathBuf) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    Some(content.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_sysfs(dir: &Path, interfaces: &[(&str, &str, &str)]) {
        for (name, mac, operstate) in interfaces {
            let iface_dir = dir.join(name);
            std::fs::create_dir_all(&iface_dir).unwrap_or(());
            std::fs::write(iface_dir.join("address"), mac).unwrap_or(());
            std::fs::write(iface_dir.join("operstate"), operstate).unwrap_or(());
        }
    }

    /// Excludes lo and docker, returns only physical interfaces.
    #[test]
    fn test_scan_excludes_lo_and_docker() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        setup_sysfs(
            dir.path(),
            &[
                ("lo", "00:00:00:00:00:00", "unknown"),
                ("docker0", "02:42:ac:11:00:02", "down"),
                ("eth0", "aa:bb:cc:dd:ee:01", "up"),
            ],
        );

        let nics = scan_nics_at(dir.path());
        assert_eq!(nics.len(), 1);
        assert_eq!(nics[0].name, "eth0");
        Ok(())
    }

    /// MAC address is read correctly.
    #[test]
    fn test_scan_reads_mac() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        setup_sysfs(
            dir.path(),
            &[("enp0s3", "08:00:27:c9:38:ab", "up")],
        );

        let nics = scan_nics_at(dir.path());
        assert_eq!(nics.len(), 1);
        assert_eq!(nics[0].mac, "08:00:27:c9:38:ab");
        assert!(nics[0].is_up);
        Ok(())
    }

    /// Empty directory returns empty vec.
    #[test]
    fn test_scan_empty_dir() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let nics = scan_nics_at(dir.path());
        assert!(nics.is_empty());
        Ok(())
    }
}
