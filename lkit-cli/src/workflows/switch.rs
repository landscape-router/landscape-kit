use super::artifacts::{WEBSERVER_BINARY, build_release};
use super::health::{self, DocsProbe, HealthOptions};
use super::manager::{ManagedService, ServiceManager};
use super::pipeline::{
    activate_current, architecture_from_state, build_switched_state, capture_before,
    check_initialization, parse_stable_version, verify_current_backend, write_unit_origin,
};
use super::plan::InstallError;
use super::repository::Release;
use super::root::InstallRoot;
use super::state::{InstallState, StateServiceManager};
use super::transaction::Phase;
use crate::deployment::layout;

/// 版本切换参数。
pub(crate) struct SwitchArgs {
    /// 允许在受管服务已停止时切换:不导出配置快照、不创建 `.lkb`。
    pub allow_no_backup: bool,
}

#[derive(Debug)]
pub(crate) enum SwitchOutcome {
    Committed {
        version: semver::Version,
        backup_id: Option<String>,
    },
    RolledBack {
        version: semver::Version,
        backup_id: Option<String>,
    },
    RollbackFailed {
        version: semver::Version,
        reason: String,
    },
}

/// 版本切换运行参数(测试可注入)。
pub(crate) struct SwitchOptions<'a, P: DocsProbe> {
    /// 配置导出 API 的 base URL。
    pub export_base_url: String,
    /// 读取 API token(生产从 `data/landscape_api_token` 读取)。
    pub token: &'a dyn Fn() -> Result<String, InstallError>,
    /// 交互确认(生产为 `/dev/tty`)。
    pub confirm: &'a dyn Fn(&str) -> Result<bool, InstallError>,
    pub health: &'a HealthOptions<P>,
}

/// 版本切换流水线。目标资产在停止当前服务前全部下载完成;
/// systemd 环境由 `lkit` 停止/启动并做健康检查,失败时用 `.lkb` 回滚;
/// 无 systemd 环境要求用户在 `/dev/tty` 确认已自行停止实例。
pub(crate) async fn switch_version<P: DocsProbe>(
    root: &InstallRoot,
    state: &InstallState,
    release: Release,
    args: &SwitchArgs,
    manager: &dyn ServiceManager,
    options: &SwitchOptions<'_, P>,
) -> Result<SwitchOutcome, InstallError> {
    let from_version = parse_stable_version(&state.active_version).map_err(|error| {
        InstallError::CorruptedState(format!("invalid active version: {error}"))
    })?;
    match release.version.cmp(&from_version) {
        std::cmp::Ordering::Less => {
            return Err(InstallError::ParameterUsage(crate::tr!(
                crate::keys::SWITCH_DOWNGRADE_NOT_SUPPORTED,
                from_version = from_version,
                version = release.version
            )));
        }
        std::cmp::Ordering::Equal => {
            return Err(InstallError::ParameterUsage(crate::tr!(
                crate::keys::SWITCH_TARGET_VERSION_ALREADY_ACTIVE
            )));
        }
        std::cmp::Ordering::Greater => {}
    }
    let architecture = architecture_from_state(state);
    check_initialization(root, state)?;
    verify_current_backend(root, state)?;

    let is_systemd = state.service.manager == StateServiceManager::Systemd;
    let service_stopped = is_systemd && !manager.is_active(ManagedService::LandscapeRouter)?;
    if service_stopped && !args.allow_no_backup {
        return Err(InstallError::ServiceNotRunning(
            "the managed service is stopped; start it with `systemctl start landscape-router.service` and retry, or re-run with --allow-no-backup to switch without a configuration snapshot"
                .into(),
        ));
    }
    if service_stopped {
        eprintln!(
            "install: {}",
            crate::tr!(crate::keys::SWITCH_WARNING_SERVICE_STOPPED_NO_BACKUP)
        );
    } else if args.allow_no_backup {
        eprintln!(
            "install: {}",
            crate::tr!(crate::keys::SWITCH_WARNING_ALLOW_NO_BACKUP_IGNORED)
        );
    }

    let transaction =
        super::transaction::TransactionFile::new_switch(root, &from_version, &release.version)?;
    let mut transaction = transaction;
    transaction.no_backup = service_stopped;
    super::transaction::begin(root, &transaction)?;
    let mut activated = false;

    let result: Result<(), InstallError> = async {
        let built = build_release(root, &release).await?;
        if !service_stopped {
            let token = (options.token)()?;
            let exported = super::export::export_config(&options.export_base_url, &token).await?;
            if exported.version != state.active_version {
                return Err(InstallError::ExportFailed(format!(
                    "exported version {} does not match the running version {}",
                    exported.version, state.active_version
                )));
            }
            let webserver = root
                .canonical
                .join("releases")
                .join(&state.active_version)
                .join(WEBSERVER_BINARY);
            let static_dir = root.canonical.join("current/static");
            let geo_tmp = root.canonical.join("data/geo_tmp");
            let backup_ref = super::backup::create_backup(
                &layout::territory_backups_dir(),
                &from_version,
                architecture.key(),
                &webserver,
                &exported.content,
                &static_dir,
                &geo_tmp,
                &crate::tr!(crate::keys::BACKUP_AUTO_REMARK_SWITCH),
                true,
                None,
            )?;
            transaction.backup = Some(backup_ref);
        }
        let unit_sha = if is_systemd {
            transaction.systemd_before =
                Some(capture_before(manager, ManagedService::LandscapeRouter)?);
            let backup_dir = layout::territory_backups_dir()
                .join(&transaction.transaction_id)
                .join("host/resolv.conf");
            let _ = super::resolv::backup(manager.resolv_conf(), &backup_dir)?;
            transaction.resolv_conf_backup = Some(format!(
                "backups/{}/host/resolv.conf",
                transaction.transaction_id
            ));
            Some(write_unit_origin(
                root,
                manager,
                ManagedService::LandscapeRouter,
                &manager.render_definition(ManagedService::LandscapeRouter, &root.canonical)?,
            )?)
        } else {
            None
        };
        if !service_stopped {
            super::rollback::write_state_snapshot(root, &transaction.transaction_id, state)?;
        }
        super::transaction::mark_phase(root, &transaction, Phase::Prepared)?;

        if is_systemd {
            super::transaction::mark_phase(root, &transaction, Phase::Stopping)?;
            manager.stop_and_wait(
                ManagedService::LandscapeRouter,
                &(|| {
                    manager
                        .active_state(ManagedService::LandscapeRouter)
                        .map(|state| state != "active")
                        .unwrap_or(true)
                }),
            )?;
        } else {
            let accepted = (options.confirm)(&crate::tr!(
                crate::keys::SWITCH_CONFIRM_STOP_WITH_OWN_MANAGER
            ))?;
            if !accepted {
                return Err(InstallError::UserRefused(
                    "user refused to stop the running instance".into(),
                ));
            }
        }
        super::transaction::mark_phase(root, &transaction, Phase::Activating)?;
        activated = true;
        activate_current(root, &release.version)?;
        if is_systemd {
            manager.register(
                ManagedService::LandscapeRouter,
                &root.canonical.join("service/landscape-router.service"),
            )?;
            manager.enable(ManagedService::LandscapeRouter)?;
            manager.start(ManagedService::LandscapeRouter)?;
            super::transaction::mark_phase(root, &transaction, Phase::Verifying)?;
            let pid = manager.main_pid(ManagedService::LandscapeRouter)?;
            if pid == 0 {
                return Err(InstallError::Systemd(
                    "service did not produce a main pid after start".into(),
                ));
            }
            let options = health::StartupOptions {
                ports: &options.health.ports,
                expected_pid: pid,
                docs: &options.health.docs,
                unit_state: Some(&(|| manager.active_state(ManagedService::LandscapeRouter).ok())),
                init_required: false,
                data_dir: &root.canonical.join("data"),
                startup_timeout: options.health.startup_timeout,
                stable_duration: options.health.stable_duration,
            };
            health::wait_for_startup(&options).await?;
            health::observe_stable(&options).await?;
        }
        let new_state = build_switched_state(
            root,
            &release,
            &built,
            state,
            &transaction.transaction_id,
            unit_sha,
        );
        super::state::write_state(root, &new_state)?;
        super::transaction::mark_phase(root, &transaction, Phase::Committed)?;
        Ok(())
    }
    .await;

    let backup_id = transaction
        .backup
        .as_ref()
        .map(|backup| backup.backup_id.clone());
    match result {
        Ok(()) => Ok(SwitchOutcome::Committed {
            version: release.version.clone(),
            backup_id,
        }),
        Err(error) => {
            if is_systemd && activated {
                if transaction.backup.is_some() {
                    match super::rollback::rollback_switch(
                        root,
                        &transaction,
                        state,
                        manager,
                        options.health,
                    )
                    .await
                    {
                        Ok(()) => Ok(SwitchOutcome::RolledBack {
                            version: from_version,
                            backup_id,
                        }),
                        Err(rollback_error) => {
                            eprintln!(
                                "install: {}",
                                crate::tr!(
                                    crate::keys::SWITCH_ROLLBACK_FAILED,
                                    rollback_error = rollback_error
                                )
                            );
                            Ok(SwitchOutcome::RollbackFailed {
                                version: from_version,
                                reason: error.to_string(),
                            })
                        }
                    }
                } else {
                    match super::rollback::rollback_no_backup(
                        root,
                        &transaction,
                        state,
                        manager,
                        options.health,
                    )
                    .await
                    {
                        Ok(()) => Ok(SwitchOutcome::RolledBack {
                            version: from_version,
                            backup_id,
                        }),
                        Err(rollback_error) => {
                            eprintln!(
                                "install: {}",
                                crate::tr!(
                                    crate::keys::SWITCH_ROLLBACK_FAILED,
                                    rollback_error = rollback_error
                                )
                            );
                            Ok(SwitchOutcome::RollbackFailed {
                                version: from_version,
                                reason: error.to_string(),
                            })
                        }
                    }
                }
            } else {
                if !activated {
                    // 激活前失败:可能发生在停止服务阶段,服务状态可能已改变;
                    // 先按 systemd_before 恢复 unit 注册与 enabled/active 状态,再标记 failed。
                    let mut systemd_restored = true;
                    if let Some(before) = &transaction.systemd_before {
                        let unit_origin = root
                            .canonical
                            .join("service")
                            .join(manager.service_name(ManagedService::LandscapeRouter));
                        let restore_error = manager
                            .restore_before(ManagedService::LandscapeRouter, before, &unit_origin)
                            .and_then(|()| {
                                if let Some(backup_path) = &transaction.resolv_conf_backup {
                                    let backup_dir = layout::territory_relative(backup_path);
                                    super::resolv::restore(manager.resolv_conf(), &backup_dir)
                                } else {
                                    Ok(())
                                }
                            });
                        if let Err(restore_error) = restore_error {
                            systemd_restored = false;
                            eprintln!(
                                "install: {}",
                                crate::tr!(
                                    crate::keys::SWITCH_ROLLBACK_FAILED,
                                    rollback_error = restore_error
                                )
                            );
                        }
                    }
                    let _ = super::transaction::mark_phase(root, &transaction, Phase::Failed);
                    if !systemd_restored {
                        // 服务状态恢复也失败:事务已终结且服务可能未恢复,
                        // 按自动恢复失败处理(退出码 6),不能按普通失败返回。
                        return Ok(SwitchOutcome::RollbackFailed {
                            version: from_version,
                            reason: error.to_string(),
                        });
                    }
                }
                Err(error)
            }
        }
    }
}
