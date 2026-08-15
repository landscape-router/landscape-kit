use std::path::PathBuf;

use clap::{ArgMatches, CommandFactory, Parser};

use crate::commands::Commands;
use crate::deployment;
use crate::i18n::Language;
use crate::keys;

#[derive(Debug, Parser)]
#[command(name = "lkit", version)]
pub(crate) struct Cli {
    #[arg(long, hide = true)]
    pub(crate) internal_systemd_worker: bool,
    /// Do not open a terminal or prompt for input
    #[arg(long, global = true)]
    pub(crate) non_interactive: bool,
    /// Output language override: en or zh; unsupported values use English
    #[arg(long, global = true, value_name = "LANG")]
    pub(crate) lang: Option<String>,
    #[command(subcommand)]
    pub(crate) command: Option<Commands>,
}

/// 读取配置预设的语言。宽容读取:安装根无法解析(相对路径、危险目录等)、
/// `config.toml` 缺失或损坏时一律返回 `None`,语言解析回落到系统 locale。
pub(crate) fn configured_language(matches: &ArgMatches) -> Option<Language> {
    let mut leaf = matches;
    while let Some((_, sub)) = leaf.subcommand() {
        leaf = sub;
    }
    // `get_one` 对未定义的参数 id 在 debug 断言下 panic(如裸控制台、check),
    // 这里用 `try_get_one` 宽容读取:未定义时回落到环境变量与默认安装根。
    let install_dir = match leaf.try_get_one::<PathBuf>("install_dir").ok().flatten() {
        Some(path) => path.clone(),
        None => std::env::var("LKIT_INSTALL_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from(deployment::plan::DEFAULT_INSTALL_ROOT)),
    };
    let root = deployment::root::normalize_install_root(&install_dir).ok()?;
    deployment::config::load_language(&root)
}

pub(crate) fn localized_command() -> clap::Command {
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
                .mut_subcommand("delete", |command| {
                    command
                        .about(crate::tr_static!(keys::MAIN_BACKUP_DELETE_ABOUT))
                        .mut_arg("backup", |arg| {
                            arg.help(crate::tr_static!(keys::MAIN_BACKUP_ID_HELP))
                        })
                        .mut_arg("yes", |arg| {
                            arg.help(crate::tr_static!(keys::MAIN_BACKUP_DELETE_YES_HELP))
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
        .mut_subcommand("reinit", |command| {
            command
                .about(crate::tr_static!(keys::MAIN_REINIT_ABOUT))
                .mut_arg("install_dir", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_INSTALL_DIR_HELP))
                })
                .mut_arg("admin_user", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_ADMIN_USER_HELP))
                })
                .mut_arg("password_file", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_PASSWORD_FILE_HELP))
                })
                .mut_arg("allow_no_backup", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_REINIT_ALLOW_NO_BACKUP_HELP))
                })
                .mut_arg("yes", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_REINIT_YES_HELP))
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
        .mut_subcommand("set-mirror", |command| {
            command
                .about(crate::tr_static!(keys::MAIN_SET_MIRROR_ABOUT))
                .mut_arg("mirror", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_SET_MIRROR_MIRROR_HELP))
                })
                .mut_arg("list", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_SET_MIRROR_LIST_HELP))
                })
                .mut_arg("show", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_SET_MIRROR_SHOW_HELP))
                })
                .mut_arg("restore", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_SET_MIRROR_RESTORE_HELP))
                })
                .mut_arg("check", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_SET_MIRROR_CHECK_HELP))
                })
                .mut_arg("replace_security", |arg| {
                    arg.help(crate::tr_static!(
                        keys::MAIN_SET_MIRROR_REPLACE_SECURITY_HELP
                    ))
                })
                .mut_arg("yes", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_SET_MIRROR_YES_HELP))
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
        .mut_subcommand("software", |command| {
            command
                .about(crate::tr_static!(keys::MAIN_SOFTWARE_ABOUT))
                .mut_subcommand("list", |command| {
                    command.about(crate::tr_static!(keys::MAIN_SOFTWARE_LIST_ABOUT))
                })
                .mut_subcommand("install", |command| {
                    command
                        .about(crate::tr_static!(keys::MAIN_SOFTWARE_INSTALL_ABOUT))
                        .mut_arg("source", |arg| {
                            arg.help(crate::tr_static!(keys::MAIN_SOFTWARE_SOURCE_HELP))
                        })
                        .mut_arg("yes", |arg| {
                            arg.help(crate::tr_static!(keys::MAIN_SOFTWARE_YES_HELP))
                        })
                })
        })
        .mut_subcommand("uninstall", |command| {
            command
                .about(crate::tr_static!(keys::MAIN_UNINSTALL_ABOUT))
                .mut_arg("yes", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_UNINSTALL_YES_HELP))
                })
                .mut_arg("allow_no_backup", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_UNINSTALL_ALLOW_NO_BACKUP_HELP))
                })
                .mut_arg("keep_data", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_UNINSTALL_KEEP_DATA_HELP))
                })
                .mut_arg("purge_root", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_UNINSTALL_PURGE_ROOT_HELP))
                })
                .mut_arg("install_dir", |arg| {
                    arg.help(crate::tr_static!(keys::MAIN_INSTALL_DIR_HELP))
                })
        })
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
