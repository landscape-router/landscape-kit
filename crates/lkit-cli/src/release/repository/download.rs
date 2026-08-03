use std::error::Error;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::StreamExt;
use reqwest::header::{HeaderMap, RETRY_AFTER};
use reqwest::redirect::Policy;
use reqwest::{Client, RequestBuilder, Response, StatusCode};
use semver::Version;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use url::Url;

#[cfg(test)]
use super::AssetEncoding;
pub(crate) use super::archive::{MAX_DECOMPRESSED_BYTES, decompress_zstd, extract_static_archive};
use super::{Asset, RepositoryError};

pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const METADATA_TIMEOUT: Duration = Duration::from_secs(60);
pub(crate) const ASSET_TIMEOUT: Duration = Duration::from_secs(30 * 60);
pub(crate) const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const METADATA_BODY_LIMIT: u64 = 10 * 1024 * 1024;
pub(crate) const MAX_ATTEMPTS: usize = 3;
pub(crate) const MAX_REDIRECTS: usize = 5;
const RETRY_AFTER_LIMIT: u64 = 60;

#[derive(Debug)]
pub(crate) struct DownloadClient {
    client: Client,
    retry_delays: [Duration; 2],
    jitter_max: Duration,
}

impl DownloadClient {
    pub(crate) fn new() -> Result<Self, RepositoryError> {
        let client = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .redirect(Policy::custom(|attempt| {
                if attempt.previous().len() >= MAX_REDIRECTS {
                    return attempt.stop();
                }
                match validate_network_url(attempt.url()) {
                    Ok(()) => attempt.follow(),
                    Err(_) => attempt.stop(),
                }
            }))
            .build()
            .map_err(RepositoryError::BuildClient)?;
        Ok(Self {
            client,
            retry_delays: [Duration::from_secs(1), Duration::from_secs(2)],
            jitter_max: Duration::from_millis(250),
        })
    }

    pub(crate) fn request(&self, url: Url) -> RequestBuilder {
        self.client.get(url)
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

            match write_asset_response(version, asset, temp_path, response).await {
                Ok(()) => return Ok(()),
                Err(error) if error.is_retryable() && attempt < MAX_ATTEMPTS - 1 => {
                    let _ = tokio::fs::remove_file(temp_path).await;
                    self.sleep_backoff(attempt, &mut seed).await;
                }
                Err(error) => {
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
        self.client
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

async fn write_asset_response(
    version: &Version,
    asset: &Asset,
    temp_path: &Path,
    response: Response,
) -> Result<(), RepositoryError> {
    let mut file = tokio::fs::File::create(temp_path)
        .await
        .map_err(RepositoryError::Io)?;
    let mut hasher = Sha256::new();
    let mut written: u64 = 0;
    let mut stream = response.bytes_stream();
    loop {
        let chunk = tokio::time::timeout(IDLE_TIMEOUT, stream.next())
            .await
            .map_err(|_| {
                RepositoryError::Timeout(
                    "no data received for 30 seconds while downloading asset".into(),
                )
            })?
            .transpose()
            .map_err(RepositoryError::Request)?;
        let Some(chunk) = chunk else { break };
        written = written.saturating_add(chunk.len() as u64);
        if written > asset.size {
            return Err(RepositoryError::AssetSizeMismatch {
                version: version.clone(),
                expected: asset.size,
                actual: written,
            });
        }
        hasher.update(&chunk);
        file.write_all(&chunk).await.map_err(RepositoryError::Io)?;
    }
    if written != asset.size {
        return Err(RepositoryError::AssetSizeMismatch {
            version: version.clone(),
            expected: asset.size,
            actual: written,
        });
    }
    let actual = hex(&hasher.finalize());
    if actual != asset.sha256 {
        return Err(RepositoryError::AssetSha256Mismatch {
            version: version.clone(),
            expected: asset.sha256.clone(),
            actual,
        });
    }
    file.sync_all().await.map_err(RepositoryError::Io)?;
    Ok(())
}

async fn read_body_limited(response: Response, limit: u64) -> Result<Vec<u8>, RepositoryError> {
    if response
        .content_length()
        .is_some_and(|length| length > limit)
    {
        return Err(RepositoryError::MetadataTooLarge);
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(RepositoryError::Request)?;
        let new_length = body.len().saturating_add(chunk.len());
        if new_length as u64 > limit {
            return Err(RepositoryError::MetadataTooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn retryable_status(status: StatusCode) -> bool {
    status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
}

/// 返回限速建议等待秒数。优先读取 `Retry-After`（整数秒），
/// 其次在 `403`/`429` 时读取 `X-RateLimit-Reset`（Unix 时间戳）。
fn rate_limit_wait(status: StatusCode, headers: &HeaderMap) -> Option<u64> {
    if let Some(value) = headers.get(RETRY_AFTER)
        && let Ok(text) = value.to_str()
        && let Ok(seconds) = text.trim().parse::<u64>()
    {
        return Some(seconds);
    }
    if (status == StatusCode::TOO_MANY_REQUESTS || status == StatusCode::FORBIDDEN)
        && let Some(reset) = headers
            .get("x-ratelimit-reset")
            .and_then(|value| value.to_str().ok())
            .and_then(|text| text.trim().parse::<u64>().ok())
    {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        return Some(reset.saturating_sub(now));
    }
    None
}

pub(crate) fn is_retryable_request_error(error: &reqwest::Error) -> bool {
    if error.is_timeout() || error.is_connect() {
        return true;
    }
    let mut source = error.source();
    while let Some(cause) = source {
        if let Some(io_error) = cause.downcast_ref::<std::io::Error>()
            && retryable_io_kind(io_error.kind())
        {
            return true;
        }
        source = cause.source();
    }
    false
}

fn retryable_io_kind(kind: std::io::ErrorKind) -> bool {
    matches!(
        kind,
        std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::ConnectionRefused
            | std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::UnexpectedEof
            | std::io::ErrorKind::AddrNotAvailable
            | std::io::ErrorKind::NetworkUnreachable
            | std::io::ErrorKind::HostUnreachable
    )
}

fn jitter_seed() -> u64 {
    let base = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    (base as u64) ^ ((base >> 32) as u64) ^ 0x9e37_79b9_7f4a_7c15
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

fn is_loopback_host(host: Option<&str>) -> bool {
    matches!(host, Some("localhost" | "127.0.0.1" | "::1"))
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read, Write};

    use semver::Version;
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

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
            .download_asset(&version, &asset, &temp)
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
            .download_asset(&version, &asset, &temp)
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
            .download_asset(&version, &asset, &temp)
            .await
            .unwrap_err();
        assert!(matches!(error, RepositoryError::AssetSha256Mismatch { .. }));
        assert!(!temp.exists());
    }

    #[test]
    fn decompresses_zstd_and_rejects_trailing_data() {
        let version = Version::parse("0.19.2").unwrap();
        let compressed = std::env::temp_dir().join("lkit-decompress-test.zst");
        let output = std::env::temp_dir().join("lkit-decompress-test.bin");
        let data = b"landscape-webserver".repeat(1024);
        let mut encoded = zstd::stream::encode_all(Cursor::new(&data), 1).unwrap();
        std::fs::write(&compressed, &encoded).unwrap();
        let written = decompress_zstd(&version, &compressed, &output, 1 << 30).unwrap();
        assert_eq!(written as usize, data.len());
        assert_eq!(std::fs::read(&output).unwrap(), data);

        encoded.extend_from_slice(b"trailing garbage");
        std::fs::write(&compressed, &encoded).unwrap();
        let error = decompress_zstd(&version, &compressed, &output, 1 << 30).unwrap_err();
        assert!(matches!(error, RepositoryError::Decompress { .. }));
        let _ = std::fs::remove_file(&compressed);
        let _ = std::fs::remove_file(&output);
    }

    #[test]
    fn rejects_decompress_bomb() {
        let version = Version::parse("0.19.2").unwrap();
        let compressed = std::env::temp_dir().join("lkit-decompress-bomb.zst");
        let output = std::env::temp_dir().join("lkit-decompress-bomb.bin");
        let data = vec![0xABu8; 1024 * 1024];
        let encoded = zstd::stream::encode_all(Cursor::new(&data), 1).unwrap();
        std::fs::write(&compressed, &encoded).unwrap();
        let error = decompress_zstd(&version, &compressed, &output, 4096).unwrap_err();
        assert!(matches!(error, RepositoryError::Decompress { .. }));
        let _ = std::fs::remove_file(&compressed);
        let _ = std::fs::remove_file(&output);
    }

    fn make_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, content) in entries {
            if content.is_empty() {
                writer
                    .add_directory(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
            } else {
                writer
                    .start_file(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                writer.write_all(content).unwrap();
            }
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn extracts_static_archive() {
        let version = Version::parse("0.19.2").unwrap();
        let archive = std::env::temp_dir().join("lkit-static-test.zip");
        let target = std::env::temp_dir().join("lkit-static-test-out");
        let _ = std::fs::remove_dir_all(&target);
        let zip = make_zip(&[
            ("static/", b""),
            ("static/index.html", b"<html></html>"),
            ("static/assets/app.js", b"console.log(1)"),
        ]);
        std::fs::write(&archive, &zip).unwrap();
        extract_static_archive(&version, &archive, zip.len() as u64, &target).unwrap();
        assert_eq!(
            std::fs::read(target.join("index.html")).unwrap(),
            b"<html></html>"
        );
        assert_eq!(
            std::fs::read(target.join("assets/app.js")).unwrap(),
            b"console.log(1)"
        );
        let _ = std::fs::remove_dir_all(&target);
        let _ = std::fs::remove_file(&archive);
    }

    #[test]
    fn rejects_zip_path_traversal() {
        let version = Version::parse("0.19.2").unwrap();
        let archive = std::env::temp_dir().join("lkit-static-evil.zip");
        let target = std::env::temp_dir().join("lkit-static-evil-out");
        let _ = std::fs::remove_dir_all(&target);
        let zip = make_zip(&[
            ("static/", b""),
            ("static/../evil", b"evil"),
            ("static/index.html", b"<html></html>"),
        ]);
        std::fs::write(&archive, &zip).unwrap();
        let error =
            extract_static_archive(&version, &archive, zip.len() as u64, &target).unwrap_err();
        assert!(matches!(error, RepositoryError::Extract { .. }));
        assert!(!target.join("evil").exists());
        let _ = std::fs::remove_dir_all(&target);
        let _ = std::fs::remove_file(&archive);
    }

    #[test]
    fn rejects_zip_without_static_prefix() {
        let version = Version::parse("0.19.2").unwrap();
        let archive = std::env::temp_dir().join("lkit-static-prefix.zip");
        let target = std::env::temp_dir().join("lkit-static-prefix-out");
        let _ = std::fs::remove_dir_all(&target);
        let zip = make_zip(&[("index.html", b"<html></html>")]);
        std::fs::write(&archive, &zip).unwrap();
        let error =
            extract_static_archive(&version, &archive, zip.len() as u64, &target).unwrap_err();
        assert!(matches!(error, RepositoryError::Extract { .. }));
        let _ = std::fs::remove_dir_all(&target);
        let _ = std::fs::remove_file(&archive);
    }

    #[test]
    fn rejects_zip_without_index_html() {
        let version = Version::parse("0.19.2").unwrap();
        let archive = std::env::temp_dir().join("lkit-static-noindex.zip");
        let target = std::env::temp_dir().join("lkit-static-noindex-out");
        let _ = std::fs::remove_dir_all(&target);
        let zip = make_zip(&[("static/", b""), ("static/assets/app.js", b"x")]);
        std::fs::write(&archive, &zip).unwrap();
        let error =
            extract_static_archive(&version, &archive, zip.len() as u64, &target).unwrap_err();
        assert!(matches!(error, RepositoryError::Extract { .. }));
        let _ = std::fs::remove_dir_all(&target);
        let _ = std::fs::remove_file(&archive);
    }

    #[test]
    fn rejects_existing_target_directory() {
        let version = Version::parse("0.19.2").unwrap();
        let archive = std::env::temp_dir().join("lkit-static-existing.zip");
        let target = std::env::temp_dir().join("lkit-static-existing-out");
        let _ = std::fs::remove_dir_all(&target);
        std::fs::create_dir(&target).unwrap();
        let zip = make_zip(&[("static/index.html", b"<html></html>")]);
        std::fs::write(&archive, &zip).unwrap();
        let error =
            extract_static_archive(&version, &archive, zip.len() as u64, &target).unwrap_err();
        assert!(matches!(error, RepositoryError::Extract { .. }));
        let _ = std::fs::remove_dir_all(&target);
        let _ = std::fs::remove_file(&archive);
    }

    #[test]
    fn rejects_duplicate_normalized_zip_paths() {
        let version = Version::parse("0.19.2").unwrap();
        let archive = std::env::temp_dir().join("lkit-static-duplicate.zip");
        let target = std::env::temp_dir().join("lkit-static-duplicate-out");
        let _ = std::fs::remove_dir_all(&target);
        let zip = make_zip(&[("static/assets/", b""), ("static/assets", b"second")]);
        std::fs::write(&archive, &zip).unwrap();
        let error =
            extract_static_archive(&version, &archive, zip.len() as u64, &target).unwrap_err();
        assert!(matches!(error, RepositoryError::Extract { .. }));
        let _ = std::fs::remove_dir_all(&target);
        let _ = std::fs::remove_file(&archive);
    }

    #[test]
    fn rejects_dot_zip_path_component() {
        let version = Version::parse("0.19.2").unwrap();
        let archive = std::env::temp_dir().join("lkit-static-dot.zip");
        let target = std::env::temp_dir().join("lkit-static-dot-out");
        let _ = std::fs::remove_dir_all(&target);
        let zip = make_zip(&[("static/./index.html", b"<html></html>")]);
        std::fs::write(&archive, &zip).unwrap();
        let error =
            extract_static_archive(&version, &archive, zip.len() as u64, &target).unwrap_err();
        assert!(matches!(error, RepositoryError::Extract { .. }));
        let _ = std::fs::remove_dir_all(&target);
        let _ = std::fs::remove_file(&archive);
    }

    #[test]
    fn rejects_zip_symlink() {
        let version = Version::parse("0.19.2").unwrap();
        let archive = std::env::temp_dir().join("lkit-static-symlink.zip");
        let target = std::env::temp_dir().join("lkit-static-symlink-out");
        let _ = std::fs::remove_dir_all(&target);
        let zip = build_raw_zip(&[
            ("static/", b"", 0o040755u32 << 16),
            ("static/index.html", b"<html></html>", 0o100644u32 << 16),
            ("static/link", b"index.html", 0o120777u32 << 16),
        ]);
        std::fs::write(&archive, &zip).unwrap();
        let error =
            extract_static_archive(&version, &archive, zip.len() as u64, &target).unwrap_err();
        assert!(matches!(error, RepositoryError::Extract { .. }));
        assert!(!target.join("link").exists());
        let _ = std::fs::remove_dir_all(&target);
        let _ = std::fs::remove_file(&archive);
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc: u32 = 0xFFFF_FFFF;
        for byte in data {
            crc ^= *byte as u32;
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    /// 手工构造 Store 模式 zip，允许写入带 Unix 文件类型位的条目。
    fn build_raw_zip(entries: &[(&str, &[u8], u32)]) -> Vec<u8> {
        let mut locals = Vec::new();
        let mut central = Vec::new();
        let mut offset = 0u32;
        for (name, data, external) in entries {
            let crc = crc32(data);
            locals.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
            locals.extend_from_slice(&20u16.to_le_bytes());
            locals.extend_from_slice(&0u16.to_le_bytes());
            locals.extend_from_slice(&0u16.to_le_bytes());
            locals.extend_from_slice(&0u16.to_le_bytes());
            locals.extend_from_slice(&0x21u16.to_le_bytes());
            locals.extend_from_slice(&crc.to_le_bytes());
            locals.extend_from_slice(&(data.len() as u32).to_le_bytes());
            locals.extend_from_slice(&(data.len() as u32).to_le_bytes());
            locals.extend_from_slice(&(name.len() as u16).to_le_bytes());
            locals.extend_from_slice(&0u16.to_le_bytes());
            locals.extend_from_slice(name.as_bytes());
            locals.extend_from_slice(data);

            central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            central.extend_from_slice(&(20u16 | (3u16 << 8)).to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0x21u16.to_le_bytes());
            central.extend_from_slice(&crc.to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&external.to_le_bytes());
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name.as_bytes());

            offset += 30 + name.len() as u32 + data.len() as u32;
        }
        let central_offset = offset;
        let central_len = central.len() as u32;
        let count = entries.len() as u16;
        let mut out = locals;
        out.append(&mut central);
        out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&central_len.to_le_bytes());
        out.extend_from_slice(&central_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }
}
