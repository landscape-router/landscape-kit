use std::error::Error;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::StatusCode;
use reqwest::header::{HeaderMap, RETRY_AFTER};

pub(super) const RETRY_AFTER_LIMIT: u64 = 60;

pub(super) fn retryable_status(status: StatusCode) -> bool {
    status == StatusCode::REQUEST_TIMEOUT
        || status == StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
}

/// 返回限速建议等待秒数。优先读取 `Retry-After`（整数秒），
/// 其次在 `403`/`429` 时读取 `X-RateLimit-Reset`（Unix 时间戳）。
pub(super) fn rate_limit_wait(status: StatusCode, headers: &HeaderMap) -> Option<u64> {
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

pub(super) fn jitter_seed() -> u64 {
    let base = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    (base as u64) ^ ((base >> 32) as u64) ^ 0x9e37_79b9_7f4a_7c15
}
