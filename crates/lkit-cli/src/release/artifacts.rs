use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use sha2::{Digest, Sha256};

use super::plan::InstallError;
use super::repository::download::{
    DownloadClient, MAX_DECOMPRESSED_BYTES, decompress_zstd, extract_static_archive,
};
use super::repository::{AssetEncoding, Release};
use super::root::InstallRoot;

pub(crate) const WEBSERVER_BINARY: &str = "landscape-webserver";
pub(crate) const STATIC_DIR: &str = "static";

pub(crate) struct BuiltRelease {
    pub webserver_sha256: String,
    pub webserver_size: u64,
}

pub(crate) async fn build_release(
    root: &InstallRoot,
    release: &Release,
) -> Result<BuiltRelease, InstallError> {
    let version = &release.version;
    let releases_dir = root.canonical.join("releases");
    std::fs::create_dir_all(&releases_dir).map_err(InstallError::Io)?;
    let final_path = releases_dir.join(version.to_string());
    if let Some(built) = reuse_existing_release(&final_path, release)? {
        return Ok(built);
    }
    let tmp = releases_dir.join(format!(".install-{version}.tmp"));
    let _ = std::fs::remove_dir_all(&tmp);
    let built = match build_release_inner(release, &tmp).await {
        Ok(built) => built,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err(error);
        }
    };
    std::fs::rename(&tmp, &final_path).map_err(|error| {
        let _ = std::fs::remove_dir_all(&tmp);
        InstallError::Io(error)
    })?;
    Ok(built)
}

/// 目标版本目录已存在时,校验其内容与 manifest 一致,可信则直接复用、跳过下载。
/// 校验规则与 `docs/repository.md` 的“下载与发布目录”一致:
/// 目录必须是真实目录(非符号链接);后端二进制、`static.zip` 与 `static/index.html`
/// 齐全;`static.zip` 大小和摘要必须与 manifest 一致;后端为 Identity 编码时
/// 二进制摘要也必须与 manifest 一致(Zstd 传输编码的 manifest 摘要是压缩产物,
/// 无法从解压后的二进制反推,只做存在性校验)。不可信或残缺目录返回
/// `ReleaseExists`,不删除、不隔离、不覆盖。
fn reuse_existing_release(
    final_path: &Path,
    release: &Release,
) -> Result<Option<BuiltRelease>, InstallError> {
    let metadata = match std::fs::symlink_metadata(final_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(InstallError::Io(error)),
    };
    if !metadata.file_type().is_dir() {
        return Err(InstallError::ReleaseExists(release.version.to_string()));
    }
    let binary = final_path.join(WEBSERVER_BINARY);
    let static_dir = final_path.join(STATIC_DIR);
    let static_zip = final_path.join("static.zip");
    if !binary.is_file() || !static_dir.is_dir() || !static_dir.join("index.html").is_file() {
        return Err(InstallError::ReleaseExists(release.version.to_string()));
    }
    if !static_zip.is_file() {
        return Err(InstallError::ReleaseExists(release.version.to_string()));
    }
    let (static_sha, static_size) = hash_file(&static_zip)?;
    let archive = &release.assets.static_archive;
    if static_sha != archive.sha256 || static_size != archive.size {
        return Err(InstallError::ReleaseExists(release.version.to_string()));
    }
    let (webserver_sha256, webserver_size) = hash_file(&binary)?;
    let webserver = &release.assets.webserver;
    if webserver.encoding == AssetEncoding::Identity
        && (webserver_sha256 != webserver.sha256 || webserver_size != webserver.size)
    {
        return Err(InstallError::ReleaseExists(release.version.to_string()));
    }
    Ok(Some(BuiltRelease {
        webserver_sha256,
        webserver_size,
    }))
}

async fn build_release_inner(release: &Release, tmp: &Path) -> Result<BuiltRelease, InstallError> {
    std::fs::create_dir_all(tmp).map_err(InstallError::Io)?;
    let built = fetch_webserver_asset(release, tmp).await?;
    fetch_static_asset(release, tmp).await?;
    Ok(built)
}

/// 下载并校验后端资产,解压到 `target_dir/landscape-webserver` 并设置执行权限。
/// 返回落盘后端的实际摘要与大小。
pub(crate) async fn fetch_webserver_asset(
    release: &Release,
    target_dir: &Path,
) -> Result<BuiltRelease, InstallError> {
    let client = DownloadClient::new()?;
    let webserver = &release.assets.webserver;
    let webserver_raw = target_dir.join("webserver.download");
    client
        .download_asset(
            &release.version,
            webserver,
            "Landscape webserver",
            &webserver_raw,
        )
        .await?;
    let webserver_binary = target_dir.join(WEBSERVER_BINARY);
    match webserver.encoding {
        AssetEncoding::Zstd => {
            decompress_zstd(
                &release.version,
                &webserver_raw,
                &webserver_binary,
                MAX_DECOMPRESSED_BYTES,
            )?;
        }
        AssetEncoding::Identity => {
            std::fs::rename(&webserver_raw, &webserver_binary).map_err(InstallError::Io)?;
        }
    }
    set_mode(&webserver_binary, 0o755)?;
    let (webserver_sha256, webserver_size) = hash_file(&webserver_binary)?;
    Ok(BuiltRelease {
        webserver_sha256,
        webserver_size,
    })
}

/// 下载并校验静态压缩包,解压到 `target_dir/static`。
pub(crate) async fn fetch_static_asset(
    release: &Release,
    target_dir: &Path,
) -> Result<(), InstallError> {
    let client = DownloadClient::new()?;
    let static_archive = &release.assets.static_archive;
    let static_zip = target_dir.join("static.zip");
    client
        .download_asset(
            &release.version,
            static_archive,
            "Landscape static assets",
            &static_zip,
        )
        .await?;
    extract_static_archive(
        &release.version,
        &static_zip,
        static_archive.size,
        &target_dir.join(STATIC_DIR),
    )?;
    Ok(())
}

fn set_mode(path: &Path, mode: u32) -> Result<(), InstallError> {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).map_err(InstallError::Io)
}

pub(crate) fn hash_file(path: &Path) -> Result<(String, u64), InstallError> {
    let file = std::fs::File::open(path).map_err(InstallError::Io)?;
    let mut reader = std::io::BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut size: u64 = 0;
    loop {
        let read = reader.read(&mut buffer).map_err(InstallError::Io)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size += read as u64;
    }
    Ok((hex(&hasher.finalize()), size))
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(crate) fn hash_str(content: &str) -> String {
    hex(&Sha256::digest(content.as_bytes()))
}

fn hash(content: &str) -> String {
    hash_str(content)
}

#[cfg(test)]
mod tests {
    use url::Url;

    use super::*;
    use crate::release::repository::{Asset, AssetEncoding, Release, ReleaseAssets};

    const BINARY: &[u8] = b"webserver-binary";
    const STATIC_ZIP: &[u8] = b"zip-content";

    fn version() -> semver::Version {
        semver::Version::new(1, 2, 3)
    }

    fn temp_root(name: &str) -> InstallRoot {
        let root =
            std::env::temp_dir().join(format!("lkit-artifacts-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        InstallRoot {
            install_root: root.clone(),
            canonical: root,
        }
    }

    fn sha256_bytes(bytes: &[u8]) -> (String, u64) {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        (hex(&hasher.finalize()), bytes.len() as u64)
    }

    fn asset(url: &str, bytes: &[u8]) -> Asset {
        let (sha, size) = sha256_bytes(bytes);
        Asset::checked(Url::parse(url).unwrap(), sha, size, AssetEncoding::Identity).unwrap()
    }

    fn release(webserver: Asset, static_archive: Asset) -> Release {
        Release {
            version: version(),
            assets: ReleaseAssets {
                webserver,
                static_archive,
            },
        }
    }

    fn write_trusted_dir(final_dir: &std::path::Path, binary: &[u8], static_zip: &[u8]) {
        let static_dir = final_dir.join(STATIC_DIR);
        std::fs::create_dir_all(&static_dir).unwrap();
        std::fs::write(final_dir.join(WEBSERVER_BINARY), binary).unwrap();
        std::fs::write(final_dir.join("static.zip"), static_zip).unwrap();
        std::fs::write(static_dir.join("index.html"), b"<html></html>").unwrap();
    }

    #[tokio::test]
    async fn reuses_trusted_existing_release_without_downloading() {
        let root = temp_root("reuse-trusted");
        let final_dir = root.canonical.join("releases/1.2.3");
        std::fs::create_dir_all(&final_dir).unwrap();
        write_trusted_dir(&final_dir, BINARY, STATIC_ZIP);
        // Zstd 传输编码:manifest 摘要是压缩产物,与落盘二进制无关,只按存在性校验。
        let release = release(
            Asset::checked(
                Url::parse("https://example.com/landscape-webserver.zst").unwrap(),
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into(),
                1,
                AssetEncoding::Zstd,
            )
            .unwrap(),
            asset("https://example.com/static.zip", STATIC_ZIP),
        );
        let (expected_sha, expected_size) = sha256_bytes(BINARY);
        let built = build_release(&root, &release).await.unwrap();
        assert_eq!(built.webserver_sha256, expected_sha);
        assert_eq!(built.webserver_size, expected_size);
        assert_eq!(
            std::fs::read(final_dir.join(WEBSERVER_BINARY)).unwrap(),
            BINARY
        );
        assert!(!root.canonical.join("releases/.install-1.2.3.tmp").exists());
        let _ = std::fs::remove_dir_all(&root.install_root);
    }

    #[tokio::test]
    async fn reuses_identity_release_when_binary_matches_manifest() {
        let root = temp_root("reuse-identity");
        let final_dir = root.canonical.join("releases/1.2.3");
        std::fs::create_dir_all(&final_dir).unwrap();
        write_trusted_dir(&final_dir, BINARY, STATIC_ZIP);
        let release = release(
            asset("https://example.com/landscape-webserver", BINARY),
            asset("https://example.com/static.zip", STATIC_ZIP),
        );
        let (expected_sha, expected_size) = sha256_bytes(BINARY);
        let built = build_release(&root, &release).await.unwrap();
        assert_eq!(built.webserver_sha256, expected_sha);
        assert_eq!(built.webserver_size, expected_size);
        let _ = std::fs::remove_dir_all(&root.install_root);
    }

    #[tokio::test]
    async fn blocks_existing_release_when_static_archive_disagrees() {
        let root = temp_root("reuse-static-mismatch");
        let final_dir = root.canonical.join("releases/1.2.3");
        std::fs::create_dir_all(&final_dir).unwrap();
        write_trusted_dir(&final_dir, BINARY, STATIC_ZIP);
        let release = release(
            asset("https://example.com/landscape-webserver", BINARY),
            asset("https://example.com/static.zip", b"other-content"),
        );
        assert!(matches!(
            build_release(&root, &release).await,
            Err(InstallError::ReleaseExists(_))
        ));
        let _ = std::fs::remove_dir_all(&root.install_root);
    }

    #[tokio::test]
    async fn blocks_identity_release_when_binary_drifted() {
        let root = temp_root("reuse-binary-drift");
        let final_dir = root.canonical.join("releases/1.2.3");
        std::fs::create_dir_all(&final_dir).unwrap();
        write_trusted_dir(&final_dir, BINARY, STATIC_ZIP);
        let release = release(
            asset(
                "https://example.com/landscape-webserver",
                b"manifest-binary",
            ),
            asset("https://example.com/static.zip", STATIC_ZIP),
        );
        assert!(matches!(
            build_release(&root, &release).await,
            Err(InstallError::ReleaseExists(_))
        ));
        let _ = std::fs::remove_dir_all(&root.install_root);
    }

    #[tokio::test]
    async fn blocks_symlink_release_directory() {
        let root = temp_root("reuse-symlink");
        let releases = root.canonical.join("releases");
        std::fs::create_dir_all(&releases).unwrap();
        let outside = root.canonical.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, releases.join("1.2.3")).unwrap();
        let release = release(
            asset("https://example.com/landscape-webserver", BINARY),
            asset("https://example.com/static.zip", STATIC_ZIP),
        );
        assert!(matches!(
            build_release(&root, &release).await,
            Err(InstallError::ReleaseExists(_))
        ));
        let _ = std::fs::remove_dir_all(&root.install_root);
    }

    #[tokio::test]
    async fn blocks_incomplete_existing_release_directory() {
        let root = temp_root("reuse-incomplete");
        std::fs::create_dir_all(root.canonical.join("releases/1.2.3")).unwrap();
        let release = release(
            asset("https://example.com/landscape-webserver", BINARY),
            asset("https://example.com/static.zip", STATIC_ZIP),
        );
        assert!(matches!(
            build_release(&root, &release).await,
            Err(InstallError::ReleaseExists(_))
        ));
        let _ = std::fs::remove_dir_all(&root.install_root);
    }

    #[tokio::test]
    async fn downloads_when_target_release_directory_is_missing() {
        use std::collections::HashMap;
        use std::io::Write;

        use crate::release::repository::test_server::{TestResponse, TestServer};

        let static_zip = {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            writer.start_file("static/index.html", options).unwrap();
            writer.write_all(b"<h1>hello</h1>").unwrap();
            writer.finish().unwrap().into_inner()
        };
        let files = HashMap::from([
            ("/landscape-webserver".to_string(), BINARY.to_vec()),
            ("/static.zip".to_string(), static_zip.clone()),
        ]);
        let server = TestServer::start(move |path| match files.get(path) {
            Some(body) => TestResponse::ok(body.clone()),
            None => TestResponse::status(404, "Not Found", Vec::new()),
        });
        let (binary_sha, binary_size) = sha256_bytes(BINARY);
        let (static_sha, static_size) = sha256_bytes(&static_zip);
        let release = release(
            Asset::checked(
                Url::parse(&format!("{}/landscape-webserver", server.base)).unwrap(),
                binary_sha.clone(),
                binary_size,
                AssetEncoding::Identity,
            )
            .unwrap(),
            Asset::checked(
                Url::parse(&format!("{}/static.zip", server.base)).unwrap(),
                static_sha,
                static_size,
                AssetEncoding::Identity,
            )
            .unwrap(),
        );
        let root = temp_root("no-existing");
        let built = build_release(&root, &release).await.unwrap();
        assert_eq!(built.webserver_sha256, binary_sha);
        assert_eq!(built.webserver_size, binary_size);
        assert_eq!(
            std::fs::read(root.canonical.join("releases/1.2.3/landscape-webserver")).unwrap(),
            BINARY
        );
        assert!(
            root.canonical
                .join("releases/1.2.3/static/index.html")
                .is_file()
        );
        assert!(!root.canonical.join("releases/.install-1.2.3.tmp").exists());
        let _ = std::fs::remove_dir_all(&root.install_root);
    }
}
