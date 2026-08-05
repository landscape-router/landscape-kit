use super::artifacts::{WEBSERVER_BINARY, build_release};
use super::health::{self, DocsProbe, HealthOptions};
use super::pipeline::{
    activate_current, architecture_from_state, build_switched_state, capture_systemd_before,
    check_initialization, parse_stable_version, verify_current_backend, write_unit_origin,
};
use super::plan::InstallError;
use super::repository::{Release, ReleaseProvider};
use super::root::InstallRoot;
use super::state::{InstallState, StateServiceManager};
use super::systemd::{self, Systemd};
use super::transaction::Phase;

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
    provider: &ReleaseProvider,
    state: &InstallState,
    release: Release,
    args: &SwitchArgs,
    systemd: &Systemd,
    options: &SwitchOptions<'_, P>,
) -> Result<SwitchOutcome, InstallError> {
    if release.version.to_string() == state.active_version {
        return Err(InstallError::ParameterUsage(
            "target version is already active".into(),
        ));
    }
    let architecture = architecture_from_state(state);
    let from_version = parse_stable_version(&state.active_version).map_err(|error| {
        InstallError::CorruptedState(format!("invalid active version: {error}"))
    })?;
    check_initialization(root, state)?;
    verify_current_backend(root, state)?;

    let is_systemd = state.service.manager == StateServiceManager::Systemd;
    let service_stopped = is_systemd && !systemd::is_active(systemd)?;
    if service_stopped && !args.allow_no_backup {
        return Err(InstallError::ServiceNotRunning(
            "the managed service is stopped; start it with `systemctl start landscape-router.service` and retry, or re-run with --allow-no-backup to switch without a configuration snapshot"
                .into(),
        ));
    }
    if service_stopped {
        eprintln!(
            "install: {}",
            crate::tr!(
                "warning: the managed service is stopped; switching without a configuration snapshot; no .lkb backup will be created and automatic rollback cannot restore data modified by the target version",
                "警告：受管服务已停止；将在没有配置快照的情况下切换；不会创建 .lkb 备份，自动回滚无法恢复目标版本修改的数据"
            )
        );
    } else if args.allow_no_backup {
        eprintln!(
            "install: {}",
            crate::tr!(
                "warning: the managed service is running; --allow-no-backup ignored and a .lkb backup will be created",
                "警告：受管服务正在运行；已忽略 --allow-no-backup，并将创建 .lkb 备份"
            )
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
                &root.canonical.join("backups"),
                &from_version,
                architecture.key(),
                &webserver,
                &exported.content,
                &static_dir,
                &geo_tmp,
            )?;
            transaction.backup = Some(backup_ref);
        }
        let unit_sha = if is_systemd {
            transaction.systemd_before = Some(capture_systemd_before(systemd)?);
            let backup_dir = root
                .canonical
                .join("backups")
                .join(&transaction.transaction_id)
                .join("host/resolv.conf");
            let _ = super::resolv::backup(&systemd.resolv_conf, &backup_dir)?;
            transaction.resolv_conf_backup = Some(format!(
                "backups/{}/host/resolv.conf",
                transaction.transaction_id
            ));
            Some(write_unit_origin(
                root,
                &systemd::render_unit(&root.canonical),
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
            super::systemd::stop_and_wait(systemd, || {
                systemd::active_state(systemd)
                    .map(|state| state != "active")
                    .unwrap_or(true)
            })?;
        } else {
            let accepted = (options.confirm)(crate::tr!(
                "stop your Landscape instance with your own process manager, then confirm",
                "请使用自己的进程管理器停止 Landscape 实例，然后输入 `yes`："
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
            super::systemd::register(
                systemd,
                &root.canonical.join("service/landscape-router.service"),
            )?;
            super::systemd::enable(systemd)?;
            super::systemd::start(systemd)?;
            super::transaction::mark_phase(root, &transaction, Phase::Verifying)?;
            let pid = systemd::main_pid(systemd)?;
            if pid == 0 {
                return Err(InstallError::Systemd(
                    "service did not produce a main pid after start".into(),
                ));
            }
            let options = health::StartupOptions {
                ports: &options.health.ports,
                expected_pid: pid,
                docs: &options.health.docs,
                unit_state: Some(&(|| systemd::active_state(systemd).ok())),
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
            provider,
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
                        systemd,
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
                                crate::trf!(
                                    ("automatic rollback failed: {rollback_error}"),
                                    ("自动回滚失败：{rollback_error}")
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
                        systemd,
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
                                crate::trf!(
                                    ("automatic rollback failed: {rollback_error}"),
                                    ("自动回滚失败：{rollback_error}")
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
                    let _ = super::transaction::mark_phase(root, &transaction, Phase::Failed);
                }
                Err(error)
            }
        }
    }
}
