use std::collections::HashSet;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use aws_config::BehaviorVersion;
use aws_sdk_s3::Client;
use aws_sdk_s3::config::{Builder as S3ConfigBuilder, Region};
use aws_sdk_s3::error::SdkError;
use aws_sdk_s3::operation::get_object::GetObjectError;
use aws_sdk_s3::operation::head_object::HeadObjectError;
use aws_sdk_s3::operation::put_object::PutObjectError;
use aws_sdk_s3::primitives::ByteStream;
use lkit_repository::{
    ProtocolError, ReleaseManifest, RepositoryAsset, RepositoryAssets, RepositoryDescriptor,
    STATIC_ARCHIVE, StableChannel, WEBSERVER_AARCH64, WEBSERVER_X86_64, WebserverAssets,
    parse_stable_version, zip_path_parts,
};
use semver::Version;
use serde::Serialize;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use thiserror::Error;
use url::Url;

const IMMUTABLE_CACHE_CONTROL: &str = "public, max-age=31536000, immutable";
const STABLE_CACHE_CONTROL: &str = "no-cache";
const MAX_DECOMPRESSED_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Debug)]
pub struct PublishConfig {
    pub version: String,
    pub directory: PathBuf,
    pub endpoint: String,
    pub bucket: String,
    pub public_base_url: Option<String>,
    pub region: String,
}

pub async fn publish(config: PublishConfig) -> Result<(), PublishError> {
    let version =
        parse_stable_version(config.version.strip_prefix('v').unwrap_or(&config.version))?;
    validate_endpoint(&config.endpoint)?;
    validate_bucket(&config.bucket)?;
    let public_base_url = normalize_public_base_url(config.public_base_url.unwrap_or_else(|| {
        format!(
            "{}/{}",
            config.endpoint.trim_end_matches('/'),
            config.bucket
        )
    }))?;
    let assets = collect_assets(&config.directory, &version)?;
    let client = build_client(&config.endpoint, &config.region).await;

    ensure_repository(&client, &config.bucket).await?;
    let release_prefix = format!("releases/{version}");
    let manifest_key = format!("{release_prefix}/manifest.json");
    if object_exists(&client, &config.bucket, &manifest_key).await? {
        return Err(PublishError::ReleaseExists(version));
    }

    for asset in assets.iter() {
        let key = format!("{release_prefix}/{}", asset.name);
        println!("Uploading {key}");
        put_asset(&client, &config.bucket, &key, asset).await?;
    }

    let manifest = ReleaseManifest::new(
        &version,
        RepositoryAssets {
            webserver: WebserverAssets {
                x86_64: Some(assets.webserver_x86_64.repository_asset()),
                aarch64: Some(assets.webserver_aarch64.repository_asset()),
            },
            static_archive: assets.static_archive.repository_asset(),
        },
    )?;
    println!("Publishing immutable {manifest_key}");
    put_json_if_absent(
        &client,
        &config.bucket,
        &manifest_key,
        &manifest,
        IMMUTABLE_CACHE_CONTROL,
    )
    .await
    .map_err(|error| match error {
        PublishError::ObjectExists(_) => PublishError::ReleaseExists(version.clone()),
        error => error,
    })?;

    update_stable(&client, &config.bucket, &version).await?;
    println!("Published release {version} to {public_base_url}");
    Ok(())
}

async fn build_client(endpoint: &str, region: &str) -> Client {
    let shared = aws_config::defaults(BehaviorVersion::latest())
        .region(Region::new(region.to_owned()))
        .load()
        .await;
    let config = S3ConfigBuilder::from(&shared)
        .endpoint_url(endpoint)
        .force_path_style(true)
        .build();
    Client::from_conf(config)
}

async fn ensure_repository(client: &Client, bucket: &str) -> Result<(), PublishError> {
    match get_json::<RepositoryDescriptor>(client, bucket, "repository.json").await? {
        Some(descriptor) => descriptor.validate()?,
        None => {
            match put_json_if_absent(
                client,
                bucket,
                "repository.json",
                &RepositoryDescriptor::v1(),
                IMMUTABLE_CACHE_CONTROL,
            )
            .await
            {
                Ok(()) | Err(PublishError::ObjectExists(_)) => {}
                Err(error) => return Err(error),
            }
            let descriptor = get_json::<RepositoryDescriptor>(client, bucket, "repository.json")
                .await?
                .ok_or_else(|| PublishError::Verification("repository.json is missing".into()))?;
            descriptor.validate()?;
        }
    }
    Ok(())
}

async fn update_stable(
    client: &Client,
    bucket: &str,
    version: &Version,
) -> Result<(), PublishError> {
    let key = "channels/stable.json";
    for _ in 0..3 {
        match get_object(client, bucket, key).await? {
            None => {
                println!("Advancing stable channel to {version}");
                match put_json_if_absent(
                    client,
                    bucket,
                    key,
                    &StableChannel::new(version)?,
                    STABLE_CACHE_CONTROL,
                )
                .await
                {
                    Ok(()) => return Ok(()),
                    Err(PublishError::ObjectExists(_)) => continue,
                    Err(error) => return Err(error),
                }
            }
            Some(object) => {
                let channel: StableChannel = serde_json::from_slice(&object.body)?;
                let current = channel.parsed_version()?;
                if current >= *version {
                    println!(
                        "Published historical release {version} without changing stable channel"
                    );
                    return Ok(());
                }
                println!("Advancing stable channel to {version}");
                let body = json_bytes(&StableChannel::new(version)?)?;
                let mut request = client
                    .put_object()
                    .bucket(bucket)
                    .key(key)
                    .body(ByteStream::from(body))
                    .content_type("application/json")
                    .cache_control(STABLE_CACHE_CONTROL);
                if let Some(etag) = object.etag {
                    request = request.if_match(etag);
                }
                match request.send().await {
                    Ok(_) => return Ok(()),
                    Err(error) if is_precondition_failed(&error) => continue,
                    Err(error) => return Err(s3_error("update stable channel", error)),
                }
            }
        }
    }
    Err(PublishError::ConcurrentUpdate)
}

async fn put_asset(
    client: &Client,
    bucket: &str,
    key: &str,
    asset: &LocalAsset,
) -> Result<(), PublishError> {
    match client.head_object().bucket(bucket).key(key).send().await {
        Ok(output) => {
            let expected_size = i64::try_from(asset.size)
                .map_err(|_| PublishError::InvalidAsset("asset is too large".into()))?;
            let matching_size = output.content_length() == Some(expected_size);
            let matching_hash = output
                .metadata()
                .and_then(|metadata| metadata.get("sha256"))
                == Some(&asset.sha256);
            if matching_size && matching_hash {
                return Ok(());
            }
            return Err(PublishError::ObjectExists(key.into()));
        }
        Err(error) if is_head_missing(&error) => {}
        Err(error) => return Err(s3_error("inspect release asset", error)),
    }

    let body = ByteStream::from_path(&asset.path)
        .await
        .map_err(|error| PublishError::Io(error.into()))?;
    let request = client
        .put_object()
        .bucket(bucket)
        .key(key)
        .body(body)
        .content_type(asset.content_type)
        .cache_control(IMMUTABLE_CACHE_CONTROL)
        .metadata("sha256", &asset.sha256)
        .if_none_match("*");
    match request.send().await {
        Ok(_) => Ok(()),
        Err(error) if is_precondition_failed(&error) => Err(PublishError::ObjectExists(key.into())),
        Err(error) => Err(s3_error("upload release asset", error)),
    }
}

async fn put_json_if_absent<T: Serialize>(
    client: &Client,
    bucket: &str,
    key: &str,
    value: &T,
    cache_control: &str,
) -> Result<(), PublishError> {
    let request = client
        .put_object()
        .bucket(bucket)
        .key(key)
        .body(ByteStream::from(json_bytes(value)?))
        .content_type("application/json")
        .cache_control(cache_control)
        .if_none_match("*");
    match request.send().await {
        Ok(_) => Ok(()),
        Err(error) if is_precondition_failed(&error) => Err(PublishError::ObjectExists(key.into())),
        Err(error) => Err(s3_error("upload JSON object", error)),
    }
}

async fn get_json<T: DeserializeOwned>(
    client: &Client,
    bucket: &str,
    key: &str,
) -> Result<Option<T>, PublishError> {
    let Some(object) = get_object(client, bucket, key).await? else {
        return Ok(None);
    };
    serde_json::from_slice(&object.body)
        .map(Some)
        .map_err(PublishError::Json)
}

async fn get_object(
    client: &Client,
    bucket: &str,
    key: &str,
) -> Result<Option<StoredObject>, PublishError> {
    match client.get_object().bucket(bucket).key(key).send().await {
        Ok(output) => {
            let etag = output.e_tag().map(ToOwned::to_owned);
            let body = output
                .body
                .collect()
                .await
                .map_err(|error| PublishError::S3(format!("read {key}: {error}")))?
                .into_bytes()
                .to_vec();
            Ok(Some(StoredObject { body, etag }))
        }
        Err(error) if is_get_missing(&error) => Ok(None),
        Err(error) => Err(s3_error("read repository object", error)),
    }
}

async fn object_exists(client: &Client, bucket: &str, key: &str) -> Result<bool, PublishError> {
    match client.head_object().bucket(bucket).key(key).send().await {
        Ok(_) => Ok(true),
        Err(error) if is_head_missing(&error) => Ok(false),
        Err(error) => Err(s3_error("inspect repository object", error)),
    }
}

fn collect_assets(directory: &Path, version: &Version) -> Result<LocalAssets, PublishError> {
    Ok(LocalAssets {
        webserver_x86_64: collect_asset(
            directory,
            version,
            WEBSERVER_X86_64,
            "application/zstd",
            validate_zstd,
        )?,
        webserver_aarch64: collect_asset(
            directory,
            version,
            WEBSERVER_AARCH64,
            "application/zstd",
            validate_zstd,
        )?,
        static_archive: collect_asset(
            directory,
            version,
            STATIC_ARCHIVE,
            "application/zip",
            validate_static_archive,
        )?,
    })
}

fn collect_asset(
    directory: &Path,
    version: &Version,
    name: &'static str,
    content_type: &'static str,
    validate: fn(&Path, &Version) -> Result<(), PublishError>,
) -> Result<LocalAsset, PublishError> {
    let path = directory.join(name);
    if !path.is_file() {
        return Err(PublishError::MissingAsset(path));
    }
    validate(&path, version)?;
    let (sha256, size) = hash_file(&path)?;
    Ok(LocalAsset {
        name,
        path,
        sha256,
        size,
        content_type,
    })
}

fn validate_zstd(path: &Path, version: &Version) -> Result<(), PublishError> {
    let input = File::open(path)?;
    let mut decoder = zstd::stream::read::Decoder::new(BufReader::new(input))?.single_frame();
    let mut buffer = [0_u8; 64 * 1024];
    let mut total = 0_u64;
    loop {
        let count = decoder.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total = total.saturating_add(count as u64);
        if total > MAX_DECOMPRESSED_BYTES {
            return Err(PublishError::InvalidAsset(format!(
                "version {version} zstd asset exceeds the 1 GiB decompressed limit"
            )));
        }
    }
    if total == 0 {
        return Err(PublishError::InvalidAsset(format!(
            "version {version} zstd asset is empty"
        )));
    }
    let mut remaining = decoder.finish();
    let mut probe = [0_u8; 1];
    if remaining.read(&mut probe)? != 0 {
        return Err(PublishError::InvalidAsset(format!(
            "version {version} zstd asset contains trailing data"
        )));
    }
    Ok(())
}

fn validate_static_archive(path: &Path, version: &Version) -> Result<(), PublishError> {
    let file = File::open(path)?;
    let mut archive = zip::ZipArchive::new(file).map_err(|error| {
        PublishError::InvalidAsset(format!("version {version} invalid zip: {error}"))
    })?;
    let compressed_size = std::fs::metadata(path)?.len();
    let limit = std::cmp::min(compressed_size.saturating_mul(20), MAX_DECOMPRESSED_BYTES);
    let mut total_read = 0_u64;
    let mut has_index = false;
    let mut output_paths = HashSet::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| {
            PublishError::InvalidAsset(format!("version {version} invalid zip entry: {error}"))
        })?;
        if entry.is_symlink() {
            return Err(PublishError::InvalidAsset(format!(
                "version {version} zip contains a symbolic link: {}",
                entry.name()
            )));
        }
        if let Some(mode) = entry.unix_mode()
            && matches!(mode & 0o170000, 0o020000 | 0o060000 | 0o010000 | 0o140000)
        {
            return Err(PublishError::InvalidAsset(format!(
                "version {version} zip contains a special file: {}",
                entry.name()
            )));
        }
        let name = entry.name().to_owned();
        if !name.starts_with("static/") {
            return Err(PublishError::InvalidAsset(format!(
                "version {version} zip entry must be under static/: {}",
                name
            )));
        }
        let relative = &name["static/".len()..];
        let parts = zip_path_parts(relative).map_err(|reason| {
            PublishError::InvalidAsset(format!(
                "version {version} zip contains an unsafe path {name}: {reason}"
            ))
        })?;
        if parts.is_empty() {
            if entry.is_dir() && name == "static/" {
                continue;
            }
            return Err(PublishError::InvalidAsset(format!(
                "version {version} zip entry has no relative path: {name}"
            )));
        }
        let normalized = parts.join("/");
        if !output_paths.insert(normalized.clone()) {
            return Err(PublishError::InvalidAsset(format!(
                "version {version} zip contains duplicate path: {normalized}"
            )));
        }
        if entry.is_dir() {
            continue;
        }
        if normalized == "index.html" {
            has_index = true;
        }
        if entry.size() > limit.saturating_sub(total_read) {
            return Err(PublishError::InvalidAsset(format!(
                "version {version} zip exceeds the decompressed size limit"
            )));
        }
        let mut buffer = [0_u8; 64 * 1024];
        loop {
            let count = entry.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            total_read = total_read.saturating_add(count as u64);
            if total_read > limit {
                return Err(PublishError::InvalidAsset(format!(
                    "version {version} zip exceeds the decompressed size limit"
                )));
            }
        }
    }
    if !has_index {
        return Err(PublishError::InvalidAsset(format!(
            "version {version} zip is missing static/index.html"
        )));
    }
    Ok(())
}

fn hash_file(path: &Path) -> Result<(String, u64), PublishError> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut size = 0_u64;
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        size += count as u64;
    }
    if size == 0 {
        return Err(PublishError::InvalidAsset(format!(
            "asset is empty: {}",
            path.display()
        )));
    }
    let digest = hasher.finalize();
    let mut sha256 = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(&mut sha256, "{byte:02x}");
    }
    Ok((sha256, size))
}

fn normalize_public_base_url(value: String) -> Result<Url, PublishError> {
    let mut url = Url::parse(&value).map_err(PublishError::InvalidUrl)?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(PublishError::InvalidConfiguration(
            "public base URL must be HTTP or HTTPS".into(),
        ));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(PublishError::InvalidConfiguration(
            "public base URL cannot contain a query or fragment".into(),
        ));
    }
    if !url.path().ends_with('/') {
        url.set_path(&format!("{}/", url.path()));
    }
    Ok(url)
}

fn validate_endpoint(value: &str) -> Result<(), PublishError> {
    let url = Url::parse(value).map_err(PublishError::InvalidUrl)?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(PublishError::InvalidConfiguration(
            "S3 endpoint must be HTTP or HTTPS".into(),
        ));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(PublishError::InvalidConfiguration(
            "S3 endpoint cannot contain a query or fragment".into(),
        ));
    }
    Ok(())
}

fn validate_bucket(bucket: &str) -> Result<(), PublishError> {
    if bucket.is_empty() || bucket.contains('/') {
        return Err(PublishError::InvalidConfiguration(
            "S3 bucket must be a non-empty bucket name".into(),
        ));
    }
    Ok(())
}

fn json_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, PublishError> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn is_get_missing(error: &SdkError<GetObjectError>) -> bool {
    error
        .raw_response()
        .is_some_and(|response| response.status().as_u16() == 404)
}

fn is_head_missing(error: &SdkError<HeadObjectError>) -> bool {
    error
        .raw_response()
        .is_some_and(|response| response.status().as_u16() == 404)
}

fn is_precondition_failed(error: &SdkError<PutObjectError>) -> bool {
    error
        .raw_response()
        .is_some_and(|response| response.status().as_u16() == 412)
}

fn s3_error<E: std::fmt::Debug>(operation: &str, error: E) -> PublishError {
    PublishError::S3(format!("{operation}: {error:?}"))
}

#[derive(Debug)]
struct StoredObject {
    body: Vec<u8>,
    etag: Option<String>,
}

#[derive(Debug)]
struct LocalAsset {
    name: &'static str,
    path: PathBuf,
    sha256: String,
    size: u64,
    content_type: &'static str,
}

#[derive(Debug)]
struct LocalAssets {
    webserver_x86_64: LocalAsset,
    webserver_aarch64: LocalAsset,
    static_archive: LocalAsset,
}

impl LocalAssets {
    fn iter(&self) -> impl Iterator<Item = &LocalAsset> {
        [
            &self.webserver_x86_64,
            &self.webserver_aarch64,
            &self.static_archive,
        ]
        .into_iter()
    }
}

impl LocalAsset {
    fn repository_asset(&self) -> RepositoryAsset {
        RepositoryAsset::new(self.name.into(), self.sha256.clone(), self.size)
            .expect("validated local asset metadata")
    }
}

#[derive(Debug, Error)]
pub enum PublishError {
    #[error("invalid publish configuration: {0}")]
    InvalidConfiguration(String),
    #[error("invalid URL: {0}")]
    InvalidUrl(url::ParseError),
    #[error("repository protocol error: {0}")]
    Protocol(#[from] ProtocolError),
    #[error("missing release asset: {0}")]
    MissingAsset(PathBuf),
    #[error("invalid release asset: {0}")]
    InvalidAsset(String),
    #[error("release {0} is already published")]
    ReleaseExists(Version),
    #[error("refusing to overwrite existing object: {0}")]
    ObjectExists(String),
    #[error("repository changed concurrently while updating stable channel")]
    ConcurrentUpdate,
    #[error("repository verification failed: {0}")]
    Verification(String),
    #[error("S3 operation failed: {0}")]
    S3(String),
    #[error("JSON operation failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("file operation failed: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn validates_publish_assets() {
        let temp = std::env::temp_dir().join(format!("lkit-publish-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(temp.join("static")).unwrap();

        for name in [WEBSERVER_X86_64, WEBSERVER_AARCH64] {
            let file = File::create(temp.join(name)).unwrap();
            let mut encoder = zstd::stream::write::Encoder::new(file, 1).unwrap();
            encoder.write_all(b"webserver").unwrap();
            encoder.finish().unwrap();
        }

        let archive = File::create(temp.join(STATIC_ARCHIVE)).unwrap();
        let mut zip = zip::ZipWriter::new(archive);
        zip.start_file(
            "static/index.html",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(b"index").unwrap();
        zip.finish().unwrap();

        let assets = collect_assets(&temp, &Version::new(1, 2, 3)).unwrap();
        assert_eq!(assets.iter().count(), 3);
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn normalizes_public_repository_url() {
        let url = normalize_public_base_url("https://example.com/releases".into()).unwrap();
        assert_eq!(url.as_str(), "https://example.com/releases/");
    }
}
