use std::path::{Path, PathBuf};

use super::artifacts::WEBSERVER_BINARY;
use super::backup;
use super::export;
use super::health::{DocsProbe, HealthOptions};
use super::pipeline;
use super::plan::InstallError;
use super::root::InstallRoot;
use super::state::{InstallState, StateArchitecture, StateServiceManager};
use super::systemd::{self, Registration, Systemd};
use super::transaction::{BackupRef, Phase, TransactionFile};
use crate::interaction::presentation::{OperationPhase, operation_progress};

/// 网络接管可能停止/disable/mask 的宿主网络服务,用于卸载前的接管特征警告。
const NETWORK_SERVICE_UNITS: [&str; 4] = [
    "NetworkManager.service",
    "networking.service",
    "firewalld.service",
    "systemd-resolved.service",
];

/// uninstall 运行参数。
pub(crate) struct UninstallArgs {
    /// 非交互模式必须显式 `--yes`,否则返回参数错误。
    pub yes: bool,
    /// 允许跳过保护 `.lkb`;`--purge-root` 必须同时给出。
    pub allow_no_backup: bool,
    /// 保留 `data/` 只卸载服务与程序。
    pub keep_data: bool,
    /// 整树删除安装根目录(含 `config.toml` 与残留文件)。
    pub purge_root: bool,
    /// 交互控制台已确认卸载计划,跳过 `/dev/tty` 二次确认。
    pub console_confirmed: bool,
}

/// uninstall 运行参数(测试可注入)。
pub(crate) struct UninstallOptions<'a, P: DocsProbe> {
    pub export_base_url: String,
    pub token: &'a dyn Fn() -> Result<String, InstallError>,
    pub confirm: &'a dyn Fn(&str) -> Result<bool, InstallError>,
    pub health: &'a HealthOptions<P>,
}

#[derive(Debug)]
pub(crate) enum UninstallOutcome {
    Committed {
        version: semver::Version,
        backup_id: Option<String>,
    },
}

/// 卸载已提交安装。确认先于事务创建;默认创建保护 `.lkb`(失败阻断,
/// `--allow-no-backup` 显式跳过)。systemd 模式停止、disable、注销受管服务后
/// 删除受管内容;none 模式要求用户确认外部实例已停止。
pub(crate) async fn uninstall_installation<P: DocsProbe>(
    root: &InstallRoot,
    state: &InstallState,
    systemd: &Systemd,
    args: &UninstallArgs,
    options: &UninstallOptions<'_, P>,
) -> Result<UninstallOutcome, InstallError> {
    validate_args(args)?;
    let is_systemd = state.service.manager == StateServiceManager::Systemd;
    let version = pipeline::parse_stable_version(&state.active_version).map_err(|error| {
        InstallError::CorruptedState(format!("invalid active version: {error}"))
    })?;
    let masked_host_network = host_network_services_masked(systemd);

    // 确认先于事务创建:拒绝或缺少 `--yes` 时不创建事务、不写任何文件。
    confirm_uninstall(options, args, &version, masked_host_network)?;

    let mut transaction = TransactionFile::new_uninstall(root, &version)?;
    super::transaction::begin(root, &transaction)?;

    // 保护备份失败时保持现场不变;`--allow-no-backup` 才允许继续。
    if let Err(error) = create_protection_backup(root, state, &mut transaction, options).await {
        if !args.allow_no_backup {
            let _ = super::transaction::mark_phase(root, &transaction, Phase::Failed);
            return Err(error);
        }
        transaction.no_backup = true;
        eprintln!(
            "uninstall: {}",
            crate::tr!(
                crate::keys::UNINSTALL_WARNING_NO_PROTECTION_BACKUP,
                error = error
            )
        );
    }
    super::transaction::persist(root, &transaction)?;

    if is_systemd {
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
    }

    let steps = if is_systemd { 3 } else { 2 };
    operation_progress(OperationPhase::Preparing, Some((1, steps)));

    let result: Result<(), InstallError> = async {
        if is_systemd {
            super::transaction::mark_phase(root, &transaction, Phase::Stopping)?;
            operation_progress(OperationPhase::Stopping, Some((2, steps)));
            deactivate_systemd(systemd, root)?;
        } else if !crate::interaction::interactive::is_non_interactive() && !args.console_confirmed
        {
            // 非交互模式的「外部实例已停止」确认由 `--yes` 覆盖(见 confirm_uninstall)。
            let accepted = (options.confirm)(&crate::tr!(
                crate::keys::UNINSTALL_CONFIRM_STOP_WITH_OWN_MANAGER
            ))?;
            if !accepted {
                return Err(InstallError::UserRefused(
                    "user refused to stop the running instance".into(),
                ));
            }
        }
        super::transaction::mark_phase(root, &transaction, Phase::Activating)?;
        operation_progress(
            OperationPhase::Activating,
            Some((if is_systemd { 3 } else { 2 }, steps)),
        );
        remove_managed_contents(root, args)?;
        Ok(())
    }
    .await;

    match result {
        Ok(()) => {
            super::transaction::mark_phase(root, &transaction, Phase::Committed)?;
            // 事务已提交,日志不再需要;清理剩余运行态目录后再整树删除(可选)。
            cleanup_runtime_dirs(root)?;
            if args.purge_root {
                purge_install_root(root)?;
            }
            Ok(UninstallOutcome::Committed {
                version,
                backup_id: transaction
                    .backup
                    .as_ref()
                    .map(|backup| backup.backup_id.clone()),
            })
        }
        Err(error) => {
            let _ = super::transaction::mark_phase(root, &transaction, Phase::Failed);
            Err(error)
        }
    }
}

/// 卸载中断恢复的共用完成路径:前向完成,不自动回滚。
/// 幂等:服务已停止、注册链接已移除时直接跳过;重复执行只删除剩余受管内容。
pub(crate) fn complete_uninstall(
    root: &InstallRoot,
    transaction: &TransactionFile,
    systemd: &Systemd,
) -> Result<(), InstallError> {
    if transaction.systemd_before.is_some() {
        deactivate_systemd(systemd, root)?;
    }
    remove_managed_contents(
        root,
        &UninstallArgs {
            yes: true,
            allow_no_backup: transaction.backup.is_none(),
            keep_data: false,
            purge_root: false,
            console_confirmed: true,
        },
    )
}

fn validate_args(args: &UninstallArgs) -> Result<(), InstallError> {
    if args.purge_root && args.keep_data {
        return Err(InstallError::ParameterUsage(
            "--purge-root and --keep-data cannot be combined".into(),
        ));
    }
    if args.purge_root && !args.allow_no_backup {
        return Err(InstallError::ParameterUsage(
            "--purge-root deletes the protection backup together with the install root; --allow-no-backup is required".into(),
        ));
    }
    Ok(())
}

/// 幂等停止、disable 并注销受管 systemd 服务,最后执行 daemon-reload。
/// 注册链接缺失视为已注销;指向其他目标的链接属于所有权冲突,阻断。
fn deactivate_systemd(systemd: &Systemd, root: &InstallRoot) -> Result<(), InstallError> {
    if systemd::is_active(systemd)? {
        systemd::stop_and_wait(systemd, || {
            systemd::active_state(systemd)
                .map(|value| value != "active")
                .unwrap_or(true)
        })?;
    }
    let origin = root.canonical.join("service/landscape-router.service");
    match systemd::query_registration(systemd)? {
        Registration::Symlink { target } => {
            let origin_canonical = origin.canonicalize().map_err(InstallError::Io)?;
            if target != origin_canonical {
                return Err(InstallError::Systemd(format!(
                    "the system registration link is not owned by the managed unit origin: {}",
                    target.display()
                )));
            }
            if systemd::is_enabled(systemd)? {
                systemd::disable(systemd)?;
            }
            systemd::unregister(systemd, &origin)?;
        }
        Registration::Missing => {}
        Registration::Conflict { file_type } => {
            return Err(InstallError::Systemd(format!(
                "cannot unregister {}: {file_type} ownership conflict",
                systemd::UNIT_NAME
            )));
        }
    }
    systemd::daemon_reload(systemd)?;
    Ok(())
}

/// 删除受管内容。默认保留 `config.toml`、`backups/` 与 `transactions/`;
/// `--keep-data` 额外保留 `data/`。`logs/` 与 `run/` 在事务提交后由
/// [`cleanup_runtime_dirs`] 删除(提交阶段需要事务日志,不能提前删除)。
/// `--purge-root` 在事务提交后由 [`purge_install_root`] 删除全部剩余内容与安装根目录。
fn remove_managed_contents(root: &InstallRoot, args: &UninstallArgs) -> Result<(), InstallError> {
    let canonical = &root.canonical;
    let mut paths: Vec<PathBuf> = vec![
        canonical.join("current"),
        canonical.join("releases"),
        canonical.join("state"),
        canonical.join("service"),
    ];
    if !args.keep_data {
        paths.push(canonical.join("data"));
    }
    for path in paths {
        remove_path_if_present(&path)?;
    }
    Ok(())
}

/// 事务提交后删除运行态目录(`logs/` 与 `run/`,含 `install.lock`)。
/// 锁文件描述符仍由调用方持有,删除路径不影响锁的生命周期。
pub(crate) fn cleanup_runtime_dirs(root: &InstallRoot) -> Result<(), InstallError> {
    for path in [root.canonical.join("logs"), root.canonical.join("run")] {
        remove_path_if_present(&path)?;
    }
    Ok(())
}

/// `--purge-root`:删除安装根目录剩余全部内容(含 `config.toml`、`backups/`、
/// `transactions/` 与已提交的卸载事务文件),然后移除根目录本身。
/// 只在事务标记 `committed` 之后调用。
fn purge_install_root(root: &InstallRoot) -> Result<(), InstallError> {
    let canonical = &root.canonical;
    for entry in std::fs::read_dir(canonical).map_err(InstallError::Io)? {
        let entry = entry.map_err(InstallError::Io)?;
        remove_path_if_present(&entry.path())?;
    }
    std::fs::remove_dir(canonical).map_err(InstallError::Io)?;
    Ok(())
}

fn remove_path_if_present(path: &Path) -> Result<(), InstallError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(InstallError::Io(error)),
    };
    if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
        std::fs::remove_dir_all(path).map_err(InstallError::Io)
    } else {
        std::fs::remove_file(path).map_err(InstallError::Io)
    }
}

/// 检测宿主网络服务是否呈现网络接管特征(被停止、disable 或 mask)。
/// 只读探测,不修改系统状态;结果用于卸载前警告,不阻断。
/// 探测失败时按无接管特征处理(警告是尽力而为,不应阻断卸载)。
pub(crate) fn host_network_services_masked(systemd: &Systemd) -> bool {
    NETWORK_SERVICE_UNITS.iter().any(|unit| {
        systemd::inspect_host_service(systemd, unit)
            .map(|before| {
                before.installed
                    && (!before.active
                        || matches!(
                            before.enable_state.as_str(),
                            "disabled" | "masked" | "masked-runtime"
                        ))
            })
            .unwrap_or(false)
    })
}

/// 交互模式确认卸载计划与数据损失范围;检测到网络接管特征时追加警告确认。
/// 非交互模式必须显式 `--yes`。
fn confirm_uninstall<P: DocsProbe>(
    options: &UninstallOptions<'_, P>,
    args: &UninstallArgs,
    version: &semver::Version,
    masked_host_network: bool,
) -> Result<(), InstallError> {
    if args.console_confirmed {
        return Ok(());
    }
    if crate::interaction::interactive::is_non_interactive() {
        if !args.yes {
            return Err(InstallError::ParameterUsage(
                "--yes is required in non-interactive mode to confirm the uninstall".into(),
            ));
        }
        return Ok(());
    }
    let accepted = (options.confirm)(&crate::tr!(
        crate::keys::UNINSTALL_CONFIRM_PLAN,
        version = version
    ))?;
    if !accepted {
        return Err(InstallError::UserRefused(
            "user refused the uninstall plan".into(),
        ));
    }
    let accepted = (options.confirm)(&crate::tr!(crate::keys::UNINSTALL_CONFIRM_DATA_LOSS))?;
    if !accepted {
        return Err(InstallError::UserRefused(
            "user refused the uninstall plan".into(),
        ));
    }
    if masked_host_network {
        let accepted = (options.confirm)(&crate::tr!(crate::keys::UNINSTALL_CONFIRM_HOST_NETWORK))?;
        if !accepted {
            return Err(InstallError::UserRefused(
                "user refused the uninstall plan".into(),
            ));
        }
    }
    Ok(())
}

async fn create_protection_backup<P: DocsProbe>(
    root: &InstallRoot,
    state: &InstallState,
    transaction: &mut TransactionFile,
    options: &UninstallOptions<'_, P>,
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
        StateArchitecture::X86_64 => "x86_64",
        StateArchitecture::Aarch64 => "aarch64",
    };
    let version = pipeline::parse_stable_version(&state.active_version).map_err(|error| {
        InstallError::CorruptedState(format!("invalid active version: {error}"))
    })?;
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
        &version,
        architecture,
        &webserver,
        &exported.content,
        &static_dir,
        &static_archive,
        &geo_tmp,
        &crate::tr!(crate::keys::BACKUP_AUTO_REMARK_UNINSTALL),
        true,
        None,
    )?;
    transaction.backup = Some(backup_ref);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use super::super::health::HealthOptions;
    use super::super::repository::test_server::{TestResponse, TestServer};
    use super::super::state::{
        ArchiveAsset, Assets, InitStatus, InitializationState, ServiceState, StateArchitecture,
        StateServiceManager, WebserverAsset,
    };
    use super::*;

    const PAYLOAD: &[u8] = b"webserver payload 1.2.3";
    const ZIP: &[u8] = b"zip payload 1.2.3";

    fn temp_root(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("lkit-uninstall-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn sha256_bytes(bytes: &[u8]) -> (String, u64) {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let hex = hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        (hex, bytes.len() as u64)
    }

    fn activate_version(root: &InstallRoot, version: &str) {
        let release = root.canonical.join("releases").join(version);
        std::fs::create_dir_all(release.join("static")).unwrap();
        std::fs::write(release.join("landscape-webserver"), PAYLOAD).unwrap();
        std::fs::write(release.join("static.zip"), ZIP).unwrap();
        std::fs::write(
            release.join("static/index.html"),
            format!("static {version}"),
        )
        .unwrap();
        let _ = std::fs::remove_file(root.canonical.join("current"));
        std::os::unix::fs::symlink(
            format!("releases/{version}"),
            root.canonical.join("current"),
        )
        .unwrap();
    }

    fn install_state(root: &InstallRoot, version: &str) -> InstallState {
        let (webserver_sha, webserver_size) = sha256_bytes(PAYLOAD);
        let (static_sha, static_size) = sha256_bytes(ZIP);
        InstallState {
            schema_version: 1,
            layout_version: 1,
            install_root: root.install_root.display().to_string(),
            canonical_install_root: root.canonical.display().to_string(),
            active_version: version.into(),
            assets: Assets {
                webserver: WebserverAsset {
                    architecture: StateArchitecture::X86_64,
                    sha256: webserver_sha,
                    size: webserver_size,
                },
                static_archive: ArchiveAsset {
                    sha256: static_sha,
                    size: static_size,
                },
            },
            initialization: InitializationState {
                status: InitStatus::Complete,
                lock_present: true,
                initialized_at: Some(chrono::Utc::now()),
            },
            service: ServiceState {
                manager: StateServiceManager::None,
                registered: false,
                enabled: false,
                verified: false,
                definition_path: None,
                definition_sha256: None,
            },
            last_transaction_id: None,
            committed_at: Some(chrono::Utc::now()),
        }
    }

    fn setup_current(root: &InstallRoot) {
        std::fs::create_dir_all(root.canonical.join("data")).unwrap();
        std::fs::write(root.canonical.join("data/landscape_init.lock"), b"").unwrap();
        std::fs::write(root.canonical.join("data/landscape.toml"), b"").unwrap();
        std::fs::create_dir_all(root.canonical.join("backups")).unwrap();
        std::fs::create_dir_all(root.canonical.join("transactions")).unwrap();
        std::fs::write(root.canonical.join("config.toml"), b"[repository]\n").unwrap();
    }

    fn fake_systemd(dir: &std::path::Path) -> Systemd {
        let script = dir.join("systemctl");
        std::fs::write(
            &script,
            r#"#!/bin/sh
case "$*" in
  "is-active landscape-router.service") echo inactive; exit 3;;
  "is-enabled landscape-router.service") echo disabled;;
  "is-active NetworkManager.service") echo inactive; exit 3;;
  "is-enabled NetworkManager.service") echo enabled;;
  *) exit 0;;
esac
"#,
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::create_dir_all(dir.join("run")).unwrap();
        std::fs::create_dir_all(dir.join("units")).unwrap();
        Systemd {
            systemctl: script,
            system_unit_dir: dir.join("units"),
            run_systemd_dir: dir.join("run"),
            pid1_is_systemd: true,
            resolv_conf: dir.join("resolv.conf"),
        }
    }

    struct FakeDocs;

    impl DocsProbe for FakeDocs {
        async fn docs_ok(&self) -> bool {
            true
        }
    }

    fn none_health() -> HealthOptions<FakeDocs> {
        HealthOptions {
            docs: FakeDocs,
            ports: Vec::new(),
            startup_timeout: std::time::Duration::from_secs(5),
            stable_duration: std::time::Duration::from_millis(100),
        }
    }

    static YES: fn(&str) -> Result<bool, InstallError> = |_| Ok(true);
    static TOKEN: fn() -> Result<String, InstallError> = || Ok("tok".into());

    static INTERACTIVE_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    async fn interactive_guard() -> tokio::sync::MutexGuard<'static, ()> {
        INTERACTIVE_MUTEX.lock().await
    }

    fn export_server(version: String) -> TestServer {
        TestServer::start(move |path| {
            if path == export::EXPORT_PATH {
                TestResponse::ok(format!(
                    r#"{{"data":{{"filename":"landscape_init_v{version}.toml","version":"{version}","content":"version = \"{version}\"\n"}}}}"#
                ).into_bytes())
            } else {
                TestResponse::status(404, "Not Found", Vec::new())
            }
        })
    }

    struct NonInteractiveGuard;

    impl Drop for NonInteractiveGuard {
        fn drop(&mut self) {
            crate::interaction::interactive::configure(false);
        }
    }

    fn options_for<'a>(
        server: &TestServer,
        health: &'a HealthOptions<FakeDocs>,
    ) -> UninstallOptions<'a, FakeDocs> {
        UninstallOptions {
            export_base_url: server.base.clone(),
            token: &TOKEN,
            confirm: &YES,
            health,
        }
    }

    fn args(yes: bool, allow_no_backup: bool, keep_data: bool, purge_root: bool) -> UninstallArgs {
        UninstallArgs {
            yes,
            allow_no_backup,
            keep_data,
            purge_root,
            console_confirmed: false,
        }
    }

    #[tokio::test]
    async fn uninstalls_none_mode_and_keeps_config_backups_transactions() {
        let _guard = interactive_guard().await;
        crate::interaction::interactive::configure(false);
        let root = temp_root("none-mode");
        let install_root = InstallRoot {
            install_root: root.clone(),
            canonical: root.clone(),
        };
        activate_version(&install_root, "1.2.3");
        setup_current(&install_root);
        std::fs::write(
            install_root.canonical.join("data/landscape_db.sqlite"),
            b"db",
        )
        .unwrap();
        let state = install_state(&install_root, "1.2.3");
        super::super::state::write_state(&install_root, &state).unwrap();
        let server = export_server("1.2.3".into());

        let outcome = uninstall_installation(
            &install_root,
            &state,
            &Systemd::host(),
            &args(true, false, false, false),
            &options_for(&server, &none_health()),
        )
        .await
        .unwrap();
        assert!(matches!(
            outcome,
            UninstallOutcome::Committed { version, backup_id } if version == semver::Version::new(1, 2, 3) && backup_id.is_some()
        ));
        assert!(!install_root.canonical.join("current").exists());
        assert!(!install_root.canonical.join("releases").exists());
        assert!(!install_root.canonical.join("data").exists());
        assert!(!install_root.canonical.join("state").exists());
        assert!(!install_root.canonical.join("service").exists());
        assert!(!install_root.canonical.join("logs").exists());
        assert!(!install_root.canonical.join("run").exists());
        assert_eq!(
            std::fs::read_to_string(install_root.canonical.join("config.toml")).unwrap(),
            "[repository]\n",
            "config.toml must be preserved byte-for-byte"
        );
        assert!(install_root.canonical.join("backups").is_dir());
        assert!(install_root.canonical.join("transactions").is_dir());
        assert!(
            super::super::state::load_state(&install_root)
                .unwrap()
                .is_none(),
            "install-state.json must be removed"
        );
        assert!(
            super::super::transaction::find_unfinished(&install_root)
                .unwrap()
                .is_none()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn keep_data_preserves_data_and_removes_the_rest() {
        let _guard = interactive_guard().await;
        crate::interaction::interactive::configure(true);
        let _reset = NonInteractiveGuard;
        let root = temp_root("keep-data");
        let install_root = InstallRoot {
            install_root: root.clone(),
            canonical: root.clone(),
        };
        activate_version(&install_root, "1.2.3");
        setup_current(&install_root);
        let state = install_state(&install_root, "1.2.3");
        super::super::state::write_state(&install_root, &state).unwrap();
        let server = export_server("1.2.3".into());

        uninstall_installation(
            &install_root,
            &state,
            &Systemd::host(),
            &args(true, false, true, false),
            &options_for(&server, &none_health()),
        )
        .await
        .unwrap();
        assert!(
            install_root
                .canonical
                .join("data/landscape_init.lock")
                .exists(),
            "data must be preserved with --keep-data"
        );
        assert!(
            !install_root
                .canonical
                .join("state/install-state.json")
                .exists()
        );
        assert!(!install_root.canonical.join("current").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn purge_root_requires_allow_no_backup() {
        let _guard = interactive_guard().await;
        crate::interaction::interactive::configure(false);
        let root = temp_root("purge-param");
        let install_root = InstallRoot {
            install_root: root.clone(),
            canonical: root.clone(),
        };
        let state = install_state(&install_root, "1.2.3");
        let server = TestServer::start(|_| TestResponse::status(500, "boom", Vec::new()));
        assert!(matches!(
            uninstall_installation(
                &install_root,
                &state,
                &Systemd::host(),
                &args(true, false, false, true),
                &options_for(&server, &none_health()),
            )
            .await,
            Err(InstallError::ParameterUsage(_))
        ));
        assert!(matches!(
            uninstall_installation(
                &install_root,
                &state,
                &Systemd::host(),
                &args(true, true, true, true),
                &options_for(&server, &none_health()),
            )
            .await,
            Err(InstallError::ParameterUsage(_))
        ));
        assert!(
            super::super::transaction::find_unfinished(&install_root)
                .unwrap()
                .is_none(),
            "parameter errors must not create a transaction"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn purge_root_deletes_the_whole_install_root_after_commit() {
        let _guard = interactive_guard().await;
        crate::interaction::interactive::configure(true);
        let _reset = NonInteractiveGuard;
        let root = temp_root("purge-root");
        let install_root = InstallRoot {
            install_root: root.clone(),
            canonical: root.clone(),
        };
        activate_version(&install_root, "1.2.3");
        setup_current(&install_root);
        let state = install_state(&install_root, "1.2.3");
        super::super::state::write_state(&install_root, &state).unwrap();
        let server = export_server("1.2.3".into());

        uninstall_installation(
            &install_root,
            &state,
            &Systemd::host(),
            &args(true, true, false, true),
            &options_for(&server, &none_health()),
        )
        .await
        .unwrap();
        assert!(!root.exists(), "the whole install root must be removed");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn requires_yes_in_non_interactive_mode() {
        let _guard = interactive_guard().await;
        crate::interaction::interactive::configure(true);
        let _reset = NonInteractiveGuard;
        let root = temp_root("non-interactive");
        let install_root = InstallRoot {
            install_root: root.clone(),
            canonical: root.clone(),
        };
        activate_version(&install_root, "1.2.3");
        setup_current(&install_root);
        let state = install_state(&install_root, "1.2.3");
        super::super::state::write_state(&install_root, &state).unwrap();
        let server = export_server("1.2.3".into());
        assert!(matches!(
            uninstall_installation(
                &install_root,
                &state,
                &Systemd::host(),
                &args(false, false, false, false),
                &options_for(&server, &none_health()),
            )
            .await,
            Err(InstallError::ParameterUsage(_))
        ));
        assert!(
            super::super::transaction::find_unfinished(&install_root)
                .unwrap()
                .is_none(),
            "missing --yes must not create a transaction"
        );
        assert_eq!(
            std::fs::read_link(install_root.canonical.join("current")).unwrap(),
            std::path::PathBuf::from("releases/1.2.3"),
            "no files must change before confirmation"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn blocks_without_allow_no_backup_when_protection_fails() {
        let _guard = interactive_guard().await;
        crate::interaction::interactive::configure(false);
        let root = temp_root("protection-blocked");
        let install_root = InstallRoot {
            install_root: root.clone(),
            canonical: root.clone(),
        };
        activate_version(&install_root, "1.2.3");
        setup_current(&install_root);
        let state = install_state(&install_root, "1.2.3");
        super::super::state::write_state(&install_root, &state).unwrap();
        let server = TestServer::start(|_| TestResponse::status(500, "boom", Vec::new()));
        assert!(
            uninstall_installation(
                &install_root,
                &state,
                &Systemd::host(),
                &args(true, false, false, false),
                &options_for(&server, &none_health()),
            )
            .await
            .is_err()
        );
        assert_eq!(
            std::fs::read_link(install_root.canonical.join("current")).unwrap(),
            std::path::PathBuf::from("releases/1.2.3"),
            "the installation must stay untouched"
        );
        assert_eq!(
            super::super::state::load_state(&install_root)
                .unwrap()
                .unwrap()
                .active_version,
            "1.2.3"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn continues_with_allow_no_backup_when_protection_fails() {
        let _guard = interactive_guard().await;
        crate::interaction::interactive::configure(true);
        let _reset = NonInteractiveGuard;
        let root = temp_root("protection-allow");
        let install_root = InstallRoot {
            install_root: root.clone(),
            canonical: root.clone(),
        };
        activate_version(&install_root, "1.2.3");
        setup_current(&install_root);
        let state = install_state(&install_root, "1.2.3");
        super::super::state::write_state(&install_root, &state).unwrap();
        let server = TestServer::start(|_| TestResponse::status(500, "boom", Vec::new()));
        assert!(matches!(
            uninstall_installation(
                &install_root,
                &state,
                &Systemd::host(),
                &args(true, true, false, false),
                &options_for(&server, &none_health()),
            )
            .await,
            Ok(UninstallOutcome::Committed { backup_id, .. }) if backup_id.is_none()
        ));
        assert!(
            !install_root
                .canonical
                .join("state/install-state.json")
                .exists()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn systemd_mode_unregisters_the_unit() {
        let _guard = interactive_guard().await;
        crate::interaction::interactive::configure(false);
        let root = temp_root("systemd-mode");
        let dir = std::env::temp_dir().join(format!(
            "lkit-uninstall-test-systemd-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let systemd = fake_systemd(&dir);
        let install_root = InstallRoot {
            install_root: root.clone(),
            canonical: root.clone(),
        };
        activate_version(&install_root, "1.2.3");
        setup_current(&install_root);
        std::fs::create_dir_all(install_root.canonical.join("service")).unwrap();
        let unit_origin = install_root
            .canonical
            .join("service/landscape-router.service");
        std::fs::write(&unit_origin, "[Unit]\n[Service]\n[Install]\n").unwrap();
        std::os::unix::fs::symlink(
            unit_origin.canonicalize().unwrap(),
            dir.join("units/landscape-router.service"),
        )
        .unwrap();
        let unit_origin_canonical = unit_origin.canonicalize().unwrap();
        let state = install_state(&install_root, "1.2.3");
        let mut state = state;
        state.service = ServiceState {
            manager: StateServiceManager::Systemd,
            registered: true,
            enabled: true,
            verified: true,
            definition_path: Some("service/landscape-router.service".into()),
            definition_sha256: Some("a".repeat(64)),
        };
        super::super::state::write_state(&install_root, &state).unwrap();
        let server = export_server("1.2.3".into());

        uninstall_installation(
            &install_root,
            &state,
            &systemd,
            &args(true, false, false, false),
            &options_for(&server, &none_health()),
        )
        .await
        .unwrap();
        assert!(
            !dir.join("units/landscape-router.service").exists(),
            "the registration link must be removed"
        );
        assert!(!install_root.canonical.join("service").exists());
        assert!(
            super::super::transaction::find_unfinished(&install_root)
                .unwrap()
                .is_none()
        );
        let tx = transaction_json(&install_root);
        assert_eq!(tx["phase"], "committed");
        assert_eq!(
            tx["systemd_before"]["registration"]["kind"], "symlink",
            "systemd_before must record the registration before the uninstall"
        );
        assert_eq!(
            tx["systemd_before"]["registration"]["target"],
            unit_origin_canonical.to_str().unwrap()
        );
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn transaction_json(root: &InstallRoot) -> serde_json::Value {
        let entries: Vec<_> = std::fs::read_dir(root.canonical.join("transactions"))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert!(!entries.is_empty());
        let newest = entries
            .into_iter()
            .max_by(|a, b| a.file_name().cmp(&b.file_name()))
            .unwrap();
        serde_json::from_slice(&std::fs::read(newest.path()).unwrap()).unwrap()
    }
}
