mod cleanup;

use super::artifacts::WEBSERVER_BINARY;
use super::backup;
use super::export;
use super::health::{DocsProbe, HealthOptions};
use super::manager::{ManagedService, ServiceManager};
use super::pipeline;
use super::plan::InstallError;
use super::root::InstallRoot;
use super::state::{InstallState, StateArchitecture, StateServiceManager};
use super::transaction::{Phase, TransactionFile};
use crate::deployment::layout;
use crate::interaction::presentation::{OperationPhase, operation_progress};

pub(crate) use self::cleanup::{cleanup_runtime_dirs, host_network_services_masked};
use self::cleanup::{deactivate, remove_managed_contents};

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
    /// 允许跳过保护 `.lkb`。
    pub allow_no_backup: bool,
    /// 保留 `data/` 只卸载服务与程序。
    pub keep_data: bool,
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
    manager: &dyn ServiceManager,
    args: &UninstallArgs,
    options: &UninstallOptions<'_, P>,
) -> Result<UninstallOutcome, InstallError> {
    validate_args(args)?;
    let is_systemd = state.service.manager == StateServiceManager::Systemd;
    let version = pipeline::parse_stable_version(&state.active_version).map_err(|error| {
        InstallError::CorruptedState(format!("invalid active version: {error}"))
    })?;
    let masked_host_network = host_network_services_masked(manager);

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
        transaction.systemd_before = Some(pipeline::capture_before(
            manager,
            ManagedService::LandscapeRouter,
        )?);
        let backup_dir = layout::territory_backups_dir()
            .join(&transaction.transaction_id)
            .join("host/resolv.conf");
        let _ = super::resolv::backup(manager.resolv_conf(), &backup_dir)?;
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
            deactivate(manager, root)?;
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
            cleanup_runtime_dirs(root)?;
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
    manager: &dyn ServiceManager,
) -> Result<(), InstallError> {
    if transaction.systemd_before.is_some() {
        deactivate(manager, root)?;
    }
    remove_managed_contents(
        root,
        &UninstallArgs {
            yes: true,
            allow_no_backup: transaction.backup.is_none(),
            keep_data: false,
            console_confirmed: true,
        },
    )
}

fn validate_args(_args: &UninstallArgs) -> Result<(), InstallError> {
    Ok(())
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
        &layout::territory_backups_dir(),
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
    use crate::service::systemd::Systemd;
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
                manager: StateServiceManager::Systemd,
                registered: true,
                enabled: true,
                verified: true,
                definition_path: Some("service/landscape-router.service".into()),
                definition_sha256: Some("d".repeat(64)),
            },
            last_transaction_id: None,
            committed_at: Some(chrono::Utc::now()),
        }
    }

    /// 与 cleanup::tests 相同的假 systemctl:对任意命令返回成功,
    /// 托管服务处于 inactive 状态(卸载路径跳过 stop)。
    fn fake_systemd(dir: &std::path::Path) -> Systemd {
        std::fs::create_dir_all(dir).unwrap();
        let script = dir.join("systemctl");
        std::fs::write(
            &script,
            r#"#!/bin/sh
case "$*" in
  "is-active landscape-router.service") echo inactive; exit 3;;
  "is-active lkit.service") echo inactive; exit 3;;
  "is-enabled landscape-router.service") echo disabled;;
  "is-enabled lkit.service") echo disabled;;
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

    fn setup_current(root: &InstallRoot) {
        std::fs::create_dir_all(root.canonical.join("data")).unwrap();
        std::fs::write(root.canonical.join("data/landscape_init.lock"), b"").unwrap();
        std::fs::write(root.canonical.join("data/landscape.toml"), b"").unwrap();
        std::fs::create_dir_all(root.canonical.join("backups")).unwrap();
        std::fs::create_dir_all(root.canonical.join("transactions")).unwrap();
        std::fs::write(root.canonical.join("config.toml"), b"[repository]\n").unwrap();
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

    async fn interactive_guard() -> std::sync::MutexGuard<'static, ()> {
        crate::interaction::interactive::test_guard()
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

    fn args(yes: bool, allow_no_backup: bool, keep_data: bool) -> UninstallArgs {
        UninstallArgs {
            yes,
            allow_no_backup,
            keep_data,
            console_confirmed: false,
        }
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
                &fake_systemd(&root.join("fake-systemd")),
                &args(false, false, false),
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
                &fake_systemd(&root.join("fake-systemd")),
                &args(true, false, false),
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
                &fake_systemd(&root.join("fake-systemd")),
                &args(true, true, false),
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
}
