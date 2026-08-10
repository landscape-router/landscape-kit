use std::path::{Path, PathBuf};

use super::NETWORK_SERVICE_UNITS;
use super::UninstallArgs;
use crate::deployment::plan::InstallError;
use crate::deployment::root::InstallRoot;
use crate::service::systemd::{self, Registration, Systemd};

/// 幂等停止、disable 并注销受管 systemd 服务,最后执行 daemon-reload。
/// 注册链接缺失视为已注销;指向其他目标的链接属于所有权冲突,阻断。
pub(super) fn deactivate_systemd(
    systemd: &Systemd,
    root: &InstallRoot,
) -> Result<(), InstallError> {
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
pub(super) fn remove_managed_contents(
    root: &InstallRoot,
    args: &UninstallArgs,
) -> Result<(), InstallError> {
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
pub(super) fn purge_install_root(root: &InstallRoot) -> Result<(), InstallError> {
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

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use super::super::super::health::HealthOptions;
    use super::super::super::repository::test_server::{TestResponse, TestServer};
    use super::super::super::state::{
        ArchiveAsset, Assets, InitStatus, InitializationState, ServiceState, StateArchitecture,
        StateServiceManager, WebserverAsset,
    };
    use super::super::*;

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
        super::super::super::state::write_state(&install_root, &state).unwrap();
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
            super::super::super::state::load_state(&install_root)
                .unwrap()
                .is_none(),
            "install-state.json must be removed"
        );
        assert!(
            super::super::super::transaction::find_unfinished(&install_root)
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
        super::super::super::state::write_state(&install_root, &state).unwrap();
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
        super::super::super::state::write_state(&install_root, &state).unwrap();
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
        super::super::super::state::write_state(&install_root, &state).unwrap();
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
            super::super::super::transaction::find_unfinished(&install_root)
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
