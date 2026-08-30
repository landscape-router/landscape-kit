//! 自定义前端应用:配置了激活的自定义前端源时,解析其 latest/stable 的
//! `static.zip` 资产,下载校验解压后原子替换目标版本目录的 `static/`。
//! `static.zip` 官方基线文件保持不变;结构安全校验与来源无关、永远强制。
//! 源不可达或元数据非法时阻断整个命令(调用方提示逃生路径)。

use std::path::Path;

use semver::Version;
use serde::Deserialize;

use crate::deployment::config::{self, FrontendSource};
use crate::deployment::plan::InstallError;
use crate::release::artifacts::STATIC_DIR;
use crate::release::repository::download::{DownloadClient, extract_static_archive};
use crate::release::repository::{Asset, ProviderKind, provider_for};

/// 若配置了激活的自定义前端源,解析其 latest/stable 资产并应用到 `target`:
/// 下载校验解压后原子替换 `target/static`。`target` 是事务临时目录
/// (`.install-<version>.tmp`)或已存在的版本目录,均以 `static` 子目录为操作面。
/// 未配置前端源时不做任何事。
pub(crate) async fn apply_frontend(
    backend_version: &Version,
    target: &Path,
) -> Result<(), InstallError> {
    // 工作目录放在 target 同级(与 `.install-<version>.tmp` 同区),不依赖地盘。
    let work = target
        .parent()
        .expect("target has a parent")
        .join(format!(".frontend-{backend_version}.tmp"));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(InstallError::Io)?;
    let result = async {
        if !fetch_active_frontend(backend_version, &work).await? {
            return Ok(());
        }
        replace_static_dir(target, &work.join(STATIC_DIR))
    }
    .await;
    let _ = std::fs::remove_dir_all(&work);
    result
}

/// 解析激活的自定义前端源并下载校验解压到 `target_dir/static`。
/// 返回 `Ok(false)` 表示未配置自定义前端源（官方前端）；`Ok(true)` 表示已应用。
/// 源不可达或元数据非法时返回 `FrontendSource` 错误并附逃生路径提示。
pub(crate) async fn fetch_active_frontend(
    backend_version: &Version,
    target_dir: &Path,
) -> Result<bool, InstallError> {
    let Some(source) = config::resolve_active_frontend()? else {
        return Ok(false);
    };
    fetch_from_source(&source, backend_version, target_dir).await?;
    Ok(true)
}

/// 按已解析的前端源下载校验解压到 `target_dir/static`。repair 等宽容路径在
/// 外部解析 source（配置损坏时按官方处理）后直接调用,不重复读取配置。
pub(crate) async fn fetch_from_source(
    source: &FrontendSource,
    backend_version: &Version,
    target_dir: &Path,
) -> Result<(), InstallError> {
    let provider = provider_for(source.provider_kind(), &source.location)?;
    let asset = provider
        .latest_static_archive()
        .await
        .map_err(|error| {
            InstallError::FrontendSource(format!(
                "unable to resolve the active frontend source {}: {error}; remove the [frontend] section or run `lkit frontend select official` to fall back to the official frontend",
                source.display_name()
            ))
        })?;
    fetch_frontend_static(backend_version, &asset, target_dir).await?;
    warn_api_min_version(source, &target_dir.join(STATIC_DIR), backend_version);
    Ok(())
}

/// 下载并校验前端 `static.zip`,解压到 `target_dir/static`。与官方静态资产
/// 使用同一套校验:声明大小与 SHA-256 流式校验 + 结构安全解压。
async fn fetch_frontend_static(
    version: &Version,
    asset: &Asset,
    target_dir: &Path,
) -> Result<(), InstallError> {
    let client = DownloadClient::new()?;
    let static_zip = target_dir.join("static.zip");
    client
        .download_asset(
            version,
            asset,
            "Landscape frontend static assets",
            &static_zip,
        )
        .await?;
    extract_static_archive(
        version,
        &static_zip,
        asset.size,
        &target_dir.join(STATIC_DIR),
    )?;
    Ok(())
}

/// 原子替换 `target/static`:旧目录改名让位,新目录 rename 到位,失败时恢复旧目录。
fn replace_static_dir(target: &Path, new_static: &Path) -> Result<(), InstallError> {
    let live = target.join(STATIC_DIR);
    let backup = target.join(".frontend-old-static");
    let _ = std::fs::remove_dir_all(&backup);
    if live.exists() {
        std::fs::rename(&live, &backup).map_err(InstallError::Io)?;
    }
    if let Err(error) = std::fs::rename(new_static, &live) {
        let _ = std::fs::rename(&backup, &live);
        return Err(InstallError::Io(error));
    }
    let _ = std::fs::remove_dir_all(&backup);
    Ok(())
}

#[derive(Debug, Deserialize, Default)]
struct FrontendMetadata {
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    api_min_version: Option<String>,
}

/// 读取解压后的 `static/frontend.json`（存在时）,对 `api_min_version` 高于当前
/// 后端版本的情况输出警告。元数据在 zip 内、随包 SHA-256 校验,声明是绑定的。
fn warn_api_min_version(source: &FrontendSource, static_dir: &Path, backend: &Version) {
    let path = static_dir.join("frontend.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    let Ok(metadata) = serde_json::from_str::<FrontendMetadata>(&text) else {
        return;
    };
    let Some(minimum) = metadata.api_min_version.as_deref().and_then(|value| {
        Version::parse(value)
            .ok()
            .filter(|parsed| parsed.to_string() == value)
    }) else {
        return;
    };
    if minimum > *backend {
        eprintln!(
            "frontend: {} {} declares api_min_version {minimum}, which is newer than the installed Landscape {backend}; the frontend may be incompatible",
            source.display_name(),
            metadata.version.as_deref().unwrap_or("(no version)")
        );
    }
}

impl FrontendSource {
    pub(crate) fn provider_kind(&self) -> ProviderKind {
        match self.kind {
            config::RepositorySourceKind::Github => ProviderKind::Github,
            config::RepositorySourceKind::Http => ProviderKind::Http,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_static_dir_atomically() {
        let temp = std::env::temp_dir().join(format!("lkit-frontend-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(temp.join("static")).unwrap();
        std::fs::create_dir_all(temp.join("new-static")).unwrap();
        std::fs::write(temp.join("static/index.html"), b"old").unwrap();
        std::fs::write(temp.join("new-static/index.html"), b"new").unwrap();
        replace_static_dir(&temp, &temp.join("new-static")).unwrap();
        assert_eq!(
            std::fs::read(temp.join("static/index.html")).unwrap(),
            b"new"
        );
        assert!(!temp.join(".frontend-old-static").exists());
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn restores_old_static_when_replacement_fails() {
        let temp =
            std::env::temp_dir().join(format!("lkit-frontend-test-fail-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(temp.join("static")).unwrap();
        std::fs::write(temp.join("static/index.html"), b"old").unwrap();
        let missing_source = temp.join("missing-new-static");
        let error = replace_static_dir(&temp, &missing_source).unwrap_err();
        assert!(matches!(error, InstallError::Io(_)));
        assert_eq!(
            std::fs::read(temp.join("static/index.html")).unwrap(),
            b"old",
            "the old static dir must be restored after a failed swap"
        );
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn ignores_absent_or_invalid_frontend_metadata() {
        let temp = std::env::temp_dir().join(format!("lkit-frontend-meta-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();
        let source = FrontendSource {
            id: "test".into(),
            name: None,
            kind: config::RepositorySourceKind::Http,
            location: "https://example.com/ui/".into(),
        };
        let backend = Version::new(0, 19, 2);
        warn_api_min_version(&source, &temp, &backend);
        std::fs::write(temp.join("frontend.json"), b"not json").unwrap();
        warn_api_min_version(&source, &temp, &backend);
        std::fs::write(
            temp.join("frontend.json"),
            b"{\"api_min_version\":\"0.20.0\"}",
        )
        .unwrap();
        warn_api_min_version(&source, &temp, &backend);
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[tokio::test]
    async fn applies_the_active_frontend_source_end_to_end() {
        use std::io::Write;

        use crate::deployment::layout;
        use crate::release::repository::test_server::{TestResponse, TestServer};

        let temp = std::env::temp_dir().join(format!("lkit-frontend-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();
        let territory = temp.join("territory");
        std::fs::create_dir_all(&territory).unwrap();
        let _guard = layout::test_territory(&territory);

        // 前端源:stable 通道 + 只声明 static 的 manifest + static.zip。
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        zip.start_file(
            "static/index.html",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(b"<h1>custom</h1>").unwrap();
        let zip_bytes = zip.finish().unwrap().into_inner();
        let (sha, size) = {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(&zip_bytes);
            let hex = hasher
                .finalize()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            (hex, zip_bytes.len() as u64)
        };
        let files = std::collections::HashMap::from([
            ("/repository.json".to_string(), br#"{"protocol_version":1}"#.to_vec()),
            (
                "/channels/stable.json".to_string(),
                br#"{"protocol_version":1,"version":"1.0.0"}"#.to_vec(),
            ),
            (
                "/releases/1.0.0/manifest.json".to_string(),
                format!(
                    r#"{{"protocol_version":1,"version":"1.0.0","assets":{{"webserver":{{}},"static":{{"url":"static.zip","sha256":"{sha}","size":{size}}}}}}}"#
                )
                .into_bytes(),
            ),
            ("/releases/1.0.0/static.zip".to_string(), zip_bytes),
        ]);
        let server = TestServer::start(move |path| match files.get(path) {
            Some(body) => TestResponse::ok(body.clone()),
            None => TestResponse::status(404, "Not Found", Vec::new()),
        });

        std::fs::write(
            territory.join("config.toml"),
            format!(
                "schema_version = 1\n\n[repository]\nkind = \"github\"\nlocation = \"ThisSeanZhang/landscape\"\n\n[frontend]\nactive = \"custom\"\n\n[[frontend.sources]]\nid = \"custom\"\nkind = \"http\"\nlocation = \"{}\"\n",
                server.base
            ),
        )
        .unwrap();

        // 目标目录预置官方 static,apply 后应被自定义前端替换。
        std::fs::create_dir_all(temp.join("target/static")).unwrap();
        std::fs::write(temp.join("target/static/index.html"), b"<h1>official</h1>").unwrap();
        let backend = Version::new(0, 19, 2);
        apply_frontend(&backend, &temp.join("target"))
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(temp.join("target/static/index.html")).unwrap(),
            b"<h1>custom</h1>"
        );
        assert!(!temp.join("target/.frontend-old-static").exists());
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[tokio::test]
    async fn apply_frontend_is_a_noop_without_a_configured_source() {
        use crate::deployment::layout;

        let temp = std::env::temp_dir().join(format!("lkit-frontend-noop-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();
        let territory = temp.join("territory");
        std::fs::create_dir_all(&territory).unwrap();
        let _guard = layout::test_territory(&territory);
        std::fs::create_dir_all(temp.join("target/static")).unwrap();
        std::fs::write(temp.join("target/static/index.html"), b"<h1>official</h1>").unwrap();
        let backend = Version::new(0, 19, 2);
        apply_frontend(&backend, &temp.join("target"))
            .await
            .unwrap();
        assert_eq!(
            std::fs::read(temp.join("target/static/index.html")).unwrap(),
            b"<h1>official</h1>",
            "no [frontend] section must leave the official pages untouched"
        );
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[tokio::test]
    async fn apply_frontend_blocks_on_unreachable_source_with_escape_hint() {
        use crate::deployment::layout;

        let temp = std::env::temp_dir().join(format!("lkit-frontend-block-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();
        let territory = temp.join("territory");
        std::fs::create_dir_all(&territory).unwrap();
        let _guard = layout::test_territory(&territory);
        std::fs::write(
            territory.join("config.toml"),
            "schema_version = 1\n\n[repository]\nkind = \"github\"\nlocation = \"ThisSeanZhang/landscape\"\n\n[frontend]\nactive = \"custom\"\n\n[[frontend.sources]]\nid = \"custom\"\nkind = \"http\"\nlocation = \"http://127.0.0.1:1/ui/\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(temp.join("target/static")).unwrap();
        std::fs::write(temp.join("target/static/index.html"), b"<h1>official</h1>").unwrap();
        let backend = Version::new(0, 19, 2);
        let error = apply_frontend(&backend, &temp.join("target"))
            .await
            .unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("frontend select official"),
            "the error must point to the escape path: {message}"
        );
        assert_eq!(
            std::fs::read(temp.join("target/static/index.html")).unwrap(),
            b"<h1>official</h1>",
            "a blocked apply must leave the official pages untouched"
        );
        let _ = std::fs::remove_dir_all(&temp);
    }
}
