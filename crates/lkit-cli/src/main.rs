mod backup;
mod check;
mod cli;
mod commands;
mod console;
mod deployment;
mod i18n;
mod interaction;
mod keys;
mod mirror;
mod network;
mod release;
mod report;
mod service;
mod software;
mod systemd_worker;
mod workflows;

use std::process::ExitCode;

use clap::FromArgMatches;

use cli::{Cli, configured_language, localized_command};
use commands::Commands;

rust_i18n::i18n!("locales", fallback = "en");

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

    i18n::preconfigure(std::env::args_os());
    let matches = match localized_command().try_get_matches() {
        Ok(matches) => matches,
        Err(error) => {
            let code = error.exit_code();
            i18n::print_clap_error(&error);
            return ExitCode::from(code.clamp(0, 255) as u8);
        }
    };
    let cli = match Cli::from_arg_matches(&matches) {
        Ok(cli) => cli,
        Err(error) => {
            let code = error.exit_code();
            i18n::print_clap_error(&error);
            return ExitCode::from(code.clamp(0, 255) as u8);
        }
    };
    i18n::configure(i18n::resolve_with(
        cli.lang.as_deref(),
        configured_language(&matches),
    ));
    interaction::interactive::configure(cli.non_interactive);
    let Some(command) = cli.command else {
        if cli.non_interactive || cli.internal_systemd_worker {
            eprintln!(
                "lkit: {}",
                crate::tr!(keys::MAIN_SUBCOMMAND_REQUIRED_NON_INTERACTIVE)
            );
            return ExitCode::from(2);
        }
        let interrupt = match interaction::presentation::InterruptGuard::install_console() {
            Ok(interrupt) => interrupt,
            Err(error) => {
                eprintln!(
                    "lkit: {}",
                    crate::tr!(keys::MAIN_UNABLE_INSTALL_CTRL_C_HANDLER, error = error)
                );
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
                eprintln!(
                    "lkit: {}",
                    crate::tr!(keys::MAIN_UNABLE_START_INTERACTIVE_CONSOLE, error = error)
                );
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
    let from_console = delegated_args.is_some();
    let delegated = !internal_worker && systemd_worker::should_delegate(&command);
    let interrupt = match interaction::presentation::InterruptGuard::install(delegated) {
        Ok(interrupt) => interrupt,
        Err(error) => {
            eprintln!(
                "lkit: {}",
                crate::tr!(keys::MAIN_UNABLE_INSTALL_CTRL_C_HANDLER, error = error)
            );
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
            Commands::Reinit(reinit) => reinit.interactive_password.take(),
            _ => None,
        };
        let network_plan = match &mut command {
            Commands::Install(install) => install.network_plan.take(),
            Commands::Reinit(reinit) => reinit.network_plan.take(),
            _ => None,
        };
        return match systemd_worker::delegate(
            &interrupt,
            args,
            interactive_password,
            network_plan,
            from_console,
        ) {
            Ok(code) => code,
            Err(error) => {
                eprintln!(
                    "install: {}",
                    crate::tr!(keys::MAIN_UNABLE_DELEGATE_SYSTEMD, error = error)
                );
                ExitCode::FAILURE
            }
        };
    }

    match command {
        Commands::Check(args) => commands::check::run(&args),
        Commands::Install(args) => commands::install::run(&args).await,
        Commands::Migrate(args) => commands::migrate::run(&args).await,
        Commands::Network(args) => commands::network::run(&args).await,
        Commands::Switch(args) => commands::switch::run(&args).await,
        Commands::Update(args) => commands::update::run(&args).await,
        Commands::Repair(args) => commands::repair::run(&args).await,
        Commands::Restore(args) => commands::restore::run(&args).await,
        Commands::Reinit(args) => commands::reinit::run(&args).await,
        Commands::Backup(args) => commands::backup::run(&args).await,
        Commands::Reconcile(args) => commands::reconcile::run(&args).await,
        Commands::ServiceManager(args) => commands::service_manager::run(&args).await,
        Commands::SetMirror(args) => commands::set_mirror::run(&args),
        Commands::Software(args) => commands::software::run(&args),
        Commands::Uninstall(args) => commands::uninstall::run(&args).await,
    }
}
