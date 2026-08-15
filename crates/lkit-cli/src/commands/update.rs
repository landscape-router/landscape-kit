use std::path::PathBuf;
use std::process::ExitCode;

use clap::Args;

use super::manage::{InstallRequest, RequestMode, repository_override};
use crate::deployment::config::RepositorySourceKind;
use crate::deployment::plan::{self, RepositoryChoice, TargetVersion};
use crate::deployment::root;
use crate::deployment::state;
use crate::release::repository::provider_for;

#[derive(Debug, Args)]
pub struct Update {
    #[arg(long, value_name = "VERSION")]
    pub version: Option<String>,
    #[arg(long, num_args = 0..=1, value_name = "BASE_URL")]
    pub repository: Option<Option<String>>,
    #[arg(long, value_name = "PATH")]
    pub install_dir: Option<PathBuf>,
    #[arg(long)]
    pub accept_service_change: bool,
    /// Allow updating while the managed service is stopped; no .lkb backup is
    /// created in this case and automatic rollback cannot restore previous data
    #[arg(long)]
    pub allow_no_backup: bool,
    /// The interactive console already asked for the repository and the
    /// upgrade confirmation; skip every /dev/tty prompt (delegated workers
    /// cannot read TUI keyboard input)
    #[arg(long, hide = true)]
    pub console_confirmed: bool,
    #[cfg(feature = "test-support")]
    #[arg(long, value_name = "PATH", hide = true)]
    pub test_runtime: Option<PathBuf>,
}

/// 交互式版本更新:选择读取渠道、解析目标版本、确认后委托 switch 流水线执行。
/// 确认发生在创建事务和备份之前,拒绝时零副作用。
pub async fn run(args: &Update) -> ExitCode {
    let result = if args.console_confirmed {
        run_update(args, None).await
    } else {
        let mut tty = match crate::interaction::interactive::Tty::open() {
            Ok(tty) => tty,
            Err(error) => {
                eprintln!(
                    "install: {}",
                    crate::tr!(
                        crate::keys::UPDATE_REQUIRES_INTERACTIVE_TERMINAL,
                        error = error
                    )
                );
                return ExitCode::FAILURE;
            }
        };
        run_update(args, Some(&mut tty)).await
    };
    match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("install: {error}");
            super::manage::exit_code(&error)
        }
    }
}

async fn run_update(
    args: &Update,
    mut tty: Option<&mut crate::interaction::interactive::Tty>,
) -> Result<ExitCode, plan::InstallError> {
    let install_root = plan::select_install_root(
        args.install_dir.as_deref(),
        std::env::var("LKIT_INSTALL_DIR").ok().as_deref(),
    )?;
    let normalized = root::normalize_install_root(&install_root)?;
    let state = state::load_state(&normalized)?.ok_or_else(|| {
        plan::InstallError::ParameterUsage(
            crate::tr!(crate::keys::MANAGE_COMMAND_REQUIRES_EXISTING_INSTALLATION).into(),
        )
    })?;

    let repository = match repository_override(&args.repository) {
        Some(choice) => choice,
        None => match &mut tty {
            Some(tty) => select_repository(tty, &normalized)?,
            None => crate::deployment::config::resolve_default_choice(&normalized)?,
        },
    };
    let target = match &args.version {
        Some(value) => TargetVersion::parse(value)?,
        None => TargetVersion::Latest,
    };
    let resolved = resolve_update_target(&state, &repository, &target).await?;
    match resolved.target.cmp(&resolved.current) {
        std::cmp::Ordering::Less => {
            return Err(plan::InstallError::ParameterUsage(crate::tr!(
                crate::keys::SWITCH_DOWNGRADE_NOT_SUPPORTED,
                from_version = resolved.current,
                version = resolved.target
            )));
        }
        std::cmp::Ordering::Equal => {
            println!(
                "install: {}",
                crate::tr!(
                    crate::keys::UPDATE_ALREADY_UP_TO_DATE,
                    version = resolved.current
                )
            );
            return Ok(ExitCode::SUCCESS);
        }
        std::cmp::Ordering::Greater => {}
    }
    let accepted = match &mut tty {
        Some(tty) => tty.confirm(&crate::tr!(
            crate::keys::UPDATE_CONFIRM_UPDATE,
            current = resolved.current,
            target = resolved.target
        ))?,
        None => true,
    };
    if !accepted {
        println!("install: {}", crate::tr!(crate::keys::UPDATE_CANCELLED));
        return Ok(ExitCode::FAILURE);
    }
    let request = switch_request(args, resolved.target.to_string(), repository);
    Ok(super::manage::run_request(&request).await)
}

/// 解析出的当前与目标版本。比较规则与 `lkit update` 命令一致。
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedUpdate {
    pub current: semver::Version,
    pub target: semver::Version,
}

/// 解析目标版本并与当前版本比较:只做网络只读解析,不创建事务、不下载资产。
/// 命令模式与交互控制台的 Update 面板共用,保证两种入口的解析语义一致。
pub(crate) async fn resolve_update_target(
    state: &crate::deployment::state::InstallState,
    repository: &RepositoryChoice,
    target: &TargetVersion,
) -> Result<ResolvedUpdate, plan::InstallError> {
    let spec = repository.clone().resolve()?;
    let provider = provider_for(spec.kind, spec.location.as_str())?;
    let architecture = match state.assets.webserver.architecture {
        crate::deployment::state::StateArchitecture::X86_64 => {
            crate::release::repository::Architecture::X86_64
        }
        crate::deployment::state::StateArchitecture::Aarch64 => {
            crate::release::repository::Architecture::Aarch64
        }
    };
    let release = match target {
        TargetVersion::Latest => provider
            .latest(architecture)
            .await?
            .ok_or(plan::InstallError::NoStableVersion)?,
        TargetVersion::Version(version) => provider.release(version, architecture).await?,
    };
    let current = lkit_repository::parse_stable_version(&state.active_version)
        .map_err(|_| plan::InstallError::CorruptedState("invalid active version".into()))?;
    Ok(ResolvedUpdate {
        current,
        target: release.version,
    })
}

fn switch_request(args: &Update, version: String, repository: RepositoryChoice) -> InstallRequest {
    InstallRequest {
        mode: RequestMode::Switch,
        version: Some(version),
        repository: Some(repository),
        install_dir: args.install_dir.clone(),
        admin_user: None,
        password_file: None,
        interactive_password: None,
        repair_static: false,
        repair_binary: false,
        allow_no_backup: args.allow_no_backup,
        accept_service_change: args.accept_service_change,
        force: false,
        takeover_network: false,
        network_plan: None,
        console_confirmed: args.console_confirmed,
        #[cfg(feature = "test-support")]
        test_runtime: args.test_runtime.clone(),
    }
}

/// 交互选择更新渠道:配置存在且有效时默认选项是记录的最新来源,其余为官方 GitHub、
/// 默认 HTTP 镜像和自定义 HTTP 仓库;文件不存在时选项从官方 GitHub 开始。
fn select_repository(
    tty: &mut crate::interaction::interactive::Tty,
    root: &crate::deployment::root::InstallRoot,
) -> Result<RepositoryChoice, plan::InstallError> {
    let recorded = crate::deployment::config::load_repository(root)?;
    let options = match &recorded {
        Some(source) => {
            let kind = match source.kind {
                RepositorySourceKind::Github => "github",
                RepositorySourceKind::Http => "http",
            };
            vec![
                crate::tr!(
                    crate::keys::UPDATE_REPOSITORY_CURRENT,
                    kind = kind,
                    location = source.location
                ),
                crate::tr!(crate::keys::UPDATE_REPOSITORY_GITHUB),
                crate::tr!(crate::keys::UPDATE_REPOSITORY_MIRROR),
                crate::tr!(crate::keys::UPDATE_REPOSITORY_CUSTOM),
            ]
        }
        None => vec![
            crate::tr!(crate::keys::UPDATE_REPOSITORY_GITHUB),
            crate::tr!(crate::keys::UPDATE_REPOSITORY_MIRROR),
            crate::tr!(crate::keys::UPDATE_REPOSITORY_CUSTOM),
        ],
    };
    let selected = tty.select_one(
        &crate::tr!(crate::keys::UPDATE_SELECT_REPOSITORY),
        &options,
        Some(0),
    )?;
    match (recorded.as_ref(), selected) {
        (Some(source), 0) => match source.kind {
            RepositorySourceKind::Github => Ok(RepositoryChoice::Github(source.location.clone())),
            RepositorySourceKind::Http => Ok(RepositoryChoice::Http(source.location.clone())),
        },
        (None, 0) | (Some(_), 1) => Ok(RepositoryChoice::Github(
            crate::release::repository::github::DEFAULT_REPOSITORY.into(),
        )),
        (Some(_), 2) | (None, 1) => Ok(RepositoryChoice::Mirror),
        _ => {
            let url = tty.input(&crate::tr!(crate::keys::UPDATE_REPOSITORY_URL))?;
            Ok(RepositoryChoice::Http(url))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args() -> Update {
        Update {
            version: None,
            repository: None,
            install_dir: Some(PathBuf::from("/tmp/lkit-update-test")),
            accept_service_change: true,
            allow_no_backup: true,
            console_confirmed: false,
            #[cfg(feature = "test-support")]
            test_runtime: Some(PathBuf::from("/tmp/lkit-update-runtime.json")),
        }
    }

    #[test]
    fn forwards_every_selected_repository_to_the_switch_request() {
        for repository in [
            RepositoryChoice::Github("ThisSeanZhang/landscape".into()),
            RepositoryChoice::Mirror,
            RepositoryChoice::Http("https://example.com/releases/".into()),
        ] {
            let request = switch_request(&args(), "1.2.4".into(), repository.clone());
            assert_eq!(request.repository, Some(repository));
            assert_eq!(request.version.as_deref(), Some("1.2.4"));
            assert_eq!(
                request.install_dir.as_deref(),
                Some(std::path::Path::new("/tmp/lkit-update-test"))
            );
            assert!(request.accept_service_change);
            assert!(request.allow_no_backup);
            assert!(!request.console_confirmed);
        }
    }

    #[test]
    fn forwards_console_confirmation_to_the_switch_request() {
        let mut args = args();
        args.console_confirmed = true;
        let request = switch_request(&args, "1.2.4".into(), RepositoryChoice::Mirror);
        assert!(request.console_confirmed);
    }
}
