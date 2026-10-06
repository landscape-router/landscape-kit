mod body;
mod retry;

use std::path::Path;
use std::time::Duration;

use reqwest::header::HeaderMap;
use reqwest::redirect::Policy;
use reqwest::{Client, RequestBuilder, Response, StatusCode};
use semver::Version;
use url::Url;

#[cfg(test)]
use super::AssetEncoding;
pub(crate) use super::archive::{MAX_DECOMPRESSED_BYTES, decompress_zstd, extract_static_archive};
use super::{Asset, RepositoryError};
use crate::interaction::presentation::DownloadProgress;
use crate::proxy::is_loopback_host;

use self::body::{read_body_limited, write_asset_response};
pub(crate) use self::retry::is_retryable_request_error;
use self::retry::{RETRY_AFTER_LIMIT, jitter_seed, rate_limit_wait, retryable_status};

pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const METADATA_TIMEOUT: Duration = Duration::from_secs(60);
pub(crate) const ASSET_TIMEOUT: Duration = Duration::from_secs(30 * 60);
pub(crate) const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const METADATA_BODY_LIMIT: u64 = 10 * 1024 * 1024;
pub(crate) const MAX_ATTEMPTS: usize = 3;
pub(crate) const MAX_REDIRECTS: usize = 5;

/// 具名双通道客户端:按目标地址分流,代理语义互不影响。
#[derive(Debug)]
struct ClientPair {
    /// 外部下载专用:遵循 `http_proxy`/`https_proxy`/`all_proxy` 环境变量。
    external: Client,
    /// 回环直连专用:禁用全部代理。目标是本机(`127.0.0.1`/`localhost`/`::1`,
    /// 见 [`is_loopback_host`])时使用,否则用户 shell 里的代理变量会把本机
    /// 请求也劫持进代理,本地 http 仓库与健康探测随之误报失败。
    loopback: Client,
}

impl ClientPair {
    fn new() -> Result<Self, RepositoryError> {
        Ok(Self {
            external: build_client(false)?,
            loopback: build_client(true)?,
        })
    }

    fn pick(&self, url: &Url) -> &Client {
        if is_loopback_host(url.host_str()) {
            &self.loopback
        } else {
            &self.external
        }
    }
}

fn build_client(direct: bool) -> Result<Client, RepositoryError> {
    let mut builder = Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .redirect(Policy::custom(move |attempt| {
            if attempt.previous().len() >= MAX_REDIRECTS {
                return attempt.stop();
            }
            // 重定向只在本通道内跟随:URL 安全校验之外,跳转目标的回环性必须
            // 与通道一致——外部通道不得借重定向进入回环,直连通道不得外发,
            // 跨界跳转按 3xx 原样返回上层报错(见 docs/network/proxy.md)。
            if validate_network_url(attempt.url()).is_err()
                || is_loopback_host(attempt.url().host_str()) != direct
            {
                return attempt.stop();
            }
            attempt.follow()
        }));
    if direct {
        builder = builder.no_proxy();
    }
    builder.build().map_err(RepositoryError::BuildClient)
}

#[derive(Debug)]
pub(crate) struct DownloadClient {
    clients: ClientPair,
    retry_delays: [Duration; 2],
    jitter_max: Duration,
}

impl DownloadClient {
    pub(crate) fn new() -> Result<Self, RepositoryError> {
        Ok(Self {
            clients: ClientPair::new()?,
            retry_delays: [Duration::from_secs(1), Duration::from_secs(2)],
            jitter_max: Duration::from_millis(250),
        })
    }

    pub(crate) fn request(&self, url: Url) -> RequestBuilder {
        self.clients.pick(&url).get(url)
    }

    /// 元数据请求：总超时 60 秒、响应体上限 10 MiB、最多 3 次尝试。
    /// `allow_missing` 为 true 时 HTTP 404 返回 `Ok(None)`。
    pub(crate) async fn get_metadata(
        &self,
        url: Url,
        headers: HeaderMap,
        allow_missing: bool,
    ) -> Result<Option<(HeaderMap, Vec<u8>)>, RepositoryError> {
        let mut seed = jitter_seed();
        for attempt in 0..MAX_ATTEMPTS {
            let response = match self
                .send_once(url.clone(), headers.clone(), METADATA_TIMEOUT)
                .await
            {
                Ok(response) => response,
                Err(error) if error.is_retryable() && attempt < MAX_ATTEMPTS - 1 => {
                    self.sleep_backoff(attempt, &mut seed).await;
                    continue;
                }
                Err(error) => return Err(error),
            };

            let status = response.status();
            if let Some(retry_after) = rate_limit_wait(status, response.headers()) {
                if retry_after > RETRY_AFTER_LIMIT {
                    return Err(RepositoryError::RateLimited(retry_after));
                }
                if attempt < MAX_ATTEMPTS - 1 {
                    tokio::time::sleep(Duration::from_secs(retry_after) + self.jitter(&mut seed))
                        .await;
                    continue;
                }
                return Err(RepositoryError::UnexpectedStatus(status));
            }
            if retryable_status(status) && attempt < MAX_ATTEMPTS - 1 {
                self.sleep_backoff(attempt, &mut seed).await;
                continue;
            }
            if allow_missing && status == StatusCode::NOT_FOUND {
                return Ok(None);
            }
            if status == StatusCode::UNAUTHORIZED {
                return Err(RepositoryError::InvalidToken);
            }
            if status != StatusCode::OK {
                return Err(RepositoryError::UnexpectedStatus(status));
            }

            let response_headers = response.headers().clone();
            match read_body_limited(response, METADATA_BODY_LIMIT).await {
                Ok(body) => return Ok(Some((response_headers, body))),
                Err(error) if error.is_retryable() && attempt < MAX_ATTEMPTS - 1 => {
                    self.sleep_backoff(attempt, &mut seed).await;
                }
                Err(error) => return Err(error),
            }
        }
        unreachable!("retry 循环必然返回")
    }

    /// 资产下载：总超时 30 分钟、连续 30 秒无数据视为超时、最多 3 次尝试。
    /// 每次尝试前删除不完整临时文件并从头下载，v1 不做 Range 续传。
    /// 成功后校验实际大小和 SHA-256。
    pub(crate) async fn download_asset(
        &self,
        version: &Version,
        asset: &Asset,
        label: &str,
        temp_path: &Path,
    ) -> Result<(), RepositoryError> {
        let mut seed = jitter_seed();
        for attempt in 0..MAX_ATTEMPTS {
            let _ = tokio::fs::remove_file(temp_path).await;
            let response = match self
                .send_once(asset.url.clone(), HeaderMap::new(), ASSET_TIMEOUT)
                .await
            {
                Ok(response) => response,
                Err(error) if error.is_retryable() && attempt < MAX_ATTEMPTS - 1 => {
                    self.sleep_backoff(attempt, &mut seed).await;
                    continue;
                }
                Err(error) => return Err(error),
            };

            let status = response.status();
            if let Some(retry_after) = rate_limit_wait(status, response.headers()) {
                if retry_after > RETRY_AFTER_LIMIT {
                    return Err(RepositoryError::RateLimited(retry_after));
                }
                if attempt < MAX_ATTEMPTS - 1 {
                    tokio::time::sleep(Duration::from_secs(retry_after) + self.jitter(&mut seed))
                        .await;
                    continue;
                }
                return Err(RepositoryError::UnexpectedStatus(status));
            }
            if retryable_status(status) && attempt < MAX_ATTEMPTS - 1 {
                self.sleep_backoff(attempt, &mut seed).await;
                continue;
            }
            if status != StatusCode::OK {
                return Err(RepositoryError::UnexpectedStatus(status));
            }

            let mut progress = DownloadProgress::new(label, asset.size);
            match write_asset_response(version, asset, temp_path, response, &mut progress).await {
                Ok(()) => {
                    progress.finish();
                    return Ok(());
                }
                Err(error) if error.is_retryable() && attempt < MAX_ATTEMPTS - 1 => {
                    progress.abandon_retrying();
                    let _ = tokio::fs::remove_file(temp_path).await;
                    self.sleep_backoff(attempt, &mut seed).await;
                }
                Err(error) => {
                    progress.abandon_failed();
                    let _ = tokio::fs::remove_file(temp_path).await;
                    return Err(error);
                }
            }
        }
        unreachable!("retry 循环必然返回")
    }

    async fn send_once(
        &self,
        url: Url,
        headers: HeaderMap,
        timeout: Duration,
    ) -> Result<Response, RepositoryError> {
        self.clients
            .pick(&url)
            .get(url)
            .headers(headers)
            .timeout(timeout)
            .send()
            .await
            .map_err(RepositoryError::Request)
    }

    async fn sleep_backoff(&self, attempt: usize, seed: &mut u64) {
        let delay = self.retry_delays[attempt] + self.jitter(seed);
        tokio::time::sleep(delay).await;
    }

    fn jitter(&self, seed: &mut u64) -> Duration {
        let mut x = *seed;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *seed = x;
        let max = self.jitter_max.as_millis().max(1) as u64;
        Duration::from_millis((x % max).max(1))
    }

    pub(crate) fn with_retry_timing(mut self, delays: [Duration; 2], jitter_max: Duration) -> Self {
        self.retry_delays = delays;
        self.jitter_max = jitter_max;
        self
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn validate_network_url(url: &Url) -> Result<(), RepositoryError> {
    if !url.username().is_empty() || url.password().is_some() {
        return Err(RepositoryError::UnsafeUrl(
            "URL must not contain a username or password".into(),
        ));
    }

    match url.scheme() {
        "https" => Ok(()),
        "http" if is_loopback_host(url.host_str()) => Ok(()),
        "http" => Err(RepositoryError::UnsafeUrl(
            "HTTP is only allowed for localhost or loopback addresses".into(),
        )),
        scheme => Err(RepositoryError::UnsafeUrl(format!(
            "unsupported URL scheme {scheme}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use reqwest::header::HeaderMap;
    use semver::Version;
    use sha2::{Digest, Sha256};
    use url::Url;

    use super::super::AssetEncoding;
    use super::*;

    fn head(status: u16, reason: &str, body_len: usize) -> String {
        format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Length: {body_len}\r\nConnection: close\r\n\r\n"
        )
    }

    fn head_with(status: u16, reason: &str, body_len: usize, extra: &str) -> String {
        format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Length: {body_len}\r\n{extra}Connection: close\r\n\r\n"
        )
    }

    fn start_server<F>(handler: F) -> (String, Arc<AtomicUsize>)
    where
        F: Fn(usize) -> (String, Vec<u8>) + Send + Sync + 'static,
    {
        let handler = Arc::new(handler);
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind 测试服务器");
        let addr = listener.local_addr().expect("读取测试服务器地址");
        let requests = Arc::new(AtomicUsize::new(0));
        let counter = requests.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut buffer = [0u8; 8192];
                if stream.read(&mut buffer).is_err() {
                    continue;
                }
                let count = counter.fetch_add(1, Ordering::SeqCst) + 1;
                let (head, body) = handler(count);
                if stream.write_all(head.as_bytes()).is_err() {
                    continue;
                }
                let _ = stream.write_all(&body);
            }
        });
        (format!("http://{addr}"), requests)
    }

    fn fast_client() -> DownloadClient {
        DownloadClient::new()
            .expect("构建客户端")
            .with_retry_timing(
                [Duration::from_millis(10), Duration::from_millis(10)],
                Duration::from_millis(1),
            )
    }

    #[test]
    fn routes_loopback_targets_to_the_direct_client() {
        let pair = ClientPair::new().expect("构建双通道客户端");
        let loopback_urls = [
            Url::parse("http://127.0.0.1:9000/repository/file.txt").unwrap(),
            Url::parse("http://localhost:9000/file.txt").unwrap(),
            Url::parse("http://[::1]:9000/file.txt").unwrap(),
        ];
        for url in &loopback_urls {
            assert!(
                std::ptr::eq(pair.pick(url), &pair.loopback),
                "{url} 应走回环直连通道"
            );
        }
        let external = Url::parse("https://github.com/ThisSeanZhang/landscape/releases")
            .expect("解析外部 URL");
        assert!(std::ptr::eq(pair.pick(&external), &pair.external));
    }

    #[tokio::test]
    async fn follows_redirects_within_the_same_boundary() {
        let (target, _) = start_server(|_| (head(200, "OK", 2), b"ok".to_vec()));
        let (base, _) = start_server(move |_| {
            (
                head_with(302, "Found", 0, &format!("Location: {target}/file\r\n")),
                Vec::new(),
            )
        });
        let client = fast_client();
        let url = Url::parse(&format!("{base}/start")).unwrap();
        let Some((_, body)) = client
            .get_metadata(url, HeaderMap::new(), false)
            .await
            .expect("同边界重定向应被跟随")
        else {
            panic!("元数据缺失");
        };
        assert_eq!(body, b"ok");
    }

    #[tokio::test]
    async fn refuses_redirects_crossing_the_loopback_boundary() {
        // 127.0.0.2 不在回环字面量集合内:若重定向被错误跟随,对它的连接会被
        // 立刻拒绝,得到 Request 错误而非 UnexpectedStatus,断言得以区分。
        let (base, requests) = start_server(|_| {
            (
                head_with(302, "Found", 0, "Location: http://127.0.0.2:1/file\r\n"),
                Vec::new(),
            )
        });
        let client = fast_client();
        let url = Url::parse(&format!("{base}/start")).unwrap();
        let error = client
            .get_metadata(url, HeaderMap::new(), false)
            .await
            .expect_err("跨界重定向应被拒绝");
        assert!(
            matches!(error, RepositoryError::UnexpectedStatus(status) if status == StatusCode::FOUND),
            "跨界重定向应按 3xx 原样返回,实际: {error:?}"
        );
        // 只有起始请求,重定向未被跟随。
        assert_eq!(requests.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn bounded_redirect_loop_returns_the_redirect_response() {
        let location = Arc::new(std::sync::Mutex::new(String::new()));
        let next = location.clone();
        let (base, requests) = start_server(move |_| {
            let target = next.lock().expect("读取重定向目标").clone();
            (
                head_with(302, "Found", 0, &format!("Location: {target}/hop\r\n")),
                Vec::new(),
            )
        });
        location
            .lock()
            .expect("写入重定向目标")
            .clone_from(&format!("{base}/hop"));
        let client = fast_client();
        let url = Url::parse(&format!("{base}/start")).unwrap();
        let error = client
            .get_metadata(url, HeaderMap::new(), false)
            .await
            .expect_err("重定向循环应有界结束");
        assert!(
            matches!(error, RepositoryError::UnexpectedStatus(status) if status == StatusCode::FOUND),
            "达到上限后应返回 3xx,实际: {error:?}"
        );
        assert!(
            requests.load(Ordering::SeqCst) <= MAX_REDIRECTS + 1,
            "重定向请求数应不超过上限+1,实际: {}",
            requests.load(Ordering::SeqCst)
        );
    }

    #[tokio::test]
    async fn retries_5xx_then_succeeds() {
        let (base, requests) = start_server(|count| {
            if count == 1 {
                (head(503, "Service Unavailable", 5), b"retry".to_vec())
            } else {
                (head(200, "OK", 2), b"ok".to_vec())
            }
        });
        let client = fast_client();
        let url = Url::parse(&format!("{base}/metadata.json")).unwrap();
        let Some((_, body)) = client
            .get_metadata(url, HeaderMap::new(), false)
            .await
            .unwrap()
        else {
            panic!("元数据缺失")
        };
        assert_eq!(body, b"ok");
        assert_eq!(requests.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn does_not_retry_4xx() {
        let (base, requests) = start_server(|_| (head(404, "Not Found", 7), b"missing".to_vec()));
        let client = fast_client();
        let url = Url::parse(&format!("{base}/missing.json")).unwrap();
        let error = client
            .get_metadata(url, HeaderMap::new(), false)
            .await
            .unwrap_err();
        assert!(matches!(error, RepositoryError::UnexpectedStatus(_)));
        assert_eq!(requests.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn allows_missing_metadata() {
        let (base, requests) = start_server(|_| (head(404, "Not Found", 7), b"missing".to_vec()));
        let client = fast_client();
        let url = Url::parse(&format!("{base}/missing.json")).unwrap();
        let result = client
            .get_metadata(url, HeaderMap::new(), true)
            .await
            .unwrap();
        assert!(result.is_none());
        assert_eq!(requests.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn rejects_rate_limited_with_long_retry_after() {
        let (base, requests) = start_server(|_| {
            let body = b"limited".to_vec();
            (
                head_with(429, "Too Many Requests", body.len(), "Retry-After: 120\r\n"),
                body,
            )
        });
        let client = fast_client();
        let url = Url::parse(&format!("{base}/limited")).unwrap();
        let error = client
            .get_metadata(url, HeaderMap::new(), false)
            .await
            .unwrap_err();
        assert!(matches!(error, RepositoryError::RateLimited(120)));
        assert_eq!(requests.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn waits_short_retry_after_then_succeeds() {
        let (base, requests) = start_server(|count| {
            let body = if count == 1 {
                b"limited".to_vec()
            } else {
                b"ok".to_vec()
            };
            if count == 1 {
                (
                    head_with(429, "Too Many Requests", body.len(), "Retry-After: 0\r\n"),
                    body,
                )
            } else {
                (head(200, "OK", body.len()), body)
            }
        });
        let client = fast_client();
        let url = Url::parse(&format!("{base}/limited")).unwrap();
        let Some((_, body)) = client
            .get_metadata(url, HeaderMap::new(), false)
            .await
            .unwrap()
        else {
            panic!("元数据缺失")
        };
        assert_eq!(body, b"ok");
        assert_eq!(requests.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn downloads_asset_and_verifies() {
        let payload = b"landscape-webserver payload".to_vec();
        let sha256 = hex(&Sha256::digest(&payload));
        let payload_for_server = payload.clone();
        let (base, requests) = start_server(move |count| {
            if count == 1 {
                (head(503, "Service Unavailable", 5), b"retry".to_vec())
            } else {
                (
                    head(200, "OK", payload_for_server.len()),
                    payload_for_server.clone(),
                )
            }
        });
        let client = fast_client();
        let asset = Asset::checked(
            Url::parse(&format!("{base}/landscape-webserver-x86_64.zst")).unwrap(),
            sha256,
            payload.len() as u64,
            AssetEncoding::Identity,
        )
        .unwrap();
        let version = Version::parse("0.19.2").unwrap();
        let temp = std::env::temp_dir().join("lkit-download-test.zst");
        client
            .download_asset(&version, &asset, "test asset", &temp)
            .await
            .unwrap();
        assert_eq!(std::fs::read(&temp).unwrap(), payload);
        assert_eq!(requests.load(Ordering::SeqCst), 2);
        let _ = std::fs::remove_file(&temp);
    }

    #[tokio::test]
    async fn rejects_size_mismatch() {
        let payload = b"actual-size-does-not-match".to_vec();
        let payload_for_server = payload.clone();
        let (base, _) = start_server(move |_| {
            (
                head(200, "OK", payload_for_server.len()),
                payload_for_server.clone(),
            )
        });
        let client = fast_client();
        let asset = Asset::checked(
            Url::parse(&format!("{base}/asset")).unwrap(),
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
            3,
            AssetEncoding::Identity,
        )
        .unwrap();
        let version = Version::parse("0.19.2").unwrap();
        let temp = std::env::temp_dir().join("lkit-download-size.bin");
        let error = client
            .download_asset(&version, &asset, "test asset", &temp)
            .await
            .unwrap_err();
        assert!(matches!(error, RepositoryError::AssetSizeMismatch { .. }));
        assert!(!temp.exists());
    }

    #[tokio::test]
    async fn rejects_sha256_mismatch_and_removes_temp() {
        let payload = b"payload".to_vec();
        let payload_for_server = payload.clone();
        let (base, _) = start_server(move |_| {
            (
                head(200, "OK", payload_for_server.len()),
                payload_for_server.clone(),
            )
        });
        let client = fast_client();
        let asset = Asset::checked(
            Url::parse(&format!("{base}/asset")).unwrap(),
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string(),
            payload.len() as u64,
            AssetEncoding::Identity,
        )
        .unwrap();
        let version = Version::parse("0.19.2").unwrap();
        let temp = std::env::temp_dir().join("lkit-download-test.bin");
        let error = client
            .download_asset(&version, &asset, "test asset", &temp)
            .await
            .unwrap_err();
        assert!(matches!(error, RepositoryError::AssetSha256Mismatch { .. }));
        assert!(!temp.exists());
    }
}
