#[cfg(feature = "test-support")]
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Args;

use super::manage::repository_override;
use crate::deployment::config::RepositorySourceKind;
use crate::deployment::plan::{self, RepositoryChoice, TargetVersion};
use crate::deployment::state;
use crate::release::repository::provider_for;

#[derive(Debug, Args)]
pub struct Update {
    #[arg(long, value_name = "VERSION")]
    pub version: Option<String>,
    #[arg(long, num_args = 0..=1, value_name = "BASE_URL")]
    pub repository: Option<Option<String>>,
    #[arg(long)]
    pub accept_service_change: bool,
    /// Allow updating while the managed service is stopped; no .lkb backup is
    /// created in this case and automatic rollback cannot restore previous data
    #[arg(long)]
    pub allow_no_backup: bool,
    /// Terminal interaction is complete; an in-memory confirmation context is required.
    #[arg(long, hide = true)]
    pub console_confirmed: bool,
    /// In-memory confirmation context; never reconstructed from CLI flags.
    #[arg(skip)]
    pub(crate) confirmed: Option<ResolvedUpdate>,
    #[cfg(feature = "test-support")]
    #[arg(long, value_name = "PATH", hide = true)]
    pub test_runtime: Option<PathBuf>,
}

/// 交互式版本更新:选择读取渠道、解析目标版本、确认后委托 switch 流水线执行。
/// 确认发生在创建事务和备份之前,拒绝时零副作用。
pub async fn run(args: &Update, internal_worker: bool) -> ExitCode {
    if internal_worker {
        eprintln!("lkit: update requires frontend confirmation and the daemon operation protocol");
        return ExitCode::FAILURE;
    }
    let result = if args.console_confirmed {
        match args.confirmed.as_ref() {
            Some(resolved) => execute_confirmed(args, resolved).await,
            None => Err(plan::InstallError::ParameterUsage(
                "missing update confirmation context".into(),
            )),
        }
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
        run_update(args, &mut tty).await
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
    tty: &mut crate::interaction::interactive::Tty,
) -> Result<ExitCode, plan::InstallError> {
    let normalized = state::discover_landscape_root()?.ok_or_else(|| {
        plan::InstallError::ParameterUsage(crate::tr!(
            crate::keys::MANAGE_COMMAND_REQUIRES_EXISTING_INSTALLATION
        ))
    })?;
    let state = state::load_state(&normalized)?.ok_or_else(|| {
        plan::InstallError::ParameterUsage(crate::tr!(
            crate::keys::MANAGE_COMMAND_REQUIRES_EXISTING_INSTALLATION
        ))
    })?;

    let repository = match repository_override(&args.repository) {
        Some(choice) => choice,
        None => select_repository(tty)?,
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
    eprintln!("update: {}", resolved.repository_location());
    let accepted = tty.confirm(&crate::tr!(
        crate::keys::UPDATE_CONFIRM_UPDATE,
        current = resolved.current,
        target = resolved.target
    ))?;
    if !accepted {
        println!("install: {}", crate::tr!(crate::keys::UPDATE_CANCELLED));
        return Ok(ExitCode::FAILURE);
    }
    execute_confirmed(args, &resolved).await
}

/// 解析出的当前与目标版本。比较规则与 `lkit update` 命令一致。
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedUpdate {
    pub current: semver::Version,
    pub target: semver::Version,
    pub install_root: String,
    pub state_identity: String,
    pub repository: RepositoryChoice,
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
        crate::deployment::state::StateArchitecture::Riscv64 => {
            crate::release::repository::Architecture::Riscv64
        }
        crate::deployment::state::StateArchitecture::Loongarch64 => {
            crate::release::repository::Architecture::Loongarch64
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
        install_root: state.canonical_install_root.clone(),
        state_identity: crate::daemon_operations::installation_identity(state)?,
        repository: match spec.kind {
            crate::release::repository::ProviderKind::Github => {
                RepositoryChoice::Github(spec.location)
            }
            crate::release::repository::ProviderKind::Http => RepositoryChoice::Http(spec.location),
        },
    })
}

impl ResolvedUpdate {
    pub(crate) fn repository_location(&self) -> &str {
        match &self.repository {
            RepositoryChoice::Github(location) | RepositoryChoice::Http(location) => location,
            RepositoryChoice::Mirror => "unresolved mirror",
        }
    }
    fn switch_request(
        &self,
        args: &Update,
    ) -> Result<crate::daemon_operations::SwitchRequest, plan::InstallError> {
        if self.target <= self.current
            || !std::path::Path::new(&self.install_root).is_absolute()
            || matches!(self.repository, RepositoryChoice::Mirror)
            || self.state_identity.len() != 64
            || !self.state_identity.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(plan::InstallError::ParameterUsage(
                "invalid update confirmation context".into(),
            ));
        }
        plan::TargetVersion::parse(&self.target.to_string())?;
        self.repository.clone().resolve()?;
        Ok(crate::daemon_operations::SwitchRequest {
            from_version: self.current.to_string(),
            target: self.target.to_string(),
            state_identity: self.state_identity.clone(),
            repository: Some(self.repository.clone()),
            accept_service_change: args.accept_service_change,
            allow_no_backup: args.allow_no_backup,
        })
    }
}
async fn execute_confirmed(
    args: &Update,
    resolved: &ResolvedUpdate,
) -> Result<ExitCode, plan::InstallError> {
    let request = resolved.switch_request(args)?;
    let runtime = runtime(args)?;
    crate::daemon_operations::submit_confirmed_switch_and_wait(
        &runtime,
        resolved.install_root.clone(),
        request,
        args.console_confirmed,
    )
    .await
    .map(ExitCode::from)
}
fn runtime(
    _args: &Update,
) -> Result<crate::deployment::runtime::InstallRuntime, plan::InstallError> {
    #[cfg(feature = "test-support")]
    if let Some(path) = &_args.test_runtime {
        return crate::deployment::runtime::InstallRuntime::from_test_file(path);
    }
    Ok(crate::deployment::runtime::InstallRuntime::production())
}

/// 交互选择更新渠道:配置存在且有效时默认选项是记录的最新来源,其余为官方 GitHub、
/// 默认 HTTP 镜像和自定义 HTTP 仓库;文件不存在时选项从官方 GitHub 开始。
fn select_repository(
    tty: &mut crate::interaction::interactive::Tty,
) -> Result<RepositoryChoice, plan::InstallError> {
    let recorded = crate::deployment::config::load_repository()?;
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
            version: Some("latest".into()),
            repository: Some(Some("https://other.example/".into())),
            accept_service_change: true,
            allow_no_backup: false,
            console_confirmed: false,
            confirmed: None,
            #[cfg(feature = "test-support")]
            test_runtime: None,
        }
    }
    fn confirmed() -> ResolvedUpdate {
        ResolvedUpdate {
            current: semver::Version::new(1, 2, 3),
            target: semver::Version::new(1, 2, 4),
            install_root: "/fixture/landscape".into(),
            state_identity: "a".repeat(64),
            repository: RepositoryChoice::Http("https://confirmed.example/releases/".into()),
        }
    }
    #[test]
    fn confirmed_request_preserves_version_source_and_state_instead_of_form_defaults() {
        let resolved = confirmed();
        let request = resolved.switch_request(&args()).unwrap();
        assert_eq!(request.target, "1.2.4");
        assert_eq!(request.repository, Some(resolved.repository));
        assert_eq!(request.state_identity, "a".repeat(64));
        assert_eq!(request.from_version, "1.2.3");
        assert!(request.accept_service_change);
        assert!(!request.allow_no_backup);
    }
    #[test]
    fn incomplete_or_non_upgrade_confirmation_is_rejected() {
        let mut resolved = confirmed();
        resolved.repository = RepositoryChoice::Mirror;
        assert!(resolved.switch_request(&args()).is_err());
        let mut resolved = confirmed();
        resolved.state_identity.clear();
        assert!(resolved.switch_request(&args()).is_err());
        let mut resolved = confirmed();
        resolved.install_root = "relative".into();
        assert!(resolved.switch_request(&args()).is_err());
        for target in [semver::Version::new(1, 2, 3), semver::Version::new(1, 0, 0)] {
            let mut resolved = confirmed();
            resolved.target = target;
            assert!(resolved.switch_request(&args()).is_err());
        }
    }
    #[tokio::test]
    async fn stale_worker_and_bare_confirmation_flag_fail_without_io() {
        assert_eq!(run(&args(), true).await, ExitCode::FAILURE);
        let mut args = args();
        args.console_confirmed = true;
        assert_eq!(run(&args, false).await, ExitCode::from(2));
    }
}
