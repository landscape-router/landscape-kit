use super::model::{CheckResult, Status};

const RESOLV_CONF: &str = "/etc/resolv.conf";

const RISK_NOTE: &str = "Landscape 启动 DNS 服务时可能把 /etc/resolv.conf 指向 127.0.0.1；停止 Landscape 后若主机无法解析域名，请优先检查该文件。本命令只读，不自动备份或修改文件。";

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
                    .unwrap_or_else(|err| format!("无法读取链接目标：{err}"));
                result = result.detail(format!("{RESOLV_CONF} 是符号链接 → {target}"));
            }
            match std::fs::read_to_string(RESOLV_CONF) {
                Ok(content) => {
                    let nameservers = parse_nameservers(&content);
                    let value = if nameservers.is_empty() {
                        String::from("无 nameserver 条目")
                    } else {
                        format!("nameserver: {}", nameservers.join(", "))
                    };
                    if nameservers.is_empty() {
                        result
                            .set(Status::Warning, value, "配置中没有可用的 nameserver")
                            .suggestion(RISK_NOTE)
                    } else if meta.file_type().is_symlink() {
                        result
                            .set(
                                Status::Warning,
                                value,
                                "配置文件是符号链接，存在可恢复性风险",
                            )
                            .suggestion(RISK_NOTE)
                    } else {
                        result.set(Status::Pass, value, "DNS 配置正常")
                    }
                }
                Err(err) => result.set(
                    Status::Unknown,
                    "不可读取",
                    format!("{RESOLV_CONF} 存在但无法读取：{err}"),
                ),
            }
        }
        Err(_) => result
            .set(Status::Warning, "不存在", format!("{RESOLV_CONF} 不存在"))
            .suggestion(RISK_NOTE),
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
