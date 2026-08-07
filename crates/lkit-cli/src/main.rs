mod backup;
mod check;
mod commands;
mod console;
mod deployment;
mod i18n;
mod interaction;
mod keys;
mod network;
mod release;
mod report;
mod service;
mod systemd_worker;
mod workflows;

use std::process::ExitCode;

use clap::{CommandFactory, FromArgMatches, Parser};

use commands::Commands;
use i18n::Language;

rust_i18n::i18n!("locales", fallback = "en");

#[derive(Debug, Parser)]
#[command(name = "lkit", version)]
struct Cli {
    #[arg(long, hide = true)]
    internal_systemd_worker: bool,
    /// Do not open a terminal or prompt for input
    #[arg(long, global = true)]
    non_interactive: bool,
    /// Output language override: en or zh; unsupported values use English
    #[arg(long, global = true, value_name = "LANG")]
    lang: Option<String>,
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
    i18n::configure(i18n::resolve(cli.lang.as_deref()));
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

fn localized_command() -> clap::Command {
    let command = Cli::command()
        .mut_arg("non_interactive", |arg| {
            arg.help(crate::tr_static!(keys::MAIN_NON_INTERACTIVE_HELP))
        })
        .mut_arg("lang", |arg| {
            arg.help(crate::tr_static!(keys::MAIN_LANG_HELP))
        });
    let command = localize_subcommands(command);
    if crate::i18n::current() == Language::Zh {
        localize_help(command)
    } else {
        command
    }
}

const ZH_ROOT_HELP_TEMPLATE: &str = "{before-help}{about-with-newline}\n用法：{usage}\n\n命令：\n{subcommands}\n选项：\n{options}{after-help}";
const ZH_COMMAND_HELP_TEMPLATE: &str =
    "{before-help}{about-with-newline}\n用法：{usage}\n\n选项：\n{options}{after-help}";

fn localize_help(mut command: clap::Command) -> clap::Command {
    let has_subcommands = command.has_subcommands();
    let has_version = command.get_version().is_some();
    command = command
        .disable_help_subcommand(true)
        .disable_help_flag(true)
        .arg(
            clap::Arg::new("help")
                .short('h')
                .long("help")
                .help("打印帮助")
                .action(clap::ArgAction::Help),
        )
        .help_template(if has_subcommands {
            ZH_ROOT_HELP_TEMPLATE
        } else {
            ZH_COMMAND_HELP_TEMPLATE
        });
    if has_version {
        command = command.disable_version_flag(true).arg(
            clap::Arg::new("version")
                .short('V')
                .long("version")
                .help("打印版本")
                .action(clap::ArgAction::Version),
        );
    }
    command.mut_subcommands(localize_help)
}

fn localize_subcommands(command: clap::Command) -> clap::Command {
    command
        .mut_subcommand("check", |command| {
            command
                .about(crate::tr_static!(keys::MAIN_CHECK_ABOUT))
                .mut_arg("verbose", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_VERBOSE_HELP))
                })
                .mut_arg("color", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_COLOR_HELP))
                        .hide_default_value(crate::i18n::current() == Language::Zh)
                        .hide_possible_values(crate::i18n::current() == Language::Zh)
                })
        })
        .mut_subcommand("install", |command| {
            command
                .about(crate::tr_static!(keys::MAIN_INSTALL_ABOUT))
                .mut_arg("version", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_VERSION_HELP))
                })
                .mut_arg("repository", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_REPOSITORY_HELP))
                })
                .mut_arg("install_dir", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_INSTALL_DIR_HELP))
                })
                .mut_arg("admin_user", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_ADMIN_USER_HELP))
                })
                .mut_arg("password_file", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_PASSWORD_FILE_HELP))
                })
                .mut_arg("service_manager", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_SERVICE_MANAGER_HELP))
                })
                .mut_arg("force", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_FORCE_HELP))
                })
                .mut_arg("takeover_network", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_TAKEOVER_NETWORK_HELP))
                })
        })
        .mut_subcommand("network", |command| {
            command
                .about(crate::tr_static!(keys::MAIN_NETWORK_ABOUT))
                .mut_arg("install_dir", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_INSTALL_DIR_HELP))
                })
                .mut_subcommand("status", |command| {
                    command.about(crate::tr_static!(keys::MAIN_NETWORK_STATUS_ABOUT))
                })
                .mut_subcommand("confirm", |command| {
                    command.about(crate::tr_static!(keys::MAIN_NETWORK_CONFIRM_ABOUT))
                })
                .mut_subcommand("rollback", |command| {
                    command.about(crate::tr_static!(keys::MAIN_NETWORK_ROLLBACK_ABOUT))
                })
        })
        .mut_subcommand("switch", |command| {
            command
                .about(crate::tr_static!(keys::MAIN_SWITCH_ABOUT))
                .mut_arg("version", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_SWITCH_VERSION_HELP))
                })
                .mut_arg("repository", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_REPOSITORY_OVERRIDE_HELP))
                })
                .mut_arg("install_dir", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_INSTALL_DIR_HELP))
                })
                .mut_arg("accept_service_change", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_ACCEPT_SERVICE_CHANGE_HELP))
                })
                .mut_arg("allow_no_backup", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_ALLOW_NO_BACKUP_HELP))
                })
        })
        .mut_subcommand("backup", |command| {
            command
                .about(crate::tr_static!(keys::MAIN_BACKUP_ABOUT))
                .mut_subcommand("create", |command| {
                    command
                        .about(crate::tr_static!(keys::MAIN_BACKUP_CREATE_ABOUT))
                        .mut_arg("remark", |arg| {
                            arg.help(crate::tr_static!(keys::MAIN_BACKUP_REMARK_HELP))
                        })
                        .mut_arg("output", |arg| {
                            arg.help(crate::tr_static!(keys::MAIN_BACKUP_OUTPUT_HELP))
                        })
                        .mut_arg("install_dir", |arg| {
                            arg.help(crate::tr_static!(keys::MAIN_INSTALL_DIR_HELP))
                        })
                })
                .mut_subcommand("list", |command| {
                    command.about(crate::tr_static!(keys::MAIN_BACKUP_LIST_ABOUT))
                })
                .mut_subcommand("show", |command| {
                    command
                        .about(crate::tr_static!(keys::MAIN_BACKUP_SHOW_ABOUT))
                        .mut_arg("backup", |arg| {
                            arg.help(crate::tr_static!(keys::MAIN_BACKUP_ID_HELP))
                        })
                        .mut_arg("file", |arg| {
                            arg.help(crate::tr_static!(keys::MAIN_BACKUP_FILE_HELP))
                        })
                        .mut_arg("install_dir", |arg| {
                            arg.help(crate::tr_static!(keys::MAIN_INSTALL_DIR_HELP))
                        })
                })
                .mut_subcommand("verify", |command| {
                    command
                        .about(crate::tr_static!(keys::MAIN_BACKUP_VERIFY_ABOUT))
                        .mut_arg("backup", |arg| {
                            arg.help(crate::tr_static!(keys::MAIN_BACKUP_ID_HELP))
                        })
                        .mut_arg("file", |arg| {
                            arg.help(crate::tr_static!(keys::MAIN_BACKUP_FILE_HELP))
                        })
                        .mut_arg("install_dir", |arg| {
                            arg.help(crate::tr_static!(keys::MAIN_INSTALL_DIR_HELP))
                        })
                })
        })
        .mut_subcommand("restore", |command| {
            command
                .about(crate::tr_static!(keys::MAIN_RESTORE_ABOUT))
                .mut_arg("backup", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_BACKUP_ID_HELP))
                })
                .mut_arg("file", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_BACKUP_FILE_HELP))
                })
                .mut_arg("install_dir", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_INSTALL_DIR_HELP))
                })
                .mut_arg("allow_no_backup", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_RESTORE_ALLOW_NO_BACKUP_HELP))
                })
                .mut_arg("yes", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_RESTORE_YES_HELP))
                })
        })
        .mut_subcommand("update", |command| {
            command
                .about(crate::tr_static!(keys::MAIN_UPDATE_ABOUT))
                .mut_arg("version", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_UPDATE_VERSION_HELP))
                })
                .mut_arg("repository", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_REPOSITORY_OVERRIDE_HELP))
                })
                .mut_arg("install_dir", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_INSTALL_DIR_HELP))
                })
                .mut_arg("accept_service_change", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_ACCEPT_SERVICE_CHANGE_HELP))
                })
                .mut_arg("allow_no_backup", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_ALLOW_NO_BACKUP_HELP))
                })
        })
        .mut_subcommand("repair", |command| {
            command
                .about(crate::tr_static!(keys::MAIN_REPAIR_ABOUT))
                .mut_arg("target", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_REPAIR_TARGET_HELP))
                })
                .mut_arg("repository", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_REPOSITORY_OVERRIDE_HELP))
                })
                .mut_arg("install_dir", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_INSTALL_DIR_HELP))
                })
        })
        .mut_subcommand("reconcile", |command| {
            command
                .about(crate::tr_static!(keys::MAIN_RECONCILE_ABOUT))
                .mut_arg("repository", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_REPOSITORY_OVERRIDE_HELP))
                })
                .mut_arg("install_dir", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_INSTALL_DIR_HELP))
                })
                .mut_arg("accept_service_change", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_ACCEPT_SERVICE_CHANGE_HELP))
                })
        })
        .mut_subcommand("service-manager", |command| {
            command
                .about(crate::tr_static!(keys::MAIN_SERVICE_MANAGER_ABOUT))
                .mut_arg("target", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_SERVICE_MANAGER_TARGET_HELP))
                })
                .mut_arg("install_dir", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_INSTALL_DIR_HELP))
                })
        })
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
            _ => None,
        };
        let network_plan = match &mut command {
            Commands::Install(install) => install.network_plan.take(),
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
        Commands::Network(args) => commands::network::run(&args).await,
        Commands::Switch(args) => commands::switch::run(&args).await,
        Commands::Update(args) => commands::update::run(&args).await,
        Commands::Repair(args) => commands::repair::run(&args).await,
        Commands::Restore(args) => commands::restore::run(&args).await,
        Commands::Backup(args) => commands::backup::run(&args).await,
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
    fn accepts_language_before_or_after_subcommand() {
        for args in [
            ["lkit", "--lang", "zh", "check"],
            ["lkit", "check", "--lang", "zh"],
        ] {
            assert_eq!(
                Cli::try_parse_from(args).unwrap().lang.as_deref(),
                Some("zh")
            );
        }
    }

    #[test]
    fn accepts_bare_command_for_interactive_console() {
        let cli = Cli::try_parse_from(["lkit"]).unwrap();
        assert!(cli.command.is_none());
        assert!(!cli.non_interactive);
    }
}
