//! Lightweight HTTP file server for mirror directories.

use std::net::SocketAddr;
use std::path::PathBuf;

use axum::Router;
use axum::extract::Path as AxumPath;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use tokio::fs;

use crate::error::MirrorError;

/// Serve configuration.
#[derive(Debug, Clone)]
pub struct ServeConfig {
    /// Root directory to serve.
    pub path: PathBuf,
    /// Port to listen on.
    pub port: u16,
    /// Address to bind to.
    pub bind: String,
}

impl Default for ServeConfig {
    fn default() -> Self {
        Self {
            path: PathBuf::from("."),
            port: 8080,
            bind: "0.0.0.0".into(),
        }
    }
}

/// Start the HTTP file server. Blocks until shutdown signal.
pub async fn serve(config: ServeConfig) -> Result<(), MirrorError> {
    let root = config.path.clone();

    let app = Router::new().route(
        "/{*path}",
        get(move |path: AxumPath<String>| {
            let root = root.clone();
            async move { handle_file(root, path.0).await }
        }),
    );

    let addr: SocketAddr = format!("{}:{}", config.bind, config.port)
        .parse()
        .map_err(|e| MirrorError::TargetError(format!("invalid bind address: {e}")))?;

    tracing::info!("serving {} on {}", config.path.display(), addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .await
        .map_err(|e| MirrorError::TargetError(e.to_string()))
}

/// Serve a single file from the mirror root.
async fn handle_file(root: PathBuf, path: String) -> Response {
    let file_path = root.join(path.trim_start_matches('/'));

    // Normalize path to prevent traversal via .. components
    let normalized = normalize_path(&file_path);
    let canonical_root = normalize_path(&root);
    if !normalized.starts_with(&canonical_root) {
        return (StatusCode::FORBIDDEN, "forbidden").into_response();
    }

    match fs::read(&file_path).await {
        Ok(data) => {
            let content_type = if path.ends_with(".json") {
                "application/json"
            } else if path.ends_with(".txt") {
                "text/plain"
            } else {
                "application/octet-stream"
            };
            (
                StatusCode::OK,
                [(axum::http::header::CONTENT_TYPE, content_type)],
                data,
            )
                .into_response()
        }
        Err(e) => match e.kind() {
            std::io::ErrorKind::NotFound => (StatusCode::NOT_FOUND, "not found").into_response(),
            _ => (StatusCode::INTERNAL_SERVER_ERROR, "server error").into_response(),
        },
    }
}

/// Normalize a path by resolving `.` and `..` without requiring the path to exist.
fn normalize_path(path: &std::path::Path) -> PathBuf {
    let mut components = Vec::new();
    for comp in path.components() {
        match comp {
            std::path::Component::ParentDir => {
                components.pop();
            }
            std::path::Component::CurDir => {}
            other => components.push(other),
        }
    }
    components.iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serve_config_default() -> Result<(), Box<dyn std::error::Error>> {
        let config = ServeConfig::default();
        assert_eq!(config.port, 8080);
        assert_eq!(config.bind, "0.0.0.0");
        Ok(())
    }

    #[tokio::test]
    async fn serve_file_returns_content() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let landscape = dir.path().join("landscape");
        tokio::fs::create_dir_all(landscape.join("v1.0")).await?;
        tokio::fs::write(landscape.join("v1.0/test.json"), r#"{"ok":true}"#).await?;

        let response =
            handle_file(dir.path().to_path_buf(), "landscape/v1.0/test.json".into()).await;
        assert_eq!(response.status(), StatusCode::OK);
        Ok(())
    }

    #[tokio::test]
    async fn serve_file_returns_404_for_missing() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let response = handle_file(dir.path().to_path_buf(), "nonexistent/file.bin".into()).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        Ok(())
    }

    #[tokio::test]
    async fn serve_file_blocks_traversal() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let response = handle_file(dir.path().to_path_buf(), "../../../etc/passwd".into()).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        Ok(())
    }
}
