use std::path::PathBuf;
use std::process::ExitCode;

use clap::Args;

use super::manage::{InstallRequest, RequestMode};
use crate::deployment::plan::{self, RepositoryChoice, TargetVersion};
use crate::deployment::root;
use crate::deployment::state::{self, StateRepositoryKind};
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
    #[cfg(feature = "test-support")]
    #[arg(long, value_name = "PATH", hide = true)]
    pub test_runtime: Option<PathBuf>,
}

/// 交互式版本更新:选择读取渠道、解析目标版本、确认后委托 switch 流水线执行。
/// 确认发生在创建事务和备份之前,拒绝时零副作用。
pub async fn run(args: &Update) -> ExitCode {
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
    match run_update(args, &mut tty).await {
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

    let repository = match &args.repository {
        Some(None) => RepositoryChoice::Mirror,
        Some(Some(url)) => RepositoryChoice::Http(url.clone()),
        None => select_repository(tty, &state)?,
    };
    let spec = repository.clone().resolve()?;
    let provider = provider_for(spec.kind, spec.location.as_str())?;
    let architecture = match state.assets.webserver.architecture {
        state::StateArchitecture::X86_64 => crate::release::repository::Architecture::X86_64,
        state::StateArchitecture::Aarch64 => crate::release::repository::Architecture::Aarch64,
    };
    let target = match &args.version {
        Some(value) => TargetVersion::parse(value)?,
        None => TargetVersion::Latest,
    };
    let release = match target {
        TargetVersion::Latest => provider
            .latest(architecture)
            .await?
            .ok_or(plan::InstallError::NoStableVersion)?,
        TargetVersion::Version(version) => provider.release(&version, architecture).await?,
    };
    let current = lkit_repository::parse_stable_version(&state.active_version)
        .map_err(|_| plan::InstallError::CorruptedState("invalid active version".into()))?;
    match release.version.cmp(&current) {
        std::cmp::Ordering::Less => {
            return Err(plan::InstallError::ParameterUsage(crate::tr!(
                crate::keys::SWITCH_DOWNGRADE_NOT_SUPPORTED,
                from_version = current,
                version = release.version
            )));
        }
        std::cmp::Ordering::Equal => {
            println!(
                "install: {}",
                crate::tr!(crate::keys::UPDATE_ALREADY_UP_TO_DATE, version = current)
            );
            return Ok(ExitCode::SUCCESS);
        }
        std::cmp::Ordering::Greater => {}
    }
    let accepted = tty.confirm(&crate::tr!(
        crate::keys::UPDATE_CONFIRM_UPDATE,
        current = current,
        target = release.version
    ))?;
    if !accepted {
        println!("install: {}", crate::tr!(crate::keys::UPDATE_CANCELLED));
        return Ok(ExitCode::FAILURE);
    }
    let request = switch_request(args, release.version.to_string(), repository);
    Ok(super::manage::run_request(&request).await)
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
        service_manager: None,
        repair_static: false,
        repair_binary: false,
        allow_no_backup: args.allow_no_backup,
        accept_service_change: args.accept_service_change,
        force: false,
        takeover_network: false,
        network_plan: None,
        #[cfg(feature = "test-support")]
        test_runtime: args.test_runtime.clone(),
    }
}

/// 交互选择更新渠道:默认选项是 state 记录的当前来源,其余为官方 GitHub、
/// 默认 HTTP 镜像和自定义 HTTP 仓库。
fn select_repository(
    tty: &mut crate::interaction::interactive::Tty,
    state: &state::InstallState,
) -> Result<RepositoryChoice, plan::InstallError> {
    let kind = match state.repository.kind {
        StateRepositoryKind::Github => "github",
        StateRepositoryKind::Http => "http",
    };
    let options = vec![
        crate::tr!(
            crate::keys::UPDATE_REPOSITORY_CURRENT,
            kind = kind,
            location = state.repository.location
        ),
        crate::tr!(crate::keys::UPDATE_REPOSITORY_GITHUB),
        crate::tr!(crate::keys::UPDATE_REPOSITORY_MIRROR),
        crate::tr!(crate::keys::UPDATE_REPOSITORY_CUSTOM),
    ];
    let selected = tty.select_one(
        &crate::tr!(crate::keys::UPDATE_SELECT_REPOSITORY),
        &options,
        Some(0),
    )?;
    match selected {
        0 => match state.repository.kind {
            StateRepositoryKind::Github => Ok(RepositoryChoice::Github),
            StateRepositoryKind::Http => {
                Ok(RepositoryChoice::Http(state.repository.location.clone()))
            }
        },
        1 => Ok(RepositoryChoice::Github),
        2 => Ok(RepositoryChoice::Mirror),
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
            #[cfg(feature = "test-support")]
            test_runtime: Some(PathBuf::from("/tmp/lkit-update-runtime.json")),
        }
    }

    #[test]
    fn forwards_every_selected_repository_to_the_switch_request() {
        for repository in [
            RepositoryChoice::Github,
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
        }
    }
}
