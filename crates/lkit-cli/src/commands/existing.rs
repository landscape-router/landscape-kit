use std::process::ExitCode;

use crate::deployment::plan;
use crate::deployment::runtime::InstallRuntime;
use crate::deployment::state;
use crate::deployment::state::{InitStatus, StateRepositoryKind, StateServiceManager};
use crate::deployment::transaction::TransactionServiceManager;
use crate::release::repository::provider_for;
use crate::workflows::install as pipeline;

use super::manage::{InstallRequest, ServiceManagerArg};

/// 已安装环境:service manager 迁移、修复、版本切换或同版本验证。
pub(super) async fn run(
    args: &InstallRequest,
    plan: &plan::Plan,
    runtime: &InstallRuntime,
) -> ExitCode {
    match run_installed_inner(args, plan, runtime).await {
        Ok(code) => code,
        Err(error) => {
            eprintln!("install: {error}");
            super::manage::exit_code(&error)
        }
    }
}

async fn run_installed_inner(
    args: &InstallRequest,
    plan: &plan::Plan,
    runtime: &InstallRuntime,
) -> Result<ExitCode, plan::InstallError> {
    let state = crate::deployment::state::load_state(&plan.root)?.ok_or_else(|| {
        plan::InstallError::CorruptedState("install state disappeared during planning".into())
    })?;

    if let Some(manager) = args.service_manager {
        if args.version.is_some()
            || args.repository.is_some()
            || args.repair_static
            || args.repair_binary
            || args.accept_service_change
            || args.admin_user.is_some()
            || args.password_file.is_some()
            || args.force
        {
            return Err(plan::InstallError::ParameterUsage(
                "--service-manager migration must be executed alone and cannot be combined with --version, --repository, --repair-static, --repair-binary, any --accept-* flag, --admin-user, --password-file, or --force"
                    .into(),
            ));
        }
        let target = match manager {
            ServiceManagerArg::Systemd => TransactionServiceManager::Systemd,
            ServiceManagerArg::None => TransactionServiceManager::None,
        };
        if target == current_manager(&state) {
            println!(
                "install: {}",
                crate::trf!(
                    ("service manager is already {}", target.key()),
                    ("服务管理器已经是 {}", target.key())
                )
            );
            return Ok(ExitCode::SUCCESS);
        }
        let health = runtime.health_options()?;
        crate::workflows::service_manager::migrate_service_manager(
            &plan.root,
            &state,
            target,
            &runtime.systemd,
            &health,
            &|prompt| crate::interaction::interactive::confirm(prompt),
        )
        .await?;
        return Ok(ExitCode::SUCCESS);
    }

    if args.repair_static || args.repair_binary {
        if args.version.is_some() {
            return Err(plan::InstallError::ParameterUsage(
                "--repair-static and --repair-binary operate on the active version and cannot be combined with --version"
                    .into(),
            ));
        }
        if args.repair_static && args.repair_binary {
            return Err(plan::InstallError::ParameterUsage(
                "--repair-static and --repair-binary cannot be combined".into(),
            ));
        }
        let (provider, _override) = resolve_provider(args, &state)?;
        let health = runtime.health_options()?;
        if args.repair_static {
            crate::workflows::repair::repair_static(&plan.root, &provider, &state).await?;
            println!(
                "install: {}",
                crate::tr!(
                    "static pages restored from the published release assets",
                    "已从发布资产恢复静态页面"
                )
            );
            return Ok(ExitCode::SUCCESS);
        }
        let data_dir = plan.root.canonical.join("data");
        let switch_options = pipeline::SwitchOptions {
            export_base_url: runtime.export_base_url.clone(),
            token: &(|| {
                crate::backup::export::read_api_token(
                    &data_dir.join("landscape_api_token"),
                    runtime.managed_uid,
                )
            }),
            confirm: &|prompt| crate::interaction::interactive::confirm(prompt),
            health: &health,
        };
        return match crate::workflows::repair::repair_binary(
            &plan.root,
            &provider,
            &state,
            &runtime.systemd,
            &switch_options,
        )
        .await
        {
            Ok(crate::workflows::repair::RepairOutcome::Committed) => {
                println!(
                    "install: {}",
                    crate::tr!(
                        "the active backend binary was restored and verified",
                        "已恢复并验证活动后端二进制文件"
                    )
                );
                Ok(ExitCode::SUCCESS)
            }
            Ok(crate::workflows::repair::RepairOutcome::RolledBack) => {
                eprintln!(
                    "install: {}",
                    crate::tr!(
                        "repairing the backend failed; rolled back to the previous binary",
                        "后端修复失败；已回滚到之前的二进制文件"
                    )
                );
                Ok(ExitCode::from(5))
            }
            Ok(crate::workflows::repair::RepairOutcome::RollbackFailed { reason }) => {
                eprintln!(
                    "install: {}",
                    crate::trf!(
                        ("repairing the backend failed and automatic rollback also failed: {reason}; manual recovery is required"),
                        ("后端修复失败，自动回滚也失败：{reason}；需要手动恢复")
                    )
                );
                Ok(ExitCode::from(6))
            }
            Err(error) => Err(error),
        };
    }

    let architecture = match state.assets.webserver.architecture {
        crate::deployment::state::StateArchitecture::X86_64 => {
            crate::release::repository::Architecture::X86_64
        }
        crate::deployment::state::StateArchitecture::Aarch64 => {
            crate::release::repository::Architecture::Aarch64
        }
    };
    let (provider, provider_override) = resolve_provider(args, &state)?;
    let resolved = match &args.version {
        Some(value) => {
            let target = plan::TargetVersion::parse(value)?;
            Some(match target {
                plan::TargetVersion::Latest => provider.latest(architecture).await?,
                plan::TargetVersion::Version(version) => {
                    Some(provider.release(&version, architecture).await?)
                }
            })
        }
        None => None,
    };
    let resolved = match resolved {
        Some(Some(release)) => Some(release),
        Some(None) => {
            return Err(plan::InstallError::NoStableVersion);
        }
        None => None,
    };
    let switching = resolved
        .as_ref()
        .is_some_and(|release| release.version.to_string() != state.active_version);
    if switching {
        let release = resolved.expect("switching requires a resolved release");
        let health_options = runtime.health_options()?;
        let data_dir = plan.root.canonical.join("data");
        let switch_options = pipeline::SwitchOptions {
            export_base_url: runtime.export_base_url.clone(),
            token: &(|| {
                crate::backup::export::read_api_token(
                    &data_dir.join("landscape_api_token"),
                    runtime.managed_uid,
                )
            }),
            confirm: &|prompt| crate::interaction::interactive::confirm(prompt),
            health: &health_options,
        };
        return match pipeline::switch_version(
            &plan.root,
            &provider,
            &state,
            release,
            &pipeline::SwitchArgs {
                allow_no_backup: args.allow_no_backup,
            },
            &runtime.systemd,
            &switch_options,
        )
        .await
        {
            Ok(pipeline::SwitchOutcome::Committed { version, backup_id }) => {
                println!(
                    "install: {}",
                    crate::trf!(
                        ("switched to version {version}"),
                        ("已切换到版本 {version}")
                    )
                );
                match backup_id {
                    Some(backup_id) => {
                        println!(
                            "install: {}",
                            crate::trf!(
                                ("backup {backup_id} preserved in backups/"),
                                ("备份 {backup_id} 已保存在 backups/")
                            )
                        );
                    }
                    None => {
                        println!(
                            "install: {}",
                            crate::tr!(
                                "no backup was created (--allow-no-backup)",
                                "未创建备份（--allow-no-backup）"
                            )
                        );
                    }
                }
                Ok(ExitCode::SUCCESS)
            }
            Ok(pipeline::SwitchOutcome::RolledBack { version, backup_id }) => {
                let backup = backup_id.map_or_else(String::new, |id| {
                    crate::trf!((" using backup {id}"), ("，使用备份 {id}"))
                });
                eprintln!(
                    "install: {}",
                    crate::trf!(
                        ("switching to the target version failed; rolled back to {version}{backup}"),
                        ("切换到目标版本失败；已回滚到 {version}{backup}")
                    )
                );
                Ok(ExitCode::from(5))
            }
            Ok(pipeline::SwitchOutcome::RollbackFailed { version, .. }) => {
                eprintln!(
                    "install: {}",
                    crate::trf!(
                        ("switching failed and automatic rollback to {version} also failed; manual recovery is required"),
                        ("切换失败，自动回滚到 {version} 也失败；需要手动恢复")
                    )
                );
                Ok(ExitCode::from(6))
            }
            Err(error) => Err(error),
        };
    }

    let data = plan.root.canonical.join("data");
    if state.initialization.status == InitStatus::Pending
        && data.join("landscape_init.lock").is_file()
        && data.join("landscape.toml").is_file()
    {
        crate::workflows::repair::observe_initialization(&plan.root, &state)?;
        println!(
            "install: {}",
            crate::tr!(
                "observed initialization completion; initialization is now complete",
                "已观察到初始化完成；初始化状态现已完成"
            )
        );
        return Ok(ExitCode::SUCCESS);
    }
    same_version_install(
        args,
        &plan.root,
        &state,
        provider_override,
        &runtime.systemd,
    )
    .await?;
    println!(
        "install: {}",
        crate::trf!(
            (
                "version {} is already installed and verified",
                state.active_version
            ),
            ("版本 {} 已安装并通过验证", state.active_version)
        )
    );
    Ok(ExitCode::SUCCESS)
}

fn current_manager(state: &state::InstallState) -> TransactionServiceManager {
    match state.service.manager {
        StateServiceManager::Systemd => TransactionServiceManager::Systemd,
        StateServiceManager::None => TransactionServiceManager::None,
    }
}

fn resolve_provider(
    args: &InstallRequest,
    state: &state::InstallState,
) -> Result<
    (
        crate::release::repository::ReleaseProvider,
        Option<plan::ProviderSpec>,
    ),
    plan::InstallError,
> {
    let provider_override = match &args.repository {
        None => None,
        Some(None) => Some(plan::RepositoryChoice::Mirror.resolve()?),
        Some(Some(url)) => Some(plan::RepositoryChoice::Http(url.clone()).resolve()?),
    };
    let state_kind = match state.repository.kind {
        StateRepositoryKind::Github => crate::release::repository::ProviderKind::Github,
        StateRepositoryKind::Http => crate::release::repository::ProviderKind::Http,
    };
    let provider = match &provider_override {
        Some(spec) => provider_for(spec.kind, spec.location.as_str())?,
        None => provider_for(state_kind, &state.repository.location)?,
    };
    Ok((provider, provider_override))
}

/// 同版本检查:验证后端摘要、初始化状态与受管 unit;显式仓库覆盖必须提供
/// 与当前安装完全相同的发布资产。
/// 所有内容正常时不下载、不重启服务。
async fn same_version_install(
    args: &InstallRequest,
    root: &crate::deployment::root::InstallRoot,
    state: &crate::deployment::state::InstallState,
    provider_override: Option<crate::deployment::plan::ProviderSpec>,
    systemd: &crate::service::systemd::Systemd,
) -> Result<(), plan::InstallError> {
    pipeline::verify_current_backend(root, state)?;
    pipeline::check_initialization(root, state)?;
    let mut updated = state.clone();

    if state.service.manager == StateServiceManager::Systemd {
        let origin = root.canonical.join("service/landscape-router.service");
        let content = std::fs::read_to_string(&origin).map_err(plan::InstallError::Io)?;
        crate::service::systemd::validate_unit(&content, &root.canonical)?;
        let actual = pipeline::hash_str(&content);
        let changed = state.service.definition_sha256.as_deref() != Some(actual.as_str());
        if changed {
            if !args.accept_service_change {
                let accepted = crate::interaction::interactive::confirm(crate::tr!(
                    "the managed service unit changed; keep it as-is? Type `yes`: ",
                    "受管服务 unit 已更改；是否原样保留？请输入 `yes`："
                ))?;
                if !accepted {
                    return Err(plan::InstallError::UserRefused(
                        "user refused to keep the modified service unit".into(),
                    ));
                }
            }
            updated.service.definition_sha256 = Some(actual);
        } else if args.accept_service_change {
            eprintln!(
                "install: {}",
                crate::tr!(
                    "warning: no managed service unit change detected; --accept-service-change ignored",
                    "警告：未检测到受管服务 unit 更改；已忽略 --accept-service-change"
                )
            );
        }
        pipeline::verify_unit_ownership(root, systemd)?;
    } else if args.accept_service_change {
        eprintln!(
            "install: {}",
            crate::tr!(
                "warning: no managed service unit exists; --accept-service-change ignored",
                "警告：不存在受管服务 unit；已忽略 --accept-service-change"
            )
        );
    }

    if let Some(spec) = provider_override {
        let spec_kind = match spec.kind {
            crate::release::repository::ProviderKind::Github => {
                crate::deployment::state::StateRepositoryKind::Github
            }
            crate::release::repository::ProviderKind::Http => {
                crate::deployment::state::StateRepositoryKind::Http
            }
        };
        let changed =
            state.repository.kind != spec_kind || state.repository.location != spec.location;
        if changed {
            let provider = provider_for(spec.kind, &spec.location)?;
            let architecture = match state.assets.webserver.architecture {
                crate::deployment::state::StateArchitecture::X86_64 => {
                    crate::release::repository::Architecture::X86_64
                }
                crate::deployment::state::StateArchitecture::Aarch64 => {
                    crate::release::repository::Architecture::Aarch64
                }
            };
            let active = plan::TargetVersion::parse(&state.active_version)?;
            let plan::TargetVersion::Version(version) = active else {
                return Err(plan::InstallError::CorruptedState(
                    "active version is not stable".into(),
                ));
            };
            let release = provider.release(&version, architecture).await?;
            if release.assets.static_archive.sha256 != state.assets.static_archive.sha256
                || release.assets.static_archive.size != state.assets.static_archive.size
            {
                return Err(plan::InstallError::UserRefused(
                    "the new repository provides different assets for the same version; refusing to switch source"
                        .into(),
                ));
            }
            // 后端摘要必须与落盘二进制一致:清单摘要针对压缩产物,须下载解压后比对。
            let check_dir = root.canonical.join("run/.source-check.tmp");
            let _ = std::fs::remove_dir_all(&check_dir);
            std::fs::create_dir_all(&check_dir).map_err(plan::InstallError::Io)?;
            let built = pipeline::fetch_webserver_asset(&release, &check_dir).await?;
            let _ = std::fs::remove_dir_all(&check_dir);
            if built.webserver_sha256 != state.assets.webserver.sha256
                || built.webserver_size != state.assets.webserver.size
            {
                return Err(plan::InstallError::UserRefused(
                    "the new repository provides a different backend binary than the recorded installation; refusing to switch source"
                        .into(),
                ));
            }
            updated.repository = crate::deployment::state::RepositorySource {
                kind: spec_kind,
                location: spec.location,
            };
        }
    }
    crate::deployment::state::write_state(root, &updated)?;
    Ok(())
}
