use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

use super::{LANDSCAPE_FIXTURE, VERSION};

pub(crate) struct RepositoryServer {
    pub(crate) base_url: String,
    requests: Arc<Mutex<Vec<String>>>,
    files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    shutdown: Arc<AtomicBool>,
    active_stream: Arc<Mutex<Option<TcpStream>>>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl RepositoryServer {
    pub(crate) fn start(files: HashMap<String, Vec<u8>>) -> Self {
        Self::start_gated(files, None)
    }

    pub(crate) fn start_gated(
        files: HashMap<String, Vec<u8>>,
        gate: Option<(String, Arc<std::sync::atomic::AtomicBool>)>,
    ) -> Self {
        let files = Arc::new(Mutex::new(files));
        let response_files = files.clone();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let request_log = requests.clone();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let shutdown = Arc::new(AtomicBool::new(false));
        let worker_shutdown = shutdown.clone();
        let active_stream = Arc::new(Mutex::new(None));
        let worker_stream = active_stream.clone();
        let worker = std::thread::spawn(move || {
            while !worker_shutdown.load(Ordering::SeqCst) {
                let mut stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                        continue;
                    }
                    Err(_) => break,
                };
                *worker_stream.lock().unwrap() = Some(stream.try_clone().unwrap());
                if worker_shutdown.load(Ordering::SeqCst) {
                    break;
                }
                // Keep cleanup outside the request body so early returns release
                // the extra socket as well as the accepted connection.
                let mut handle_request = || {
                    let mut request = [0u8; 8192];
                    let Ok(size) = stream.read(&mut request) else {
                        return;
                    };
                    let request = String::from_utf8_lossy(&request[..size]);
                    let path = request.split_whitespace().nth(1).unwrap_or("/");
                    request_log.lock().unwrap().push(path.to_string());
                    if let Some((suffix, released)) = &gate
                        && path.ends_with(suffix)
                    {
                        let deadline =
                            std::time::Instant::now() + std::time::Duration::from_secs(30);
                        while !released.load(std::sync::atomic::Ordering::SeqCst)
                            && !worker_shutdown.load(Ordering::SeqCst)
                            && std::time::Instant::now() < deadline
                        {
                            std::thread::sleep(std::time::Duration::from_millis(20));
                        }
                    }
                    if worker_shutdown.load(Ordering::SeqCst) {
                        return;
                    }
                    let (status, reason, body) = match response_files.lock().unwrap().get(path) {
                        Some(body) => (200, "OK", body.clone()),
                        None => (404, "Not Found", Vec::new()),
                    };
                    let head = format!(
                        "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    if stream.write_all(head.as_bytes()).is_ok() {
                        let _ = stream.write_all(&body);
                        let _ = stream.flush();
                    }
                };
                handle_request();
                *worker_stream.lock().unwrap() = None;
            }
        });
        Self {
            base_url: format!("http://{address}/"),
            requests,
            files,
            shutdown,
            active_stream,
            worker: Some(worker),
        }
    }

    pub(crate) fn replace_file(&self, path: &str, bytes: Vec<u8>) {
        self.files.lock().unwrap().insert(path.into(), bytes);
    }

    pub(crate) fn request_paths(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }

    pub(crate) fn has_active_connection(&self) -> bool {
        self.active_stream.lock().unwrap().is_some()
    }
}

impl Drop for RepositoryServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Ok(stream) = self.active_stream.lock()
            && let Some(stream) = stream.as_ref()
        {
            let _ = stream.shutdown(Shutdown::Both);
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// 本地 GitHub Release 形状的双服务器 fixture:self upgrade 的 API 根和资产
/// 下载根分别可注入,避免测试访问真实 GitHub。
pub(crate) struct SelfUpgradeFixture {
    pub(crate) api: RepositoryServer,
    pub(crate) downloads: RepositoryServer,
}

impl SelfUpgradeFixture {
    pub(crate) fn start(
        tag: &str,
        asset_name: &str,
        asset: Vec<u8>,
        checksum_override: Option<&str>,
    ) -> Self {
        let (digest, size) = sha256(&asset);
        let checksum = checksum_override.unwrap_or(&digest);
        let downloads = RepositoryServer::start(HashMap::from([
            (
                format!("/{tag}/SHA256SUMS"),
                format!("{checksum}  {asset_name}\n").into_bytes(),
            ),
            (format!("/{tag}/{asset_name}"), asset),
        ]));
        let prerelease = !semver::Version::parse(tag.strip_prefix('v').unwrap_or(tag))
            .unwrap()
            .pre
            .is_empty();
        let release = serde_json::json!({
            "tag_name": tag,
            "draft": false,
            "prerelease": prerelease,
            "assets": [{
                "name": asset_name,
                "size": size,
                "browser_download_url": format!("{}{tag}/{asset_name}", downloads.base_url),
            }],
        });
        let root = "/repos/landscape-router/landscape-kit/releases";
        let mut files = HashMap::from([
            (
                format!("{root}/tags/{tag}"),
                serde_json::to_vec(&release).unwrap(),
            ),
            (
                format!("{root}?per_page=100&page=1"),
                serde_json::to_vec(&vec![&release]).unwrap(),
            ),
        ]);
        if !prerelease {
            files.insert(
                format!("{root}/latest"),
                serde_json::to_vec(&release).unwrap(),
            );
        }
        let api = RepositoryServer::start(files);
        Self { api, downloads }
    }
}

pub(crate) fn repository_files() -> HashMap<String, Vec<u8>> {
    repository_files_for(VERSION)
}

/// 自定义前端源的静态-only 仓库:stable 通道 + `webserver` 空对象 manifest +
/// 自定义 `static.zip`。版本可独立于后端版本,解析协议见
/// `docs/frontend/developer.md`。
pub(crate) fn frontend_files_for(version: &str, html: &str) -> HashMap<String, Vec<u8>> {
    let static_zip = static_zip_for(html);
    let (static_sha, static_size) = sha256(&static_zip);
    let manifest = serde_json::json!({
        "protocol_version": 1,
        "version": version,
        "assets": {
            "webserver": {},
            "static": {
                "url": "static.zip",
                "sha256": static_sha,
                "size": static_size,
            }
        }
    });
    HashMap::from([
        (
            "/repository.json".into(),
            br#"{"protocol_version":1}"#.to_vec(),
        ),
        (
            "/channels/stable.json".into(),
            format!(r#"{{"protocol_version":1,"version":"{version}"}}"#).into_bytes(),
        ),
        (
            format!("/releases/{version}/manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        ),
        (format!("/releases/{version}/static.zip"), static_zip),
    ])
}

pub(crate) fn repository_files_for(version: &str) -> HashMap<String, Vec<u8>> {
    let executable = std::fs::read(LANDSCAPE_FIXTURE).unwrap();
    let compressed = zstd::encode_all(executable.as_slice(), 3).unwrap();
    let static_zip = static_zip();
    let (webserver_sha, webserver_size) = sha256(&compressed);
    let (static_sha, static_size) = sha256(&static_zip);
    let architecture = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        architecture => panic!("unsupported test architecture {architecture}"),
    };
    let asset_name = format!("landscape-webserver-{architecture}.zst");
    let manifest = serde_json::json!({
        "protocol_version": 1,
        "version": version,
        "assets": {
            "webserver": {
                architecture: {
                    "url": asset_name,
                    "sha256": webserver_sha,
                    "size": webserver_size,
                }
            },
            "static": {
                "url": "static.zip",
                "sha256": static_sha,
                "size": static_size,
            }
        }
    });
    HashMap::from([
        (
            "/repository.json".into(),
            br#"{"protocol_version":1}"#.to_vec(),
        ),
        (
            "/channels/stable.json".into(),
            format!(r#"{{"protocol_version":1,"version":"{version}"}}"#).into_bytes(),
        ),
        (
            format!("/releases/{version}/manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        ),
        (format!("/releases/{version}/{asset_name}"), compressed),
        (format!("/releases/{version}/static.zip"), static_zip),
    ])
}

fn static_zip() -> Vec<u8> {
    static_zip_for("<h1>Landscape fixture</h1>")
}

pub(crate) fn static_zip_for(html: &str) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    writer.start_file("static/index.html", options).unwrap();
    writer.write_all(html.as_bytes()).unwrap();
    writer.finish().unwrap().into_inner()
}

pub(crate) fn sha256(bytes: &[u8]) -> (String, u64) {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    (
        digest.iter().map(|byte| format!("{byte:02x}")).collect(),
        bytes.len() as u64,
    )
}
