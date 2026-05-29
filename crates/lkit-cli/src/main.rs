//! lkit — Landscape local CLI management and rescue tool.

mod cli;
mod commands;
mod launcher;
mod messages;
mod wizard;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use lkit_app::AppState;
use lkit_client::{FileLogReader, LandscapeClient, SystemInstaller, SystemdManager};
use lkit_core::{HostInstaller, LandscapePaths, ManagerPaths};

use crate::cli::{Cli, Commands};
use crate::messages::msg;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Initialize tracing: stderr, default WARN, -v for INFO, -vv for DEBUG.
    let log_level = match cli.verbose {
        0 => "warn",
        1 => "info",
        _ => "debug",
    };
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(log_level)),
        )
        .init();

    // Self version can run without Landscape HOME.
    if let Some(Commands::SelfCmd(ref args)) = cli.command
        && let crate::cli::SelfAction::Version = args.action
    {
        return commands::self_cmd::run(crate::cli::SelfArgs {
            action: crate::cli::SelfAction::Version,
        })
        .await;
    }

    let host_installer: Arc<dyn HostInstaller> = Arc::new(SystemInstaller::new());

    // Install command runs without requiring existing Landscape HOME.
    if let Some(Commands::Install(args)) = cli.command {
        return commands::install::run(args, host_installer).await;
    }

    // All other commands require an existing Landscape installation.
    let landscape_home = std::env::var("LANDSCAPE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dirs_or_default());

    let landscape_paths = LandscapePaths::new(landscape_home.clone());

    if !landscape_paths.home.exists() {
        eprintln!("{}", msg("error.not_installed"));
        std::process::exit(3);
    }

    let manager_paths = ManagerPaths::new(manager_home());

    let base_url = parse_api_listen(&landscape_paths.landscape_config)
        .unwrap_or_else(|| "http://127.0.0.1:8080".to_string());

    let client: Arc<dyn lkit_core::LkitClient> = Arc::new(LandscapeClient::new(base_url)?);
    let service_manager: Arc<dyn lkit_core::ServiceManager> =
        Arc::new(SystemdManager::new("landscape.service"));
    let log_reader: Arc<dyn lkit_core::LogReader> =
        Arc::new(FileLogReader::new(landscape_paths.logs_dir.clone()));

    let state = AppState::new(
        client,
        service_manager,
        log_reader,
        landscape_paths,
        manager_paths,
    );

    // Dispatch remaining commands (Install already handled above).
    match cli.command {
        Some(Commands::Install(args)) => commands::install::run(args, host_installer).await,
        Some(cmd) => commands::dispatch(cmd, &state).await,
        None => launcher::run(&state).await,
    }
}

/// Default Landscape HOME: ~/.landscape-router
fn dirs_or_default() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    PathBuf::from(home).join(".landscape-router")
}

/// Default manager HOME: ~/.landscape-kit
fn manager_home() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    PathBuf::from(home).join(".landscape-kit")
}

/// Parse api.listen from landscape.toml. Returns None if file missing or unparseable.
fn parse_api_listen(config_path: &Path) -> Option<String> {
    let content = std::fs::read_to_string(config_path).ok()?;
    let value: toml::Value = content.parse().ok()?;
    let listen = value.get("api")?.get("listen")?.as_str()?;
    Some(format!("http://{listen}"))
}
