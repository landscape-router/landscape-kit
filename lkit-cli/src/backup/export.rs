use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::time::Duration;

use lkit_repository::parse_stable_version;
use reqwest::header::ACCEPT;
use serde_json::Value;

use super::plan::InstallError;

pub(crate) const EXPORT_PATH: &str = "/api/v1/system/config/export";
pub(crate) const MAX_TOKEN_BYTES: u64 = 1024 * 1024;

pub(crate) struct ExportResult {
    pub version: String,
    pub content: String,
}

pub(crate) fn read_api_token(path: &Path, required_uid: u32) -> Result<String, InstallError> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| {
            InstallError::ExportFailed(format!("{} is not a readable regular file", path.display()))
        })?;
    let metadata = file.metadata().map_err(InstallError::Io)?;
    if !metadata.is_file() {
        return Err(InstallError::ExportFailed(format!(
            "{} is not a regular file",
            path.display()
        )));
    }
    if metadata.uid() != required_uid {
        return Err(InstallError::ExportFailed(format!(
            "{} must be owned by uid {required_uid}",
            path.display()
        )));
    }
    let mode = metadata.mode() & 0o777;
    if mode & 0o377 != 0 || mode & 0o300 != 0 {
        return Err(InstallError::ExportFailed(format!(
            "{} must not be broader than 0400",
            path.display()
        )));
    }
    let size = metadata.len();
    if size == 0 || size > MAX_TOKEN_BYTES {
        return Err(InstallError::ExportFailed(format!(
            "{} must be between 1 byte and 1 MiB",
            path.display()
        )));
    }
    let mut content = String::new();
    file.take(MAX_TOKEN_BYTES + 1)
        .read_to_string(&mut content)
        .map_err(|_| {
            InstallError::ExportFailed(format!("{} is not valid UTF-8", path.display()))
        })?;
    if content.ends_with("\r\n") {
        content.truncate(content.len() - 2);
    } else if content.ends_with('\n') {
        content.truncate(content.len() - 1);
    }
    if content.is_empty() {
        return Err(InstallError::ExportFailed(format!(
            "{} is empty",
            path.display()
        )));
    }
    if content.chars().any(char::is_control) {
        return Err(InstallError::ExportFailed(format!(
            "{} must not contain control characters",
            path.display()
        )));
    }
    Ok(content)
}

pub(crate) async fn export_config(
    base_url: &str,
    token: &str,
) -> Result<ExportResult, InstallError> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(60))
        .danger_accept_invalid_certs(true)
        .build()
        .map_err(|error| {
            InstallError::ExportFailed(format!("failed to build HTTP client: {error}"))
        })?;
    let url = format!("{base_url}{EXPORT_PATH}");
    let response = client
        .get(&url)
        .bearer_auth(token)
        .header(ACCEPT, "application/json")
        .send()
        .await
        .map_err(|error| {
            InstallError::ExportFailed(format!("config export request failed: {error}"))
        })?;
    if response.status() != reqwest::StatusCode::OK {
        return Err(InstallError::ExportFailed(format!(
            "config export returned status {}",
            response.status()
        )));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| InstallError::ExportFailed(format!("failed to read response: {error}")))?;
    let body: Value = serde_json::from_slice(&bytes).map_err(|error| {
        InstallError::ExportFailed(format!("response is not valid JSON: {error}"))
    })?;
    if let Some(error_id) = body.get("error_id")
        && !error_id.is_null()
    {
        return Err(InstallError::ExportFailed(format!(
            "config export business failure: {error_id}"
        )));
    }
    let Some(data) = body.get("data").and_then(Value::as_object) else {
        return Err(InstallError::ExportFailed(
            "response is missing a non-null data object".into(),
        ));
    };
    let Some(filename) = data.get("filename").and_then(Value::as_str) else {
        return Err(InstallError::ExportFailed(
            "data.filename must be a string".into(),
        ));
    };
    let Some(version) = data.get("version").and_then(Value::as_str) else {
        return Err(InstallError::ExportFailed(
            "data.version must be a string".into(),
        ));
    };
    let Some(content) = data.get("content").and_then(Value::as_str) else {
        return Err(InstallError::ExportFailed(
            "data.content must be a string".into(),
        ));
    };
    let version = parse_stable_version(version).map_err(|error| {
        InstallError::ExportFailed(format!("invalid exported version: {error}"))
    })?;
    if filename != format!("landscape_init_v{version}.toml") {
        return Err(InstallError::ExportFailed(format!(
            "exported filename {filename} does not match version {version}"
        )));
    }
    Ok(ExportResult {
        version: version.to_string(),
        content: content.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::super::repository::test_server::{TestResponse, TestServer};
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("lkit-export-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn current_uid() -> u32 {
        unsafe { libc::geteuid() }
    }

    #[test]
    fn reads_valid_token() {
        let dir = temp_dir("valid");
        let path = dir.join("token");
        std::fs::write(&path, b"tok12345\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).unwrap();
        assert_eq!(read_api_token(&path, current_uid()).unwrap(), "tok12345");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_invalid_tokens() {
        fn write_token(path: &Path, content: &[u8]) {
            if path.exists() {
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
            }
            std::fs::write(path, content).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o400)).unwrap();
        }

        let dir = temp_dir("invalid");
        let path = dir.join("token");
        write_token(&path, b"tok12345\n");

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(read_api_token(&path, current_uid()).is_err());
        write_token(&path, b"tok12345\n");

        write_token(&path, b"");
        assert!(read_api_token(&path, current_uid()).is_err());

        write_token(&path, b"a\nb\n");
        assert!(read_api_token(&path, current_uid()).is_err());

        write_token(&path, &vec![b'a'; 1024 * 1024 + 1]);
        assert!(read_api_token(&path, current_uid()).is_err());

        write_token(&path, b"tok12345\n");
        assert!(read_api_token(&path, current_uid() + 1).is_err());
        let link = dir.join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(read_api_token(&link, current_uid()).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn start_server(body: Vec<u8>) -> TestServer {
        TestServer::start(move |path| {
            if path == EXPORT_PATH {
                TestResponse::ok(body.clone())
            } else {
                TestResponse::status(404, "Not Found", Vec::new())
            }
        })
    }

    fn ok_body() -> Vec<u8> {
        br#"{"data":{"filename":"landscape_init_v1.2.3.toml","version":"1.2.3","content":"version = \"1.2.3\"\n"}}"#
            .to_vec()
    }

    #[tokio::test]
    async fn exports_config_successfully() {
        let server = start_server(ok_body());
        let result = export_config(&server.base, "token").await.unwrap();
        assert_eq!(result.version, "1.2.3");
        assert_eq!(result.content, "version = \"1.2.3\"\n");
        assert!(server.request_paths().contains(&EXPORT_PATH.to_string()));
    }

    #[tokio::test]
    async fn rejects_business_error_id() {
        let server =
            start_server(br#"{"error_id":"export_failed","message":"boom","data":null}"#.to_vec());
        assert!(matches!(
            export_config(&server.base, "token").await,
            Err(InstallError::ExportFailed(_))
        ));
    }

    #[tokio::test]
    async fn rejects_invalid_responses() {
        let server = start_server(br#"{"data":null}"#.to_vec());
        assert!(export_config(&server.base, "token").await.is_err());

        let server = start_server(
            br#"{"data":{"filename":"landscape_init_v1.2.3.toml","version":"1.2.3"}}"#.to_vec(),
        );
        assert!(export_config(&server.base, "token").await.is_err());

        let server = start_server(
            br#"{"data":{"filename":"wrong.toml","version":"1.2.3","content":"x"}}"#.to_vec(),
        );
        assert!(export_config(&server.base, "token").await.is_err());

        let server = start_server(
            br#"{"data":{"filename":"landscape_init_v1.2.3.toml","version":"1.2.3-rc.1","content":"x"}}"#
                .to_vec(),
        );
        assert!(export_config(&server.base, "token").await.is_err());
    }

    #[tokio::test]
    async fn rejects_non_200_status() {
        let server =
            TestServer::start(|_| TestResponse::status(500, "Internal Server Error", Vec::new()));
        assert!(export_config(&server.base, "token").await.is_err());
    }
}
