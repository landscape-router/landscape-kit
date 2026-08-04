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
    let tmp = releases_dir.join(format!(".install-{version}.tmp"));
    let _ = std::fs::remove_dir_all(&tmp);
    let built = match build_release_inner(release, &tmp).await {
        Ok(built) => built,
        Err(error) => {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err(error);
        }
    };
    let final_path = releases_dir.join(version.to_string());
    if final_path.exists() {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(InstallError::ReleaseExists(version.to_string()));
    }
    std::fs::rename(&tmp, &final_path).map_err(|error| {
        let _ = std::fs::remove_dir_all(&tmp);
        InstallError::Io(error)
    })?;
    Ok(built)
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
