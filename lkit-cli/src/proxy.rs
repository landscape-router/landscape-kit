//! 出站 HTTP 请求的代理分流规则,行为规范见 `docs/network/proxy.md`。
//!
//! lkit 的出站 client 分三个具名角色:
//! - 外部下载(`release::repository::download`)与外部镜像探测(`mirror::availability`):
//!   按请求 URL 的 host 分流——回环地址走禁用代理的直连通道,其余走遵循
//!   `http_proxy`/`https_proxy`/`all_proxy` 的代理通道;
//! - 内部验证(`service::health`、`backup::export`): 目标恒为本机 Landscape 服务,
//!   client 构建时即禁用代理,不参与分流。

/// host 是否为本机回环地址。只认 URL 字面量，不做 DNS 解析；
/// 与仓库协议允许明文 HTTP 的回环集合一致(`localhost`、`127.0.0.1`、`[::1]`)。
/// `Url::host_str` 对 IPv6 返回带方括号的 `[::1]`,两种形式都认。
pub(crate) fn is_loopback_host(host: Option<&str>) -> bool {
    matches!(host, Some("localhost" | "127.0.0.1" | "::1" | "[::1]"))
}

#[cfg(test)]
mod tests {
    use url::Url;

    use super::*;

    fn assert_loopback(raw: &str, expected: bool) {
        let url = Url::parse(raw).expect("解析测试 URL");
        assert_eq!(is_loopback_host(url.host_str()), expected, "{url}");
    }

    #[test]
    fn classifies_loopback_literals() {
        assert_loopback("http://127.0.0.1:9000/repository/file", true);
        assert_loopback("http://localhost:9000/file", true);
        assert_loopback("http://[::1]:9000/file", true);
        assert_loopback("https://127.0.0.1:6443/api/docs", true);
    }

    #[test]
    fn classifies_external_hosts() {
        assert_loopback("https://github.com/ThisSeanZhang/landscape/releases", false);
        assert_loopback(
            "https://mirror.nju.edu.cn/debian/dists/trixie/Release",
            false,
        );
        // 私网地址不是回环,不享受直连豁免。
        assert_loopback("http://192.168.1.10:8080/file", false);
    }
}
