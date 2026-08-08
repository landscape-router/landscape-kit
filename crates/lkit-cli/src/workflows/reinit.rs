use chrono::Utc;

use super::artifacts::{WEBSERVER_BINARY, hash_str};
use super::backup;
use super::export;
use super::health::{DocsProbe, HealthOptions, StartupOptions};
use super::pipeline;
use super::plan::InstallError;
use super::restore::{restore_previous_data, write_file_atomic};
use super::rollback;
use super::root::InstallRoot;
use super::state::{
    self, InitStatus, InitializationState, InstallState, ServiceState, StateServiceManager,
};
use super::systemd::Systemd;
use super::transaction::{Phase, TransactionFile};
use crate::deployment::runtime::InstallRuntime;
use crate::interaction::credentials::Credentials;
use crate::interaction::presentation::{OperationPhase, operation_progress};
use crate::network::config::NetworkPlan;

/// reinit 运行参数。
pub(crate) struct ReinitArgs {
    /// 允许在保护备份无法创建时继续,不产生可移植的当前配置快照。
    pub allow_no_backup: bool,
    /// 非交互模式必须显式 `--yes`,否则返回参数错误。
    pub yes: bool,
    /// 交互控制台已确认破坏性计划,跳过 `/dev/tty` 二次确认。
    pub console_confirmed: bool,
}

/// reinit 运行参数(测试可注入)。
pub(crate) struct ReinitOptions<'a, P: DocsProbe> {
    pub export_base_url: String,
    pub token: &'a dyn Fn() -> Result<String, InstallError>,
    pub confirm: &'a dyn Fn(&str) -> Result<bool, InstallError>,
    pub health: &'a HealthOptions<P>,
}

#[derive(Debug)]
pub(crate) enum ReinitOutcome {
    Committed {
        version: semver::Version,
        backup_id: Option<String>,
        pending_network_address: Option<std::net::Ipv4Addr>,
    },
    RolledBack {
        version: semver::Version,
    },
    RollbackFailed {
        version: semver::Version,
        reason: String,
    },
}

/// 重新初始化已安装的 Landscape:同版本配置重建,不改变版本关系与 release 资产。
///
/// 流程:确认破坏性计划 → 创建保护 `.lkb` → 停止服务 → 旧 `data/` 移入事务目录 →
/// 新建空 `data/` 并写入新 `landscape_init.toml`(版本固定为当前活动版本,凭据为新
/// 输入,网络为新计划)→ 协调 `br_lan` 与所选 LAN 地址 → 启动并通过健康检查 →
/// 一律进入网络确认窗口。失败回滚优先使用事务目录中的旧 data 现场。
#[allow(clippy::too_many_arguments)]
pub(crate) async fn reinit_installation<P: DocsProbe>(
    root: &InstallRoot,
    state: &InstallState,
    systemd: &Systemd,
    credentials: &Credentials,
    network: &NetworkPlan,
    args: &ReinitArgs,
    options: &ReinitOptions<'_, P>,
    runtime: &InstallRuntime,
) -> Result<ReinitOutcome, InstallError> {
    let version = pipeline::parse_stable_version(&state.active_version).map_err(|error| {
        InstallError::CorruptedState(format!("invalid active version: {error}"))
    })?;

    // 确认先于事务创建:拒绝或缺少 `--yes` 时不创建事务、不写任何文件。
    confirm_reinit(options, args, state, network)?;

    let mut transaction = super::transaction::TransactionFile::new_reinit(root, &version)?;
    super::transaction::begin(root, &transaction)?;
    let tx_dir = root
        .canonical
        .join("transactions")
        .join(&transaction.transaction_id);

    // 保护备份失败时保持现场不变;`--allow-no-backup` 才允许继续。
    if let Err(error) = create_protection_backup(root, state, &mut transaction, options).await {
        if !args.allow_no_backup {
            let _ = super::transaction::mark_phase(root, &transaction, Phase::Failed);
            return Err(error);
        }
        transaction.no_backup = true;
        eprintln!(
            "install: {}",
            crate::tr!(
                crate::keys::REINIT_WARNING_NO_PROTECTION_BACKUP,
                error = error
            )
        );
    }
    super::transaction::persist(root, &transaction)?;

    let unit_sha = {
        transaction.systemd_before = Some(pipeline::capture_systemd_before(systemd)?);
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
        super::transaction::persist(root, &transaction)?;
        hash_str(
            &std::fs::read_to_string(root.canonical.join("service/landscape-router.service"))
                .map_err(InstallError::Io)?,
        )
    };
    rollback::write_state_snapshot(root, &transaction.transaction_id, state)?;

    // 停止服务前生成完整的新初始化配置(版本固定为当前活动版本)。
    let init_config = pipeline::build_init_config(&version, credentials, Some(network))?;
    super::transaction::mark_phase(root, &transaction, Phase::Prepared)?;
    operation_progress(OperationPhase::Preparing, Some((1, 4)));

    let mut activated = false;
    let result: Result<(), InstallError> = async {
        super::transaction::mark_phase(root, &transaction, Phase::Stopping)?;
        operation_progress(OperationPhase::Stopping, Some((2, 4)));
        super::systemd::stop_and_wait(systemd, || {
            super::systemd::active_state(systemd)
                .map(|value| value != "active")
                .unwrap_or(true)
        })?;
        super::transaction::mark_phase(root, &transaction, Phase::Activating)?;
        operation_progress(OperationPhase::Activating, Some((3, 4)));
        activated = true;
        std::fs::create_dir_all(&tx_dir).map_err(InstallError::Io)?;
        rollback::move_data_aside(&root.canonical.join("data"), &tx_dir.join("previous-data"))?;
        let data = root.canonical.join("data");
        std::fs::create_dir_all(&data).map_err(InstallError::Io)?;
        write_file_atomic(
            &data.join("landscape_init.toml"),
            init_config.as_bytes(),
            0o600,
        )?;
        crate::network::takeover::clear_selected_lan_addresses(network, &runtime.ip_command)?;
        super::systemd::start(systemd)?;
        super::transaction::mark_phase(root, &transaction, Phase::Verifying)?;
        operation_progress(OperationPhase::Verifying, Some((4, 4)));
        let pid = super::systemd::main_pid(systemd)?;
        if pid == 0 {
            return Err(InstallError::Systemd(
                "reinit did not produce a main pid after start".into(),
            ));
        }
        let startup = StartupOptions {
            ports: &options.health.ports,
            expected_pid: pid,
            docs: &options.health.docs,
            unit_state: Some(&(|| super::systemd::active_state(systemd).ok())),
            init_required: true,
            data_dir: &data,
            startup_timeout: options.health.startup_timeout,
            stable_duration: options.health.stable_duration,
        };
        super::health::wait_for_startup(&startup).await?;
        super::health::observe_stable(&startup).await?;

        let new_state = build_reinit_state(root, state, &transaction, &version, Some(&unit_sha))?;
        let takeover = crate::network::takeover::prepare_transaction(
            &transaction.transaction_id,
            network,
            runtime,
        )?;
        transaction.network_takeover = Some(takeover);
        super::transaction::persist(root, &transaction)?;
        let takeover = transaction.network_takeover.as_ref().ok_or_else(|| {
            InstallError::CorruptedTransaction("reinit network takeover state disappeared".into())
        })?;
        crate::network::takeover::write_pending_state(root, takeover, &new_state)?;
        crate::network::takeover::arm_recovery(root, takeover, runtime)?;
        super::transaction::mark_phase(root, &transaction, Phase::AwaitingNetworkConfirmation)?;
        Ok(())
    }
    .await;

    match result {
        Ok(()) => Ok(ReinitOutcome::Committed {
            version,
            backup_id: transaction
                .backup
                .as_ref()
                .map(|backup| backup.backup_id.clone()),
            pending_network_address: network.management_address().map(|address| address.address),
        }),
        Err(error) if activated => {
            match rollback_reinit(root, &transaction, systemd, options.health).await {
                Ok(()) => Ok(ReinitOutcome::RolledBack { version }),
                Err(rollback_error) => {
                    eprintln!(
                        "install: {}",
                        crate::tr!(
                            crate::keys::REINIT_ROLLBACK_FAILED,
                            rollback_error = rollback_error
                        )
                    );
                    Ok(ReinitOutcome::RollbackFailed {
                        version,
                        reason: error.to_string(),
                    })
                }
            }
        }
        Err(error) => {
            if !activated {
                let mut systemd_restored = true;
                if let Some(before) = &transaction.systemd_before {
                    let unit_origin = root.canonical.join("service/landscape-router.service");
                    let restore_error =
                        super::systemd::restore_systemd_before(systemd, before, &unit_origin)
                            .and_then(|()| {
                                if let Some(backup_path) = &transaction.resolv_conf_backup {
                                    let backup_dir = root.canonical.join(backup_path);
                                    super::resolv::restore(&systemd.resolv_conf, &backup_dir)
                                } else {
                                    Ok(())
                                }
                            });
                    if let Err(restore_error) = restore_error {
                        systemd_restored = false;
                        eprintln!(
                            "install: {}",
                            crate::tr!(
                                crate::keys::REINIT_ROLLBACK_FAILED,
                                rollback_error = restore_error
                            )
                        );
                    }
                }
                let _ = super::transaction::mark_phase(root, &transaction, Phase::Failed);
                if !systemd_restored {
                    return Ok(ReinitOutcome::RollbackFailed {
                        version,
                        reason: error.to_string(),
                    });
                }
            }
            Err(error)
        }
    }
}

/// 交互模式确认破坏性计划:清空范围、保护备份、网络确认窗口与断线风险;
/// 非交互模式必须显式 `--yes`。
fn confirm_reinit<P: DocsProbe>(
    options: &ReinitOptions<'_, P>,
    args: &ReinitArgs,
    state: &InstallState,
    network: &NetworkPlan,
) -> Result<(), InstallError> {
    if args.console_confirmed {
        return Ok(());
    }
    if crate::interaction::interactive::is_non_interactive() {
        if !args.yes {
            return Err(InstallError::ParameterUsage(
                "--yes is required in non-interactive mode to confirm the reinit".into(),
            ));
        }
        return Ok(());
    }
    let lan = network.lan();
    let lan = if lan.is_empty() {
        crate::tr!(crate::keys::REINIT_LAN_NONE)
    } else {
        lan.join(", ")
    };
    let accepted = (options.confirm)(&crate::tr!(
        crate::keys::REINIT_CONFIRM_PLAN,
        version = state.active_version,
        wan = network.wan(),
        lan = lan
    ))?;
    if !accepted {
        return Err(InstallError::UserRefused(
            "user refused the reinit plan".into(),
        ));
    }
    if !args.allow_no_backup {
        let accepted =
            (options.confirm)(&crate::tr!(crate::keys::REINIT_CONFIRM_PROTECTION_BACKUP))?;
        if !accepted {
            return Err(InstallError::UserRefused(
                "user refused the reinit protection backup".into(),
            ));
        }
    }
    let accepted = (options.confirm)(&crate::tr!(crate::keys::REINIT_CONFIRM_NETWORK_WINDOW))?;
    if !accepted {
        return Err(InstallError::UserRefused(
            "user refused the reinit network confirmation window".into(),
        ));
    }
    Ok(())
}

/// 创建 `reinit 前自动备份` 保护 `.lkb`,记录进事务。导出失败或与运行版本不一致时
/// 中止;只有 `.lkb` 完整落盘并自校验后调用方才能停止服务。
async fn create_protection_backup<P: DocsProbe>(
    root: &InstallRoot,
    state: &InstallState,
    transaction: &mut TransactionFile,
    options: &ReinitOptions<'_, P>,
) -> Result<(), InstallError> {
    pipeline::check_initialization(root, state)?;
    pipeline::verify_current_backend(root, state)?;
    let token = (options.token)()?;
    let exported = export::export_config(&options.export_base_url, &token).await?;
    if exported.version != state.active_version {
        return Err(InstallError::ExportFailed(format!(
            "exported version {} does not match the running version {}",
            exported.version, state.active_version
        )));
    }
    let architecture = match state.assets.webserver.architecture {
        state::StateArchitecture::X86_64 => "x86_64",
        state::StateArchitecture::Aarch64 => "aarch64",
    };
    let webserver = root
        .canonical
        .join("releases")
        .join(&state.active_version)
        .join(WEBSERVER_BINARY);
    let static_dir = root.canonical.join("current/static");
    let static_archive = root
        .canonical
        .join("releases")
        .join(&state.active_version)
        .join("static.zip");
    let geo_tmp = root.canonical.join("data/geo_tmp");
    let backup_ref = backup::create_backup(
        &root.canonical.join("backups"),
        &version_from_state(state)?,
        architecture,
        &webserver,
        &exported.content,
        &static_dir,
        &static_archive,
        &geo_tmp,
        &crate::tr!(crate::keys::BACKUP_AUTO_REMARK_REINIT),
        true,
        None,
    )?;
    transaction.backup = Some(backup_ref);
    Ok(())
}

/// reinit 提交的 state:版本与资产身份保持不变,初始化与服务按 systemd 运行态重建。
fn build_reinit_state(
    root: &InstallRoot,
    previous: &InstallState,
    transaction: &TransactionFile,
    version: &semver::Version,
    unit_sha: Option<&str>,
) -> Result<InstallState, InstallError> {
    let lock_present = root.canonical.join("data/landscape_init.lock").is_file();
    Ok(InstallState {
        schema_version: state::STATE_SCHEMA_VERSION,
        layout_version: state::STATE_LAYOUT_VERSION,
        install_root: root.install_root.display().to_string(),
        canonical_install_root: root.canonical.display().to_string(),
        active_version: version.to_string(),
        assets: previous.assets.clone(),
        initialization: InitializationState {
            status: InitStatus::Complete,
            lock_present,
            initialized_at: Some(Utc::now()),
        },
        service: ServiceState {
            manager: StateServiceManager::Systemd,
            registered: true,
            enabled: true,
            verified: true,
            definition_path: Some("service/landscape-router.service".into()),
            definition_sha256: unit_sha.map(str::to_string),
        },
        last_transaction_id: Some(transaction.transaction_id.clone()),
        committed_at: Some(Utc::now()),
    })
}

fn version_from_state(state: &InstallState) -> Result<semver::Version, InstallError> {
    pipeline::parse_stable_version(&state.active_version)
        .map_err(|error| InstallError::CorruptedState(format!("invalid active version: {error}")))
}

/// reinit 失败回滚:停止服务 → 恢复 `/etc/resolv.conf` → 把事务目录中的旧 `data/`
/// 移回 → 启动旧配置并通过完整健康检查 → 提交恢复前 state。
pub(crate) async fn rollback_reinit<P: DocsProbe>(
    root: &InstallRoot,
    transaction: &TransactionFile,
    systemd: &Systemd,
    health: &HealthOptions<P>,
) -> Result<(), InstallError> {
    super::transaction::mark_phase(root, transaction, Phase::RollingBack)?;
    let result = rollback_reinit_inner(root, transaction, systemd, health).await;
    if result.is_err() {
        let _ = super::transaction::mark_phase(root, transaction, Phase::Failed);
    }
    result
}

/// reinit 回滚的实际工作,不标记阶段(供 `lkit network rollback` 在移除恢复 unit
/// 前调用;`rollback_reinit` 是带阶段标记的完整入口)。
pub(crate) async fn rollback_reinit_inner<P: DocsProbe>(
    root: &InstallRoot,
    transaction: &TransactionFile,
    systemd: &Systemd,
    health: &HealthOptions<P>,
) -> Result<(), InstallError> {
    super::systemd::stop_and_wait(systemd, || {
        super::systemd::active_state(systemd)
            .map(|value| value != "active")
            .unwrap_or(true)
    })?;
    if let Some(backup_path) = &transaction.resolv_conf_backup {
        let backup_dir = root.canonical.join(backup_path);
        super::resolv::restore(&systemd.resolv_conf, &backup_dir)?;
    }
    let tx_dir = root
        .canonical
        .join("transactions")
        .join(&transaction.transaction_id);
    let data = root.canonical.join("data");
    let previous_data = tx_dir.join("previous-data");
    restore_previous_data(&data, &previous_data)?;
    super::systemd::start(systemd)?;
    let pid = super::systemd::main_pid(systemd)?;
    if pid == 0 {
        return Err(InstallError::Systemd(
            "restored service did not produce a main pid".into(),
        ));
    }
    let startup = StartupOptions {
        ports: &health.ports,
        expected_pid: pid,
        docs: &health.docs,
        unit_state: Some(&(|| super::systemd::active_state(systemd).ok())),
        init_required: true,
        data_dir: &data,
        startup_timeout: health.startup_timeout,
        stable_duration: health.stable_duration,
    };
    super::health::wait_for_startup(&startup).await?;
    super::health::observe_stable(&startup).await?;
    let snapshot = rollback::read_state_snapshot(root, &transaction.transaction_id)?;
    let mut restored = snapshot.clone();
    restored.last_transaction_id = Some(transaction.transaction_id.clone());
    restored.committed_at = Some(Utc::now());
    super::state::write_state(root, &restored)?;
    super::transaction::mark_phase(root, transaction, Phase::RolledBack)?;
    Ok(())
}
