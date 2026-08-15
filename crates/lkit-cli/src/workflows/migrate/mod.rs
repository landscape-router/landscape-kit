use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use chrono::Utc;

use super::artifacts::{STATIC_DIR, WEBSERVER_BINARY, fetch_static_asset, hash_file};
use super::backup as lkb;
use super::backup::extract_lkb;
use super::export;
use super::health::{DocsProbe, HealthOptions, StartupOptions};
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
use super::transaction::{BackupRef, Phase, TransactionFile};
use crate::interaction::interactive;

mod legacy;
mod rollback;
#[cfg(all(test, feature = "test-support"))]
mod tests;

pub(crate) use legacy::{stop_legacy_instance, stop_legacy_unit};
pub(crate) use rollback::{cleanup_migrated_root, restore_legacy_unit, rollback_migrate};

pub(crate) fn pid_alive(pid: u32) -> bool {
    unsafe {
        let result = libc::kill(pid as i32, 0);
        result == 0 || *libc::__errno_location() == libc::EPERM
    }
}

/// 迁移目标的服务管理模式。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MigrateManager {
    Auto,
    Systemd,
    None,
}

impl MigrateManager {
    fn resolve(self, systemd: &Systemd) -> Result<super::pipeline::ServiceManager, InstallError> {
        let choice = match self {
            Self::Auto => super::pipeline::ManagerChoice::Auto,
            Self::Systemd => super::pipeline::ManagerChoice::Systemd,
            Self::None => super::pipeline::ManagerChoice::None,
        };
        super::pipeline::select_manager(choice, systemd)
    }
}

/// `lkit migrate` 运行参数。
pub(crate) struct MigrateArgs {
    /// 旧手工部署的配置目录(`--from`)。
    pub config_dir: PathBuf,
    pub manager: MigrateManager,
    /// 非交互模式必须显式 `--yes`。
    pub yes: bool,
    /// 交互控制台已确认迁移计划(worker 进程无法读取 TUI 输入)。
    pub console_confirmed: bool,
    /// 仅用于 static.zip 缺失时从发布仓库下载;None 按 配置 > 官方 GitHub 解析。
    pub repository: Option<RepositoryChoice>,
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
    RollbackFailed {
        version: semver::Version,
        reason: String,
    },
}

/// 从非 lkit 手工部署迁移:先创建迁移 `.lkb`(旧版本,不升级),确认后停止旧实例,
/// 从备份重建 release/data/current,注册并启动受管服务,提交状态。
/// 旧实例的停止与恢复与安装在同一事务内,失败自动回滚避免断连。
pub(crate) async fn migrate_version<P: DocsProbe>(
    root: &InstallRoot,
    systemd: &Systemd,
    args: &MigrateArgs,
    options: &MigrateOptions<'_, P>,
) -> Result<MigrateOutcome, InstallError> {
    let source = validate_source_dir(&args.config_dir)?;
    let manager = args.manager.resolve(systemd)?;
    let is_systemd = manager == super::pipeline::ServiceManager::Systemd;

    // 导出配置要求旧实例运行中;运行实例同时提供后端版本与二进制来源。
    let instance = identify_running_instance(&source, options.probe_ports)?;
    let token = export::read_api_token(&source.join("landscape_api_token"), options.managed_uid)?;
    let exported = export::export_config(&options.export_base_url, &token).await?;
    let version = super::pipeline::parse_stable_version(&exported.version).map_err(|error| {
        InstallError::ExportFailed(format!(
            "invalid exported backend version {}: {error}",
            exported.version
        ))
    })?;

    let mut transaction = super::transaction::TransactionFile::new_migrate(root, &version)?;
    super::transaction::begin(root, &transaction)?;
    let tx_dir = root
        .canonical
        .join("transactions")
        .join(&transaction.transaction_id);

    let mut stopping_started = false;
    let result: Result<(), InstallError> = async {
        // ---- preparing:迁移备份 ----
        crate::interaction::presentation::operation_phase(
            crate::interaction::presentation::OperationPhase::Downloading,
        );
        let architecture = Architecture::host().ok_or_else(|| {
            InstallError::UnsupportedPlatform(
                crate::tr!(crate::keys::INSTALL_ONLY_X86_64_AND_AARCH64_SUPPORTED).into(),
            )
        })?;
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

        // ---- 确认:备份已完成,展示迁移计划 ----
        confirm_migrate(options, args, &source, &version, manager)?;

        super::transaction::mark_phase(root, &transaction, Phase::Prepared)?;

        // ---- stopping:停止旧实例 ----
        super::transaction::mark_phase(root, &transaction, Phase::Stopping)?;
        stopping_started = true;
        if is_systemd {
            transaction.systemd_before = Some(super::pipeline::capture_systemd_before(systemd)?);
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
        }
        let legacy = stop_legacy_instance(
            root,
            &transaction,
            systemd,
            &source,
            &instance,
            options,
            args.console_confirmed,
        )?;
        transaction.legacy_unit = legacy;
        super::transaction::persist(root, &transaction)?;

        // ---- activating:从迁移备份重建 ----
        super::transaction::mark_phase(root, &transaction, Phase::Activating)?;
        crate::interaction::presentation::operation_phase(
            crate::interaction::presentation::OperationPhase::Applying,
        );
        let backup_bytes =
            std::fs::read(root.canonical.join(&backup.path)).map_err(InstallError::Io)?;
        let restore_dir = tx_dir.join("restore");
        let _ = std::fs::remove_dir_all(&restore_dir);
        let metadata = extract_lkb(&backup_bytes, &restore_dir)?;
        super::rollback::rebuild_release_from_backup(
            root,
            &tx_dir,
            &version.to_string(),
            &restore_dir,
        )?;
        let data = root.canonical.join("data");
        std::fs::create_dir_all(&data).map_err(InstallError::Io)?;
        let geo_tmp_source = restore_dir.join("geo_tmp");
        if geo_tmp_source.is_dir() {
            super::rollback::copy_tree_into(&geo_tmp_source, &data.join("geo_tmp"))?;
        }
        let init_config =
            std::fs::read(restore_dir.join("landscape_init.toml")).map_err(InstallError::Io)?;
        super::rollback::write_file_atomic(&data.join("landscape_init.toml"), &init_config, 0o600)?;
        super::rollback::restore_current(root, &format!("releases/{version}"))?;

        // ---- verifying(systemd):注册、启动与健康检查 ----
        let unit_sha = if is_systemd {
            let unit_sha =
                super::pipeline::write_unit_origin(root, &systemd::render_unit(&root.canonical))?;
            super::systemd::register(
                systemd,
                &root.canonical.join("service/landscape-router.service"),
            )?;
            super::systemd::enable(systemd)?;
            super::systemd::start(systemd)?;
            super::transaction::mark_phase(root, &transaction, Phase::Verifying)?;
            let pid = super::systemd::main_pid(systemd)?;
            if pid == 0 {
                return Err(InstallError::Systemd(
                    "service did not produce a main pid after start".into(),
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
            Some(unit_sha)
        } else {
            None
        };

        // ---- 提交 ----
        let new_state = build_migrate_state(root, &transaction, &metadata, &restore_dir, unit_sha)?;
        super::state::write_state(root, &new_state)?;
        super::transaction::mark_phase(root, &transaction, Phase::Committed)?;
        Ok(())
    }
    .await;

    match result {
        Ok(()) => Ok(MigrateOutcome::Committed {
            version,
            backup_id: backup_id(&transaction),
        }),
        Err(error) if stopping_started => match rollback_migrate(root, &transaction, systemd) {
            Ok(()) => Ok(MigrateOutcome::RolledBack { version }),
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
        },
        Err(error) => {
            let _ = super::transaction::mark_phase(root, &transaction, Phase::Failed);
            Err(error)
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
    let static_dir = web
        .map(PathBuf::from)
        .unwrap_or_else(|| source.join(STATIC_DIR));
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
        &root.canonical.join("backups"),
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
    root: &InstallRoot,
    tx_dir: &Path,
    version: &semver::Version,
    architecture: Architecture,
    args: &MigrateArgs,
) -> Result<Option<PathBuf>, InstallError> {
    let spec = match &args.repository {
        Some(choice) => choice.clone().resolve()?,
        None => super::config::resolve_default_choice(root)?.resolve()?,
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
    InstallError::Io(std::io::Error::new(std::io::ErrorKind::Other, error))
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
            .map_err(|error| {
                InstallError::Io(std::io::Error::new(std::io::ErrorKind::Other, error))
            })?
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
    manager: super::pipeline::ServiceManager,
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
        manager = match manager {
            super::pipeline::ServiceManager::Systemd => "systemd",
            super::pipeline::ServiceManager::None => "none",
        }
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
    unit_sha: Option<String>,
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
    let (initialization, service) = match unit_sha {
        Some(unit_sha) => (
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
        ),
        None => (
            InitializationState {
                status: InitStatus::Pending,
                lock_present: false,
                initialized_at: None,
            },
            ServiceState {
                manager: StateServiceManager::None,
                registered: false,
                enabled: false,
                verified: false,
                definition_path: None,
                definition_sha256: None,
            },
        ),
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
