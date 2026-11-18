use super::artifacts::{WEBSERVER_BINARY, build_release};
use super::health::{self, DocsProbe, HealthOptions};
use super::manager::{ManagedService, ServiceManager};
use super::operation_control as control;
use super::pipeline::{
    activate_current, architecture_from_state, build_switched_state, capture_before,
    check_initialization, parse_stable_version, verify_current_backend, write_unit_origin,
};
use super::plan::InstallError;
use super::repository::Release;
use super::root::InstallRoot;
use super::state::{InstallState, StateServiceManager};
use super::transaction::Phase;
use crate::daemon_operations::Stage;
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

    let native = control::active();
    let is_systemd = native || state.service.manager == StateServiceManager::Systemd;
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
        control::message(
            "--allow-no-backup ignored: running service receives a protection backup",
        )?;
        eprintln!(
            "install: {}",
            crate::tr!(crate::keys::SWITCH_WARNING_ALLOW_NO_BACKUP_IGNORED)
        );
    }

    let transaction =
        super::transaction::TransactionFile::new_switch(root, &from_version, &release.version)?;
    let mut transaction = transaction;
    transaction.no_backup = service_stopped;
    crate::deployment::transaction::workspace::select(&mut transaction);
    if native {
        transaction.schema_version = 8;
        control::associate(&transaction)?;
    }
    super::transaction::begin(root, &transaction)?;
    let mut activated = false;

    let result: Result<(), InstallError> = async {
        super::transaction::workspace::prepare(root, &transaction)?;
        let built = if native {
            control::download(build_staged(root, &transaction, &release)).await?
        } else {
            build_release(root, &release).await?
        };
        control::phase(Stage::Preparing)?;
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
            if native {
                Some(
                    crate::release::artifacts::hash_file(
                        &root.canonical.join("service/landscape-router.service"),
                    )?
                    .0,
                )
            } else {
                Some(write_unit_origin(
                    root,
                    manager,
                    ManagedService::LandscapeRouter,
                    &manager.render_definition(ManagedService::LandscapeRouter, &root.canonical)?,
                )?)
            }
        } else {
            None
        };
        if native || !service_stopped {
            super::rollback::write_state_snapshot(root, &transaction.transaction_id, state)?;
        }
        if native {
            super::repair_snapshot::sync_tree(
                &layout::territory_transactions_dir().join(&transaction.transaction_id),
            )?;
        }
        super::transaction::mark_phase(root, &transaction, Phase::Prepared)?;

        if is_systemd {
            control::phase(Stage::Stopping)?;
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
        #[cfg(feature = "test-support")]
        control::checkpoint("switch-stopped")?;
        control::phase(Stage::Activating)?;
        super::transaction::mark_phase(root, &transaction, Phase::Activating)?;
        activated = true;
        if native {
            #[cfg(feature = "test-support")]
            control::checkpoint("switch-before-publish")?;
            publish_staged(root, &transaction)?;
        }
        activate_current(root, &release.version)?;
        #[cfg(feature = "test-support")]
        control::checkpoint("switch-activated")?;
        if is_systemd {
            manager.register(
                ManagedService::LandscapeRouter,
                &root.canonical.join("service/landscape-router.service"),
            )?;
            manager.enable(ManagedService::LandscapeRouter)?;
            #[cfg(feature = "test-support")]
            control::checkpoint("switch-before-start")?;
            manager.start(ManagedService::LandscapeRouter)?;
            control::phase(Stage::Verifying)?;
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
        let mut new_state = build_switched_state(
            root,
            &release,
            &built,
            state,
            &transaction.transaction_id,
            unit_sha,
        );
        if native {
            new_state.service.manager = state.service.manager;
        }
        #[cfg(feature = "test-support")]
        control::checkpoint("switch-before-state")?;
        super::state::write_state(root, &new_state)?;
        #[cfg(feature = "test-support")]
        control::checkpoint("switch-state-written")?;
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
                control::phase(Stage::RollingBack)?;
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
                if native && !activated {
                    // Preparing has no host changes; later stages restore captured state.
                    let persisted =
                        super::transaction::find_unfinished(root)?.ok_or_else(|| {
                            InstallError::CorruptedTransaction("missing switch transaction".into())
                        })?;
                    if recover_native(root, &persisted, manager, options.health)
                        .await
                        .is_err()
                    {
                        return Ok(SwitchOutcome::RollbackFailed {
                            version: from_version,
                            reason: error.to_string(),
                        });
                    }
                    return Err(error);
                }
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

pub(crate) async fn build_staged(
    root: &InstallRoot,
    tx: &super::transaction::TransactionFile,
    release: &Release,
) -> Result<super::artifacts::BuiltRelease, InstallError> {
    let scratch = layout::territory_transactions_dir()
        .join(&tx.transaction_id)
        .join("target");
    let staged = InstallRoot {
        install_root: scratch.clone(),
        canonical: scratch,
    };
    let existing = root
        .canonical
        .join("releases")
        .join(release.version.to_string());
    if super::artifacts::reuse_existing_release(&existing, release)?.is_some() {
        super::rollback::copy_tree_into(
            &existing,
            &staged
                .canonical
                .join("releases")
                .join(release.version.to_string()),
        )?;
    }
    build_release(&staged, release).await
}
fn publish_staged(
    root: &InstallRoot,
    tx: &super::transaction::TransactionFile,
) -> Result<(), InstallError> {
    let target = tx
        .target_release
        .as_ref()
        .ok_or_else(|| InstallError::CorruptedTransaction("missing switch target".into()))?;
    let source = layout::territory_transactions_dir()
        .join(&tx.transaction_id)
        .join("target")
        .join(target);
    // Publish on the installation filesystem, even when territory is a different mount.
    let staging = root
        .canonical
        .join("releases")
        .join(format!(".switch-{}.tmp", tx.transaction_id));
    super::rollback::copy_tree_into(&source, &staging)?;
    super::repair_snapshot::sync_tree(&staging)?;
    let destination = root.canonical.join(target);
    if destination.exists() {
        let previous = root
            .canonical
            .join("releases")
            .join(format!(".switch-{}.previous", tx.transaction_id));
        std::fs::rename(&destination, previous).map_err(InstallError::Io)?;
    }
    std::fs::rename(&staging, &destination).map_err(InstallError::Io)?;
    std::fs::File::open(destination.parent().unwrap())
        .and_then(|dir| dir.sync_all())
        .map_err(InstallError::Io)
}
/// Strict cleanup: no live release is changed before activation, including caches.
pub(crate) fn cleanup_preparing(
    root: &InstallRoot,
    tx: &super::transaction::TransactionFile,
) -> Result<(), InstallError> {
    super::transaction::workspace::cleanup_preparing(root, tx)?;
    let dir = layout::territory_transactions_dir().join(&tx.transaction_id);
    match std::fs::remove_dir_all(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(InstallError::Io(e)),
    }
    super::transaction::mark_phase(root, tx, Phase::Failed)
}
pub(crate) async fn recover_native<P: DocsProbe>(
    root: &InstallRoot,
    tx: &super::transaction::TransactionFile,
    manager: &dyn ServiceManager,
    health: &HealthOptions<P>,
) -> Result<(), InstallError> {
    match tx.phase {
        Phase::Preparing => cleanup_preparing(root, tx),
        Phase::Prepared | Phase::Stopping => {
            let snapshot = super::rollback::read_state_snapshot(root, &tx.transaction_id)?;
            if snapshot.service.manager != manager.kind() {
                return Err(InstallError::Preflight("recovery backend mismatch".into()));
            }
            if let Some(before) = &tx.systemd_before {
                manager.restore_before(
                    ManagedService::LandscapeRouter,
                    before,
                    &root.canonical.join("service/landscape-router.service"),
                )?;
            }
            if let Some(backup) = &tx.resolv_conf_backup {
                super::resolv::restore(manager.resolv_conf(), &layout::territory_relative(backup))?;
            }
            write_restored_enablement(root, tx)?;
            cleanup_preparing(root, tx)
        }
        Phase::Activating | Phase::Verifying | Phase::RollingBack => {
            let snapshot = super::rollback::read_state_snapshot(root, &tx.transaction_id)?;
            if snapshot.service.manager != manager.kind() {
                return Err(InstallError::Preflight("recovery backend mismatch".into()));
            }
            if tx.no_backup {
                super::rollback::rollback_no_backup(root, tx, &snapshot, manager, health).await
            } else {
                super::rollback::rollback_switch(root, tx, &snapshot, manager, health).await
            }
        }
        _ => Err(InstallError::CorruptedTransaction(
            "unexpected switch recovery phase".into(),
        )),
    }
}

/// Persist the effective service fact after all pre-activation recovery succeeds.
/// This does not create a new installation commit or replace its version identity.
pub(super) fn write_restored_enablement(
    root: &InstallRoot,
    tx: &super::transaction::TransactionFile,
) -> Result<(), InstallError> {
    let Some(before) = &tx.systemd_before else {
        return Ok(());
    };
    let previous = super::state::load_state(root)?.ok_or_else(|| {
        InstallError::CorruptedState(
            "installation state disappeared during service recovery".into(),
        )
    })?;
    let boot = if before.systemd_links.is_some() {
        crate::network::takeover::current_boot_id()?
    } else {
        String::new()
    };
    let restored = restored_enablement_state(&previous, before, &boot)?;
    super::state::write_state(root, &restored)
}

fn restored_enablement_state(
    previous: &InstallState,
    before: &super::manager::ServiceBefore,
    boot_id: &str,
) -> Result<InstallState, InstallError> {
    before.validate_for_backend(previous.service.manager)?;
    let mut restored = previous.clone();
    restored.service.enabled = before.enablement().enabled_for_boot(boot_id);
    Ok(restored)
}

/// Called with the installation lock held on the daemon's operation thread.
pub(crate) async fn run_operation(
    runtime: &crate::deployment::runtime::InstallRuntime,
    record: &crate::daemon_operations::Record,
) -> Result<u8, InstallError> {
    use crate::deployment::plan::TargetVersion;
    let args = record
        .switch
        .as_ref()
        .ok_or_else(|| InstallError::ParameterUsage("missing switch parameters".into()))?;
    let root = super::root::normalize_install_root(std::path::Path::new(&record.install_root))?;
    let installed = super::state::load_state(&root)?
        .ok_or_else(|| InstallError::Preflight("missing installation".into()))?;
    if installed.active_version != args.from_version
        || crate::daemon_operations::installation_identity(&installed)? != args.state_identity
    {
        return Err(InstallError::Preflight(
            "installation state changed since submission".into(),
        ));
    }
    let ports: Vec<_> = runtime
        .health_ports
        .iter()
        .map(|p| (p.protocol, p.port))
        .collect();
    super::process::check_conflicts(&root.canonical, &installed, &ports)?;
    super::preflight::run_preflight(&root.canonical, Some(&installed), false, runtime, false)?;
    let manager = runtime.service_manager.as_ref();
    if manager.kind() != installed.service.manager
        || !matches!(
            manager.probe(),
            super::manager::Availability::Available { .. }
        )
    {
        return Err(InstallError::Preflight(
            "matching service backend is unavailable".into(),
        ));
    }
    let origin = root.canonical.join("service/landscape-router.service");
    if !matches!(manager.query_registration(ManagedService::LandscapeRouter)?, super::manager::SystemRegistration::Symlink { target } if target == origin)
    {
        return Err(InstallError::Preflight(
            "service registration is not owned by this installation".into(),
        ));
    }
    let definition = std::fs::read_to_string(&origin).map_err(InstallError::Io)?;
    manager.validate_definition(
        ManagedService::LandscapeRouter,
        &definition,
        &root.canonical,
    )?;
    if installed.service.definition_sha256.as_deref()
        != Some(super::artifacts::hash_str(&definition).as_str())
        && !args.accept_service_change
    {
        return Err(InstallError::ParameterUsage(
            "service definition changed; use --accept-service-change".into(),
        ));
    }
    check_initialization(&root, &installed)?;
    verify_current_backend(&root, &installed)?;
    let choice = match &args.repository {
        Some(choice) => choice.clone(),
        None => super::config::resolve_default_choice()?,
    };
    let spec = choice.resolve()?;
    let provider = super::repository::provider_for(spec.kind, &spec.location)?;
    let architecture = architecture_from_state(&installed);
    let release = match TargetVersion::parse(&args.target)? {
        TargetVersion::Latest => provider
            .latest(architecture)
            .await?
            .ok_or(InstallError::NoStableVersion)?,
        TargetVersion::Version(version) => provider.release(&version, architecture).await?,
    };
    control::resolved_version(&release.version.to_string())?;
    if release.version.to_string() == installed.active_version {
        crate::commands::existing::same_version_install(
            args.accept_service_change,
            false,
            &root,
            &installed,
            args.repository.as_ref().map(|_| spec),
            manager,
        )
        .await?;
        return Ok(0);
    }
    let health = runtime.health_options()?;
    let data = root.canonical.join("data");
    let options = SwitchOptions {
        export_base_url: runtime.export_base_url.clone(),
        token: &|| {
            crate::backup::export::read_api_token(
                &data.join("landscape_api_token"),
                runtime.managed_uid,
            )
        },
        confirm: &|_| {
            Err(InstallError::Preflight(
                "daemon cannot read terminal input".into(),
            ))
        },
        health: &health,
    };
    match switch_version(
        &root,
        &installed,
        release,
        &SwitchArgs {
            allow_no_backup: args.allow_no_backup,
        },
        manager,
        &options,
    )
    .await?
    {
        SwitchOutcome::Committed { .. } => Ok(0),
        SwitchOutcome::RolledBack { .. } => Ok(5),
        SwitchOutcome::RollbackFailed { .. } => Ok(6),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn early_recovery_updates_expired_runtime_enablement_without_replacing_commit() {
        use super::super::manager::{
            Registration, RegistrationKind, ServiceBefore, SystemdStartupLinks,
        };
        use super::super::state::{
            ArchiveAsset, Assets, InitStatus, InitializationState, ServiceState, StateArchitecture,
            WebserverAsset,
        };
        let previous = InstallState {
            schema_version: super::super::state::STATE_SCHEMA_VERSION,
            layout_version: super::super::state::STATE_LAYOUT_VERSION,
            install_root: "/fixture/installation".into(),
            canonical_install_root: "/fixture/installation".into(),
            active_version: "1.2.3".into(),
            assets: Assets {
                webserver: WebserverAsset {
                    architecture: StateArchitecture::X86_64,
                    sha256: "a".repeat(64),
                    size: 5,
                },
                static_archive: ArchiveAsset {
                    sha256: "b".repeat(64),
                    size: 7,
                },
            },
            initialization: InitializationState {
                status: InitStatus::Complete,
                lock_present: true,
                initialized_at: Some(chrono::Utc::now()),
            },
            service: ServiceState {
                manager: StateServiceManager::Systemd,
                registered: true,
                enabled: true,
                verified: true,
                definition_path: Some("service/landscape-router.service".into()),
                definition_sha256: Some("c".repeat(64)),
            },
            last_transaction_id: Some(uuid::Uuid::now_v7().to_string()),
            committed_at: Some(chrono::Utc::now()),
        };
        let boot = "11111111-1111-4111-8111-111111111111";
        let next = "22222222-2222-4222-8222-222222222222";
        let mut before = ServiceBefore {
            registration: Registration {
                kind: RegistrationKind::Symlink,
                target: Some("/fixture/installation/service/landscape-router.service".into()),
            },
            enabled: true,
            active: true,
            systemd_links: Some(SystemdStartupLinks {
                version: 1,
                boot_id: boot.into(),
                persistent: vec![],
                runtime: vec!["multi-user.target.wants".into()],
            }),
        };
        assert_eq!(
            restored_enablement_state(&previous, &before, boot).unwrap(),
            previous
        );
        let restored = restored_enablement_state(&previous, &before, next).unwrap();
        let mut expected = previous.clone();
        expected.service.enabled = false;
        assert_eq!(
            restored, expected,
            "only the expired enablement fact changes"
        );
        before.systemd_links = None;
        assert_eq!(
            restored_enablement_state(&previous, &before, next).unwrap(),
            previous,
            "legacy snapshots have no evidence of runtime expiry"
        );
    }
    #[test]
    fn cleanup_failure_keeps_switch_transaction_recoverable() {
        let temp =
            std::env::temp_dir().join(format!("lkit-switch-cleanup-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&temp).unwrap();
        let _guard = layout::test_territory(&temp.join("territory"));
        let root = super::super::root::normalize_install_root(&temp.join("landscape")).unwrap();
        let mut tx = super::super::transaction::TransactionFile::new_switch(
            &root,
            &semver::Version::new(1, 0, 0),
            &semver::Version::new(2, 0, 0),
        )
        .unwrap();
        tx.schema_version = 8;
        super::super::transaction::begin(&root, &tx).unwrap();
        let dir = layout::territory_transactions_dir().join(&tx.transaction_id);
        std::fs::write(&dir, b"obstruction").unwrap();
        assert!(cleanup_preparing(&root, &tx).is_err());
        assert!(
            super::super::transaction::find_unfinished(&root)
                .unwrap()
                .is_some()
        );
        std::fs::remove_file(&dir).unwrap();
        cleanup_preparing(&root, &tx).unwrap();
        assert!(
            super::super::transaction::find_unfinished(&root)
                .unwrap()
                .is_none()
        );
    }
}
