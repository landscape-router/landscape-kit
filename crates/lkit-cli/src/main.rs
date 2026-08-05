mod backup;
mod check;
mod commands;
mod console;
mod deployment;
mod i18n;
mod interaction;
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
                crate::tr!(
                    "a subcommand is required in non-interactive mode",
                    "非交互模式必须指定子命令"
                )
            );
            return ExitCode::from(2);
        }
        let interrupt = match interaction::presentation::InterruptGuard::install_console() {
            Ok(interrupt) => interrupt,
            Err(error) => {
                eprintln!(
                    "lkit: {}",
                    crate::trf!(
                        ("unable to install Ctrl+C handler: {error}"),
                        ("无法安装 Ctrl+C 处理器：{error}")
                    )
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
                    crate::trf!(
                        ("unable to start interactive console: {error}"),
                        ("无法启动交互控制台：{error}")
                    )
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
            arg.help(crate::tr!(
                "Do not open a terminal or prompt for input",
                "不打开终端，也不提示输入"
            ))
        })
        .mut_arg("lang", |arg| {
            arg.help(crate::tr!(
                "Output language override: en or zh; unsupported values use English",
                "输出语言覆盖：en 或 zh；不支持的值使用英文"
            ))
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
                .about(crate::tr!("Check host readiness", "检查主机部署条件"))
                .mut_arg("verbose", |arg| {
                    arg.help(crate::tr!(
                        "Show details for every check",
                        "输出每个检查项的详细信息"
                    ))
                })
                .mut_arg("color", |arg| {
                    arg.help(crate::tr!(
                        "Color output: auto (default), always, or never",
                        "颜色输出：auto（默认）、always 或 never"
                    ))
                    .hide_default_value(crate::i18n::current() == Language::Zh)
                    .hide_possible_values(crate::i18n::current() == Language::Zh)
                })
        })
        .mut_subcommand("install", |command| {
            command
                .about(crate::tr!("Install Landscape", "安装 Landscape"))
                .mut_arg("version", |arg| {
                    arg.help(crate::tr!(
                        "Target version: a stable version or latest",
                        "目标版本：稳定版本号或 latest"
                    ))
                })
                .mut_arg("repository", |arg| {
                    arg.help(crate::tr!(
                        "Release repository; omit the value to use the default HTTP mirror",
                        "发布仓库；省略值时使用默认 HTTP 镜像"
                    ))
                })
                .mut_arg("install_dir", |arg| {
                    arg.help(crate::tr!("Full install root directory", "完整安装根目录"))
                })
                .mut_arg("admin_user", |arg| {
                    arg.help(crate::tr!(
                        "Initial administrator username (default: admin)",
                        "初始管理员用户名（默认：admin）"
                    ))
                })
                .mut_arg("password_file", |arg| {
                    arg.help(crate::tr!(
                        "Read the initial password from a restricted file",
                        "从权限受限的文件读取初始密码"
                    ))
                })
                .mut_arg("service_manager", |arg| {
                    arg.help(crate::tr!(
                        "Service manager: systemd or none",
                        "服务管理器：systemd 或 none"
                    ))
                })
                .mut_arg("force", |arg| {
                    arg.help(crate::tr!(
                        "Prompt for manual cleanup of an existing directory",
                        "提示手动清理现有目录"
                    ))
                })
                .mut_arg("takeover_network", |arg| {
                    arg.help(crate::tr!(
                        "Interactively hand host network ownership to Landscape",
                        "交互式将主机网络交给 Landscape 管理"
                    ))
                })
        })
        .mut_subcommand("network", |command| {
            command
                .about(crate::tr!(
                    "Manage host network takeover",
                    "管理主机网络接管"
                ))
                .mut_arg("install_dir", |arg| {
                    arg.help(crate::tr!("Full install root directory", "完整安装根目录"))
                })
                .mut_subcommand("status", |command| {
                    command.about(crate::tr!(
                        "Show the network takeover transaction",
                        "显示网络接管事务"
                    ))
                })
                .mut_subcommand("confirm", |command| {
                    command.about(crate::tr!(
                        "Confirm the new network after reconnecting",
                        "重新连接后确认新网络"
                    ))
                })
                .mut_subcommand("rollback", |command| {
                    command.about(crate::tr!(
                        "Restore the host network state saved before takeover",
                        "恢复接管前保存的主机网络状态"
                    ))
                })
        })
        .mut_subcommand("switch", |command| {
            command
                .about(crate::tr!(
                    "Switch Landscape versions",
                    "切换 Landscape 版本"
                ))
                .mut_arg("version", |arg| {
                    arg.help(crate::tr!(
                        "Target stable version or latest",
                        "目标稳定版本或 latest"
                    ))
                })
                .mut_arg("repository", |arg| {
                    arg.help(crate::tr!(
                        "Optional release repository override",
                        "可选的发布仓库覆盖"
                    ))
                })
                .mut_arg("install_dir", |arg| {
                    arg.help(crate::tr!("Full install root directory", "完整安装根目录"))
                })
                .mut_arg("accept_service_change", |arg| {
                    arg.help(crate::tr!(
                        "Accept a modified managed systemd unit",
                        "接受已修改的受管 systemd unit"
                    ))
                })
                .mut_arg("allow_no_backup", |arg| {
                    arg.help(crate::tr!(
                        "Allow switching a stopped service without a configuration backup",
                        "允许在服务停止时不创建配置备份进行切换"
                    ))
                })
        })
        .mut_subcommand("repair", |command| {
            command
                .about(crate::tr!("Repair an installation", "修复现有安装"))
                .mut_arg("target", |arg| {
                    arg.help(crate::tr!(
                        "Asset to repair: static or binary",
                        "修复目标：static 或 binary"
                    ))
                })
                .mut_arg("repository", |arg| {
                    arg.help(crate::tr!(
                        "Optional release repository override",
                        "可选的发布仓库覆盖"
                    ))
                })
                .mut_arg("install_dir", |arg| {
                    arg.help(crate::tr!("Full install root directory", "完整安装根目录"))
                })
        })
        .mut_subcommand("reconcile", |command| {
            command
                .about(crate::tr!(
                    "Reconcile managed installation state",
                    "协调受管安装状态"
                ))
                .mut_arg("repository", |arg| {
                    arg.help(crate::tr!(
                        "Optional release repository override",
                        "可选的发布仓库覆盖"
                    ))
                })
                .mut_arg("install_dir", |arg| {
                    arg.help(crate::tr!("Full install root directory", "完整安装根目录"))
                })
                .mut_arg("accept_service_change", |arg| {
                    arg.help(crate::tr!(
                        "Accept a modified managed systemd unit",
                        "接受已修改的受管 systemd unit"
                    ))
                })
        })
        .mut_subcommand("service-manager", |command| {
            command
                .about(crate::tr!("Change the service manager", "变更服务管理器"))
                .mut_arg("target", |arg| {
                    arg.help(crate::tr!(
                        "Target service manager: systemd or none",
                        "目标服务管理器：systemd 或 none"
                    ))
                })
                .mut_arg("install_dir", |arg| {
                    arg.help(crate::tr!("Full install root directory", "完整安装根目录"))
                })
        })
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
            eprintln!(
                "lkit: {}",
                crate::trf!(
                    ("unable to install Ctrl+C handler: {error}"),
                    ("无法安装 Ctrl+C 处理器：{error}")
                )
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
        return match systemd_worker::delegate(&interrupt, args, interactive_password) {
            Ok(code) => code,
            Err(error) => {
                eprintln!(
                    "install: {}",
                    crate::trf!(
                        ("unable to delegate operation to systemd: {error}"),
                        ("无法将操作委托给 systemd：{error}")
                    )
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
