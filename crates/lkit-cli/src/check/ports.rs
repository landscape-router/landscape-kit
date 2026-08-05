use super::model::{CheckResult, Status};

const LISTEN_STATE_TCP: &str = "0A";

#[derive(Debug, Clone)]
struct Listener {
    protocol: &'static str,
    address: String,
    port: u16,
    process: Option<(String, String)>,
}

pub fn run() -> Vec<CheckResult> {
    vec![
        port_check("port.dns", crate::tr!("DNS port", "DNS 端口"), 53, true),
        port_check(
            "port.http",
            crate::tr!("HTTP management port", "HTTP 管理端口"),
            6300,
            false,
        ),
        port_check(
            "port.https",
            crate::tr!("HTTPS management port", "HTTPS 管理端口"),
            6443,
            false,
        ),
    ]
}

fn port_check(id: &'static str, title: &'static str, port: u16, include_udp: bool) -> CheckResult {
    let mut listeners = Vec::new();
    let mut read_errors = Vec::new();
    let mut files: Vec<(&str, &'static str, bool)> = vec![
        ("/proc/net/tcp", "tcp", true),
        ("/proc/net/tcp6", "tcp6", true),
    ];
    if include_udp {
        files.push(("/proc/net/udp", "udp", false));
        files.push(("/proc/net/udp6", "udp6", false));
    }
    for (path, protocol, is_tcp) in files {
        let raw = match std::fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(err) => {
                read_errors.push(format!("{path}: {err}"));
                continue;
            }
        };
        for (address, inode) in parse_proc_net(&raw, port, is_tcp) {
            listeners.push(Listener {
                protocol,
                address,
                port,
                process: find_process(inode),
            });
        }
    }
    let mut result = build_port_result(id, title, port, listeners.clone());
    for error in &read_errors {
        result = result.detail(crate::trf!(
            ("Unable to read listener information: {error}"),
            ("无法读取监听信息：{error}")
        ));
    }
    if listeners.is_empty() && !read_errors.is_empty() {
        result = result
            .set(
                Status::Unknown,
                crate::trf!(("{port} unknown"), ("{port} 无法确认")),
                crate::tr!("Unable to read all required kernel listener tables; the port cannot be confirmed as free", "无法读取全部所需的内核监听表，不能确认端口空闲"),
            )
            .suggestion(crate::tr!("Run as root and confirm that the relevant /proc/net files are readable", "以 root 身份运行并确认 /proc/net 相关文件可读取"));
    }
    result
}

fn parse_proc_net(raw: &str, port: u16, is_tcp: bool) -> Vec<(String, u64)> {
    raw.lines()
        .skip(1)
        .filter_map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 10 {
                return None;
            }
            let local = parts[1];
            let state = parts[3];
            if is_tcp && state != LISTEN_STATE_TCP {
                return None;
            }
            let (addr, port_hex) = local.rsplit_once(':')?;
            let local_port = u16::from_str_radix(port_hex, 16).ok()?;
            if local_port != port {
                return None;
            }
            let inode = parts[9].parse::<u64>().ok()?;
            Some((addr.to_string(), inode))
        })
        .collect()
}

fn build_port_result(
    id: &'static str,
    title: &'static str,
    port: u16,
    listeners: Vec<Listener>,
) -> CheckResult {
    let mut result = CheckResult::new(id, title);
    if listeners.is_empty() {
        return result.set(
            Status::Pass,
            crate::trf!(("{port} not listening"), ("{port} 无监听")),
            crate::tr!("Port is free", "端口空闲"),
        );
    }
    result = result.set(
        Status::Error,
        crate::trf!(("{port} occupied"), ("{port} 已被占用")),
        crate::tr!(
            "Another service is listening on this port, so Landscape cannot start the service",
            "端口被其他服务监听，Landscape 无法启动该服务"
        ),
    );
    result.suggestion = crate::tr!(
        "Stop the service using this port or move it to another port",
        "停止占用该端口的服务，或为其更换端口"
    )
    .to_string();
    for listener in &listeners {
        match &listener.process {
            Some((comm, pid)) => {
                result = result.detail(crate::trf!(
                    (
                        "{} {}:{} is used by {} (pid={})",
                        listener.protocol,
                        listener.address,
                        listener.port,
                        comm,
                        pid
                    ),
                    (
                        "{} {}:{} 被 {}（pid={}）占用",
                        listener.protocol,
                        listener.address,
                        listener.port,
                        comm,
                        pid
                    )
                ))
            }
            None => {
                result = result.detail(crate::trf!(
                    (
                        "{} {}:{} is listening, but owner information is unreadable",
                        listener.protocol,
                        listener.address,
                        listener.port
                    ),
                    (
                        "{} {}:{} 被监听，但监听者信息不可读取",
                        listener.protocol,
                        listener.address,
                        listener.port
                    )
                ))
            }
        }
    }
    result
}

fn find_process(inode: u64) -> Option<(String, String)> {
    let entries = std::fs::read_dir("/proc").ok()?;
    for entry in entries.flatten() {
        let pid_name = entry.file_name();
        let pid_name = pid_name.to_string_lossy();
        if !pid_name.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let dir = entry.path();
        let comm = std::fs::read_to_string(dir.join("comm"))
            .ok()
            .map(|c| c.trim().to_string())
            .unwrap_or_default();
        let Ok(fd_dir) = std::fs::read_dir(dir.join("fd")) else {
            continue;
        };
        for fd in fd_dir.flatten() {
            let Ok(target) = std::fs::read_link(fd.path()) else {
                continue;
            };
            let target = target.to_string_lossy();
            let Some(rest) = target.strip_prefix("socket:[") else {
                continue;
            };
            let Some(socket_inode) = rest.strip_suffix(']') else {
                continue;
            };
            if socket_inode.parse::<u64>().ok() == Some(inode) {
                return Some((comm, pid_name.to_string()));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const TCP_SAMPLE: &str = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000:0035 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 20683 1 0000000000000000 100 0 0 10 0
   1: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 20684 1 0000000000000000 100 0 0 10 0
   2: 00000000:1388 00000000:0000 06 00000000:00000000 00:00000000 00000000     0        0 20685 1 0000000000000000 100 0 0 10 0";

    const UDP_SAMPLE: &str = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000:0035 00000000:0000 07 00000000:00000000 00:00000000 00000000     0        0 20686 1 0000000000000000 100 0 0 10 0";

    const TCP6_SAMPLE: &str = "\
  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000000000000000000000000000:189C 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 20687 1 0000000000000000 100 0 0 10 0";

    #[test]
    fn parses_listening_tcp_entries() {
        let found = parse_proc_net(TCP_SAMPLE, 53, true);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0], ("00000000".to_string(), 20683));
    }

    #[test]
    fn ignores_non_listening_tcp_states() {
        assert!(parse_proc_net(TCP_SAMPLE, 5000, true).is_empty());
    }

    #[test]
    fn filters_by_port() {
        assert!(parse_proc_net(TCP_SAMPLE, 80, true).is_empty());
    }

    #[test]
    fn parses_udp_entries() {
        let found = parse_proc_net(UDP_SAMPLE, 53, false);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0], ("00000000".to_string(), 20686));
    }

    #[test]
    fn parses_tcp6_entries() {
        let found = parse_proc_net(TCP6_SAMPLE, 6300, true);
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0],
            ("00000000000000000000000000000000".to_string(), 20687)
        );
    }

    #[test]
    fn free_port_reports_pass() {
        let result = build_port_result("port.test", "test port", 1234, Vec::new());
        assert_eq!(result.status, Status::Pass);
        assert_eq!(result.value, "1234 not listening");
    }

    #[test]
    fn occupied_port_reports_error_with_details() {
        let listeners = vec![Listener {
            protocol: "tcp",
            address: "00000000".to_string(),
            port: 53,
            process: Some(("named".to_string(), "123".to_string())),
        }];
        let result = build_port_result("port.test", "test port", 53, listeners);
        assert_eq!(result.status, Status::Error);
        assert_eq!(result.value, "53 occupied");
        assert!(
            result
                .details
                .iter()
                .any(|d| d.contains("named") && d.contains("123"))
        );
    }

    #[test]
    fn occupied_port_reports_error_without_process() {
        let listeners = vec![Listener {
            protocol: "udp",
            address: "00000000".to_string(),
            port: 53,
            process: None,
        }];
        let result = build_port_result("port.test", "test port", 53, listeners);
        assert_eq!(result.status, Status::Error);
        assert!(
            result
                .details
                .iter()
                .any(|d| d.contains("owner information is unreadable"))
        );
    }
}
