use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};

use super::{LANDSCAPE_FIXTURE, VERSION};

pub(crate) struct RepositoryServer {
    pub(crate) base_url: String,
    requests: Arc<Mutex<Vec<String>>>,
}

impl RepositoryServer {
    pub(crate) fn start(files: HashMap<String, Vec<u8>>) -> Self {
        let files = Arc::new(files);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let request_log = requests.clone();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut request = [0u8; 8192];
                let Ok(size) = stream.read(&mut request) else {
                    continue;
                };
                let request = String::from_utf8_lossy(&request[..size]);
                let path = request.split_whitespace().nth(1).unwrap_or("/");
                request_log.lock().unwrap().push(path.to_string());
                let (status, reason, body) = match files.get(path) {
                    Some(body) => (200, "OK", body.as_slice()),
                    None => (404, "Not Found", &[][..]),
                };
                let head = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                if stream.write_all(head.as_bytes()).is_ok() {
                    let _ = stream.write_all(body);
                    let _ = stream.flush();
                }
            }
        });
        Self {
            base_url: format!("http://{address}/"),
            requests,
        }
    }

    pub(crate) fn request_paths(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

pub(crate) fn repository_files() -> HashMap<String, Vec<u8>> {
    repository_files_for(VERSION)
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
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    writer.start_file("static/index.html", options).unwrap();
    writer.write_all(b"<h1>Landscape fixture</h1>").unwrap();
    writer.finish().unwrap().into_inner()
}

fn sha256(bytes: &[u8]) -> (String, u64) {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    (
        digest.iter().map(|byte| format!("{byte:02x}")).collect(),
        bytes.len() as u64,
    )
}
