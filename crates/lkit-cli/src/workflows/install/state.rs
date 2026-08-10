use std::os::unix::fs::MetadataExt;

use chrono::Utc;

use super::super::artifacts::{BuiltRelease, WEBSERVER_BINARY, hash_file};
use super::super::plan::InstallError;
use super::super::repository::{Architecture, Release};
use super::super::root::InstallRoot;
use super::super::state::{
    ArchiveAsset, Assets, InitStatus, InitializationState, InstallState, STATE_LAYOUT_VERSION,
    STATE_SCHEMA_VERSION, ServiceState, StateArchitecture, StateServiceManager, WebserverAsset,
};
use super::super::systemd::{self, Systemd};

/// 提交到状态中的初始化与服务信息。
pub(super) struct UnitActivation {
    pub unit_sha: String,
    pub initialization: InitializationState,
    pub service: ServiceState,
}

/// 初始化状态检查:初始化锁高危异常阻断;pending 状态下保证一次性初始化输入
/// 是当前运行用户所有的 `0600` 普通文件。complete 状态不再读取该文件内容。
pub(crate) fn check_initialization(
    root: &InstallRoot,
    state: &InstallState,
) -> Result<(), InstallError> {
    let data = root.canonical.join("data");
    let lock_present = initialization_lock_present(&data.join("landscape_init.lock"))?;
    let has_database = data.join("landscape_db.sqlite").exists();
    let has_persistent = data.join("landscape.toml").is_file();
    if state.initialization.status == InitStatus::Complete && !lock_present {
        return Err(InstallError::CorruptedState(
            "initialization lock is missing although initialization completed; Landscape may re-read the init file and wipe configuration"
                .into(),
        ));
    }
    if state.initialization.status == InitStatus::Pending
        && (has_database || has_persistent)
        && !lock_present
    {
        return Err(InstallError::CorruptedState(
            "initialization lock is missing although database or persistent config appeared".into(),
        ));
    }
    if state.initialization.status == InitStatus::Pending && !lock_present {
        validate_pending_init_config(&data.join("landscape_init.toml"))?;
    }
    Ok(())
}

fn initialization_lock_present(path: &std::path::Path) -> Result<bool, InstallError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(true),
        Ok(_) => Err(InstallError::CorruptedState(
            "data/landscape_init.lock must be a regular file".into(),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(InstallError::Io(error)),
    }
}

fn validate_pending_init_config(path: &std::path::Path) -> Result<(), InstallError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            InstallError::CorruptedState(
                "data/landscape_init.toml is missing while initialization is pending".into(),
            )
        } else {
            InstallError::Io(error)
        }
    })?;
    if !metadata.file_type().is_file() {
        return Err(InstallError::CorruptedState(
            "data/landscape_init.toml must be a regular file while initialization is pending"
                .into(),
        ));
    }
    let expected_uid = unsafe { libc::geteuid() };
    if metadata.uid() != expected_uid {
        return Err(InstallError::CorruptedState(format!(
            "data/landscape_init.toml must be owned by uid {expected_uid} while initialization is pending"
        )));
    }
    if metadata.mode() & 0o7777 != 0o600 {
        return Err(InstallError::CorruptedState(
            "data/landscape_init.toml must have mode 0600 while initialization is pending".into(),
        ));
    }
    Ok(())
}

/// 验证当前后端二进制摘要与状态记录一致。
pub(crate) fn verify_current_backend(
    root: &InstallRoot,
    state: &InstallState,
) -> Result<(), InstallError> {
    let binary = root
        .canonical
        .join("releases")
        .join(&state.active_version)
        .join(WEBSERVER_BINARY);
    let (actual, size) = hash_file(&binary)?;
    if actual != state.assets.webserver.sha256 || size != state.assets.webserver.size {
        return Err(InstallError::CorruptedState(format!(
            "the active backend binary drifted from the recorded checksum (expected {}, got {}); repair with --repair-binary first",
            state.assets.webserver.sha256, actual
        )));
    }
    Ok(())
}

/// 验证受管 unit 原件仍满足安全不变量,且系统注册链接仍指向该原件。
/// 系统注册链接缺失、指向其他目标或为普通文件时属于所有权冲突,不能自动修复。
pub(crate) fn verify_unit_ownership(
    root: &InstallRoot,
    systemd: &Systemd,
) -> Result<(), InstallError> {
    let origin = root.canonical.join("service/landscape-router.service");
    let content = std::fs::read_to_string(&origin).map_err(InstallError::Io)?;
    systemd::validate_unit(&content, &root.canonical)?;
    let origin_canonical = origin.canonicalize().map_err(InstallError::Io)?;
    match systemd::query_registration(systemd)? {
        systemd::Registration::Symlink { target } if target == origin_canonical => Ok(()),
        other => Err(InstallError::Systemd(format!(
            "the system registration link is not owned by the managed unit origin: {other:?}"
        ))),
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn build_switched_state(
    root: &InstallRoot,
    release: &Release,
    built: &BuiltRelease,
    previous: &InstallState,
    transaction_id: &str,
    unit_sha: Option<String>,
) -> InstallState {
    let architecture = architecture_from_state(previous);
    let lock_present = root.canonical.join("data/landscape_init.lock").is_file();
    let service = match previous.service.manager {
        StateServiceManager::Systemd => ServiceState {
            manager: StateServiceManager::Systemd,
            registered: true,
            enabled: true,
            verified: true,
            definition_path: Some("service/landscape-router.service".into()),
            definition_sha256: unit_sha,
        },
        StateServiceManager::None => ServiceState {
            manager: StateServiceManager::None,
            registered: false,
            enabled: false,
            verified: false,
            definition_path: None,
            definition_sha256: None,
        },
    };
    InstallState {
        schema_version: STATE_SCHEMA_VERSION,
        layout_version: STATE_LAYOUT_VERSION,
        install_root: root.install_root.display().to_string(),
        canonical_install_root: root.canonical.display().to_string(),
        active_version: release.version.to_string(),
        assets: Assets {
            webserver: WebserverAsset {
                architecture: match architecture {
                    Architecture::X86_64 => StateArchitecture::X86_64,
                    Architecture::Aarch64 => StateArchitecture::Aarch64,
                },
                sha256: built.webserver_sha256.clone(),
                size: built.webserver_size,
            },
            static_archive: ArchiveAsset {
                sha256: release.assets.static_archive.sha256.clone(),
                size: release.assets.static_archive.size,
            },
        },
        initialization: InitializationState {
            status: previous.initialization.status,
            lock_present,
            initialized_at: previous.initialization.initialized_at,
        },
        service,
        last_transaction_id: Some(transaction_id.to_string()),
        committed_at: Some(Utc::now()),
    }
}

pub(crate) fn architecture_from_state(state: &InstallState) -> Architecture {
    match state.assets.webserver.architecture {
        StateArchitecture::X86_64 => Architecture::X86_64,
        StateArchitecture::Aarch64 => Architecture::Aarch64,
    }
}

pub(super) fn build_state(
    root: &InstallRoot,
    release: &Release,
    architecture: Architecture,
    built: &BuiltRelease,
    activation: &UnitActivation,
) -> InstallState {
    let architecture = match architecture {
        Architecture::X86_64 => StateArchitecture::X86_64,
        Architecture::Aarch64 => StateArchitecture::Aarch64,
    };
    InstallState {
        schema_version: STATE_SCHEMA_VERSION,
        layout_version: STATE_LAYOUT_VERSION,
        install_root: root.install_root.display().to_string(),
        canonical_install_root: root.canonical.display().to_string(),
        active_version: release.version.to_string(),
        assets: Assets {
            webserver: WebserverAsset {
                architecture,
                sha256: built.webserver_sha256.clone(),
                size: built.webserver_size,
            },
            static_archive: ArchiveAsset {
                sha256: release.assets.static_archive.sha256.clone(),
                size: release.assets.static_archive.size,
            },
        },
        initialization: activation.initialization.clone(),
        service: activation.service.clone(),
        last_transaction_id: None,
        committed_at: Some(Utc::now()),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn temp_root(name: &str) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("lkit-pipeline-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn initialization_state(status: InitStatus) -> InstallState {
        InstallState {
            schema_version: STATE_SCHEMA_VERSION,
            layout_version: STATE_LAYOUT_VERSION,
            install_root: "/tmp/lkit-init-check".into(),
            canonical_install_root: "/tmp/lkit-init-check".into(),
            active_version: "1.2.3".into(),
            assets: Assets {
                webserver: WebserverAsset {
                    architecture: StateArchitecture::X86_64,
                    sha256: "a".repeat(64),
                    size: 1,
                },
                static_archive: ArchiveAsset {
                    sha256: "b".repeat(64),
                    size: 1,
                },
            },
            initialization: InitializationState {
                status,
                lock_present: status == InitStatus::Complete,
                initialized_at: (status == InitStatus::Complete).then(Utc::now),
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
            committed_at: Some(Utc::now()),
        }
    }

    #[test]
    fn complete_initialization_ignores_init_file_content_and_absence() {
        let path = temp_root("complete-init-file");
        let root = InstallRoot {
            install_root: path.clone(),
            canonical: path.clone(),
        };
        let data = path.join("data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("landscape_init.lock"), b"").unwrap();
        std::fs::write(data.join("landscape_init.toml"), b"user_modified = true\n").unwrap();
        let state = initialization_state(InitStatus::Complete);

        check_initialization(&root, &state).unwrap();
        assert_eq!(
            std::fs::read(data.join("landscape_init.toml")).unwrap(),
            b"user_modified = true\n"
        );

        std::fs::remove_file(data.join("landscape_init.toml")).unwrap();
        check_initialization(&root, &state).unwrap();

        std::fs::remove_file(data.join("landscape_init.lock")).unwrap();
        assert!(matches!(
            check_initialization(&root, &state),
            Err(InstallError::CorruptedState(_))
        ));
        let lock_target = data.join("lock-target");
        std::fs::write(&lock_target, b"").unwrap();
        std::os::unix::fs::symlink(&lock_target, data.join("landscape_init.lock")).unwrap();
        assert!(matches!(
            check_initialization(&root, &state),
            Err(InstallError::CorruptedState(_))
        ));
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn pending_initialization_requires_a_safe_init_file() {
        let path = temp_root("pending-init-file");
        let root = InstallRoot {
            install_root: path.clone(),
            canonical: path.clone(),
        };
        let data = path.join("data");
        std::fs::create_dir_all(&data).unwrap();
        let init = data.join("landscape_init.toml");
        let state = initialization_state(InitStatus::Pending);

        assert!(matches!(
            check_initialization(&root, &state),
            Err(InstallError::CorruptedState(_))
        ));

        std::fs::write(&init, b"\xffcontent is not parsed\n").unwrap();
        std::fs::set_permissions(&init, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            check_initialization(&root, &state),
            Err(InstallError::CorruptedState(_))
        ));

        std::fs::set_permissions(&init, std::fs::Permissions::from_mode(0o600)).unwrap();
        check_initialization(&root, &state).unwrap();

        std::fs::remove_file(&init).unwrap();
        let target = data.join("init-target.toml");
        std::fs::write(&target, b"version = \"1.2.3\"\n").unwrap();
        std::os::unix::fs::symlink(&target, &init).unwrap();
        assert!(matches!(
            check_initialization(&root, &state),
            Err(InstallError::CorruptedState(_))
        ));
        let _ = std::fs::remove_dir_all(path);
    }
}
