mod backup;
mod check;
mod commands;
mod deployment;
mod interaction;
mod network;
mod release;
mod report;
mod service;
mod systemd_worker;
mod workflows;

use std::process::ExitCode;

use clap::Parser;

use commands::Commands;

#[derive(Debug, Parser)]
#[command(name = "lkit", version)]
struct Cli {
    #[arg(long, hide = true)]
    internal_systemd_worker: bool,
    #[command(subcommand)]
    command: Commands,
}

#[tokio::main]
async fn main() -> ExitCode {
    let mut raw_args = std::env::args_os();
    let _program = raw_args.next();
    if raw_args.next().as_deref() == Some(std::ffi::OsStr::new("__systemd-worker")) {
        let Some(request) = raw_args.next() else {
            eprintln!("lkit worker: missing request path");
            return ExitCode::FAILURE;
        };
        return systemd_worker::run_worker(std::path::Path::new(&request));
    }

    dotenvy::dotenv().ok();

    let cli = Cli::parse();

    if !cli.internal_systemd_worker && systemd_worker::should_delegate(&cli.command) {
        return match systemd_worker::delegate() {
            Ok(code) => code,
            Err(error) => {
                eprintln!("install: unable to delegate operation to systemd: {error}");
                ExitCode::FAILURE
            }
        };
    }

    match cli.command {
        Commands::Check(args) => commands::check::run(&args),
        Commands::Install(args) => commands::install::run(&args).await,
        Commands::Network(args) => commands::network::run(&args).await,
        Commands::Switch(args) => commands::switch::run(&args).await,
        Commands::Repair(args) => commands::repair::run(&args).await,
        Commands::Reconcile(args) => commands::reconcile::run(&args).await,
        Commands::ServiceManager(args) => commands::service_manager::run(&args).await,
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::Cli;

    #[test]
    fn reports_package_version() {
        assert_eq!(
            Cli::command().render_version().to_string().trim(),
            concat!("lkit ", env!("CARGO_PKG_VERSION"))
        );
    }
}
