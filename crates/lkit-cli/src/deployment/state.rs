use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use chrono::{DateTime, Utc};
use lkit_repository::parse_stable_version;
use serde::{Deserialize, Serialize};

use super::plan::InstallError;
use super::root::InstallRoot;

/// 安装状态中序列化的服务管理器后端标识。
pub(crate) use crate::service::manager::ServiceManagerKind as StateServiceManager;

pub(crate) const INSTALL_STATE_RELATIVE: &str = "state/install-state.json";
pub(crate) const STATE_SCHEMA_VERSION: u64 = 1;
pub(crate) const STATE_LAYOUT_VERSION: u64 = 1;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct InstallState {
    pub schema_version: u64,
    pub layout_version: u64,
    pub install_root: String,
    pub canonical_install_root: String,
    pub active_version: String,
    pub assets: Assets,
    pub initialization: InitializationState,
    pub service: ServiceState,
    pub last_transaction_id: Option<String>,
    pub committed_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct Assets {
    pub webserver: WebserverAsset,
    pub static_archive: ArchiveAsset,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct WebserverAsset {
    pub architecture: StateArchitecture,
    pub sha256: String,
    pub size: u64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct ArchiveAsset {
    pub sha256: String,
    pub size: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum StateArchitecture {
    X86_64,
    Aarch64,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct InitializationState {
    pub status: InitStatus,
    pub lock_present: bool,
    pub initialized_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum InitStatus {
    Pending,
    Complete,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct ServiceState {
    pub manager: StateServiceManager,
    pub registered: bool,
    pub enabled: bool,
    pub verified: bool,
    pub definition_path: Option<String>,
    pub definition_sha256: Option<String>,
}

pub(crate) fn load_state(root: &InstallRoot) -> Result<Option<InstallState>, InstallError> {
    let path = root.canonical.join(INSTALL_STATE_RELATIVE);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(InstallError::Io(std::io::Error::new(
                error.kind(),
                format!("failed to read {}: {error}", path.display()),
            )));
        }
    };
    let state: InstallState = serde_json::from_slice(&bytes).map_err(|error| {
        InstallError::CorruptedState(format!(
            "{} is not a valid install state: {error}",
            path.display()
        ))
    })?;
    validate_state(&state)?;
    validate_canonical_root(&state, &root.canonical)?;
    check_current(&root.canonical, &state.active_version)?;
    Ok(Some(state))
}

pub(crate) fn validate_state(state: &InstallState) -> Result<(), InstallError> {
    if state.schema_version != STATE_SCHEMA_VERSION {
        return Err(corrupted(format!(
            "unsupported schema version {}",
            state.schema_version
        )));
    }
    if state.layout_version != STATE_LAYOUT_VERSION {
        return Err(corrupted(format!(
            "unsupported layout version {}",
            state.layout_version
        )));
    }
    if let Err(error) = parse_stable_version(&state.active_version) {
        return Err(corrupted(format!(
            "invalid active version {:?}: {error}",
            state.active_version
        )));
    }
    if !is_sha256(&state.assets.webserver.sha256) {
        return Err(corrupted(
            "webserver asset sha256 must be 64 lowercase hex characters".into(),
        ));
    }
    if state.assets.webserver.size == 0 {
        return Err(corrupted(
            "webserver asset size must be greater than 0".into(),
        ));
    }
    if !is_sha256(&state.assets.static_archive.sha256) {
        return Err(corrupted(
            "static archive sha256 must be 64 lowercase hex characters".into(),
        ));
    }
    if state.assets.static_archive.size == 0 {
        return Err(corrupted(
            "static archive size must be greater than 0".into(),
        ));
    }
    if let Some(definition_sha256) = &state.service.definition_sha256
        && !is_sha256(definition_sha256)
    {
        return Err(corrupted(
            "service definition_sha256 must be 64 lowercase hex characters".into(),
        ));
    }
    match state.initialization.status {
        InitStatus::Pending => {
            if state.initialization.lock_present || state.initialization.initialized_at.is_some() {
                return Err(corrupted(
                    "pending initialization must not observe an init lock or record an initialized_at"
                        .into(),
                ));
            }
        }
        InitStatus::Complete => {
            if !state.initialization.lock_present || state.initialization.initialized_at.is_none() {
                return Err(corrupted(
                    "complete initialization must observe the init lock and record initialized_at"
                        .into(),
                ));
            }
        }
    }
    match state.service.manager {
        StateServiceManager::Systemd => {
            if state.service.definition_path.is_none() || state.service.definition_sha256.is_none()
            {
                return Err(corrupted(
                    "systemd service state must record the definition path and sha256".into(),
                ));
            }
        }
        StateServiceManager::None => {
            if state.service.registered
                || state.service.enabled
                || state.service.verified
                || state.service.definition_path.is_some()
                || state.service.definition_sha256.is_some()
            {
                return Err(corrupted(
                    "service manager none requires registered, enabled, verified, and definitions to be false or null"
                        .into(),
                ));
            }
        }
    }
    Ok(())
}

fn validate_canonical_root(state: &InstallState, canonical: &Path) -> Result<(), InstallError> {
    if Path::new(&state.canonical_install_root) != canonical {
        return Err(corrupted(format!(
            "canonical_install_root {} does not match the real install root {}",
            state.canonical_install_root,
            canonical.display()
        )));
    }
    Ok(())
}

fn check_current(canonical: &Path, active_version: &str) -> Result<(), InstallError> {
    let current = canonical.join("current");
    let metadata = match std::fs::symlink_metadata(&current) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(corrupted("current is missing".into()));
        }
        Err(error) => return Err(InstallError::Io(error)),
    };
    if !metadata.file_type().is_symlink() {
        return Err(corrupted("current is not a symbolic link".into()));
    }
    let target = std::fs::canonicalize(&current)
        .map_err(|_| corrupted("current is a broken symbolic link".into()))?;
    let releases = std::fs::canonicalize(canonical.join("releases"))
        .map_err(|_| corrupted("releases is missing or is a broken symbolic link".into()))?;
    if !target.starts_with(&releases) {
        if target.starts_with(canonical) {
            return Err(InstallError::ActivationDrift(format!(
                "current points to {} instead of a release",
                target.display()
            )));
        }
        return Err(corrupted("current points outside the install root".into()));
    }
    let expected = releases.join(active_version);
    if target != expected {
        return Err(InstallError::ActivationDrift(format!(
            "current points to {} but the active version is {}",
            target.display(),
            active_version
        )));
    }
    Ok(())
}

pub(crate) fn write_state(root: &InstallRoot, state: &InstallState) -> Result<(), InstallError> {
    let state_dir = root.canonical.join("state");
    std::fs::create_dir_all(&state_dir).map_err(InstallError::Io)?;
    let bytes = serde_json::to_vec_pretty(state).map_err(InstallError::StateWrite)?;
    let path = state_dir.join("install-state.json");
    let tmp = state_dir.join(format!(".install-state.{}.tmp", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(InstallError::Io)?;
    file.write_all(&bytes).map_err(InstallError::Io)?;
    file.sync_all().map_err(InstallError::Io)?;
    std::fs::rename(&tmp, &path).map_err(InstallError::Io)?;
    Ok(())
}

fn corrupted(reason: String) -> InstallError {
    InstallError::CorruptedState(reason)
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("lkit-state-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn new_root(path: &std::path::Path) -> InstallRoot {
        InstallRoot {
            install_root: path.to_path_buf(),
            canonical: std::fs::canonicalize(path).unwrap(),
        }
    }

    fn valid_state(root: &InstallRoot) -> InstallState {
        InstallState {
            schema_version: STATE_SCHEMA_VERSION,
            layout_version: STATE_LAYOUT_VERSION,
            install_root: root.install_root.display().to_string(),
            canonical_install_root: root.canonical.display().to_string(),
            active_version: "0.19.2".into(),
            assets: Assets {
                webserver: WebserverAsset {
                    architecture: StateArchitecture::X86_64,
                    sha256: "a".repeat(64),
                    size: 10,
                },
                static_archive: ArchiveAsset {
                    sha256: "b".repeat(64),
                    size: 20,
                },
            },
            initialization: InitializationState {
                status: InitStatus::Complete,
                lock_present: true,
                initialized_at: Some(Utc::now()),
            },
            service: ServiceState {
                manager: StateServiceManager::Systemd,
                registered: true,
                enabled: true,
                verified: true,
                definition_path: Some("service/landscape-router.service".into()),
                definition_sha256: Some("d".repeat(64)),
            },
            last_transaction_id: Some("0198c3d2-0000-7000-8000-000000000001".into()),
            committed_at: Some(Utc::now()),
        }
    }

    fn activate(root: &InstallRoot, active_version: &str) {
        let release = root.canonical.join("releases").join(active_version);
        std::fs::create_dir_all(&release).unwrap();
        let current = root.canonical.join("current");
        let _ = std::fs::remove_file(&current);
        std::os::unix::fs::symlink(&release, current).unwrap();
    }

    #[test]
    fn round_trips_through_disk() {
        let temp = temp_root("round-trip");
        let root = new_root(&temp);
        activate(&root, "0.19.2");
        let state = valid_state(&root);
        write_state(&root, &state).unwrap();
        assert_eq!(load_state(&root).unwrap().unwrap(), state);
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn missing_state_returns_none() {
        let temp = temp_root("missing");
        let root = new_root(&temp);
        assert!(load_state(&root).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn reads_legacy_state_with_initialization_checksum_but_does_not_write_it() {
        let root = InstallRoot {
            install_root: "/x".into(),
            canonical: "/x".into(),
        };
        let mut value = serde_json::to_value(valid_state(&root)).unwrap();
        value["initialization"]["config_sha256"] = serde_json::Value::String("c".repeat(64));

        let state: InstallState = serde_json::from_value(value).unwrap();
        validate_state(&state).unwrap();
        let serialized = serde_json::to_value(state).unwrap();

        assert!(serialized["initialization"].get("config_sha256").is_none());
    }

    #[test]
    fn reads_legacy_state_with_repository_field_but_ignores_it() {
        let root = InstallRoot {
            install_root: "/x".into(),
            canonical: "/x".into(),
        };
        let mut value = serde_json::to_value(valid_state(&root)).unwrap();
        value["repository"] = serde_json::json!({
            "kind": "http",
            "location": "https://repo.example.com/landscape/",
        });

        let state: InstallState = serde_json::from_value(value).unwrap();
        validate_state(&state).unwrap();
        let serialized = serde_json::to_value(state).unwrap();

        assert!(serialized.get("repository").is_none());
    }

    #[test]
    fn rejects_invalid_json_and_schema_version() {
        let temp = temp_root("corrupt");
        let root = new_root(&temp);
        std::fs::create_dir_all(temp.join("state")).unwrap();
        std::fs::write(temp.join("state/install-state.json"), b"not json").unwrap();
        assert!(matches!(
            load_state(&root),
            Err(InstallError::CorruptedState(_))
        ));
        let mut state = valid_state(&root);
        state.schema_version = 2;
        assert!(matches!(
            validate_state(&state),
            Err(InstallError::CorruptedState(_))
        ));
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn rejects_invalid_field_values() {
        let mut state = valid_state(&InstallRoot {
            install_root: "/x".into(),
            canonical: "/x".into(),
        });
        state.active_version = "0.20.0-rc.1".into();
        assert!(validate_state(&state).is_err());
        let mut state = valid_state(&InstallRoot {
            install_root: "/x".into(),
            canonical: "/x".into(),
        });
        state.assets.webserver.sha256 = "ABCD".into();
        assert!(validate_state(&state).is_err());
        let mut state = valid_state(&InstallRoot {
            install_root: "/x".into(),
            canonical: "/x".into(),
        });
        state.assets.static_archive.size = 0;
        assert!(validate_state(&state).is_err());
    }

    #[test]
    fn rejects_invalid_initialization_combinations() {
        let mut state = valid_state(&InstallRoot {
            install_root: "/x".into(),
            canonical: "/x".into(),
        });
        state.initialization.status = InitStatus::Pending;
        assert!(validate_state(&state).is_err());
        let mut state = valid_state(&InstallRoot {
            install_root: "/x".into(),
            canonical: "/x".into(),
        });
        state.initialization.initialized_at = None;
        assert!(validate_state(&state).is_err());
    }

    #[test]
    fn rejects_invalid_service_combinations() {
        let mut state = valid_state(&InstallRoot {
            install_root: "/x".into(),
            canonical: "/x".into(),
        });
        state.service.manager = StateServiceManager::None;
        assert!(validate_state(&state).is_err());
        let mut state = valid_state(&InstallRoot {
            install_root: "/x".into(),
            canonical: "/x".into(),
        });
        state.service.definition_path = None;
        assert!(validate_state(&state).is_err());
    }

    #[test]
    fn rejects_canonical_root_mismatch() {
        let temp = temp_root("canonical");
        let root = new_root(&temp);
        activate(&root, "0.19.2");
        let mut state = valid_state(&root);
        state.canonical_install_root = "/other/root".into();
        write_state(&root, &state).unwrap();
        assert!(matches!(
            load_state(&root),
            Err(InstallError::CorruptedState(_))
        ));
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn rejects_missing_current() {
        let temp = temp_root("no-current");
        let root = new_root(&temp);
        let state = valid_state(&root);
        write_state(&root, &state).unwrap();
        assert!(matches!(
            load_state(&root),
            Err(InstallError::CorruptedState(_))
        ));
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn rejects_current_outside_the_root() {
        let temp = temp_root("outside-current");
        let root_path = temp.join("root");
        std::fs::create_dir_all(&root_path).unwrap();
        let root = new_root(&root_path);
        let outside = temp.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root_path.join("current")).unwrap();
        let state = valid_state(&root);
        write_state(&root, &state).unwrap();
        assert!(matches!(
            load_state(&root),
            Err(InstallError::CorruptedState(_))
        ));
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn rejects_current_targeting_a_different_release() {
        let temp = temp_root("drift");
        let root = new_root(&temp);
        activate(&root, "0.20.0");
        let state = valid_state(&root);
        write_state(&root, &state).unwrap();
        assert!(matches!(
            load_state(&root),
            Err(InstallError::ActivationDrift(_))
        ));
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn accepts_releases_dir_symlink_inside_the_root() {
        let temp = temp_root("releases-symlink");
        std::fs::create_dir_all(temp.join("real-releases/0.19.2")).unwrap();
        std::os::unix::fs::symlink("real-releases", temp.join("releases")).unwrap();
        let root = new_root(&temp);
        let current = temp.join("current");
        std::os::unix::fs::symlink("releases/0.19.2", &current).unwrap();
        let state = valid_state(&root);
        write_state(&root, &state).unwrap();
        assert_eq!(load_state(&root).unwrap().unwrap(), state);
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn ignores_unknown_fields() {
        let temp = temp_root("unknown-fields");
        let root = new_root(&temp);
        activate(&root, "0.19.2");
        let state = valid_state(&root);
        write_state(&root, &state).unwrap();
        let path = temp.join("state/install-state.json");
        let mut text = std::fs::read_to_string(&path).unwrap();
        text = text.replace(
            "\"last_transaction_id\"",
            "\"future_field\": {\"nested\": true}, \"last_transaction_id\"",
        );
        std::fs::write(&path, text).unwrap();
        assert!(load_state(&root).is_ok());
        let _ = std::fs::remove_dir_all(&temp);
    }
}
