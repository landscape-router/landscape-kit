use super::health::{DocsProbe, HealthOptions, StartupOptions};
use super::pipeline;
use super::plan::InstallError;
use super::root::InstallRoot;
use super::state::{InitStatus, InstallState, ServiceState, StateServiceManager};
use super::systemd::{self, Availability, Systemd};
use super::transaction::{Phase, TransactionFile, TransactionServiceManager};

/// service manager 迁移入口。迁移只改变 Landscape 的进程管理方式,
/// 不下载版本资产、不修改 `current` 或 Landscape data,也不创建 `.lkb`。
/// 已安装环境显式指定当前相同 manager 时允许并忽略。
pub(crate) async fn migrate_service_manager<P: DocsProbe>(
    root: &InstallRoot,
    state: &InstallState,
    target: TransactionServiceManager,
    systemd: &Systemd,
    health: &HealthOptions<P>,
    confirm: &dyn Fn(&str) -> Result<bool, InstallError>,
) -> Result<(), InstallError> {
    let from = match state.service.manager {
        StateServiceManager::Systemd => TransactionServiceManager::Systemd,
        StateServiceManager::None => TransactionServiceManager::None,
    };
    if from == target {
        return Ok(());
    }
    match (from, target) {
        (TransactionServiceManager::Systemd, TransactionServiceManager::None) => {
            migrate_to_none(root, state, systemd).await
        }
        (TransactionServiceManager::None, TransactionServiceManager::Systemd) => {
            migrate_to_systemd(root, state, systemd, health, confirm).await
        }
        _ => unreachable!("migration directions are covered by the match above"),
    }
}

/// systemd → none:验证受管 unit 所有权,停止并注销服务,保留 unit 原件,
/// 提交 `manager: none`,输出参考启动命令但不启动 Landscape。
async fn migrate_to_none(
    root: &InstallRoot,
    state: &InstallState,
    systemd: &Systemd,
) -> Result<(), InstallError> {
    pipeline::verify_unit_ownership(root, systemd)?;
    let before = pipeline::capture_systemd_before(systemd)?;
    let origin = root.canonical.join("service/landscape-router.service");
    let transaction = TransactionFile::new_service_migration(
        root,
        TransactionServiceManager::Systemd,
        TransactionServiceManager::None,
        before.clone(),
    )?;
    super::transaction::begin(root, &transaction)?;
    let result: Result<(), InstallError> = (|| {
        super::transaction::mark_phase(root, &transaction, Phase::Prepared)?;
        super::transaction::mark_phase(root, &transaction, Phase::Stopping)?;
        super::systemd::stop_and_wait(systemd, || {
            systemd::active_state(systemd)
                .map(|state| state != "active")
                .unwrap_or(true)
        })?;
        super::transaction::mark_phase(root, &transaction, Phase::Activating)?;
        systemd::disable(systemd)?;
        systemd::unregister(systemd, &origin)?;
        let mut updated = state.clone();
        updated.service = ServiceState {
            manager: StateServiceManager::None,
            registered: false,
            enabled: false,
            verified: false,
            definition_path: None,
            definition_sha256: None,
        };
        updated.last_transaction_id = Some(transaction.transaction_id.clone());
        updated.committed_at = Some(chrono::Utc::now());
        super::state::write_state(root, &updated)?;
        super::transaction::mark_phase(root, &transaction, Phase::Committed)?;
        Ok(())
    })();
    match result {
        Ok(()) => {
            println!(
                "install: {}",
                crate::tr!(crate::keys::SERVICE_MANAGER_MIGRATED_TO_NONE)
            );
            println!(
                "install: {}",
                crate::tr!(
                    crate::keys::SERVICE_MANAGER_START_MANUALLY_WITH,
                    command = pipeline::reference_command(root)
                )
            );
            Ok(())
        }
        Err(error) => match systemd::restore_systemd_before(systemd, &before, &origin) {
            Ok(()) => {
                let _ = super::transaction::mark_phase(root, &transaction, Phase::Failed);
                Err(error)
            }
            Err(restore_error) => {
                let _ = super::transaction::mark_phase(root, &transaction, Phase::Failed);
                Err(InstallError::Systemd(format!(
                    "{error}; additionally restoring the managed service state failed: {restore_error}; manual recovery is required"
                )))
            }
        },
    }
}

/// none → systemd:验证 systemd 可用性与受管资产,要求用户确认外部实例已停止
/// 且固定端口已释放,备份 `/etc/resolv.conf` 与 unit 原件后注册、启用、启动,
/// 执行完整健康检查并提交 `manager: systemd`。
async fn migrate_to_systemd<P: DocsProbe>(
    root: &InstallRoot,
    state: &InstallState,
    systemd: &Systemd,
    health: &HealthOptions<P>,
    confirm: &dyn Fn(&str) -> Result<bool, InstallError>,
) -> Result<(), InstallError> {
    if !matches!(systemd.probe(), Availability::Available { .. }) {
        return Err(InstallError::Systemd(
            "--service-manager systemd requested but systemd is not available".into(),
        ));
    }
    pipeline::verify_current_backend(root, state)?;
    pipeline::check_initialization(root, state)?;
    let accepted =
        confirm("stop your Landscape instance with your own process manager, then confirm")?;
    if !accepted {
        return Err(InstallError::UserRefused(
            "user refused to stop the external instance".into(),
        ));
    }
    let ports: Vec<(super::process::Protocol, u16)> = super::health::default_port_checks()
        .iter()
        .map(|check| (check.protocol, check.port))
        .collect();
    let pids = super::process::pids_for_ports(&ports);
    if !pids.is_empty() {
        return Err(InstallError::ProcessConflict(format!(
            "the fixed ports are still occupied by processes {pids:?}; release them before taking over with systemd"
        )));
    }

    let before = pipeline::capture_systemd_before(systemd)?;
    let origin = root.canonical.join("service/landscape-router.service");
    let mut transaction = TransactionFile::new_service_migration(
        root,
        TransactionServiceManager::None,
        TransactionServiceManager::Systemd,
        before.clone(),
    )?;
    super::transaction::begin(root, &transaction)?;
    let mut changed = false;

    let result: Result<(), InstallError> = async {
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
        let unit_sha = pipeline::write_unit_origin(root, &systemd::render_unit(&root.canonical))?;
        super::transaction::mark_phase(root, &transaction, Phase::Prepared)?;

        super::transaction::mark_phase(root, &transaction, Phase::Activating)?;
        changed = true;
        systemd::register(systemd, &origin)?;
        systemd::enable(systemd)?;
        systemd::start(systemd)?;
        super::transaction::mark_phase(root, &transaction, Phase::Verifying)?;
        let pid = systemd::main_pid(systemd)?;
        if pid == 0 {
            return Err(InstallError::Systemd(
                "service did not produce a main pid after start".into(),
            ));
        }
        let initialization_pending = state.initialization.status == InitStatus::Pending;
        let startup = StartupOptions {
            ports: &health.ports,
            expected_pid: pid,
            docs: &health.docs,
            unit_state: Some(&(|| systemd::active_state(systemd).ok())),
            init_required: initialization_pending,
            data_dir: &root.canonical.join("data"),
            startup_timeout: health.startup_timeout,
            stable_duration: health.stable_duration,
        };
        super::health::wait_for_startup(&startup).await?;
        super::health::observe_stable(&startup).await?;

        let mut updated = state.clone();
        updated.initialization = if initialization_pending {
            super::state::InitializationState {
                status: InitStatus::Complete,
                lock_present: true,
                initialized_at: Some(chrono::Utc::now()),
            }
        } else {
            state.initialization.clone()
        };
        updated.service = ServiceState {
            manager: StateServiceManager::Systemd,
            registered: true,
            enabled: true,
            verified: true,
            definition_path: Some("service/landscape-router.service".into()),
            definition_sha256: Some(unit_sha),
        };
        updated.last_transaction_id = Some(transaction.transaction_id.clone());
        updated.committed_at = Some(chrono::Utc::now());
        super::state::write_state(root, &updated)?;
        super::transaction::mark_phase(root, &transaction, Phase::Committed)?;
        Ok(())
    }
    .await;

    match result {
        Ok(()) => {
            println!(
                "install: {}",
                crate::tr!(crate::keys::SERVICE_MANAGER_MIGRATED_TO_SYSTEMD)
            );
            println!(
                "install: {}",
                crate::tr!(crate::keys::MANAGE_MANAGEMENT_INTERFACE)
            );
            Ok(())
        }
        Err(error) => {
            if changed {
                let _ = systemd::stop(systemd);
                match systemd::restore_systemd_before(systemd, &before, &origin) {
                    Ok(()) => {}
                    Err(restore_error) => {
                        let _ = super::transaction::mark_phase(root, &transaction, Phase::Failed);
                        return Err(InstallError::Systemd(format!(
                            "{error}; additionally revoking the systemd takeover failed: {restore_error}; manual recovery is required"
                        )));
                    }
                }
                if let Some(backup_path) = &transaction.resolv_conf_backup {
                    let backup_dir = root.canonical.join(backup_path);
                    if let Err(restore_error) =
                        super::resolv::restore(&systemd.resolv_conf, &backup_dir)
                    {
                        let _ = super::transaction::mark_phase(root, &transaction, Phase::Failed);
                        return Err(InstallError::Systemd(format!(
                            "{error}; additionally restoring /etc/resolv.conf failed: {restore_error}; manual recovery is required"
                        )));
                    }
                }
                let _ = super::transaction::mark_phase(root, &transaction, Phase::Failed);
                Err(InstallError::Systemd(format!(
                    "{error}; the systemd takeover was rolled back; Landscape is currently not running and must be started with your own process manager"
                )))
            } else {
                let _ = super::transaction::mark_phase(root, &transaction, Phase::Failed);
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::{TcpListener, UdpSocket};
    use std::os::unix::fs::PermissionsExt;

    use super::super::health::HealthOptions;
    use super::super::health::{DocsProbe, PortCheck};
    use super::super::root::InstallRoot;
    use super::super::state::{
        ArchiveAsset, Assets, InitializationState, ServiceState, StateArchitecture,
        StateServiceManager, WebserverAsset,
    };
    use super::*;

    const PAYLOAD: &[u8] = b"landscape webserver payload";

    fn temp_root(name: &str) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("lkit-migrate-test-{name}-{}", std::process::id()));
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

    fn fake_systemd(dir: &std::path::Path, main_pid: u32) -> Systemd {
        std::fs::create_dir_all(dir.join("units")).unwrap();
        std::fs::create_dir_all(dir.join("run")).unwrap();
        std::fs::write(dir.join("state"), b"active").unwrap();
        let script = dir.join("systemctl");
        std::fs::write(
            &script,
            format!(
                r#"#!/bin/sh
STATE_FILE="{}"
case "$*" in
  "show --property=Version") echo "Version=252.19";;
  "start landscape-router.service") echo active > "$STATE_FILE"; exit 0;;
  "stop landscape-router.service") echo inactive > "$STATE_FILE"; exit 0;;
  "show --property=ActiveState --value landscape-router.service") cat "$STATE_FILE";;
  "show --property=MainPID --value landscape-router.service") echo {main_pid};;
  "is-enabled landscape-router.service") echo enabled;;
  "is-active landscape-router.service") cat "$STATE_FILE";;
  "enable landscape-router.service") exit 0;;
  "disable landscape-router.service") exit 0;;
  "daemon-reload") exit 0;;
  *) exit 0;;
esac
"#,
                dir.join("state").display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        Systemd {
            systemctl: script,
            system_unit_dir: dir.join("units"),
            run_systemd_dir: dir.join("run"),
            pid1_is_systemd: true,
            resolv_conf: dir.join("resolv.conf"),
        }
    }

    fn setup_install(
        root: &InstallRoot,
        systemd: &Systemd,
        manager: StateServiceManager,
    ) -> InstallState {
        let (webserver_sha, webserver_size) = sha256_bytes(PAYLOAD);
        let release = root.canonical.join("releases/1.2.3");
        std::fs::create_dir_all(release.join("static")).unwrap();
        std::fs::write(release.join("landscape-webserver"), PAYLOAD).unwrap();
        std::fs::set_permissions(
            release.join("landscape-webserver"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let _ = std::fs::remove_file(root.canonical.join("current"));
        std::os::unix::fs::symlink("releases/1.2.3", root.canonical.join("current")).unwrap();
        std::fs::create_dir_all(root.canonical.join("data")).unwrap();
        std::fs::write(root.canonical.join("data/landscape_init.lock"), b"").unwrap();
        std::fs::write(root.canonical.join("data/landscape.toml"), b"").unwrap();
        let state = InstallState {
            schema_version: 1,
            layout_version: 1,
            install_root: root.install_root.display().to_string(),
            canonical_install_root: root.canonical.display().to_string(),
            active_version: "1.2.3".into(),
            assets: Assets {
                webserver: WebserverAsset {
                    architecture: StateArchitecture::X86_64,
                    sha256: webserver_sha,
                    size: webserver_size,
                },
                static_archive: ArchiveAsset {
                    sha256: "b".repeat(64),
                    size: 1,
                },
            },
            initialization: InitializationState {
                status: InitStatus::Complete,
                lock_present: true,
                initialized_at: Some(chrono::Utc::now()),
            },
            service: ServiceState {
                manager,
                registered: manager == StateServiceManager::Systemd,
                enabled: manager == StateServiceManager::Systemd,
                verified: manager == StateServiceManager::Systemd,
                definition_path: (manager == StateServiceManager::Systemd)
                    .then(|| "service/landscape-router.service".into()),
                definition_sha256: (manager == StateServiceManager::Systemd)
                    .then(|| "d".repeat(64)),
            },
            last_transaction_id: None,
            committed_at: Some(chrono::Utc::now()),
        };
        if manager == StateServiceManager::Systemd {
            std::fs::create_dir_all(root.canonical.join("service")).unwrap();
            let origin = root.canonical.join("service/landscape-router.service");
            std::fs::write(&origin, super::super::systemd::render_unit(&root.canonical)).unwrap();
            std::os::unix::fs::symlink(
                origin.canonicalize().unwrap(),
                systemd.system_unit_dir.join("landscape-router.service"),
            )
            .unwrap();
        }
        state
    }

    struct FakeDocs;

    impl DocsProbe for FakeDocs {
        async fn docs_ok(&self) -> bool {
            true
        }
    }

    static YES: fn(&str) -> Result<bool, InstallError> = |_| Ok(true);

    #[tokio::test]
    async fn migrates_systemd_to_none() {
        let dir = temp_root("to-none");
        let systemd = fake_systemd(&dir, std::process::id());
        let root_path = temp_root("to-none-root");
        let root = InstallRoot {
            install_root: root_path.clone(),
            canonical: root_path.clone(),
        };
        let state = setup_install(&root, &systemd, StateServiceManager::Systemd);
        let health = HealthOptions {
            docs: FakeDocs,
            ports: Vec::new(),
            startup_timeout: std::time::Duration::from_secs(5),
            stable_duration: std::time::Duration::from_millis(100),
        };

        migrate_service_manager(
            &root,
            &state,
            TransactionServiceManager::None,
            &systemd,
            &health,
            &YES,
        )
        .await
        .unwrap();

        assert!(
            !systemd
                .system_unit_dir
                .join("landscape-router.service")
                .exists()
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("state")).unwrap().trim(),
            "inactive"
        );
        let updated = super::super::state::load_state(&root).unwrap().unwrap();
        assert_eq!(updated.service.manager, StateServiceManager::None);
        assert!(!updated.service.registered);
        assert!(!updated.service.verified);
        assert!(updated.service.definition_path.is_none());
        assert!(
            root.canonical
                .join("service/landscape-router.service")
                .is_file()
        );
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&root_path);
    }

    #[tokio::test]
    async fn migrates_none_to_systemd() {
        let dir = temp_root("to-systemd");
        let systemd = fake_systemd(&dir, std::process::id());
        let root_path = temp_root("to-systemd-root");
        let root = InstallRoot {
            install_root: root_path.clone(),
            canonical: root_path.clone(),
        };
        let state = setup_install(&root, &systemd, StateServiceManager::None);

        let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
        let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
        let ports = vec![
            PortCheck {
                protocol: super::super::process::Protocol::Tcp,
                port: tcp.local_addr().unwrap().port(),
            },
            PortCheck {
                protocol: super::super::process::Protocol::Udp,
                port: udp.local_addr().unwrap().port(),
            },
        ];
        let health = HealthOptions {
            docs: FakeDocs,
            ports,
            startup_timeout: std::time::Duration::from_secs(10),
            stable_duration: std::time::Duration::from_millis(100),
        };

        migrate_service_manager(
            &root,
            &state,
            TransactionServiceManager::Systemd,
            &systemd,
            &health,
            &YES,
        )
        .await
        .unwrap();

        assert!(
            systemd
                .system_unit_dir
                .join("landscape-router.service")
                .is_symlink()
        );
        let updated = super::super::state::load_state(&root).unwrap().unwrap();
        assert_eq!(updated.service.manager, StateServiceManager::Systemd);
        assert!(updated.service.registered);
        assert!(updated.service.enabled);
        assert!(updated.service.verified);
        assert_eq!(
            updated.service.definition_path.as_deref(),
            Some("service/landscape-router.service")
        );
        assert!(
            super::super::transaction::find_unfinished(&root)
                .unwrap()
                .is_none()
        );

        drop(tcp);
        drop(udp);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&root_path);
    }
}
