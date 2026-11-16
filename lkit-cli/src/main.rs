mod backup;
mod check;
mod cli;
mod commands;
mod console;
mod daemon;
mod daemon_operations;
mod daemon_supervisor;
mod deployment;
mod flare;
mod frontend;
mod i18n;
mod interaction;
mod keys;
mod mirror;
mod network;
mod proxy;
mod release;
mod report;
mod service;
mod software;
mod task_names;
mod workflows;

use std::process::ExitCode;

use clap::FromArgMatches;

use cli::{Cli, configured_language, localized_command};
use commands::Commands;

rust_i18n::i18n!("locales", fallback = "en");

fn main() -> ExitCode {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name_fn(
            landscape_terrain_proto::concurrency::runtime_thread_name_fn(task_names::RUNTIME),
        )
        .build()
        .expect("build lkit runtime")
        .block_on(async_main())
}

async fn async_main() -> ExitCode {
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
        if cli.non_interactive {
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
            Ok(console::ConsoleAction::Command { command, .. }) => run_command(command, true).await,
            Err(error) => {
                eprintln!(
                    "lkit: {}",
                    crate::tr!(keys::MAIN_UNABLE_START_INTERACTIVE_CONSOLE, error = error)
                );
                ExitCode::FAILURE
            }
        };
    };
    run_command(command, false).await
}

async fn run_command(command: Commands, from_console: bool) -> ExitCode {
    let interrupt_result = if from_console
        && matches!(
            &command,
            Commands::Update(_) | Commands::Restore(_) | Commands::Reinit(_) | Commands::Install(_)
        ) {
        interaction::presentation::InterruptGuard::install_console()
    } else {
        interaction::presentation::InterruptGuard::install()
    };
    let interrupt = match interrupt_result {
        Ok(interrupt) => interrupt,
        Err(error) => {
            eprintln!(
                "lkit: {}",
                crate::tr!(keys::MAIN_UNABLE_INSTALL_CTRL_C_HANDLER, error = error)
            );
            return ExitCode::FAILURE;
        }
    };

    match command {
        Commands::Check(args) => commands::check::run(&args),
        Commands::Install(args) => commands::install::run(&args).await,
        Commands::Migrate(args) => commands::migrate::run(&args, &interrupt).await,
        Commands::Network(args) => commands::network::run(&args).await,
        Commands::Switch(args) => commands::switch::run(&args).await,
        Commands::Update(args) => commands::update::run(&args).await,
        Commands::Repair(args) => commands::repair::run(&args).await,
        Commands::Restore(args) => commands::restore::run(&args).await,
        Commands::Reinit(args) => commands::reinit::run(&args).await,
        Commands::Backup(args) => commands::backup::run(&args).await,
        Commands::Reconcile(args) => commands::reconcile::run(&args).await,
        Commands::SetMirror(args) => commands::set_mirror::run(&args),
        Commands::Software(args) => commands::software::run(&args),
        Commands::Uninstall(args) => commands::uninstall::run(&args).await,
        Commands::Self_(args) => commands::lkit_self::run(&args).await,
        Commands::Flare(args) => commands::flare::run(&args).await,
        Commands::Frontend(args) => commands::frontend::run(&args),
        Commands::Operation(args) => commands::operation::run(&args),
        Commands::Daemon(args) => daemon::run(&args).await,
    }
}
