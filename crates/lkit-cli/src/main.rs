use clap::Command;
use tracing_subscriber::EnvFilter;

/// Current version of lkit, set by Cargo from the workspace package version.
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing: stderr output, default WARN, overridable via RUST_LOG.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .init();

    let _matches = Command::new("lkit")
        .version(VERSION)
        .about("Landscape local CLI management and rescue tool")
        .get_matches();

    // Subcommands will be added in M1.
    // With no subcommand, clap prints help and exits.

    Ok(())
}
