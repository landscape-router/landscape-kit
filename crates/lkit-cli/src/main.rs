mod backup;
mod check;
mod commands;
mod console;
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
    /// Do not open a terminal or prompt for input
    #[arg(long, global = true)]
    non_interactive: bool,
    #[command(subcommand)]
    command: Option<Commands>,
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
    interaction::interactive::configure(cli.non_interactive);
    let Some(command) = cli.command else {
        if cli.non_interactive || cli.internal_systemd_worker {
            eprintln!("lkit: a subcommand is required in non-interactive mode");
            return ExitCode::from(2);
        }
        let interrupt = match interaction::presentation::InterruptGuard::install_console() {
            Ok(interrupt) => interrupt,
            Err(error) => {
                eprintln!("lkit: unable to install Ctrl+C handler: {error}");
                return ExitCode::FAILURE;
            }
        };
        let action = console::run();
        drop(interrupt);
        return match action {
            Ok(console::ConsoleAction::Quit) => ExitCode::SUCCESS,
            Ok(console::ConsoleAction::Command { command, args }) => {
                run_command(command, Some(args), false).await
            }
            Err(error) => {
                eprintln!("lkit: unable to start interactive console: {error}");
                ExitCode::FAILURE
            }
        };
    };
    run_command(command, None, cli.internal_systemd_worker).await
}

async fn run_command(
    mut command: Commands,
    delegated_args: Option<Vec<String>>,
    internal_worker: bool,
) -> ExitCode {
    let delegated = !internal_worker && systemd_worker::should_delegate(&command);
    let interrupt = match interaction::presentation::InterruptGuard::install(delegated) {
        Ok(interrupt) => interrupt,
        Err(error) => {
            eprintln!("lkit: unable to install Ctrl+C handler: {error}");
            return ExitCode::FAILURE;
        }
    };

    if delegated {
        let args = match delegated_args {
            Some(args) => args,
            None => match systemd_worker::string_args() {
                Ok(args) => args,
                Err(error) => {
                    eprintln!("lkit: {error}");
                    return ExitCode::FAILURE;
                }
            },
        };
        let interactive_password = match &mut command {
            Commands::Install(install) => install.interactive_password.take(),
            _ => None,
        };
        return match systemd_worker::delegate(&interrupt, args, interactive_password) {
            Ok(code) => code,
            Err(error) => {
                eprintln!("install: unable to delegate operation to systemd: {error}");
                ExitCode::FAILURE
            }
        };
    }

    match command {
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
    use clap::{CommandFactory, Parser};

    use super::Cli;

    #[test]
    fn reports_package_version() {
        assert_eq!(
            Cli::command().render_version().to_string().trim(),
            concat!("lkit ", env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn accepts_non_interactive_before_or_after_subcommand() {
        for args in [
            ["lkit", "--non-interactive", "install"],
            ["lkit", "install", "--non-interactive"],
        ] {
            assert!(Cli::try_parse_from(args).unwrap().non_interactive);
        }
    }

    #[test]
    fn accepts_bare_command_for_interactive_console() {
        let cli = Cli::try_parse_from(["lkit"]).unwrap();
        assert!(cli.command.is_none());
        assert!(!cli.non_interactive);
    }
}
