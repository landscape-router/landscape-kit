use std::process::ExitCode;

use crate::deployment::plan;
use crate::deployment::runtime::InstallRuntime;
use crate::deployment::state;
use crate::deployment::state::{InitStatus, StateServiceManager};
use crate::release::repository::provider_for;
use crate::workflows::install as pipeline;

use super::manage::InstallRequest;

/// 已安装环境:修复、版本切换或同版本验证。
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
        let (provider, _override) = resolve_provider(args, &plan.root)?;
        let health = runtime.health_options()?;
        if args.repair_static {
            crate::workflows::repair::repair_static(&plan.root, &provider, &state).await?;
            println!(
                "install: {}",
                crate::tr!(crate::keys::EXISTING_STATIC_PAGES_RESTORED)
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
            runtime.service_manager.as_ref(),
            &switch_options,
        )
        .await
        {
            Ok(crate::workflows::repair::RepairOutcome::Committed) => {
                println!(
                    "install: {}",
                    crate::tr!(crate::keys::EXISTING_BACKEND_RESTORED_AND_VERIFIED)
                );
                Ok(ExitCode::SUCCESS)
            }
            Ok(crate::workflows::repair::RepairOutcome::RolledBack) => {
                eprintln!(
                    "install: {}",
                    crate::tr!(crate::keys::EXISTING_REPAIR_FAILED_ROLLED_BACK)
                );
                Ok(ExitCode::from(5))
            }
            Ok(crate::workflows::repair::RepairOutcome::RollbackFailed { reason }) => {
                eprintln!(
                    "install: {}",
                    crate::tr!(
                        crate::keys::EXISTING_REPAIR_FAILED_ROLLBACK_FAILED,
                        reason = reason
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
    // 只有需要解析版本时才读取配置来源;显式 `--repository` 完全绕过配置。
    let version_request = args
        .version
        .as_deref()
        .map(plan::TargetVersion::parse)
        .transpose()?;
    let release = if let Some(target) = version_request {
        let (provider, _override) = resolve_provider(args, &plan.root)?;
        Some(match target {
            plan::TargetVersion::Latest => provider
                .latest(architecture)
                .await?
                .ok_or(plan::InstallError::NoStableVersion)?,
            plan::TargetVersion::Version(version) => {
                provider.release(&version, architecture).await?
            }
        })
    } else {
        None
    };
    if let Some(release) = release {
        if release.version.to_string() != state.active_version {
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
                confirm: &|prompt| {
                    if args.console_confirmed {
                        // 控制台分发路径:确认已在 TUI 内完成,worker 无法读取键盘。
                        Ok(true)
                    } else {
                        crate::interaction::interactive::confirm(prompt)
                    }
                },
                health: &health_options,
            };
            return match pipeline::switch_version(
                &plan.root,
                &state,
                release,
                &pipeline::SwitchArgs {
                    allow_no_backup: args.allow_no_backup,
                },
                runtime.service_manager.as_ref(),
                &switch_options,
            )
            .await
            {
                Ok(pipeline::SwitchOutcome::Committed { version, backup_id }) => {
                    println!(
                        "install: {}",
                        crate::tr!(crate::keys::EXISTING_SWITCHED_TO_VERSION, version = version)
                    );
                    match backup_id {
                        Some(backup_id) => {
                            println!(
                                "install: {}",
                                crate::tr!(
                                    crate::keys::EXISTING_BACKUP_PRESERVED,
                                    backup_id = backup_id
                                )
                            );
                        }
                        None => {
                            println!(
                                "install: {}",
                                crate::tr!(crate::keys::EXISTING_NO_BACKUP_CREATED)
                            );
                        }
                    }
                    Ok(ExitCode::SUCCESS)
                }
                Ok(pipeline::SwitchOutcome::RolledBack { version, backup_id }) => {
                    let backup = backup_id.map_or_else(String::new, |id| {
                        crate::tr!(crate::keys::EXISTING_ROLLED_BACK_USING_BACKUP, id = id)
                    });
                    eprintln!(
                        "install: {}",
                        crate::tr!(
                            crate::keys::EXISTING_SWITCH_FAILED_ROLLED_BACK,
                            version = version,
                            backup = backup
                        )
                    );
                    Ok(ExitCode::from(5))
                }
                Ok(pipeline::SwitchOutcome::RollbackFailed { version, .. }) => {
                    eprintln!(
                        "install: {}",
                        crate::tr!(
                            crate::keys::EXISTING_SWITCH_FAILED_ROLLBACK_FAILED,
                            version = version
                        )
                    );
                    Ok(ExitCode::from(6))
                }
                Err(error) => Err(error),
            };
        }
    }

    let data = plan.root.canonical.join("data");
    if state.initialization.status == InitStatus::Pending
        && data.join("landscape_init.lock").is_file()
        && data.join("landscape.toml").is_file()
    {
        crate::workflows::repair::observe_initialization(&plan.root, &state)?;
        println!(
            "install: {}",
            crate::tr!(crate::keys::EXISTING_OBSERVED_INITIALIZATION_COMPLETION)
        );
        return Ok(ExitCode::SUCCESS);
    }
    let provider_override = args
        .repository
        .clone()
        .map(plan::RepositoryChoice::resolve)
        .transpose()?;
    same_version_install(
        args,
        &plan.root,
        &state,
        provider_override,
        runtime.service_manager.as_ref(),
    )
    .await?;
    println!(
        "install: {}",
        crate::tr!(
            crate::keys::EXISTING_VERSION_INSTALLED_AND_VERIFIED,
            version = state.active_version
        )
    );
    Ok(ExitCode::SUCCESS)
}

fn resolve_provider(
    args: &InstallRequest,
    root: &crate::deployment::root::InstallRoot,
) -> Result<
    (
        crate::release::repository::ReleaseProvider,
        Option<plan::ProviderSpec>,
    ),
    plan::InstallError,
> {
    let provider_override = args
        .repository
        .clone()
        .map(plan::RepositoryChoice::resolve)
        .transpose()?;
    let provider = match &provider_override {
        Some(spec) => provider_for(spec.kind, spec.location.as_str())?,
        None => {
            let spec = crate::deployment::config::resolve_default_choice(root)?.resolve()?;
            provider_for(spec.kind, spec.location.as_str())?
        }
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
    systemd: &dyn crate::service::manager::ServiceManager,
) -> Result<(), plan::InstallError> {
    pipeline::verify_current_backend(root, state)?;
    pipeline::check_initialization(root, state)?;
    let mut updated = state.clone();

    if state.service.manager == StateServiceManager::Systemd {
        let origin = root.canonical.join("service/landscape-router.service");
        let content = std::fs::read_to_string(&origin).map_err(plan::InstallError::Io)?;
        systemd.validate_definition(
            crate::service::manager::ManagedService::LandscapeRouter,
            &content,
            &root.canonical,
        )?;
        let actual = pipeline::hash_str(&content);
        let changed = state.service.definition_sha256.as_deref() != Some(actual.as_str());
        if changed {
            if !args.accept_service_change {
                let accepted = crate::interaction::interactive::confirm(&crate::tr!(
                    crate::keys::EXISTING_KEEP_MODIFIED_UNIT
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
                crate::tr!(crate::keys::EXISTING_ACCEPT_SERVICE_CHANGE_IGNORED_UNIT)
            );
        }
        pipeline::verify_unit_ownership(root, systemd)?;
    } else if args.accept_service_change {
        eprintln!(
            "install: {}",
            crate::tr!(crate::keys::EXISTING_ACCEPT_SERVICE_CHANGE_IGNORED_NO_UNIT)
        );
    }

    if let Some(spec) = provider_override {
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
    }
    crate::deployment::state::write_state(root, &updated)?;
    Ok(())
}
