use std::process::ExitCode;

use clap::{Args, Subcommand};

use crate::interaction::interactive::Tty;
use crate::interaction::plan;
use crate::mirror::{self, Host};
use crate::software::{self, DockerSource, SoftwareError};

#[derive(Debug, Args)]
pub struct Software {
    #[command(subcommand)]
    pub action: Option<SoftwareAction>,
}

#[derive(Debug, Subcommand)]
pub enum SoftwareAction {
    /// List common software with installed status
    List,
    /// Install a common software
    Install(SoftwareInstall),
}

#[derive(Debug, Args)]
pub struct SoftwareInstall {
    /// Software to install: docker
    pub software: software::Software,
    /// Source to install from: official, aliyun, tencent, huawei, tuna or ustc
    #[arg(long, value_enum, value_name = "SOURCE")]
    pub source: Option<DockerSource>,
    /// Skip the interactive confirmation
    #[arg(long)]
    pub yes: bool,
}

pub fn run(args: &Software) -> ExitCode {
    let host = match mirror::detect_host() {
        Ok(host) => host,
        Err(error) => return fail(error),
    };
    match &args.action {
        Some(SoftwareAction::List) => run_list(&host),
        Some(SoftwareAction::Install(install)) => run_install(&host, install),
        None => run_interactive(&host),
    }
}

fn run_list(host: &Host) -> ExitCode {
    println!(
        "software: {}",
        crate::tr!(
            crate::keys::SOFTWARE_LIST_HEADER,
            family = host.family.label()
        )
    );
    for software in software::Software::all() {
        let status = if software.installed() {
            crate::tr!(crate::keys::SOFTWARE_INSTALLED)
        } else {
            crate::tr!(crate::keys::SOFTWARE_NOT_INSTALLED)
        };
        println!("  - {} ({}) [{}]", software.label(), software.id(), status);
    }
    ExitCode::SUCCESS
}

fn run_install(host: &Host, install: &SoftwareInstall) -> ExitCode {
    let source = match resolve_source(host, install) {
        Ok(source) => source,
        Err(code) => return code,
    };
    if let Err(error) = require_root() {
        return fail(error);
    }
    if !install.yes && !crate::interaction::interactive::is_non_interactive() {
        let mut tty = match Tty::open() {
            Ok(tty) => tty,
            Err(error) => return fail_install(&error),
        };
        let confirmed = match tty.confirm(&crate::tr!(
            crate::keys::SOFTWARE_CONFIRM_INSTALL,
            software = install.software.label(),
            source = source.label()
        )) {
            Ok(confirmed) => confirmed,
            Err(error) => return fail_install(&error),
        };
        if !confirmed {
            println!("software: {}", crate::tr!(crate::keys::SOFTWARE_CANCELLED));
            return ExitCode::FAILURE;
        }
    }
    execute_install(host, install.software, source, true)
}

/// 无参数且非交互：需要至少一个参数。
fn run_interactive(host: &Host) -> ExitCode {
    if crate::interaction::interactive::is_non_interactive() {
        return fail_usage(crate::tr!(crate::keys::SOFTWARE_REQUIRES_ARGS));
    }
    if let Err(error) = require_root() {
        return fail(error);
    }
    let mut tty = match Tty::open() {
        Ok(tty) => tty,
        Err(error) => return fail_install(&error),
    };
    let options: Vec<String> = software::Software::all()
        .into_iter()
        .map(|software| {
            let status = if software.installed() {
                crate::tr!(crate::keys::SOFTWARE_INSTALLED)
            } else {
                crate::tr!(crate::keys::SOFTWARE_NOT_INSTALLED)
            };
            format!("{} [{}]", software.label(), status)
        })
        .collect();
    let selected = match tty.select_one(
        &crate::tr!(crate::keys::SOFTWARE_SELECT_SOFTWARE),
        &options,
        None,
    ) {
        Ok(selected) => selected,
        Err(error) => return fail_install(&error),
    };
    let software = software::Software::all()[selected];
    if software.installed() {
        println!(
            "software: {}",
            crate::tr!(
                crate::keys::SOFTWARE_ALREADY_INSTALLED,
                software = software.label()
            )
        );
        return ExitCode::FAILURE;
    }
    let source = match prompt_source(host, &mut tty) {
        Ok(source) => source,
        Err(code) => return code,
    };
    execute_install(host, software, source, false)
}

/// 解析安装来源：显式 `--source` 直接使用；否则交互选择，非交互时
/// 报参数使用错误（与 `set-mirror` 的 `MIRROR` 语义一致）。
fn resolve_source(host: &Host, install: &SoftwareInstall) -> Result<DockerSource, ExitCode> {
    if let Some(source) = install.source {
        return Ok(source);
    }
    if crate::interaction::interactive::is_non_interactive() {
        return Err(fail_usage(crate::tr!(crate::keys::SOFTWARE_REQUIRES_ARGS)));
    }
    let mut tty = match Tty::open() {
        Ok(tty) => tty,
        Err(error) => return Err(fail_install(&error)),
    };
    prompt_source(host, &mut tty)
}

fn prompt_source(host: &Host, tty: &mut Tty) -> Result<DockerSource, ExitCode> {
    let options: Vec<String> = DockerSource::all().into_iter().map(|s| s.label()).collect();
    match tty.select_one(
        &crate::tr!(
            crate::keys::SOFTWARE_SELECT_SOURCE,
            family = host.family.label()
        ),
        &options,
        Some(0),
    ) {
        Ok(selected) => Ok(DockerSource::all()[selected]),
        Err(error) => Err(fail_install(&error)),
    }
}

/// 执行安装并把软件包管理器输出流到终端。
fn execute_install(
    host: &Host,
    software: software::Software,
    source: DockerSource,
    stream: bool,
) -> ExitCode {
    let mut phase = |_phase: software::InstallPhase| {};
    match software::install(host, software, source, stream, &mut phase) {
        Ok(()) => {
            println!(
                "software: {}",
                crate::tr!(
                    crate::keys::SOFTWARE_INSTALLED_OK,
                    software = software.label(),
                    source = source.label()
                )
            );
            ExitCode::SUCCESS
        }
        Err(error) => fail(error),
    }
}

fn require_root() -> Result<(), SoftwareError> {
    if software::root_allowed() {
        Ok(())
    } else {
        Err(SoftwareError::Message(crate::tr!(
            crate::keys::SOFTWARE_ROOT_REQUIRED
        )))
    }
}

/// 输出 `software: <error>` 并返回失败退出码。
fn fail(error: impl std::fmt::Display) -> ExitCode {
    eprintln!("software: {error}");
    ExitCode::FAILURE
}

/// 输出 `software: <error>` 并按错误类型映射退出码：参数类错误返回 `2`，
/// 其余普通失败返回 `1`。
fn fail_install(error: &plan::InstallError) -> ExitCode {
    match error {
        plan::InstallError::ParameterUsage(_) => fail_usage(error),
        _ => fail(error),
    }
}

/// 输出 `software: <error>` 并返回参数使用错误退出码 `2`。
fn fail_usage(error: impl std::fmt::Display) -> ExitCode {
    eprintln!("software: {error}");
    ExitCode::from(2)
}

#[cfg(test)]
mod tests {
    use clap::{Command, FromArgMatches};

    use super::*;

    fn parse(args: &[&str]) -> Result<Software, clap::Error> {
        let command = <Software as Args>::augment_args(Command::new("software"));
        let matches = command.try_get_matches_from(args)?;
        Software::from_arg_matches(&matches)
    }

    #[test]
    fn parses_install_with_explicit_source() {
        let software = parse(&["software", "install", "docker", "--source", "aliyun"]).unwrap();
        let SoftwareAction::Install(install) = software.action.unwrap() else {
            panic!("expected install action");
        };
        assert_eq!(install.software, software::Software::Docker);
        assert_eq!(install.source, Some(DockerSource::Aliyun));
        assert!(!install.yes);
    }

    #[test]
    fn parses_install_without_source() {
        let software = parse(&["software", "install", "docker"]).unwrap();
        let SoftwareAction::Install(install) = software.action.unwrap() else {
            panic!("expected install action");
        };
        assert_eq!(install.software, software::Software::Docker);
        assert_eq!(install.source, None);
    }

    #[test]
    fn parses_list_action() {
        let software = parse(&["software", "list"]).unwrap();
        assert!(matches!(software.action, Some(SoftwareAction::List)));
    }

    #[test]
    fn bare_software_command_is_interactive() {
        let software = parse(&["software"]).unwrap();
        assert!(software.action.is_none());
    }

    #[test]
    fn rejects_unknown_software() {
        assert!(parse(&["software", "install", "podman"]).is_err());
    }

    #[test]
    fn rejects_invalid_source() {
        assert!(parse(&["software", "install", "docker", "--source", "evil"]).is_err());
    }
}
