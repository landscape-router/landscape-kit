use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse};
use axum::routing::get;
use axum::{Json, Router};
use axum_server::tls_rustls::RustlsConfig;
use clap::Parser;
use lkit_test_fixture::contract::{
    API_TOKEN, DATABASE, DOCS_PATH, EXPORT_PATH, INIT_CONFIG, INIT_LOCK, LANDSCAPE_CONFIG,
};
use lkit_test_fixture::{
    FIXTURE_BUILD_VERSION, FIXTURE_CONFIG_ENV, FIXTURE_CONFIG_FILE, LandscapeApiResponse,
    LandscapeFixtureConfig, Scenario, export_response_with_content,
};
use tokio::net::{TcpListener, UdpSocket};

#[derive(Debug, Parser)]
#[command(name = "landscape-webserver")]
struct Args {
    #[arg(short, long)]
    web: Option<PathBuf>,

    #[arg(short, long)]
    config_dir: PathBuf,
}

#[derive(Clone)]
struct AppState {
    config: LandscapeFixtureConfig,
    config_dir: PathBuf,
}

#[tokio::main]
pub async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("landscape fixture: {error:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<()> {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let args = Args::parse();
    let config_path = resolve_config_path(
        std::env::var_os(FIXTURE_CONFIG_ENV).map(PathBuf::from),
        args.web.as_deref(),
    )?;
    let config = LandscapeFixtureConfig::read(&config_path)?;
    validate_build_version(&config, FIXTURE_BUILD_VERSION)?;

    if config.scenario == Scenario::StartExit {
        let code = u8::try_from(config.start_exit_code.clamp(1, 255)).unwrap_or(1);
        std::process::exit(code.into());
    }
    if config.scenario == Scenario::DelayedReady {
        tokio::time::sleep(Duration::from_millis(config.ready_delay_ms)).await;
    }

    prepare_data(&args.config_dir, &config)?;
    if let Some(web) = &args.web {
        anyhow::ensure!(
            web.is_dir(),
            "web directory {} does not exist",
            web.display()
        );
    }

    let dns_tcp = TcpListener::bind(SocketAddr::new(config.listen_address, config.dns_tcp_port))
        .await
        .with_context(|| format!("bind DNS TCP port {}", config.dns_tcp_port))?;
    let dns_udp = UdpSocket::bind(SocketAddr::new(config.listen_address, config.dns_udp_port))
        .await
        .with_context(|| format!("bind DNS UDP port {}", config.dns_udp_port))?;
    let http = TcpListener::bind(SocketAddr::new(config.listen_address, config.http_port))
        .await
        .with_context(|| format!("bind HTTP port {}", config.http_port))?;

    let router = Router::new()
        .route(DOCS_PATH, get(docs))
        .route(EXPORT_PATH, get(export_config))
        .with_state(AppState {
            config: config.clone(),
            config_dir: args.config_dir.clone(),
        });
    let tls = tls_config().await?;
    let https_address = SocketAddr::new(config.listen_address, config.https_port);
    let https_handle = axum_server::Handle::new();

    let http_task = tokio::spawn({
        let router = router.clone();
        async move {
            axum::serve(http, router)
                .await
                .context("serve fixture HTTP endpoint")
        }
    });
    let https_task = tokio::spawn({
        let handle = https_handle.clone();
        async move {
            axum_server::bind_rustls(https_address, tls)
                .handle(handle)
                .serve(router.into_make_service())
                .await
                .context("serve fixture HTTPS endpoint")
        }
    });
    let dns_tcp_task = tokio::spawn(hold_tcp_listener(dns_tcp));
    let dns_udp_task = tokio::spawn(hold_udp_socket(dns_udp));

    if config.scenario == Scenario::ExitDuringStability {
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(config.exit_after_ms)).await;
            std::process::exit(1);
        });
    }

    tokio::select! {
        signal = shutdown_signal() => signal?,
        result = http_task => result.context("join fixture HTTP task")??,
        result = https_task => result.context("join fixture HTTPS task")??,
        result = dns_tcp_task => result.context("join fixture DNS TCP task")??,
        result = dns_udp_task => result.context("join fixture DNS UDP task")??,
    }
    https_handle.shutdown();
    Ok(())
}

fn resolve_config_path(env_path: Option<PathBuf>, web: Option<&Path>) -> Result<PathBuf> {
    env_path
        .or_else(|| web.map(|path| path.join(FIXTURE_CONFIG_FILE)))
        .context(
            "LKIT_LANDSCAPE_FIXTURE_CONFIG is not set and fixture config is missing from --web",
        )
}

fn validate_build_version(
    config: &LandscapeFixtureConfig,
    build_version: Option<&str>,
) -> Result<()> {
    if let Some(build_version) = build_version {
        anyhow::ensure!(
            build_version == config.export_version,
            "fixture build version {build_version} does not match configured export version {}",
            config.export_version
        );
    }
    Ok(())
}

fn prepare_data(data_dir: &Path, config: &LandscapeFixtureConfig) -> Result<()> {
    std::fs::create_dir_all(data_dir)
        .with_context(|| format!("create data directory {}", data_dir.display()))?;
    let database = data_dir.join(DATABASE);
    if !database.exists() {
        write_private(&database, b"lkit fixture database\n")?;
    }
    let api_token = data_dir.join(API_TOKEN);
    if !api_token.exists() {
        write_readonly(&api_token, b"lkit-fixture-api-token\n")?;
    }
    if config.scenario != Scenario::MissingInitArtifacts {
        let landscape_config = data_dir.join(LANDSCAPE_CONFIG);
        if !landscape_config.exists() {
            let content = match std::fs::read(data_dir.join(INIT_CONFIG)) {
                Ok(content) => content,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    config.export_content.as_bytes().to_vec()
                }
                Err(error) => return Err(error).context("read fixture init config"),
            };
            write_private(&landscape_config, &content)?;
        }
        let init_lock = data_dir.join(INIT_LOCK);
        if !init_lock.exists() {
            write_private(
                &init_lock,
                b"Landscape initialization completed by lkit test fixture.\n",
            )?;
        }
    }
    Ok(())
}

fn write_private(path: &Path, content: &[u8]) -> Result<()> {
    std::fs::write(path, content).with_context(|| format!("write {}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("set permissions on {}", path.display()))
}

fn write_readonly(path: &Path, content: &[u8]) -> Result<()> {
    std::fs::write(path, content).with_context(|| format!("write {}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o400))
        .with_context(|| format!("set permissions on {}", path.display()))
}

async fn tls_config() -> Result<RustlsConfig> {
    let certified = rcgen::generate_simple_self_signed(vec!["localhost".into()])
        .context("generate fixture TLS certificate")?;
    let cert = certified.cert.pem();
    let key = certified.signing_key.serialize_pem();
    RustlsConfig::from_pem(cert.into_bytes(), key.into_bytes())
        .await
        .context("load fixture TLS certificate")
}

async fn docs(State(state): State<AppState>) -> impl IntoResponse {
    if state.config.scenario == Scenario::HealthError {
        return (StatusCode::SERVICE_UNAVAILABLE, Html("fixture unhealthy"));
    }
    (StatusCode::OK, Html("Landscape fixture API documentation"))
}

async fn export_config(State(state): State<AppState>) -> impl IntoResponse {
    if state.config.scenario == Scenario::ExportError {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"message": "fixture export failure"})),
        )
            .into_response();
    }
    let content = match tokio::fs::read_to_string(state.config_dir.join(LANDSCAPE_CONFIG)).await {
        Ok(content) => content,
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "message": format!("fixture config read failure: {error}")
                })),
            )
                .into_response();
        }
    };
    let response: LandscapeApiResponse<_> = export_response_with_content(&state.config, content);
    (StatusCode::OK, Json(response)).into_response()
}

async fn hold_tcp_listener(listener: TcpListener) -> Result<()> {
    loop {
        let (_stream, _) = listener.accept().await.context("accept fixture DNS TCP")?;
    }
}

async fn hold_udp_socket(socket: UdpSocket) -> Result<()> {
    let mut buffer = [0u8; 512];
    loop {
        let (size, peer) = socket
            .recv_from(&mut buffer)
            .await
            .context("receive fixture DNS UDP")?;
        socket
            .send_to(&buffer[..size], peer)
            .await
            .context("reply from fixture DNS UDP")?;
    }
}

async fn shutdown_signal() -> Result<()> {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .context("install SIGTERM handler")?;
    tokio::select! {
        result = tokio::signal::ctrl_c() => result.context("wait for Ctrl-C")?,
        _ = terminate.recv() => {},
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "lkit-landscape-fixture-{name}-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn environment_config_takes_precedence() {
        let env_path = PathBuf::from("/tmp/from-env.json");
        let web = Path::new("/tmp/static");
        assert_eq!(
            resolve_config_path(Some(env_path.clone()), Some(web)).unwrap(),
            env_path
        );
    }

    #[test]
    fn static_config_is_used_without_environment() {
        let web = Path::new("/tmp/static");
        assert_eq!(
            resolve_config_path(None, Some(web)).unwrap(),
            web.join(FIXTURE_CONFIG_FILE)
        );
    }

    #[test]
    fn build_version_must_match_export_version() {
        let config = LandscapeFixtureConfig::default();
        assert!(validate_build_version(&config, Some("0.22.0")).is_ok());
        assert!(validate_build_version(&config, Some("1.0.0")).is_err());
    }

    #[test]
    fn existing_data_is_not_overwritten() {
        let root = temp_dir("preserve-data");
        std::fs::write(root.join(DATABASE), b"database marker\n").unwrap();
        std::fs::write(root.join(LANDSCAPE_CONFIG), b"config marker\n").unwrap();
        std::fs::write(root.join(INIT_LOCK), b"lock marker\n").unwrap();
        prepare_data(&root, &LandscapeFixtureConfig::default()).unwrap();
        assert_eq!(
            std::fs::read(root.join(DATABASE)).unwrap(),
            b"database marker\n"
        );
        assert_eq!(
            std::fs::read(root.join(LANDSCAPE_CONFIG)).unwrap(),
            b"config marker\n"
        );
        assert_eq!(
            std::fs::read(root.join(INIT_LOCK)).unwrap(),
            b"lock marker\n"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn init_config_seeds_runtime_config() {
        let root = temp_dir("init-config");
        std::fs::write(root.join(INIT_CONFIG), b"restored marker = true\n").unwrap();
        prepare_data(&root, &LandscapeFixtureConfig::default()).unwrap();
        assert_eq!(
            std::fs::read(root.join(LANDSCAPE_CONFIG)).unwrap(),
            b"restored marker = true\n"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
