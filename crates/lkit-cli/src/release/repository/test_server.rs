use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

/// 本地回环 HTTP 测试服务器：绑定 `127.0.0.1` 随机端口，
/// 记录每个请求的路径（含 query），由 handler 决定响应。
pub(crate) struct TestServer {
    pub(crate) base: String,
    requests: Arc<Mutex<Vec<String>>>,
}

pub(crate) struct TestResponse {
    pub(crate) status: u16,
    pub(crate) reason: String,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: Vec<u8>,
}

impl TestResponse {
    pub(crate) fn ok(body: Vec<u8>) -> Self {
        Self {
            status: 200,
            reason: "OK".into(),
            headers: Vec::new(),
            body,
        }
    }

    pub(crate) fn status(status: u16, reason: &str, body: Vec<u8>) -> Self {
        Self {
            status,
            reason: reason.into(),
            headers: Vec::new(),
            body,
        }
    }

    pub(crate) fn redirect(status: u16, location: &str) -> Self {
        Self {
            status,
            reason: "Redirect".into(),
            headers: vec![("Location".into(), location.into())],
            body: Vec::new(),
        }
    }

    /// 完全控制响应头和响应体，不自动补充 Content-Length。
    /// 用于模拟截断响应、声明大小与实际不符等异常。
    pub(crate) fn raw(
        status: u16,
        reason: &str,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    ) -> Self {
        Self {
            status,
            reason: reason.into(),
            headers,
            body,
        }
    }
}

impl TestServer {
    pub(crate) fn start<F>(handler: F) -> Self
    where
        F: Fn(&str) -> TestResponse + Send + Sync + 'static,
    {
        let handler = Arc::new(handler);
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind 测试服务器");
        let addr = listener.local_addr().expect("读取测试服务器地址");
        let requests: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let log = requests.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut buffer = [0u8; 8192];
                if stream.read(&mut buffer).is_err() {
                    continue;
                }
                let request = String::from_utf8_lossy(&buffer);
                let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
                if let Ok(mut log) = log.lock() {
                    log.push(path.clone());
                }
                let response = handler(&path);
                let mut header_block = String::new();
                let mut has_content_length = false;
                for (name, value) in &response.headers {
                    if name.eq_ignore_ascii_case("content-length") {
                        has_content_length = true;
                    }
                    header_block.push_str(&format!("{name}: {value}\r\n"));
                }
                if !has_content_length {
                    header_block.push_str(&format!("Content-Length: {}\r\n", response.body.len()));
                }
                let head = format!(
                    "HTTP/1.1 {} {}\r\n{header_block}Connection: close\r\n\r\n",
                    response.status, response.reason
                );
                if stream.write_all(head.as_bytes()).is_err() {
                    continue;
                }
                if stream.write_all(&response.body).is_err() {
                    continue;
                }
                let _ = stream.flush();
            }
        });
        Self {
            base: format!("http://{addr}"),
            requests,
        }
    }

    pub(crate) fn request_paths(&self) -> Vec<String> {
        self.requests.lock().expect("读取请求日志").clone()
    }

    pub(crate) fn request_count(&self) -> usize {
        self.requests.lock().expect("读取请求日志").len()
    }
}
