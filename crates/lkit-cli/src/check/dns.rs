use super::model::{CheckResult, Status};

const RESOLV_CONF: &str = "/etc/resolv.conf";

fn risk_note() -> String {
    crate::tr!(crate::keys::DNS_RISK_NOTE)
}

pub fn run() -> Vec<CheckResult> {
    vec![resolv_conf()]
}

fn resolv_conf() -> CheckResult {
    let mut result = CheckResult::new("dns.resolv_conf", "/etc/resolv.conf");
    let meta = std::fs::symlink_metadata(RESOLV_CONF);
    match meta {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                let target = std::fs::read_link(RESOLV_CONF)
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|err| {
                        crate::tr!(crate::keys::DNS_UNABLE_READ_SYMLINK_TARGET, err = err)
                    });
                result = result.detail(crate::tr!(
                    crate::keys::DNS_RESOLV_CONF_SYMLINK,
                    RESOLV_CONF = RESOLV_CONF,
                    target = target
                ));
            }
            match std::fs::read_to_string(RESOLV_CONF) {
                Ok(content) => {
                    let nameservers = parse_nameservers(&content);
                    let value = if nameservers.is_empty() {
                        crate::tr!(crate::keys::DNS_NO_NAMESERVER_ENTRIES).to_string()
                    } else {
                        format!("nameserver: {}", nameservers.join(", "))
                    };
                    if nameservers.is_empty() {
                        result
                            .set(
                                Status::Warning,
                                value,
                                crate::tr!(crate::keys::DNS_NO_USABLE_NAMESERVER),
                            )
                            .suggestion(risk_note())
                    } else if meta.file_type().is_symlink() {
                        result
                            .set(
                                Status::Warning,
                                value,
                                crate::tr!(crate::keys::DNS_SYMLINK_RECOVERY_RISK),
                            )
                            .suggestion(risk_note())
                    } else {
                        result.set(
                            Status::Pass,
                            value,
                            crate::tr!(crate::keys::DNS_CONFIGURATION_VALID),
                        )
                    }
                }
                Err(err) => result.set(
                    Status::Unknown,
                    crate::tr!(crate::keys::DNS_UNREADABLE),
                    crate::tr!(
                        crate::keys::DNS_RESOLV_CONF_EXISTS_CANNOT_READ,
                        RESOLV_CONF = RESOLV_CONF,
                        err = err
                    ),
                ),
            }
        }
        Err(_) => result
            .set(
                Status::Warning,
                crate::tr!(crate::keys::DNS_MISSING),
                crate::tr!(
                    crate::keys::DNS_RESOLV_CONF_DOES_NOT_EXIST,
                    RESOLV_CONF = RESOLV_CONF
                ),
            )
            .suggestion(risk_note()),
    }
}

fn parse_nameservers(content: &str) -> Vec<String> {
    content
        .lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix("nameserver")?;
            let value = rest.trim();
            if value.is_empty() {
                None
            } else {
                Some(value.to_string())
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nameservers() {
        let content = "# comment\nnameserver 127.0.0.53\nnameserver 10.1.1.10\noptions edns0\n";
        assert_eq!(parse_nameservers(content), vec!["127.0.0.53", "10.1.1.10"]);
    }

    #[test]
    fn ignores_empty_nameserver_lines() {
        assert!(parse_nameservers("nameserver\nnameserver  \n").is_empty());
    }
}
