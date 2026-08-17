//! Shared CLI argument parsers for the lndp-client and lndp-server binaries.

/// Parse `--ethertype`: hex (`0x88b6`) or decimal, restricted to the local
/// experimental range 0x88B5-0x88B7 used by Landscape.
pub fn parse_ethertype(s: &str) -> Result<u16, String> {
    let (raw, radix) = match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        Some(hex) => (hex, 16),
        None => (s, 10),
    };
    let v = u16::from_str_radix(raw, radix).map_err(|_| format!("invalid ethertype: '{s}'"))?;
    if !matches!(v, 0x88B5 | 0x88B6 | 0x88B7) {
        return Err(format!(
            "ethertype 0x{v:04x} is not a local experimental ethertype (must be 0x88B5-0x88B7)"
        ));
    }
    Ok(v)
}

/// Parse a MAC address like `aa:bb:cc:dd:ee:ff` or `AA-BB-CC-DD-EE-01`.
pub fn parse_mac(s: &str) -> Result<[u8; 6], String> {
    let parts: Vec<&str> = s.split([':', '-']).collect();
    if parts.len() != 6 {
        return Err(format!("invalid MAC: '{s}' (expected AA:BB:CC:DD:EE:FF)"));
    }
    let mut out = [0u8; 6];
    for (i, p) in parts.iter().enumerate() {
        out[i] = u8::from_str_radix(p, 16).map_err(|_| format!("invalid MAC: '{s}'"))?;
    }
    Ok(out)
}

/// Parse a `--dev` value: `any`, a single device, or a comma-separated list.
pub fn parse_devs(s: &str) -> Result<Vec<String>, String> {
    let devs: Vec<String> = s
        .split(',')
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(String::from)
        .collect();
    if devs.is_empty() {
        return Err("empty device list".to_string());
    }
    if devs.iter().any(|d| d == "any") && devs.len() > 1 {
        return Err("'any' cannot be combined with other devices".to_string());
    }
    Ok(devs)
}

/// Parse a `--forward` value: `LOCAL:DST` (listen on local port LOCAL,
/// forward to the server's 127.0.0.1:DST).
pub fn parse_forward(s: &str) -> Result<(u16, u16), String> {
    let (local, dst) = s.split_once(':').ok_or_else(|| {
        format!("invalid forward '{s}' (expected LOCAL_PORT:DST_PORT)")
    })?;
    let local = local
        .trim()
        .parse::<u16>()
        .map_err(|_| format!("invalid local port in '{s}'"))?;
    let dst = dst
        .trim()
        .parse::<u16>()
        .map_err(|_| format!("invalid destination port in '{s}'"))?;
    if local == 0 || dst == 0 {
        return Err(format!("invalid forward '{s}' (ports must be non-zero)"));
    }
    Ok((local, dst))
}

/// Parse a comma-separated list of ports, e.g. `--forward-ports 22,6443`.
pub fn parse_port_list(s: &str) -> Result<Vec<u16>, String> {
    let ports: Vec<u16> = s
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| p.parse::<u16>().map_err(|_| format!("invalid port '{p}'")))
        .collect::<Result<_, _>>()?;
    if ports.is_empty() {
        return Err("empty port list".to_string());
    }
    if ports.contains(&0) {
        return Err("invalid port list (ports must be non-zero)".to_string());
    }
    Ok(ports)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_parsing() {
        assert_eq!(parse_mac("aa:bb:cc:dd:ee:ff").unwrap(), [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]);
        assert_eq!(parse_mac("AA-BB-CC-DD-EE-01").unwrap(), [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0x01]);
        assert!(parse_mac("aa:bb:cc:dd:ee").is_err());
        assert!(parse_mac("xx:bb:cc:dd:ee:ff").is_err());
    }

    #[test]
    fn ethertype_parsing() {
        assert_eq!(parse_ethertype("0x88b5").unwrap(), 0x88B5);
        assert_eq!(parse_ethertype("34998").unwrap(), 0x88B6);
        assert!(parse_ethertype("0x1234").is_err());
        assert!(parse_ethertype("xyz").is_err());
    }

    #[test]
    fn devs_parsing() {
        assert_eq!(parse_devs("any").unwrap(), ["any"]);
        assert_eq!(parse_devs("eth0").unwrap(), ["eth0"]);
        assert_eq!(parse_devs("eth0,eth1").unwrap(), ["eth0", "eth1"]);
        assert_eq!(parse_devs(" eth0 , eth1 ").unwrap(), ["eth0", "eth1"]);
        assert!(parse_devs("").is_err());
        assert!(parse_devs("any,eth0").is_err());
    }

    #[test]
    fn forward_parsing() {
        assert_eq!(parse_forward("2222:22").unwrap(), (2222, 22));
        assert_eq!(parse_forward(" 2222 : 22 ").unwrap(), (2222, 22));
        assert!(parse_forward("2222").is_err());
        assert!(parse_forward("x:22").is_err());
        assert!(parse_forward("0:22").is_err());
        assert!(parse_forward("2222:0").is_err());
    }

    #[test]
    fn port_list_parsing() {
        assert_eq!(parse_port_list("22").unwrap(), [22]);
        assert_eq!(parse_port_list("22,6443").unwrap(), [22, 6443]);
        assert_eq!(parse_port_list(" 22 , 6443 ").unwrap(), [22, 6443]);
        assert!(parse_port_list("").is_err());
        assert!(parse_port_list("22,x").is_err());
        assert!(parse_port_list("0").is_err());
    }
}
