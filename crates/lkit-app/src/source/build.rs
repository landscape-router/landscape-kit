//! Build `ReleaseSource` instances from `SourceConfig`.

use std::sync::Arc;

use lkit_client::{GithubSource, HttpMirrorSource, LocalSource};
use lkit_core::source::config::{SourceConfig, SourceType};
use lkit_core::ReleaseSource;

/// Convert a list of `SourceConfig` into `Arc<dyn ReleaseSource>` instances.
///
/// Skips configs that fail to construct (logs a warning).
pub fn build_release_sources(
    configs: &[SourceConfig],
    client: reqwest::Client,
) -> Vec<Arc<dyn ReleaseSource>> {
    configs
        .iter()
        .filter_map(|cfg| match build_one(cfg, &client) {
            Ok(src) => Some(src),
            Err(e) => {
                tracing::warn!("跳过源 '{}': {e}", cfg.name);
                None
            }
        })
        .collect()
}

fn build_one(
    cfg: &SourceConfig,
    client: &reqwest::Client,
) -> Result<Arc<dyn ReleaseSource>, String> {
    match cfg.source_type {
        SourceType::Github => {
            let repo = cfg
                .repo
                .as_deref()
                .ok_or("github 类型缺少 repo 字段")?;
            let src = GithubSource::new(&cfg.name, repo, client.clone())
                .map_err(|e| format!("创建 GithubSource 失败: {e}"))?;
            Ok(Arc::new(src))
        }
        SourceType::Http => {
            let url = cfg
                .base_url
                .as_deref()
                .ok_or("http 类型缺少 base_url 字段")?;
            Ok(Arc::new(HttpMirrorSource::new(
                &cfg.name,
                url,
                client.clone(),
            )))
        }
        SourceType::Local => {
            let path = cfg
                .path
                .as_deref()
                .ok_or("local 类型缺少 path 字段")?;
            Ok(Arc::new(LocalSource::new(&cfg.name, path)))
        }
    }
}
