use super::model::{CheckResult, Status};

const RESOLV_CONF: &str = "/etc/resolv.conf";

fn risk_note() -> &'static str {
    crate::tr!(
        "When Landscape starts its DNS service it may point /etc/resolv.conf at 127.0.0.1. If name resolution fails after Landscape stops, check this file first. This command is read-only and does not back up or modify the file.",
        "Landscape 启动 DNS 服务时可能把 /etc/resolv.conf 指向 127.0.0.1；停止 Landscape 后若主机无法解析域名，请优先检查该文件。本命令只读，不自动备份或修改文件。"
    )
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
                        crate::trf!(
                            ("unable to read symlink target: {err}"),
                            ("无法读取链接目标：{err}")
                        )
                    });
                result = result.detail(crate::trf!(
                    ("{RESOLV_CONF} is a symlink -> {target}"),
                    ("{RESOLV_CONF} 是符号链接 → {target}")
                ));
            }
            match std::fs::read_to_string(RESOLV_CONF) {
                Ok(content) => {
                    let nameservers = parse_nameservers(&content);
                    let value = if nameservers.is_empty() {
                        crate::tr!("no nameserver entries", "无 nameserver 条目").to_string()
                    } else {
                        format!("nameserver: {}", nameservers.join(", "))
                    };
                    if nameservers.is_empty() {
                        result
                            .set(
                                Status::Warning,
                                value,
                                crate::tr!(
                                    "The configuration has no usable nameserver",
                                    "配置中没有可用的 nameserver"
                                ),
                            )
                            .suggestion(risk_note())
                    } else if meta.file_type().is_symlink() {
                        result
                            .set(
                                Status::Warning,
                                value,
                                crate::tr!("The configuration file is a symlink, which creates a recovery risk", "配置文件是符号链接，存在可恢复性风险"),
                            )
                            .suggestion(risk_note())
                    } else {
                        result.set(
                            Status::Pass,
                            value,
                            crate::tr!("DNS configuration is valid", "DNS 配置正常"),
                        )
                    }
                }
                Err(err) => result.set(
                    Status::Unknown,
                    crate::tr!("unreadable", "不可读取"),
                    crate::trf!(
                        ("{RESOLV_CONF} exists but cannot be read: {err}"),
                        ("{RESOLV_CONF} 存在但无法读取：{err}")
                    ),
                ),
            }
        }
        Err(_) => result
            .set(
                Status::Warning,
                crate::tr!("missing", "不存在"),
                crate::trf!(("{RESOLV_CONF} does not exist"), ("{RESOLV_CONF} 不存在")),
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
