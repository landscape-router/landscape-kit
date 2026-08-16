use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use chrono::{DateTime, Utc};
use lkit_repository::parse_stable_version;
use serde::{Deserialize, Serialize};

use super::layout;
use super::plan::InstallError;
use super::root::InstallRoot;

/// 安装状态中序列化的服务管理器后端标识。
pub(crate) use crate::service::manager::ServiceManagerKind as StateServiceManager;

pub(crate) const STATE_SCHEMA_VERSION: u64 = 1;
/// 双地盘布局:状态位于 lkit 地盘,`install_root` 记录 landscape 安装根。
pub(crate) const STATE_LAYOUT_VERSION: u64 = 2;

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
    let path = layout::territory_state_path();
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

/// 只读 lkit 地盘的状态文件并校验,不做 canonical/current 检查
/// (install 单实例守卫用)。
pub(crate) fn read_state() -> Result<Option<InstallState>, InstallError> {
    let path = layout::territory_state_path();
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
    Ok(Some(state))
}

/// 从 lkit 地盘的状态文件发现 landscape 安装根。状态缺失返回 `Ok(None)`。
pub(crate) fn discover_landscape_root() -> Result<Option<InstallRoot>, InstallError> {
    let Some(state) = read_state()? else {
        return Ok(None);
    };
    let root = super::root::normalize_install_root(Path::new(&state.canonical_install_root))?;
    Ok(Some(root))
}

/// 状态缺失时,从未完成事务记录的根发现 landscape 安装根。网络接管待确认阶段
/// 尚未提交状态,`network status/confirm/rollback` 必须从事务发现根;daemon 的
/// 周期恢复同样依赖此回退。没有未完成事务时返回 `Ok(None)`。
pub(crate) fn discover_landscape_root_from_unfinished_transaction()
-> Result<Option<InstallRoot>, InstallError> {
    let dir = super::layout::territory_transactions_dir();
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(InstallError::Io(error)),
    };
    for entry in entries {
        let entry = entry.map_err(InstallError::Io)?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(content) = std::fs::read(&path) else {
            continue;
        };
        let Ok(transaction) =
            serde_json::from_slice::<super::transaction::TransactionFile>(&content)
        else {
            continue;
        };
        if transaction.phase.is_terminal() {
            continue;
        }
        let root =
            super::root::normalize_install_root(Path::new(&transaction.canonical_install_root))?;
        return Ok(Some(root));
    }
    Ok(None)
}

pub(crate) fn validate_state(state: &InstallState) -> Result<(), InstallError> {
    if state.schema_version != STATE_SCHEMA_VERSION {
        return Err(corrupted(format!(
            "unsupported schema version {}",
            state.schema_version
        )));
    }
    // layout_version 1 是旧单根布局,字段结构相同,读兼容不迁移。
    if state.layout_version != STATE_LAYOUT_VERSION && state.layout_version != 1 {
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
    // lkit 明确依赖发行版自启服务;状态必须记录受管服务定义。
    if !StateServiceManager::supported().contains(&state.service.manager) {
        return Err(corrupted(format!(
            "unsupported service manager {:?}",
            state.service.manager
        )));
    }
    if state.service.definition_path.is_none() || state.service.definition_sha256.is_none() {
        return Err(corrupted(
            "service state must record the definition path and sha256".into(),
        ));
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

pub(crate) fn write_state(_root: &InstallRoot, state: &InstallState) -> Result<(), InstallError> {
    let path = layout::territory_state_path();
    let state_dir = path.parent().ok_or_else(|| {
        InstallError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("state path {} has no parent directory", path.display()),
        ))
    })?;
    std::fs::create_dir_all(state_dir).map_err(InstallError::Io)?;
    let bytes = serde_json::to_vec_pretty(state).map_err(InstallError::StateWrite)?;
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
    use crate::deployment::layout;

    /// 建立隔离测试现场:lkit 地盘与 landscape 根位于同一临时目录树下,
    /// 地盘由 `test_territory` 指向,返回 (守卫, 地盘, landscape 根)。
    fn setup(
        name: &str,
    ) -> (
        layout::TerritoryOverride,
        std::path::PathBuf,
        std::path::PathBuf,
    ) {
        let temp =
            std::env::temp_dir().join(format!("lkit-state-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();
        let territory = temp.join("territory");
        std::fs::create_dir_all(&territory).unwrap();
        let guard = layout::test_territory(&territory);
        let root = temp.join("landscape");
        std::fs::create_dir_all(&root).unwrap();
        (guard, territory, root)
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
        let (_guard, territory, root) = setup("round-trip");
        let root = new_root(&root);
        activate(&root, "0.19.2");
        let state = valid_state(&root);
        write_state(&root, &state).unwrap();
        assert!(territory.join("state/install-state.json").is_file());
        assert_eq!(load_state(&root).unwrap().unwrap(), state);
        assert_eq!(read_state().unwrap().unwrap(), state);
        let discovered = discover_landscape_root().unwrap().unwrap();
        assert_eq!(discovered, root);
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    #[test]
    fn missing_state_returns_none() {
        let (_guard, territory, root) = setup("missing");
        let root = new_root(&root);
        assert!(load_state(&root).unwrap().is_none());
        assert!(read_state().unwrap().is_none());
        assert!(discover_landscape_root().unwrap().is_none());
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    #[test]
    fn discovers_the_recorded_landscape_root() {
        let (_guard, territory, root) = setup("discover");
        let root = new_root(&root);
        activate(&root, "0.19.2");
        let state = valid_state(&root);
        write_state(&root, &state).unwrap();
        let discovered = discover_landscape_root().unwrap().unwrap();
        assert_eq!(discovered.canonical, root.canonical);
        assert_eq!(discovered.install_root, root.canonical);
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
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
    fn accepts_layout_version_1_for_read_compatibility() {
        let root = InstallRoot {
            install_root: "/x".into(),
            canonical: "/x".into(),
        };
        let mut state = valid_state(&root);
        state.layout_version = 1;
        assert!(validate_state(&state).is_ok());
        state.layout_version = 3;
        assert!(matches!(
            validate_state(&state),
            Err(InstallError::CorruptedState(_))
        ));
    }

    #[test]
    fn rejects_invalid_json_and_schema_version() {
        let (_guard, territory, root) = setup("corrupt");
        let root = new_root(&root);
        std::fs::create_dir_all(territory.join("state")).unwrap();
        std::fs::write(territory.join("state/install-state.json"), b"not json").unwrap();
        assert!(matches!(
            load_state(&root),
            Err(InstallError::CorruptedState(_))
        ));
        assert!(matches!(read_state(), Err(InstallError::CorruptedState(_))));
        let mut state = valid_state(&root);
        state.schema_version = 2;
        assert!(matches!(
            validate_state(&state),
            Err(InstallError::CorruptedState(_))
        ));
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
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
        state.service.definition_path = None;
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
        let (_guard, territory, root) = setup("canonical");
        let root = new_root(&root);
        activate(&root, "0.19.2");
        let mut state = valid_state(&root);
        state.canonical_install_root = "/other/root".into();
        write_state(&root, &state).unwrap();
        assert!(matches!(
            load_state(&root),
            Err(InstallError::CorruptedState(_))
        ));
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    #[test]
    fn rejects_missing_current() {
        let (_guard, territory, root) = setup("no-current");
        let root = new_root(&root);
        let state = valid_state(&root);
        write_state(&root, &state).unwrap();
        assert!(matches!(
            load_state(&root),
            Err(InstallError::CorruptedState(_))
        ));
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    #[test]
    fn rejects_current_outside_the_root() {
        let (_guard, territory, root_path) = setup("outside-current");
        let root = new_root(&root_path);
        let outside = territory.parent().unwrap().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root_path.join("current")).unwrap();
        let state = valid_state(&root);
        write_state(&root, &state).unwrap();
        assert!(matches!(
            load_state(&root),
            Err(InstallError::CorruptedState(_))
        ));
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    #[test]
    fn rejects_current_targeting_a_different_release() {
        let (_guard, territory, root) = setup("drift");
        let root = new_root(&root);
        activate(&root, "0.20.0");
        let state = valid_state(&root);
        write_state(&root, &state).unwrap();
        assert!(matches!(
            load_state(&root),
            Err(InstallError::ActivationDrift(_))
        ));
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    #[test]
    fn accepts_releases_dir_symlink_inside_the_root() {
        let (_guard, territory, root) = setup("releases-symlink");
        std::fs::create_dir_all(root.join("real-releases/0.19.2")).unwrap();
        std::os::unix::fs::symlink("real-releases", root.join("releases")).unwrap();
        let root = new_root(&root);
        let current = root.canonical.join("current");
        std::os::unix::fs::symlink("releases/0.19.2", &current).unwrap();
        let state = valid_state(&root);
        write_state(&root, &state).unwrap();
        assert_eq!(load_state(&root).unwrap().unwrap(), state);
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }

    #[test]
    fn ignores_unknown_fields() {
        let (_guard, territory, root) = setup("unknown-fields");
        let root = new_root(&root);
        activate(&root, "0.19.2");
        let state = valid_state(&root);
        write_state(&root, &state).unwrap();
        let path = territory.join("state/install-state.json");
        let mut text = std::fs::read_to_string(&path).unwrap();
        text = text.replace(
            "\"last_transaction_id\"",
            "\"future_field\": {\"nested\": true}, \"last_transaction_id\"",
        );
        std::fs::write(&path, text).unwrap();
        assert!(load_state(&root).is_ok());
        let _ = std::fs::remove_dir_all(territory.parent().unwrap());
    }
}
