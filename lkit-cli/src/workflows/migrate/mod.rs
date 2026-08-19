use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::Utc;

use super::artifacts::{STATIC_DIR, WEBSERVER_BINARY, fetch_static_asset, hash_file};
use super::backup as lkb;
use super::backup::extract_lkb;
use super::export;
use super::health::{DocsProbe, HealthOptions, StartupOptions};
use super::manager::{ManagedService, ServiceManager, ServiceManagerKind};
use super::pipeline;
use super::plan::{InstallError, RepositoryChoice};
use super::process::{self, Process};
use super::repository::{Architecture, provider_for};
use super::root::InstallRoot;
use super::state::{
    ArchiveAsset, Assets, InitStatus, InitializationState, InstallState, STATE_LAYOUT_VERSION,
    STATE_SCHEMA_VERSION, ServiceState, StateArchitecture, StateServiceManager, WebserverAsset,
};
use super::systemd::{self, Systemd};
use super::transaction::{BackupRef, Operation, Phase, TransactionFile};
use crate::deployment::layout;
use crate::interaction::interactive;

mod legacy;
mod rollback;
#[cfg(all(test, feature = "test-support"))]
mod tests;

pub(crate) use legacy::{preempt_registration_conflict, stop_legacy_instance, stop_legacy_unit};
pub(crate) use rollback::{cleanup_migrated_root, restore_legacy_unit, rollback_migrate};

pub(crate) fn pid_alive(pid: u32) -> bool {
    unsafe {
        let result = libc::kill(pid as i32, 0);
        result == 0 || *libc::__errno_location() == libc::EPERM
    }
}

/// daemon worker 以 SIGTERM 取消迁移切换(见 daemon_worker 的 cancel 流程)。
/// 安装处理器把信号转成回滚请求,而不是默认终止进程;进程组 SIGKILL 兜底
/// 仍然存在,回滚通常在宽限期内完成。
static SIGTERM_REQUESTED: AtomicBool = AtomicBool::new(false);
static SIGTERM_HANDLER_INSTALLED: Once = Once::new();

extern "C" fn handle_sigterm(_signal: libc::c_int) {
    SIGTERM_REQUESTED.store(true, Ordering::SeqCst);
}

pub(crate) fn install_sigterm_handler() {
    SIGTERM_HANDLER_INSTALLED.call_once(|| unsafe {
        let handler = handle_sigterm as *const () as libc::sighandler_t;
        libc::signal(libc::SIGTERM, handler);
    });
}

pub(crate) fn sigterm_requested() -> bool {
    SIGTERM_REQUESTED.load(Ordering::SeqCst)
}

/// 取消是否已请求:前台 ^C(`interrupted`,内联路径)或 daemon SIGTERM
/// (委托 worker)任一触发即进入回滚。
fn cancel_requested<P: DocsProbe>(options: &MigrateOptions<'_, P>) -> bool {
    (options.interrupted)() || sigterm_requested()
}

/// 切换阶段取消检查:在安全点调用,取消后由 `migrate_switch_outcome` 回滚,
/// 不留下半迁移现场。
fn ensure_not_cancelled<P: DocsProbe>(options: &MigrateOptions<'_, P>) -> Result<(), InstallError> {
    if cancel_requested(options) {
        return Err(InstallError::UserRefused(crate::tr!(
            crate::keys::MIGRATE_CANCELLED_DURING_SWITCH
        )));
    }
    Ok(())
}

/// `lkit migrate` 运行参数。
#[derive(Clone)]
pub(crate) struct MigrateArgs {
    /// 旧手工部署的配置目录(`--from`)。
    pub config_dir: PathBuf,
    /// 非交互模式必须显式 `--yes`。
    pub yes: bool,
    /// 交互控制台已确认迁移计划(worker 进程无法读取 TUI 输入)。
    pub console_confirmed: bool,
    /// 仅用于 static.zip 缺失时从发布仓库下载;None 按 配置 > 官方 GitHub 解析。
    pub repository: Option<RepositoryChoice>,
    /// 前台已准备好的迁移事务 id(daemon worker 恢复路径专用)。
    pub resume_transaction: Option<String>,
}

/// migrate 运行参数(测试可注入)。
pub(crate) struct MigrateOptions<'a, P: DocsProbe> {
    pub export_base_url: String,
    pub managed_uid: u32,
    pub confirm: &'a dyn Fn(&str) -> Result<bool, InstallError>,
    pub health: &'a HealthOptions<P>,
    /// 固定端口探测参数:用于识别运行中的旧实例。生产与 `health.ports` 相同,
    /// 测试可注入不同端口。
    pub probe_ports: &'a [super::health::PortCheck],
    /// 前台阶段的中断轮询:委托式 Ctrl+C 只置位标志不退出进程,迁移流程在
    /// 步骤之间检查以中止(测试注入恒 false 的探针)。
    pub interrupted: &'a dyn Fn() -> bool,
}

/// 前台前置检查完成后的交接现场:事务已标记 `prepared`,切换阶段
/// (停止旧实例、重建、接管)由内联执行或 daemon worker 以事务 id 续跑。
#[derive(Debug)]
pub(crate) struct PreparedMigration {
    pub transaction_id: String,
    pub source: PathBuf,
    pub instance: super::process::Process,
}

#[derive(Debug)]
pub(crate) enum MigrateOutcome {
    Committed {
        version: semver::Version,
        backup_id: String,
    },
    RolledBack {
        version: semver::Version,
    },
    /// 切换期间用户取消:旧实例已回滚恢复。
    Cancelled {
        version: semver::Version,
    },
    RollbackFailed {
        version: semver::Version,
        reason: String,
    },
}

/// 从非 lkit 手工部署迁移:先创建迁移 `.lkb`(旧版本,不升级),确认后停止旧实例,
/// 从备份重建 release/data/current,注册并启动受管服务,提交状态。
/// 整条流程在当前进程内联执行(非 root 或测试 runtime 的前台路径)。
/// root 下由命令层拆分:前置检查在前台(`prepare_migration`),切换阶段委托
/// daemon worker 以 `resume_migrate_switch` 续跑。
pub(crate) async fn migrate_version<P: DocsProbe>(
    root: &InstallRoot,
    manager: &dyn ServiceManager,
    args: &MigrateArgs,
    options: &MigrateOptions<'_, P>,
) -> Result<MigrateOutcome, InstallError> {
    // 旧部署识别与 unit 操作是 systemd 专属能力;不可用时代理到前台进程确认路径。
    let systemd = systemd::downcast(manager)?;
    let prepared = prepare_migration(root, manager, args, options).await?;
    migrate_switch_outcome(root, systemd, args, options, &prepared).await
}

/// worker 恢复路径:认领前台标记为 `prepared` 的迁移事务,重新识别运行实例,
/// 只执行切换阶段(停止旧实例、重建、接管、提交),失败回滚。
pub(crate) async fn resume_migrate_switch<P: DocsProbe>(
    root: &InstallRoot,
    manager: &dyn ServiceManager,
    args: &MigrateArgs,
    options: &MigrateOptions<'_, P>,
) -> Result<MigrateOutcome, InstallError> {
    let systemd = systemd::downcast(manager)?;
    let transaction = load_prepared_transaction(root, args.resume_transaction.as_deref())?;
    let source = validate_source_dir(&args.config_dir)?;
    let instance = identify_running_instance(&source, options.probe_ports)?;
    let prepared = PreparedMigration {
        transaction_id: transaction.transaction_id.clone(),
        source,
        instance,
    };
    migrate_switch_outcome(root, systemd, args, options, &prepared).await
}

/// 前台阶段:校验源目录、识别运行实例、检查并调用导出 API、创建迁移备份、
/// 确认迁移计划,最后把事务标记为 `prepared`。每一步打印进度(用户能看到
/// 迁移进展),全部失败点都在停止旧实例之前,不产生任何运行态影响。
/// 中断(`options.interrupted`)在步骤之间检查,中止时不执行切换。
pub(crate) async fn prepare_migration<P: DocsProbe>(
    root: &InstallRoot,
    manager: &dyn ServiceManager,
    args: &MigrateArgs,
    options: &MigrateOptions<'_, P>,
) -> Result<PreparedMigration, InstallError> {
    let source = validate_source_dir(&args.config_dir)?;
    eprintln!(
        "migrate: {}",
        crate::tr!(
            crate::keys::MIGRATE_VALIDATING_SOURCE,
            source = source.display()
        )
    );
    check_interrupted(options.interrupted)?;
    let manager_kind = super::pipeline::require_manager(manager)?;

    eprintln!(
        "migrate: {}",
        crate::tr!(crate::keys::MIGRATE_IDENTIFYING_INSTANCE)
    );
    let instance = identify_running_instance(&source, options.probe_ports)?;
    check_interrupted(options.interrupted)?;

    // 导出配置要求旧实例运行中;运行实例同时提供后端版本与二进制来源。
    // export API 404(部署的 Landscape 不支持)在此显式报 ExportUnsupported。
    let token = export::read_api_token(&source.join("landscape_api_token"), options.managed_uid)?;
    eprintln!(
        "migrate: {}",
        crate::tr!(crate::keys::MIGRATE_CHECKING_EXPORT_API)
    );
    let exported = export::export_config(&options.export_base_url, &token).await?;
    let version = super::pipeline::parse_stable_version(&exported.version).map_err(|error| {
        InstallError::ExportFailed(format!(
            "invalid exported backend version {}: {error}",
            exported.version
        ))
    })?;
    eprintln!(
        "migrate: {}",
        crate::tr!(crate::keys::MIGRATE_EXPORT_API_SUPPORTED, version = version)
    );
    check_interrupted(options.interrupted)?;

    let mut transaction = super::transaction::TransactionFile::new_migrate(root, &version)?;
    super::transaction::begin(root, &transaction)?;
    let tx_dir = layout::territory_transactions_dir().join(&transaction.transaction_id);

    let result: Result<(), InstallError> = async {
        // ---- preparing:迁移备份 ----
        crate::interaction::presentation::operation_phase(
            crate::interaction::presentation::OperationPhase::Downloading,
        );
        let architecture = Architecture::host().ok_or_else(|| {
            InstallError::UnsupportedPlatform(crate::tr!(
                crate::keys::INSTALL_ONLY_X86_64_AND_AARCH64_SUPPORTED
            ))
        })?;
        eprintln!(
            "migrate: {}",
            crate::tr!(crate::keys::MIGRATE_CREATING_BACKUP)
        );
        let backup = create_migration_backup(
            root,
            &tx_dir,
            &source,
            &instance,
            &exported,
            &version,
            architecture,
            args,
        )
        .await?;
        transaction.backup = Some(backup.clone());
        super::transaction::persist(root, &transaction)?;
        check_interrupted(options.interrupted)?;
        eprintln!(
            "migrate: {}",
            crate::tr!(
                crate::keys::MIGRATE_BACKUP_CREATED,
                backup_id = backup.backup_id
            )
        );

        // ---- 确认:备份已完成,展示迁移计划 ----
        confirm_migrate(options, args, &source, &version, manager_kind)?;

        super::transaction::mark_phase(root, &transaction, Phase::Prepared)?;
        Ok(())
    }
    .await;

    match result {
        Ok(()) => Ok(PreparedMigration {
            transaction_id: transaction.transaction_id,
            source,
            instance,
        }),
        Err(error) => {
            // 事务可能尚未 begin(log 不存在时 mark_phase 幂等失败)。
            if let Err(mark_error) =
                super::transaction::mark_phase(root, &transaction, Phase::Failed)
            {
                eprintln!(
                    "migrate: {}",
                    crate::tr!(
                        crate::keys::MIGRATE_TX_FAILED_MARK_WARNING,
                        error = mark_error
                    )
                );
            }
            Err(error)
        }
    }
}

fn check_interrupted(interrupted: &dyn Fn() -> bool) -> Result<(), InstallError> {
    if interrupted() {
        return Err(InstallError::UserRefused(crate::tr!(
            crate::keys::MIGRATE_INTERRUPTED_BEFORE_SWITCH
        )));
    }
    Ok(())
}

/// 加载并校验前台准备好的迁移事务:必须是 migrate 事务、`prepared` 阶段,
/// 且已记录迁移备份与目标版本。
fn load_prepared_transaction(
    root: &InstallRoot,
    transaction_id: Option<&str>,
) -> Result<TransactionFile, InstallError> {
    let Some(transaction_id) = transaction_id else {
        return Err(InstallError::CorruptedTransaction(
            "a prepared migration transaction id is required to resume the switch".into(),
        ));
    };
    let Some(transaction) = super::transaction::find_unfinished(root)? else {
        return Err(InstallError::CorruptedTransaction(format!(
            "prepared migration transaction {transaction_id} is not present; it may have been recovered as failed — run `lkit migrate` again"
        )));
    };
    if transaction.transaction_id != transaction_id {
        return Err(InstallError::BlockedByTransaction(format!(
            "another unfinished transaction {} is present; cannot continue migration transaction {transaction_id}",
            transaction.transaction_id
        )));
    }
    if transaction.operation != Operation::Migrate {
        return Err(InstallError::CorruptedTransaction(format!(
            "transaction {} is a {} transaction, not a migration",
            transaction.transaction_id,
            transaction.operation.key()
        )));
    }
    if transaction.phase != Phase::Prepared {
        return Err(InstallError::CorruptedTransaction(format!(
            "migration transaction {} is in phase {} instead of prepared",
            transaction.transaction_id,
            transaction.phase.key()
        )));
    }
    if transaction.backup.is_none() || transaction.target_version.is_none() {
        return Err(InstallError::CorruptedTransaction(
            "prepared migration transaction must record the migration backup and target version"
                .into(),
        ));
    }
    Ok(transaction)
}

fn transaction_version(transaction: &TransactionFile) -> Result<semver::Version, InstallError> {
    let raw = transaction.target_version.as_deref().ok_or_else(|| {
        InstallError::CorruptedTransaction("migrate transaction is missing target_version".into())
    })?;
    semver::Version::parse(raw).map_err(|error| {
        InstallError::CorruptedTransaction(format!(
            "migrate transaction target_version {raw} is invalid: {error}"
        ))
    })
}

/// 执行切换阶段并处理失败:停止旧实例前失败标记 `failed` 原样返回;
/// 停止后失败自动回滚(恢复旧 unit 与事务前 systemd 状态、清理新根)。
async fn migrate_switch_outcome<P: DocsProbe>(
    root: &InstallRoot,
    systemd: &Systemd,
    args: &MigrateArgs,
    options: &MigrateOptions<'_, P>,
    prepared: &PreparedMigration,
) -> Result<MigrateOutcome, InstallError> {
    let transaction = load_prepared_transaction(root, Some(&prepared.transaction_id))?;
    let version = transaction_version(&transaction)?;

    let mut stopping_started = false;
    let result: Result<(), InstallError> = run_migrate_switch(
        root,
        systemd,
        args,
        options,
        prepared,
        &transaction,
        &version,
        &mut stopping_started,
    )
    .await;

    match result {
        Ok(()) => Ok(MigrateOutcome::Committed {
            version,
            backup_id: backup_id(&transaction),
        }),
        Err(error) if stopping_started => {
            // 回滚需要切换期间已持久化的最新现场(legacy_unit/systemd_before/
            // resolv_conf_backup),prepared 快照没有这些字段,从磁盘重读。
            let latest = super::transaction::find_unfinished(root)?.ok_or_else(|| {
                InstallError::CorruptedTransaction(format!(
                    "migration transaction {} disappeared before the rollback",
                    transaction.transaction_id
                ))
            })?;
            match rollback_migrate(root, &latest, systemd) {
                Ok(()) => {
                    // 回滚成功也要给出原始失败原因:委托路径下这条 stderr
                    // 会流回前端,内联路径直接打印,用户都能看到失败点。
                    eprintln!("migrate: {error}");
                    if cancel_requested(options) {
                        Ok(MigrateOutcome::Cancelled { version })
                    } else {
                        Ok(MigrateOutcome::RolledBack { version })
                    }
                }
                Err(rollback_error) => {
                    eprintln!(
                        "migrate: {}",
                        crate::tr!(
                            crate::keys::MIGRATE_ROLLBACK_FAILED,
                            rollback_error = rollback_error
                        )
                    );
                    Ok(MigrateOutcome::RollbackFailed {
                        version,
                        reason: error.to_string(),
                    })
                }
            }
        }
        Err(error) => {
            if let Err(mark_error) =
                super::transaction::mark_phase(root, &transaction, Phase::Failed)
            {
                eprintln!(
                    "migrate: {}",
                    crate::tr!(
                        crate::keys::MIGRATE_TX_FAILED_MARK_WARNING,
                        error = mark_error
                    )
                );
            }
            Err(error)
        }
    }
}

/// 切换阶段:停止旧实例、从迁移备份重建 release/data/current、注册并启动
/// 受管服务、完整健康检查后提交状态。`stopping_started` 在标记 `stopping`
/// 阶段后置位,供调用方决定失败后走回滚还是仅标记失败。
#[allow(clippy::too_many_arguments)]
async fn run_migrate_switch<P: DocsProbe>(
    root: &InstallRoot,
    systemd: &Systemd,
    args: &MigrateArgs,
    options: &MigrateOptions<'_, P>,
    prepared: &PreparedMigration,
    transaction: &TransactionFile,
    version: &semver::Version,
    stopping_started: &mut bool,
) -> Result<(), InstallError> {
    let mut transaction = transaction.clone();
    let tx_dir = layout::territory_transactions_dir().join(&transaction.transaction_id);
    let source = &prepared.source;
    let instance = &prepared.instance;
    let data = root.canonical.join("data");

    ensure_not_cancelled(options)?;

    // ---- stopping:停止旧实例 ----
    super::transaction::mark_phase(root, &transaction, Phase::Stopping)?;
    *stopping_started = true;
    {
        // 旧部署可能是旧安装器直接写入受管注册路径的普通文件 unit;systemd
        // 注册的所有权保护会拒绝覆盖。实例识别已确认该 unit 属于旧部署,先
        // 停止并移入事务目录,回滚时放回原位。
        let preempted =
            preempt_registration_conflict(root, &transaction, systemd, source, instance)?;
        if preempted.is_some() {
            transaction.legacy_unit = preempted;
            super::transaction::persist(root, &transaction)?;
        }
        transaction.systemd_before = Some(super::pipeline::capture_before(
            systemd,
            ManagedService::LandscapeRouter,
        )?);
        let backup_dir = layout::territory_backups_dir()
            .join(&transaction.transaction_id)
            .join("host/resolv.conf");
        let _ = super::resolv::backup(systemd.resolv_conf(), &backup_dir)?;
        transaction.resolv_conf_backup = Some(format!(
            "backups/{}/host/resolv.conf",
            transaction.transaction_id
        ));
        super::transaction::persist(root, &transaction)?;
    }
    eprintln!(
        "migrate: {}",
        crate::tr!(crate::keys::MIGRATE_STOPPING_OLD_INSTANCE)
    );
    // preempt 已停止并移走旧 unit 时不再重复扫描:再次扫描只会得到空结果,
    // 被误判为前台实例并在 worker 里触发无法交互完成的确认。
    if transaction.legacy_unit.is_none() {
        let legacy = stop_legacy_instance(
            root,
            &transaction,
            systemd,
            source,
            instance,
            options,
            args.console_confirmed,
        )?;
        if legacy.is_some() {
            transaction.legacy_unit = legacy;
        }
        super::transaction::persist(root, &transaction)?;
    }
    confirm_instance_stopped(options, args.console_confirmed, instance)?;
    ensure_not_cancelled(options)?;

    // ---- activating:从迁移备份重建 ----
    super::transaction::mark_phase(root, &transaction, Phase::Activating)?;
    crate::interaction::presentation::operation_phase(
        crate::interaction::presentation::OperationPhase::Applying,
    );
    eprintln!(
        "migrate: {}",
        crate::tr!(crate::keys::MIGRATE_REBUILDING_INSTALLATION)
    );
    let backup = transaction.backup.as_ref().ok_or_else(|| {
        InstallError::CorruptedTransaction(
            "prepared migration transaction is missing the migration backup".into(),
        )
    })?;
    let backup_bytes =
        std::fs::read(layout::territory_relative(&backup.path)).map_err(InstallError::Io)?;
    let restore_dir = tx_dir.join("restore");
    let _ = std::fs::remove_dir_all(&restore_dir);
    let metadata = extract_lkb(&backup_bytes, &restore_dir)?;
    super::rollback::rebuild_release_from_backup(
        root,
        &tx_dir,
        &version.to_string(),
        &restore_dir,
    )?;
    std::fs::create_dir_all(&data).map_err(InstallError::Io)?;
    let geo_tmp_source = restore_dir.join("geo_tmp");
    if geo_tmp_source.is_dir() {
        super::rollback::copy_tree_into(&geo_tmp_source, &data.join("geo_tmp"))?;
    }
    let init_config =
        std::fs::read(restore_dir.join("landscape_init.toml")).map_err(InstallError::Io)?;
    super::rollback::write_file_atomic(&data.join("landscape_init.toml"), &init_config, 0o600)?;
    super::rollback::restore_current(root, &format!("releases/{version}"))?;
    ensure_not_cancelled(options)?;

    // ---- verifying(systemd):注册、启动与健康检查 ----
    let unit_sha = {
        eprintln!(
            "migrate: {}",
            crate::tr!(crate::keys::MIGRATE_STARTING_MANAGED_SERVICE)
        );
        let unit_sha = super::pipeline::write_unit_origin(
            root,
            systemd,
            ManagedService::LandscapeRouter,
            &systemd.render_definition(ManagedService::LandscapeRouter, &root.canonical)?,
        )?;
        systemd.register(
            ManagedService::LandscapeRouter,
            &root.canonical.join("service/landscape-router.service"),
        )?;
        systemd.enable(ManagedService::LandscapeRouter)?;
        systemd.start(ManagedService::LandscapeRouter)?;
        super::transaction::mark_phase(root, &transaction, Phase::Verifying)?;
        let pid = systemd.main_pid(ManagedService::LandscapeRouter)?;
        if pid == 0 {
            return Err(InstallError::Systemd(
                "service did not produce a main pid after start".into(),
            ));
        }
        eprintln!(
            "migrate: {}",
            crate::tr!(crate::keys::MIGRATE_VERIFYING_HEALTH)
        );
        let startup = StartupOptions {
            ports: &options.health.ports,
            expected_pid: pid,
            docs: &options.health.docs,
            unit_state: Some(&(|| systemd.active_state(ManagedService::LandscapeRouter).ok())),
            init_required: true,
            data_dir: &data,
            startup_timeout: options.health.startup_timeout,
            stable_duration: options.health.stable_duration,
        };
        wait_for_health(&startup, options).await?;
        unit_sha
    };
    ensure_not_cancelled(options)?;

    // ---- 提交 ----
    let new_state = build_migrate_state(root, &transaction, &metadata, &restore_dir, unit_sha)?;
    super::state::write_state(root, &new_state)?;
    super::transaction::mark_phase(root, &transaction, Phase::Committed)?;
    Ok(())
}

/// 旧实例停止后确认进程消失(前台实例由用户负责;systemd 实例应已停止)。
/// 与 [`legacy::stop_legacy_instance`] 的尾部检查共用,preempt 接管路径也执行。
fn confirm_instance_stopped<P: DocsProbe>(
    options: &MigrateOptions<'_, P>,
    console_confirmed: bool,
    instance: &Process,
) -> Result<(), InstallError> {
    if pid_alive(instance.pid) {
        legacy::confirm_foreground_stopped(options, console_confirmed)?;
        if pid_alive(instance.pid) {
            eprintln!(
                "migrate: {}",
                crate::tr!(crate::keys::MIGRATE_OLD_INSTANCE_STILL_RUNNING)
            );
        }
    }
    Ok(())
}

/// 健康检查等待期间响应取消(前台 ^C / worker SIGTERM),避免卡满
/// `startup_timeout` 才回滚。
async fn wait_for_health<P: DocsProbe>(
    startup: &super::health::StartupOptions<'_, P>,
    options: &MigrateOptions<'_, P>,
) -> Result<(), InstallError> {
    let health = async {
        super::health::wait_for_startup(startup).await?;
        super::health::observe_stable(startup).await?;
        Ok::<(), InstallError>(())
    };
    tokio::pin!(health);
    let mut ticker = tokio::time::interval(Duration::from_millis(100));
    loop {
        tokio::select! {
            result = &mut health => return result,
            _ = ticker.tick() => ensure_not_cancelled(options)?,
        }
    }
}

fn backup_id(transaction: &TransactionFile) -> String {
    transaction
        .backup
        .as_ref()
        .map(|backup| backup.backup_id.clone())
        .unwrap_or_default()
}

/// 校验源配置目录:必须是真实目录,含 Landscape 特征文件,且不是受管安装的 data。
pub(crate) fn validate_source_dir(path: &Path) -> Result<PathBuf, InstallError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        InstallError::ParameterUsage(format!(
            "--from {} is not accessible: {error}",
            path.display()
        ))
    })?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(InstallError::ParameterUsage(format!(
            "--from {} is not a real directory",
            path.display()
        )));
    }
    let source = std::fs::canonicalize(path).map_err(InstallError::Io)?;
    if !source.join("landscape.toml").is_file() && !source.join("landscape_init.lock").is_file() {
        return Err(InstallError::ParameterUsage(format!(
            "{} does not look like a Landscape configuration directory: neither landscape.toml nor landscape_init.lock is present",
            source.display()
        )));
    }
    if let Some(parent) = source.parent()
        && parent.join("state/install-state.json").is_file()
    {
        return Err(InstallError::ParameterUsage(format!(
            "{} appears to be the data directory of an lkit-managed installation; use lkit install/switch/repair commands instead of migrate",
            source.display()
        )));
    }
    Ok(source)
}

/// 通过固定端口识别指向源目录的运行中旧 Landscape 实例。
/// 端口上有无法确认身份的进程时阻断;实例未运行时返回明确错误(导出要求运行态)。
fn identify_running_instance(
    source: &Path,
    probe_ports: &[super::health::PortCheck],
) -> Result<Process, InstallError> {
    let ports: Vec<(super::process::Protocol, u16)> = probe_ports
        .iter()
        .map(|check| (check.protocol, check.port))
        .collect();
    let mut instance = None;
    let mut unidentified = Vec::new();
    for pid in super::process::pids_for_ports(&ports) {
        match super::process::read_process(pid) {
            Some(process) if super::process::is_external_landscape(&process, source) => {
                instance = Some(process);
            }
            _ => unidentified.push(pid),
        }
    }
    if !unidentified.is_empty() {
        return Err(InstallError::ProcessConflict(format!(
            "the fixed ports are occupied by unidentified processes {unidentified:?}; refusing to migrate while they run"
        )));
    }
    instance.ok_or_else(|| {
        InstallError::ExportFailed(format!(
            "no running Landscape instance serves {}; the migration backup requires the running config export API — start the old instance and retry",
            source.display()
        ))
    })
}

/// 创建迁移 `.lkb`:从运行实例 `/proc/<pid>/exe` 读后端二进制(文件已删除也可靠),
/// static 目录取进程 `--web` 参数(缺省 `config-dir/static`),static.zip 本地缺失时
/// 从发布仓库下载,仓库不可用则从 static 现场打包。
#[allow(clippy::too_many_arguments)]
async fn create_migration_backup(
    root: &InstallRoot,
    tx_dir: &Path,
    source: &Path,
    instance: &Process,
    exported: &export::ExportResult,
    version: &semver::Version,
    architecture: Architecture,
    args: &MigrateArgs,
) -> Result<BackupRef, InstallError> {
    std::fs::create_dir_all(tx_dir).map_err(InstallError::Io)?;
    let staged_binary = tx_dir.join(WEBSERVER_BINARY);
    copy_from_proc_exe(instance.pid, &staged_binary)?;

    let (_, web) = process::path_args(&instance.args);
    let static_dir = web.unwrap_or_else(|| source.join(STATIC_DIR));
    if !static_dir.is_dir() {
        return Err(InstallError::ExportFailed(format!(
            "cannot locate the static directory: the process runs without --web and {} is not a directory; extract the release static pages there or re-run the old instance with --web",
            static_dir.display()
        )));
    }

    let local_zip = source.join("static.zip");
    let static_zip = if local_zip.is_file() {
        local_zip
    } else {
        match fetch_static_zip(root, tx_dir, version, architecture, args).await? {
            Some(zip) => zip,
            None => {
                eprintln!(
                    "migrate: {}",
                    crate::tr!(crate::keys::MIGRATE_PACKING_STATIC_LOCALLY)
                );
                pack_static_zip(&static_dir, &tx_dir.join("static.zip"))?
            }
        }
    };

    let geo_tmp = source.join("geo_tmp");
    let remark = format!("migration from {}", source.display());
    lkb::create_backup(
        &layout::territory_backups_dir(),
        version,
        architecture.key(),
        &staged_binary,
        &exported.content,
        &static_dir,
        &static_zip,
        &geo_tmp,
        &remark,
        false,
        None,
    )
}

/// 从 `/proc/<pid>/exe` 复制运行中二进制到目标路径(文件已被删除时仍可读取)。
fn copy_from_proc_exe(pid: u32, target: &Path) -> Result<(), InstallError> {
    use std::io::{Read, Write};
    let mut file = std::fs::File::open(format!("/proc/{pid}/exe")).map_err(|error| {
        InstallError::ExportFailed(format!("cannot read /proc/{pid}/exe: {error}"))
    })?;
    let tmp = target.with_extension("tmp");
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(InstallError::Io)?;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(InstallError::Io)?;
        if read == 0 {
            break;
        }
        out.write_all(&buffer[..read]).map_err(InstallError::Io)?;
    }
    out.sync_all().map_err(InstallError::Io)?;
    drop(out);
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))
        .map_err(InstallError::Io)?;
    std::fs::rename(&tmp, target).map_err(|error| {
        let _ = std::fs::remove_file(&tmp);
        InstallError::Io(error)
    })
}

/// 从发布仓库下载指定版本的 `static.zip`;仓库不可用或版本不存在时返回 None。
async fn fetch_static_zip(
    _root: &InstallRoot,
    tx_dir: &Path,
    version: &semver::Version,
    architecture: Architecture,
    args: &MigrateArgs,
) -> Result<Option<PathBuf>, InstallError> {
    let spec = match &args.repository {
        Some(choice) => choice.clone().resolve()?,
        None => super::config::resolve_default_choice()?.resolve()?,
    };
    let provider = provider_for(spec.kind, spec.location.as_str())?;
    let release = match provider.release(version, architecture).await {
        Ok(release) => release,
        Err(_) => return Ok(None),
    };
    let download_dir = tx_dir.join("static-download");
    let _ = std::fs::remove_dir_all(&download_dir);
    std::fs::create_dir_all(&download_dir).map_err(InstallError::Io)?;
    match fetch_static_asset(&release, &download_dir).await {
        Ok(()) => Ok(Some(download_dir.join("static.zip"))),
        Err(_) => Ok(None),
    }
}

/// 从解压后的 static 目录现场打包 `static.zip`(条目带 `static/` 前缀,
/// 只允许目录与普通文件),打包后按仓库规则自校验。
fn pack_static_zip(static_dir: &Path, target: &Path) -> Result<PathBuf, InstallError> {
    let file = std::fs::File::create(target).map_err(InstallError::Io)?;
    let mut writer = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let prefix = Path::new(STATIC_DIR);
    pack_entry(&mut writer, &options, prefix, static_dir, static_dir)?;
    writer.finish().map_err(zip_error)?;

    // 按仓库解包规则自校验,保证恢复时可被正常消费。
    let (_, size) = hash_file(target)?;
    let check_dir = target
        .parent()
        .expect("packed zip has a parent")
        .join("static-pack-check");
    let _ = std::fs::remove_dir_all(&check_dir);
    super::repository::archive::extract_static_archive(
        &semver::Version::new(0, 0, 0),
        target,
        size,
        &check_dir,
    )
    .map_err(|error| {
        InstallError::ExportFailed(format!("packed static.zip failed self-validation: {error}"))
    })?;
    let _ = std::fs::remove_dir_all(&check_dir);
    Ok(target.to_path_buf())
}

fn zip_error(error: zip::result::ZipError) -> InstallError {
    InstallError::Io(std::io::Error::other(error))
}

fn pack_entry<W: std::io::Write + std::io::Seek>(
    writer: &mut zip::ZipWriter<W>,
    options: &zip::write::SimpleFileOptions,
    prefix: &Path,
    root: &Path,
    dir: &Path,
) -> Result<(), InstallError> {
    for entry in std::fs::read_dir(dir).map_err(InstallError::Io)? {
        let entry = entry.map_err(InstallError::Io)?;
        let file_type = entry.file_type().map_err(InstallError::Io)?;
        let entry_path = entry.path();
        let relative = entry_path
            .strip_prefix(root)
            .map_err(|error| InstallError::Io(std::io::Error::other(error)))?
            .to_path_buf();
        let zip_name = prefix.join(relative);
        if file_type.is_dir() {
            writer
                .add_directory(format!("{}/", zip_name.display()), *options)
                .map_err(zip_error)?;
            pack_entry(writer, options, prefix, root, &entry_path)?;
        } else if file_type.is_file() {
            writer
                .start_file(zip_name.display().to_string(), *options)
                .map_err(zip_error)?;
            let bytes = std::fs::read(&entry_path).map_err(InstallError::Io)?;
            use std::io::Write;
            writer.write_all(&bytes).map_err(InstallError::Io)?;
        } else {
            return Err(InstallError::ExportFailed(format!(
                "the static directory contains an unsupported entry {}",
                entry_path.display()
            )));
        }
    }
    Ok(())
}

/// 交互模式确认迁移计划;非交互模式必须显式 `--yes`。
fn confirm_migrate<P: DocsProbe>(
    options: &MigrateOptions<'_, P>,
    args: &MigrateArgs,
    source: &Path,
    version: &semver::Version,
    manager: ServiceManagerKind,
) -> Result<(), InstallError> {
    if args.console_confirmed {
        return Ok(());
    }
    if interactive::is_non_interactive() {
        if !args.yes {
            return Err(InstallError::ParameterUsage(
                "--yes is required in non-interactive mode to confirm the migration".into(),
            ));
        }
        return Ok(());
    }
    let accepted = (options.confirm)(&crate::tr!(
        crate::keys::MIGRATE_CONFIRM_PLAN,
        source = source.display(),
        version = version,
        manager = manager.key()
    ))?;
    if !accepted {
        return Err(InstallError::UserRefused(
            "user refused the migration plan".into(),
        ));
    }
    Ok(())
}

/// 构建迁移提交后的安装状态。
fn build_migrate_state(
    root: &InstallRoot,
    transaction: &TransactionFile,
    metadata: &lkb::BackupMetadata,
    restore_dir: &Path,
    unit_sha: String,
) -> Result<InstallState, InstallError> {
    let binary = restore_dir.join(WEBSERVER_BINARY);
    let (webserver_sha256, webserver_size) = hash_file(&binary)?;
    let static_zip = restore_dir.join("static.zip");
    let (static_sha256, static_size) = hash_file(&static_zip)?;
    let architecture = match metadata.architecture {
        lkb::BackupArchitecture::X86_64 => StateArchitecture::X86_64,
        lkb::BackupArchitecture::Aarch64 => StateArchitecture::Aarch64,
    };
    let version = transaction.target_version.clone().ok_or_else(|| {
        InstallError::CorruptedTransaction("migrate transaction is missing target_version".into())
    })?;
    let (initialization, service) = {
        (
            InitializationState {
                status: InitStatus::Complete,
                lock_present: true,
                initialized_at: Some(Utc::now()),
            },
            ServiceState {
                manager: StateServiceManager::Systemd,
                registered: true,
                enabled: true,
                verified: true,
                definition_path: Some("service/landscape-router.service".into()),
                definition_sha256: Some(unit_sha),
            },
        )
    };
    Ok(InstallState {
        schema_version: STATE_SCHEMA_VERSION,
        layout_version: STATE_LAYOUT_VERSION,
        install_root: root.install_root.display().to_string(),
        canonical_install_root: root.canonical.display().to_string(),
        active_version: version,
        assets: Assets {
            webserver: WebserverAsset {
                architecture,
                sha256: webserver_sha256,
                size: webserver_size,
            },
            static_archive: ArchiveAsset {
                sha256: static_sha256,
                size: static_size,
            },
        },
        initialization,
        service,
        last_transaction_id: Some(transaction.transaction_id.clone()),
        committed_at: Some(Utc::now()),
    })
}
