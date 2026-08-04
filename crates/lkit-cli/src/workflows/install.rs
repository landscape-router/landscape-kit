use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

use chrono::Utc;
use serde::Serialize;

use super::artifacts::build_release;
#[cfg(test)]
use super::artifacts::hex;
pub(crate) use super::artifacts::{
    BuiltRelease, STATIC_DIR, WEBSERVER_BINARY, fetch_static_asset, fetch_webserver_asset,
    hash_file, hash_str,
};
use super::credentials::Credentials;
use super::health::{self, DocsProbe, HealthOptions};
use super::plan::{InstallError, TargetVersion};
pub(crate) use super::preflight::run_preflight;
use super::repository::{Architecture, ProviderKind, Release, ReleaseProvider};
use super::resolv;
use super::root::InstallRoot;
use super::state::{
    ArchiveAsset, Assets, InitStatus, InitializationState, InstallState, RepositorySource,
    STATE_LAYOUT_VERSION, STATE_SCHEMA_VERSION, ServiceState, StateArchitecture,
    StateRepositoryKind, StateServiceManager, WebserverAsset,
};
pub(crate) use super::switch::{SwitchArgs, SwitchOptions, SwitchOutcome, switch_version};
use super::systemd::{self, Availability, Systemd};
use super::transaction::{Phase, Registration, RegistrationKind, SystemdBefore};
use crate::deployment::runtime::InstallRuntime;

/// 服务管理模式选择。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ManagerChoice {
    /// 未指定:systemd 可用则使用,明确不是 systemd init 时使用 none,
    /// 看似 systemd 但环境损坏时失败。
    Auto,
    /// 显式要求 systemd,不可用或环境损坏时失败。
    Systemd,
    /// 显式要求无 systemd,只管理文件和事务。
    None,
}

/// 实际选择的服务管理模式。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ServiceManager {
    Systemd,
    None,
}

pub(crate) struct FirstInstallOutcome {
    pub release: Release,
    pub manager: ServiceManager,
    pub pending_network_address: Option<std::net::Ipv4Addr>,
}

pub(crate) async fn first_install<P: DocsProbe>(
    root: &InstallRoot,
    provider: &ReleaseProvider,
    target: &TargetVersion,
    credentials: &Credentials,
    manager_choice: ManagerChoice,
    systemd: &Systemd,
    health_options: &HealthOptions<P>,
) -> Result<FirstInstallOutcome, InstallError> {
    first_install_impl(
        root,
        provider,
        target,
        credentials,
        manager_choice,
        systemd,
        health_options,
        None,
        None,
    )
    .await
}

pub(crate) async fn first_install_with_network<P: DocsProbe>(
    root: &InstallRoot,
    provider: &ReleaseProvider,
    target: &TargetVersion,
    credentials: &Credentials,
    manager_choice: ManagerChoice,
    systemd: &Systemd,
    health_options: &HealthOptions<P>,
    network: &crate::network::config::NetworkPlan,
    runtime: &InstallRuntime,
) -> Result<FirstInstallOutcome, InstallError> {
    first_install_impl(
        root,
        provider,
        target,
        credentials,
        manager_choice,
        systemd,
        health_options,
        Some(network),
        Some(runtime),
    )
    .await
}

async fn first_install_impl<P: DocsProbe>(
    root: &InstallRoot,
    provider: &ReleaseProvider,
    target: &TargetVersion,
    credentials: &Credentials,
    manager_choice: ManagerChoice,
    systemd: &Systemd,
    health_options: &HealthOptions<P>,
    network: Option<&crate::network::config::NetworkPlan>,
    runtime: Option<&InstallRuntime>,
) -> Result<FirstInstallOutcome, InstallError> {
    let architecture = Architecture::host().ok_or_else(|| {
        InstallError::UnsupportedPlatform("only x86_64 and aarch64 are supported".into())
    })?;
    let release = match target {
        TargetVersion::Latest => provider
            .latest(architecture)
            .await?
            .ok_or(InstallError::NoStableVersion)?,
        TargetVersion::Version(version) => provider.release(version, architecture).await?,
    };
    let manager = select_manager(manager_choice, systemd)?;
    if network.is_some() && manager != ServiceManager::Systemd {
        return Err(InstallError::ParameterUsage(
            "network takeover requires the systemd service manager".into(),
        ));
    }
    let mut transaction = super::transaction::TransactionFile::new_install(root, &release.version)?;
    if let Some(network) = network {
        let runtime = runtime.ok_or_else(|| {
            InstallError::CorruptedTransaction("network takeover runtime is missing".into())
        })?;
        transaction.network_takeover = Some(crate::network::takeover::prepare_transaction(
            &transaction.transaction_id,
            network,
            runtime,
        )?);
    }
    super::transaction::begin(root, &transaction)?;
    let result: Result<(Release, bool), InstallError> = async {
        let built = build_release(root, &release).await?;
        let init_config = build_init_config(&release.version, credentials, network)?;
        write_init_config(root, &init_config)?;
        let activation = if manager == ServiceManager::Systemd {
            let before = capture_systemd_before(systemd)?;
            let backup_dir = root
                .canonical
                .join("backups")
                .join(&transaction.transaction_id)
                .join("host/resolv.conf");
            let _ = resolv::backup(&systemd.resolv_conf, &backup_dir)?;
            let unit_sha = write_unit_origin(root, &systemd::render_unit(&root.canonical))?;
            transaction.systemd_before = Some(before);
            transaction.resolv_conf_backup = Some(format!(
                "backups/{}/host/resolv.conf",
                transaction.transaction_id
            ));
            if let Some(takeover) = transaction.network_takeover.as_mut() {
                let runtime = runtime.ok_or_else(|| {
                    InstallError::CorruptedTransaction(
                        "network takeover runtime disappeared".into(),
                    )
                })?;
                crate::network::takeover::refresh_confirmation_deadline(takeover, runtime)?;
            }
            super::transaction::persist(root, &transaction)?;
            super::transaction::mark_phase(root, &transaction, Phase::Prepared)?;
            if let Some(takeover) = transaction.network_takeover.as_ref() {
                let runtime = runtime.ok_or_else(|| {
                    InstallError::CorruptedTransaction(
                        "network takeover runtime disappeared".into(),
                    )
                })?;
                crate::network::takeover::arm_recovery(root, takeover, runtime)?;
                super::transaction::mark_phase(root, &transaction, Phase::Stopping)?;
                crate::network::takeover::stop_host_services(takeover, systemd)?;
            }
            UnitActivation {
                unit_sha: unit_sha.clone(),
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
                    definition_sha256: Some(unit_sha.clone()),
                },
            }
        } else {
            super::transaction::mark_phase(root, &transaction, Phase::Prepared)?;
            UnitActivation {
                unit_sha: String::new(),
                initialization: InitializationState {
                    status: InitStatus::Pending,
                    lock_present: false,
                    initialized_at: None,
                },
                service: ServiceState {
                    manager: StateServiceManager::None,
                    registered: false,
                    enabled: false,
                    verified: false,
                    definition_path: None,
                    definition_sha256: None,
                },
            }
        };
        super::transaction::mark_phase(root, &transaction, Phase::Activating)?;
        activate_current(root, &release.version)?;
        if manager == ServiceManager::Systemd {
            systemd::register(
                systemd,
                &root.canonical.join("service/landscape-router.service"),
            )?;
            systemd::enable(systemd)?;
            systemd::start(systemd)?;
            super::transaction::mark_phase(root, &transaction, Phase::Verifying)?;
            let pid = systemd::main_pid(systemd)?;
            if pid == 0 {
                return Err(InstallError::Systemd(
                    "service did not produce a main pid after start".into(),
                ));
            }
            let options = health::StartupOptions {
                ports: &health_options.ports,
                expected_pid: pid,
                docs: &health_options.docs,
                unit_state: Some(&(|| systemd::active_state(systemd).ok())),
                init_required: true,
                data_dir: &root.canonical.join("data"),
                startup_timeout: health_options.startup_timeout,
                stable_duration: health_options.stable_duration,
            };
            health::wait_for_startup(&options).await?;
            health::observe_stable(&options).await?;
        }
        let state = build_state(root, provider, &release, architecture, &built, &activation);
        if let Some(takeover) = transaction.network_takeover.as_ref() {
            crate::network::takeover::write_pending_state(root, takeover, &state)?;
            super::transaction::mark_phase(root, &transaction, Phase::AwaitingNetworkConfirmation)?;
            Ok((release, true))
        } else {
            super::state::write_state(root, &state)?;
            super::transaction::mark_phase(root, &transaction, Phase::Committed)?;
            Ok((release, false))
        }
    }
    .await;
    match result {
        Ok((release, pending_network)) => Ok(FirstInstallOutcome {
            release,
            manager,
            pending_network_address: pending_network.then(|| {
                network
                    .expect("pending network has a plan")
                    .management_address()
                    .address
            }),
        }),
        Err(error) => {
            if let Some(takeover) = transaction.network_takeover.as_ref() {
                let _ = super::transaction::mark_phase(root, &transaction, Phase::RollingBack);
                let cleanup =
                    super::transaction::cleanup_failed_first_install(root, &transaction, systemd)
                        .and_then(|()| {
                            crate::network::takeover::cleanup_failed_takeover(
                                root, takeover, systemd,
                            )
                        });
                if let Err(cleanup_error) = cleanup {
                    eprintln!("install: network takeover cleanup failed: {cleanup_error}");
                    return Err(error);
                }
            } else if let Err(cleanup_error) =
                super::transaction::cleanup_failed_first_install(root, &transaction, systemd)
            {
                eprintln!("install: first install cleanup failed: {cleanup_error}");
            }
            let _ = super::transaction::mark_phase(root, &transaction, Phase::Failed);
            Err(error)
        }
    }
}

fn select_manager(
    choice: ManagerChoice,
    systemd: &Systemd,
) -> Result<ServiceManager, InstallError> {
    match choice {
        ManagerChoice::None => Ok(ServiceManager::None),
        ManagerChoice::Systemd => match systemd.probe() {
            Availability::Available { .. } => Ok(ServiceManager::Systemd),
            availability => Err(InstallError::Systemd(format!(
                "--service-manager systemd requested but systemd is not available: {availability:?}"
            ))),
        },
        ManagerChoice::Auto => match systemd.probe() {
            Availability::Available { .. } => Ok(ServiceManager::Systemd),
            Availability::NotSystemdInit => Ok(ServiceManager::None),
            availability => Err(InstallError::Systemd(format!(
                "the host appears to run systemd but it is damaged: {availability:?}"
            ))),
        },
    }
}

pub(crate) fn capture_systemd_before(systemd: &Systemd) -> Result<SystemdBefore, InstallError> {
    let (kind, target) = match systemd::query_registration(systemd)? {
        systemd::Registration::Missing => (RegistrationKind::Missing, None),
        systemd::Registration::Symlink { target } => (
            RegistrationKind::Symlink,
            Some(target.display().to_string()),
        ),
        systemd::Registration::Conflict { file_type } => {
            return Err(InstallError::Systemd(format!(
                "cannot take over {}: {file_type} ownership conflict",
                systemd::UNIT_NAME
            )));
        }
    };
    Ok(SystemdBefore {
        registration: Registration { kind, target },
        enabled: systemd::is_enabled(systemd)?,
        active: systemd::is_active(systemd)?,
    })
}

/// 提交到状态中的初始化与服务信息。
pub(crate) struct UnitActivation {
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
    provider: &ReleaseProvider,
    release: &Release,
    built: &BuiltRelease,
    previous: &InstallState,
    transaction_id: &str,
    unit_sha: Option<String>,
) -> InstallState {
    let architecture = architecture_from_state(previous);
    let repository_kind = match provider.kind() {
        ProviderKind::Github => StateRepositoryKind::Github,
        ProviderKind::Http => StateRepositoryKind::Http,
    };
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
        repository: RepositorySource {
            kind: repository_kind,
            location: provider.location().to_string(),
        },
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

pub(crate) fn parse_stable_version(
    value: &str,
) -> Result<semver::Version, lkit_repository::ProtocolError> {
    lkit_repository::parse_stable_version(value)
}

pub(crate) fn activate_current(
    root: &InstallRoot,
    version: &semver::Version,
) -> Result<(), InstallError> {
    let current = root.canonical.join("current");
    let tmp_link = root.canonical.join("run").join(".current.tmp");
    std::fs::create_dir_all(tmp_link.parent().expect("run dir has a parent"))
        .map_err(InstallError::Io)?;
    let _ = std::fs::remove_file(&tmp_link);
    std::os::unix::fs::symlink(format!("releases/{version}"), &tmp_link)
        .map_err(InstallError::Io)?;
    std::fs::rename(&tmp_link, &current).map_err(InstallError::Io)?;
    Ok(())
}

#[derive(Serialize)]
struct InitConfigFile<'a> {
    version: &'a str,
    config: InitAuth<'a>,
}

#[derive(Serialize)]
struct InitAuth<'a> {
    auth: AdminAuth<'a>,
}

#[derive(Serialize)]
struct AdminAuth<'a> {
    admin_user: &'a str,
    admin_pass: &'a str,
}

fn build_init_config(
    version: &semver::Version,
    credentials: &Credentials,
    network: Option<&crate::network::config::NetworkPlan>,
) -> Result<String, InstallError> {
    if let Some(network) = network {
        let config = crate::network::config::LandscapeInit::new(
            version,
            &credentials.admin_user,
            &credentials.password,
            network,
        )?;
        return toml::to_string(&config).map_err(|error| {
            InstallError::ParameterUsage(format!(
                "failed to serialize Landscape network init config: {error}"
            ))
        });
    }
    let config = InitConfigFile {
        version: &version.to_string(),
        config: InitAuth {
            auth: AdminAuth {
                admin_user: &credentials.admin_user,
                admin_pass: &credentials.password,
            },
        },
    };
    toml::to_string(&config).map_err(|error| {
        InstallError::InvalidPassword(format!("failed to serialize init config: {error}"))
    })
}

fn write_init_config(root: &InstallRoot, content: &str) -> Result<(), InstallError> {
    let data_dir = root.canonical.join("data");
    std::fs::create_dir_all(&data_dir).map_err(InstallError::Io)?;
    let path = data_dir.join("landscape_init.toml");
    let tmp = data_dir.join(".landscape_init.toml.tmp");
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(InstallError::Io)?;
    file.write_all(content.as_bytes())
        .map_err(InstallError::Io)?;
    file.sync_all().map_err(InstallError::Io)?;
    std::fs::rename(&tmp, &path).map_err(InstallError::Io)?;
    Ok(())
}

fn build_state(
    root: &InstallRoot,
    provider: &ReleaseProvider,
    release: &Release,
    architecture: Architecture,
    built: &BuiltRelease,
    activation: &UnitActivation,
) -> InstallState {
    let architecture = match architecture {
        Architecture::X86_64 => StateArchitecture::X86_64,
        Architecture::Aarch64 => StateArchitecture::Aarch64,
    };
    let repository_kind = match provider.kind() {
        ProviderKind::Github => StateRepositoryKind::Github,
        ProviderKind::Http => StateRepositoryKind::Http,
    };
    InstallState {
        schema_version: STATE_SCHEMA_VERSION,
        layout_version: STATE_LAYOUT_VERSION,
        install_root: root.install_root.display().to_string(),
        canonical_install_root: root.canonical.display().to_string(),
        active_version: release.version.to_string(),
        repository: RepositorySource {
            kind: repository_kind,
            location: provider.location().to_string(),
        },
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

/// 写入受管 unit 原件(0600,原子替换),返回其 SHA-256。
pub(crate) fn write_unit_origin(root: &InstallRoot, content: &str) -> Result<String, InstallError> {
    systemd::validate_unit(content, &root.canonical)?;
    let service_dir = root.canonical.join("service");
    std::fs::create_dir_all(&service_dir).map_err(InstallError::Io)?;
    let path = service_dir.join("landscape-router.service");
    let tmp = service_dir.join(".landscape-router.service.tmp");
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(InstallError::Io)?;
    file.write_all(content.as_bytes())
        .map_err(InstallError::Io)?;
    file.sync_all().map_err(InstallError::Io)?;
    std::fs::rename(&tmp, &path).map_err(InstallError::Io)?;
    Ok(hash_str(content))
}

pub(crate) fn reference_command(root: &InstallRoot) -> String {
    format!(
        "{} --config-dir {} --web {}",
        shell_escape(
            &root
                .canonical
                .join("current/landscape-webserver")
                .display()
                .to_string()
        ),
        shell_escape(&root.canonical.join("data").display().to_string()),
        shell_escape(&root.canonical.join("current/static").display().to_string()),
    )
}

fn shell_escape(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::net::UdpSocket;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    use super::super::health::PortCheck;

    use sha2::{Digest, Sha256};

    use super::super::repository::provider_for;
    use super::super::repository::test_server::{TestResponse, TestServer};
    use super::*;

    const WEBSERVER_PAYLOAD: &[u8] = b"landscape webserver payload";

    fn temp_root(name: &str) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("lkit-pipeline-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn sha256_bytes(bytes: &[u8]) -> (String, u64) {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        (hex(&hasher.finalize()), bytes.len() as u64)
    }

    fn build_static_zip() -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        writer.start_file("static/index.html", options).unwrap();
        writer.write_all(b"<h1>hello</h1>").unwrap();
        writer.start_file("static/app.js", options).unwrap();
        writer.write_all(b"console.log(1);").unwrap();
        writer.finish().unwrap().into_inner()
    }

    fn repository_files_for(version: &str, payload: &[u8]) -> HashMap<String, Vec<u8>> {
        let webserver_zst = zstd::encode_all(payload, 3).unwrap();
        let (webserver_sha, webserver_size) = sha256_bytes(&webserver_zst);
        let static_zip = build_static_zip();
        let (static_sha, static_size) = sha256_bytes(&static_zip);
        let manifest = serde_json::json!({
            "protocol_version": 1,
            "version": version,
            "assets": {
                "webserver": {
                    "x86_64": {
                        "url": "landscape-webserver-x86_64.zst",
                        "sha256": webserver_sha,
                        "size": webserver_size,
                    }
                },
                "static": {
                    "url": "static.zip",
                    "sha256": static_sha,
                    "size": static_size,
                }
            }
        })
        .to_string();
        HashMap::from([
            (
                "/repository.json".into(),
                br#"{"protocol_version":1}"#.to_vec(),
            ),
            (
                "/channels/stable.json".to_string(),
                format!(r#"{{"protocol_version":1,"version":"{version}"}}"#).into_bytes(),
            ),
            (
                format!("/releases/{version}/manifest.json"),
                manifest.into_bytes(),
            ),
            (
                format!("/releases/{version}/landscape-webserver-x86_64.zst"),
                webserver_zst,
            ),
            (format!("/releases/{version}/static.zip"), static_zip),
        ])
    }

    fn repository_files() -> (HashMap<String, Vec<u8>>, Vec<u8>) {
        (
            repository_files_for("1.2.3", WEBSERVER_PAYLOAD),
            WEBSERVER_PAYLOAD.to_vec(),
        )
    }

    fn start_repository(
        name: &str,
        files: HashMap<String, Vec<u8>>,
    ) -> (TestServer, InstallRoot, ReleaseProvider) {
        let server = TestServer::start(move |path| match files.get(path) {
            Some(body) => TestResponse::ok(body.clone()),
            None => TestResponse::status(404, "Not Found", Vec::new()),
        });
        let root = temp_root(name);
        let install_root = InstallRoot {
            install_root: root.clone(),
            canonical: root,
        };
        let provider = provider_for(ProviderKind::Http, &server.base).unwrap();
        (server, install_root, provider)
    }

    fn credentials() -> Credentials {
        Credentials {
            admin_user: "admin".into(),
            password: "Secret123".into(),
        }
    }

    struct FakeDocs;

    impl DocsProbe for FakeDocs {
        async fn docs_ok(&self) -> bool {
            true
        }
    }

    struct ToggleDocs {
        ok: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }

    impl DocsProbe for ToggleDocs {
        async fn docs_ok(&self) -> bool {
            self.ok.load(std::sync::atomic::Ordering::Relaxed)
        }
    }

    fn test_options() -> HealthOptions<FakeDocs> {
        HealthOptions {
            docs: FakeDocs,
            ports: Vec::new(),
            startup_timeout: Duration::from_secs(10),
            stable_duration: Duration::from_millis(100),
        }
    }

    fn none_options() -> HealthOptions<FakeDocs> {
        test_options()
    }

    fn version() -> semver::Version {
        semver::Version::new(1, 2, 3)
    }

    #[tokio::test]
    async fn performs_first_install_from_http_repository() {
        let (server, root, provider) = start_repository("e2e-explicit", repository_files().0);
        let outcome = first_install(
            &root,
            &provider,
            &TargetVersion::Version(version()),
            &credentials(),
            ManagerChoice::None,
            &Systemd::host(),
            &none_options(),
        )
        .await
        .unwrap();
        assert_eq!(outcome.release.version, version());

        let binary = root.canonical.join("releases/1.2.3/landscape-webserver");
        assert_eq!(std::fs::read(&binary).unwrap(), WEBSERVER_PAYLOAD);
        let mode = std::fs::metadata(&binary).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0o111);

        let index = root.canonical.join("releases/1.2.3/static/index.html");
        assert_eq!(std::fs::read_to_string(&index).unwrap(), "<h1>hello</h1>");

        assert_eq!(
            std::fs::read_link(root.canonical.join("current")).unwrap(),
            std::path::PathBuf::from("releases/1.2.3")
        );

        let init_config =
            std::fs::read_to_string(root.canonical.join("data/landscape_init.toml")).unwrap();
        assert!(init_config.contains("version = \"1.2.3\""));
        assert!(init_config.contains("admin_user = \"admin\""));
        assert!(init_config.contains("admin_pass = \"Secret123\""));

        let state = super::super::state::load_state(&root).unwrap().unwrap();
        assert_eq!(state.active_version, "1.2.3");
        assert_eq!(state.initialization.status, InitStatus::Pending);
        assert!(!state.initialization.lock_present);
        assert!(state.initialization.initialized_at.is_none());
        assert_eq!(state.service.manager, StateServiceManager::None);
        assert_eq!(
            state.assets.webserver.sha256,
            super::hex(&{
                let mut hasher = Sha256::new();
                hasher.update(WEBSERVER_PAYLOAD);
                hasher.finalize()
            })
        );

        assert!(
            server
                .request_paths()
                .contains(&"/releases/1.2.3/landscape-webserver-x86_64.zst".into())
        );
        assert!(
            server
                .request_paths()
                .contains(&"/releases/1.2.3/static.zip".into())
        );

        assert!(
            super::super::transaction::find_unfinished(&root)
                .unwrap()
                .is_none()
        );
        let tx_files: Vec<_> = std::fs::read_dir(root.canonical.join("transactions"))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert_eq!(tx_files.len(), 1);
        let _ = std::fs::remove_dir_all(&root.install_root);
    }

    fn fake_systemd(dir: &std::path::Path, main_pid: u32) -> Systemd {
        let script = dir.join("systemctl");
        std::fs::write(
            &script,
            format!(
                r#"#!/bin/sh
case "$*" in
  "show --property=MainPID --value landscape-router.service") echo {main_pid};;
  "show --property=ActiveState --value landscape-router.service") echo active;;
  "is-enabled landscape-router.service") echo enabled;;
  "is-active landscape-router.service") echo active;;
  *) exit 0;;
esac
"#
            ),
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

    #[tokio::test]
    async fn auto_selects_none_without_systemd() {
        let (files, _) = repository_files();
        let (_server, root, provider) = start_repository("e2e-auto-none", files);
        let dir = std::env::temp_dir().join(format!(
            "lkit-pipeline-test-auto-none-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let systemd = Systemd {
            systemctl: dir.join("systemctl"),
            system_unit_dir: dir.join("units"),
            run_systemd_dir: dir.join("run"),
            pid1_is_systemd: true,
            resolv_conf: dir.join("resolv.conf"),
        };
        let outcome = first_install(
            &root,
            &provider,
            &TargetVersion::Version(version()),
            &credentials(),
            ManagerChoice::Auto,
            &systemd,
            &none_options(),
        )
        .await
        .unwrap();
        assert_eq!(outcome.manager, ServiceManager::None);
        let state = super::super::state::load_state(&root).unwrap().unwrap();
        assert_eq!(state.service.manager, StateServiceManager::None);
        assert_eq!(state.initialization.status, InitStatus::Pending);
        let _ = std::fs::remove_dir_all(&root.install_root);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn systemd_request_fails_when_unavailable() {
        let (files, _) = repository_files();
        let (_server, root, provider) = start_repository("e2e-systemd-unavail", files);
        let dir = std::env::temp_dir().join(format!(
            "lkit-pipeline-test-systemd-unavail-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let systemd = Systemd {
            systemctl: dir.join("systemctl"),
            system_unit_dir: dir.join("units"),
            run_systemd_dir: dir.join("missing-run"),
            pid1_is_systemd: true,
            resolv_conf: dir.join("resolv.conf"),
        };
        assert!(matches!(
            first_install(
                &root,
                &provider,
                &TargetVersion::Version(version()),
                &credentials(),
                ManagerChoice::Systemd,
                &systemd,
                &none_options(),
            )
            .await,
            Err(InstallError::Systemd(_))
        ));
        assert!(!root.canonical.join("state/install-state.json").exists());
        let _ = std::fs::remove_dir_all(&root.install_root);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn auto_rejects_damaged_systemd_environment() {
        let dir = std::env::temp_dir().join(format!(
            "lkit-pipeline-test-auto-damaged-systemd-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("run")).unwrap();
        let systemd = Systemd {
            systemctl: dir.join("missing-systemctl"),
            system_unit_dir: dir.join("units"),
            run_systemd_dir: dir.join("run"),
            pid1_is_systemd: true,
            resolv_conf: dir.join("resolv.conf"),
        };

        assert!(matches!(
            select_manager(ManagerChoice::Auto, &systemd),
            Err(InstallError::Systemd(_))
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn first_install_with_systemd_registers_and_verifies() {
        assert_systemd_first_install(ManagerChoice::Systemd, "e2e-systemd-explicit").await;
    }

    #[tokio::test]
    async fn auto_selects_systemd_when_available() {
        assert_systemd_first_install(ManagerChoice::Auto, "e2e-systemd-auto").await;
    }

    async fn assert_systemd_first_install(choice: ManagerChoice, case: &str) {
        use std::net::{TcpListener, UdpSocket};

        let (files, payload) = repository_files();
        let (server, root, provider) = start_repository(case, files);
        let dir =
            std::env::temp_dir().join(format!("lkit-pipeline-test-{case}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let tcp1 = TcpListener::bind("127.0.0.1:0").unwrap();
        let tcp2 = TcpListener::bind("127.0.0.1:0").unwrap();
        let tcp3 = TcpListener::bind("127.0.0.1:0").unwrap();
        let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
        let ports = vec![
            PortCheck {
                protocol: super::super::process::Protocol::Tcp,
                port: tcp1.local_addr().unwrap().port(),
            },
            PortCheck {
                protocol: super::super::process::Protocol::Tcp,
                port: tcp2.local_addr().unwrap().port(),
            },
            PortCheck {
                protocol: super::super::process::Protocol::Tcp,
                port: tcp3.local_addr().unwrap().port(),
            },
            PortCheck {
                protocol: super::super::process::Protocol::Udp,
                port: udp.local_addr().unwrap().port(),
            },
        ];

        let systemd = fake_systemd(&dir, std::process::id());
        let data_dir = root.canonical.join("data");
        let watcher = std::thread::spawn(move || {
            loop {
                if data_dir.join("landscape_init.toml").is_file() {
                    std::fs::write(data_dir.join("landscape_init.lock"), b"").unwrap();
                    std::fs::write(data_dir.join("landscape.toml"), b"").unwrap();
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });

        let options = HealthOptions {
            docs: FakeDocs,
            ports: ports.clone(),
            startup_timeout: Duration::from_secs(15),
            stable_duration: Duration::from_millis(100),
        };
        let outcome = first_install(
            &root,
            &provider,
            &TargetVersion::Version(version()),
            &credentials(),
            choice,
            &systemd,
            &options,
        )
        .await
        .unwrap();
        watcher.join().unwrap();
        assert_eq!(outcome.manager, ServiceManager::Systemd);

        assert!(dir.join("units/landscape-router.service").is_symlink());
        let unit_origin = root.canonical.join("service/landscape-router.service");
        assert!(unit_origin.is_file());
        assert_eq!(
            std::fs::read_link(dir.join("units/landscape-router.service")).unwrap(),
            unit_origin.canonicalize().unwrap()
        );

        let state = super::super::state::load_state(&root).unwrap().unwrap();
        assert_eq!(state.service.manager, StateServiceManager::Systemd);
        assert!(state.service.registered);
        assert!(state.service.enabled);
        assert!(state.service.verified);
        assert_eq!(
            state.service.definition_path.as_deref(),
            Some("service/landscape-router.service")
        );
        assert_eq!(state.initialization.status, InitStatus::Complete);
        assert!(state.initialization.lock_present);
        assert!(state.initialization.initialized_at.is_some());

        assert!(
            super::super::transaction::find_unfinished(&root)
                .unwrap()
                .is_none()
        );
        let tx = load_transaction_json(&root);
        assert_eq!(tx["phase"], "committed");
        assert!(tx["systemd_before"]["registration"]["kind"] == "missing");
        let resolv_backup = tx["resolv_conf_backup"].as_str().unwrap();
        assert!(
            root.canonical.join(resolv_backup).is_dir(),
            "resolv backup dir missing: {resolv_backup}"
        );

        assert!(!server.request_paths().is_empty());
        let binary = root.canonical.join("releases/1.2.3/landscape-webserver");
        assert_eq!(std::fs::read(&binary).unwrap(), payload);

        drop(tcp1);
        drop(tcp2);
        drop(tcp3);
        drop(udp);
        let _ = std::fs::remove_dir_all(&root.install_root);
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn load_transaction_json(root: &InstallRoot) -> serde_json::Value {
        let entries: Vec<_> = std::fs::read_dir(root.canonical.join("transactions"))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert!(!entries.is_empty());
        // uuid v7 按时间排序,取最新的交易。
        let newest = entries
            .into_iter()
            .max_by(|a, b| a.file_name().cmp(&b.file_name()))
            .unwrap();
        let bytes = std::fs::read(newest.path()).unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn resolves_latest_stable_from_channel() {
        let (_server, root, provider) = start_repository("e2e-latest", repository_files().0);
        let outcome = first_install(
            &root,
            &provider,
            &TargetVersion::Latest,
            &credentials(),
            ManagerChoice::None,
            &Systemd::host(),
            &none_options(),
        )
        .await
        .unwrap();
        assert_eq!(outcome.release.version, version());
        let _ = std::fs::remove_dir_all(&root.install_root);
    }

    #[tokio::test]
    async fn fails_without_stable_channel() {
        let (files, _) = repository_files();
        let mut files = files;
        files.remove("/channels/stable.json");
        let (_server, root, provider) = start_repository("e2e-missing", files);
        assert!(matches!(
            first_install(
                &root,
                &provider,
                &TargetVersion::Latest,
                &credentials(),
                ManagerChoice::None,
                &Systemd::host(),
                &none_options(),
            )
            .await,
            Err(InstallError::NoStableVersion)
        ));
        let _ = std::fs::remove_dir_all(&root.install_root);
    }

    #[tokio::test]
    async fn cleans_up_on_asset_download_failure() {
        let (mut files, _) = repository_files();
        let asset_path = "/releases/1.2.3/landscape-webserver-x86_64.zst";
        files.remove(asset_path);
        let (server, root, provider) = start_repository("e2e-download-failure", files);

        assert!(
            first_install(
                &root,
                &provider,
                &TargetVersion::Version(version()),
                &credentials(),
                ManagerChoice::None,
                &Systemd::host(),
                &none_options(),
            )
            .await
            .is_err()
        );
        assert!(server.request_paths().contains(&asset_path.to_string()));
        assert_failed_first_install_cleanup(&root);
        let _ = std::fs::remove_dir_all(&root.install_root);
    }

    #[tokio::test]
    async fn cleans_up_on_corrupted_webserver_archive() {
        let (files, _) = repository_files();
        let (webserver_sha, webserver_size) = sha256_bytes(b"garbage");
        let manifest = serde_json::json!({
            "protocol_version": 1,
            "version": "1.2.3",
            "assets": {
                "webserver": {
                    "x86_64": {
                        "url": "landscape-webserver-x86_64.zst",
                        "sha256": webserver_sha,
                        "size": webserver_size,
                    }
                },
                "static": {
                    "url": "static.zip",
                    "sha256": "b".repeat(64),
                    "size": 1,
                }
            }
        })
        .to_string();
        let mut files = files;
        files.insert(
            "/releases/1.2.3/manifest.json".into(),
            manifest.into_bytes(),
        );
        files.insert(
            "/releases/1.2.3/landscape-webserver-x86_64.zst".into(),
            b"garbage".to_vec(),
        );
        let (_server, root, provider) = start_repository("e2e-corrupt", files);
        assert!(
            first_install(
                &root,
                &provider,
                &TargetVersion::Version(version()),
                &credentials(),
                ManagerChoice::None,
                &Systemd::host(),
                &none_options(),
            )
            .await
            .is_err()
        );
        assert_failed_first_install_cleanup(&root);
        let _ = std::fs::remove_dir_all(&root.install_root);
    }

    #[tokio::test]
    async fn cleans_up_on_invalid_static_archive() {
        let (files, _) = repository_files();
        let invalid_static = b"not a zip archive";
        let (static_sha, static_size) = sha256_bytes(invalid_static);
        let webserver_zst = files
            .get("/releases/1.2.3/landscape-webserver-x86_64.zst")
            .unwrap();
        let (webserver_sha, webserver_size) = sha256_bytes(webserver_zst);
        let manifest = serde_json::json!({
            "protocol_version": 1,
            "version": "1.2.3",
            "assets": {
                "webserver": {
                    "x86_64": {
                        "url": "landscape-webserver-x86_64.zst",
                        "sha256": webserver_sha,
                        "size": webserver_size,
                    }
                },
                "static": {
                    "url": "static.zip",
                    "sha256": static_sha,
                    "size": static_size,
                }
            }
        })
        .to_string();
        let mut files = files;
        files.insert(
            "/releases/1.2.3/manifest.json".into(),
            manifest.into_bytes(),
        );
        files.insert("/releases/1.2.3/static.zip".into(), invalid_static.to_vec());
        let (server, root, provider) = start_repository("e2e-invalid-static", files);

        assert!(
            first_install(
                &root,
                &provider,
                &TargetVersion::Version(version()),
                &credentials(),
                ManagerChoice::None,
                &Systemd::host(),
                &none_options(),
            )
            .await
            .is_err()
        );
        assert!(
            server
                .request_paths()
                .contains(&"/releases/1.2.3/static.zip".to_string())
        );
        assert_failed_first_install_cleanup(&root);
        let _ = std::fs::remove_dir_all(&root.install_root);
    }

    fn assert_failed_first_install_cleanup(root: &InstallRoot) {
        assert!(!root.canonical.join("current").exists());
        assert!(!root.canonical.join("releases/1.2.3").exists());
        assert!(!root.canonical.join("data/landscape_init.toml").exists());
        assert!(!root.canonical.join("state/install-state.json").exists());
        assert!(!root.canonical.join("releases/.install-1.2.3.tmp").exists());
        assert!(
            super::super::transaction::find_unfinished(root)
                .unwrap()
                .is_none()
        );
        assert_eq!(load_transaction_json(root)["phase"], "failed");
    }

    #[tokio::test]
    async fn rejects_existing_release_directory() {
        let (_server, root, provider) = start_repository("e2e-exists", repository_files().0);
        std::fs::create_dir_all(root.canonical.join("releases/1.2.3")).unwrap();
        assert!(matches!(
            first_install(
                &root,
                &provider,
                &TargetVersion::Version(version()),
                &credentials(),
                ManagerChoice::None,
                &Systemd::host(),
                &none_options(),
            )
            .await,
            Err(InstallError::ReleaseExists(_))
        ));
        let _ = std::fs::remove_dir_all(&root.install_root);
    }

    fn export_body(version: &str) -> Vec<u8> {
        serde_json::json!({
            "data": {
                "filename": format!("landscape_init_v{version}.toml"),
                "version": version,
                "content": format!("version = \"{version}\"\n\n[config.auth]\nadmin_user = \"admin\"\nadmin_pass = \"Secret123\"\n"),
            }
        })
        .to_string()
        .into_bytes()
    }

    fn start_switch_repository(
        name: &str,
        from: &str,
        to: &str,
        payload_to: &[u8],
    ) -> (TestServer, InstallRoot, ReleaseProvider) {
        let mut files = repository_files_for(from, WEBSERVER_PAYLOAD);
        files.extend(repository_files_for(to, payload_to));
        let export = export_body(from);
        let server = TestServer::start(move |path| match path {
            "/api/v1/system/config/export" => TestResponse::ok(export.clone()),
            other => match files.get(other) {
                Some(body) => TestResponse::ok(body.clone()),
                None => TestResponse::status(404, "Not Found", Vec::new()),
            },
        });
        let root = temp_root(name);
        let install_root = InstallRoot {
            install_root: root.clone(),
            canonical: root,
        };
        let provider = provider_for(ProviderKind::Http, &server.base).unwrap();
        (server, install_root, provider)
    }

    fn switch_options<'a, P: DocsProbe>(
        base_url: &str,
        health: &'a HealthOptions<P>,
        confirmed: bool,
    ) -> SwitchOptions<'a, P> {
        static TOKEN: fn() -> Result<String, InstallError> = || Ok("tok".into());
        static YES: fn(&str) -> Result<bool, InstallError> = |_| Ok(true);
        static NO: fn(&str) -> Result<bool, InstallError> = |_| Ok(false);
        SwitchOptions {
            export_base_url: base_url.to_string(),
            token: &TOKEN,
            confirm: if confirmed { &YES } else { &NO },
            health,
        }
    }

    #[tokio::test]
    async fn switches_version_without_systemd() {
        let (server, root, provider) = start_switch_repository(
            "e2e-switch-none",
            "1.2.3",
            "1.3.0",
            b"webserver 1.3.0 payload",
        );
        first_install(
            &root,
            &provider,
            &TargetVersion::Version(version()),
            &credentials(),
            ManagerChoice::None,
            &Systemd::host(),
            &none_options(),
        )
        .await
        .unwrap();
        let state = super::super::state::load_state(&root).unwrap().unwrap();
        assert_eq!(state.active_version, "1.2.3");

        let target = provider
            .release(&semver::Version::new(1, 3, 0), Architecture::X86_64)
            .await
            .unwrap();
        let health = test_options();
        let outcome = switch_version(
            &root,
            &provider,
            &state,
            target,
            &SwitchArgs {
                allow_no_backup: false,
            },
            &Systemd::host(),
            &switch_options(&server.base, &health, true),
        )
        .await
        .unwrap();
        let SwitchOutcome::Committed { version, backup_id } = outcome else {
            panic!("expected committed switch, got {outcome:?}");
        };
        assert_eq!(version.to_string(), "1.3.0");
        assert!(backup_id.is_some());

        assert_eq!(
            std::fs::read_link(root.canonical.join("current")).unwrap(),
            std::path::PathBuf::from("releases/1.3.0")
        );
        assert!(root.canonical.join("releases/1.2.3").is_dir());
        assert!(
            root.canonical
                .join("backups")
                .join(format!("{}.lkb", backup_id.as_ref().unwrap()))
                .is_file()
        );
        let state = super::super::state::load_state(&root).unwrap().unwrap();
        assert_eq!(state.active_version, "1.3.0");
        assert_eq!(state.service.manager, StateServiceManager::None);
        assert!(!state.service.verified);
        assert!(
            super::super::transaction::find_unfinished(&root)
                .unwrap()
                .is_none()
        );
        let tx = load_transaction_json(&root);
        assert_eq!(tx["phase"], "committed");
        assert_eq!(tx["operation"], "switch");
        assert_eq!(tx["from_version"], "1.2.3");
        assert_eq!(tx["target_version"], "1.3.0");
        assert_eq!(tx["previous_current"], "releases/1.2.3");
        assert!(tx["backup"]["backup_id"].as_str().unwrap() == backup_id.as_deref().unwrap());
        assert!(
            server
                .request_paths()
                .contains(&"/api/v1/system/config/export".to_string())
        );
        let _ = std::fs::remove_dir_all(&root.install_root);
    }

    #[tokio::test]
    async fn refuses_switch_when_user_does_not_confirm() {
        let (server, root, provider) = start_switch_repository(
            "e2e-switch-refuse",
            "1.2.3",
            "1.3.0",
            b"webserver 1.3.0 payload",
        );
        first_install(
            &root,
            &provider,
            &TargetVersion::Version(version()),
            &credentials(),
            ManagerChoice::None,
            &Systemd::host(),
            &none_options(),
        )
        .await
        .unwrap();
        let state = super::super::state::load_state(&root).unwrap().unwrap();
        let target = provider
            .release(&semver::Version::new(1, 3, 0), Architecture::X86_64)
            .await
            .unwrap();
        let health = test_options();
        assert!(matches!(
            switch_version(
                &root,
                &provider,
                &state,
                target,
                &SwitchArgs {
                    allow_no_backup: false,
                },
                &Systemd::host(),
                &switch_options(&server.base, &health, false),
            )
            .await,
            Err(InstallError::UserRefused(_))
        ));
        assert_eq!(
            std::fs::read_link(root.canonical.join("current")).unwrap(),
            std::path::PathBuf::from("releases/1.2.3")
        );
        let _ = std::fs::remove_dir_all(&root.install_root);
    }

    #[tokio::test]
    async fn repository_override_does_not_require_a_second_confirmation() {
        let (_first_server, first_root, first_provider) = start_switch_repository(
            "e2e-switch-repo-a",
            "1.2.3",
            "1.3.0",
            b"webserver 1.3.0 payload",
        );
        first_install(
            &first_root,
            &first_provider,
            &TargetVersion::Version(version()),
            &credentials(),
            ManagerChoice::None,
            &Systemd::host(),
            &none_options(),
        )
        .await
        .unwrap();
        let state = super::super::state::load_state(&first_root)
            .unwrap()
            .unwrap();
        assert_eq!(state.repository.location, first_provider.location());

        let (second_server, second_root, second_provider) = start_switch_repository(
            "e2e-switch-repo-b",
            "1.2.3",
            "1.3.0",
            b"webserver 1.3.0 payload",
        );
        let target = second_provider
            .release(&semver::Version::new(1, 3, 0), Architecture::X86_64)
            .await
            .unwrap();
        let health = test_options();
        let token = || Ok("tok".to_string());
        let prompts = std::cell::RefCell::new(Vec::new());
        let confirm = |prompt: &str| {
            prompts.borrow_mut().push(prompt.to_string());
            Ok(true)
        };
        let options = SwitchOptions {
            export_base_url: second_server.base.clone(),
            token: &token,
            confirm: &confirm,
            health: &health,
        };
        let outcome = switch_version(
            &first_root,
            &second_provider,
            &state,
            target,
            &SwitchArgs {
                allow_no_backup: false,
            },
            &Systemd::host(),
            &options,
        )
        .await
        .unwrap();
        assert!(matches!(outcome, SwitchOutcome::Committed { .. }));
        assert_eq!(
            prompts.into_inner(),
            vec!["stop your Landscape instance with your own process manager, then confirm"]
        );
        assert_eq!(
            std::fs::read_link(first_root.canonical.join("current")).unwrap(),
            std::path::PathBuf::from("releases/1.3.0")
        );
        assert!(
            super::super::transaction::find_unfinished(&first_root)
                .unwrap()
                .is_none()
        );
        let state = super::super::state::load_state(&first_root)
            .unwrap()
            .unwrap();
        assert_eq!(state.active_version, "1.3.0");
        assert_eq!(state.repository.location, second_provider.location());
        let _ = std::fs::remove_dir_all(&first_root.install_root);
        let _ = std::fs::remove_dir_all(&second_root.install_root);
    }

    /// 有状态假 systemctl:start/stop 维护 state 文件,stop 后 ActiveState 为 inactive。
    fn fake_systemd_stateful(dir: &std::path::Path, main_pid: u32) -> Systemd {
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
  "start landscape-router.service") echo active > "$STATE_FILE"; exit 0;;
  "stop landscape-router.service") echo inactive > "$STATE_FILE"; exit 0;;
  "show --property=ActiveState --value landscape-router.service") cat "$STATE_FILE";;
  "show --property=MainPID --value landscape-router.service") echo {main_pid};;
  "is-enabled landscape-router.service") echo enabled;;
  "is-active landscape-router.service") cat "$STATE_FILE";;
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

    fn init_watcher(
        data_dir: std::path::PathBuf,
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) {
        std::thread::spawn(move || {
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                if data_dir.join("landscape_init.toml").is_file()
                    && !data_dir.join("landscape_init.lock").exists()
                {
                    std::fs::write(data_dir.join("landscape_init.lock"), b"").unwrap();
                    std::fs::write(data_dir.join("landscape.toml"), b"").unwrap();
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });
    }

    #[tokio::test]
    async fn switches_version_with_systemd() {
        use std::net::{TcpListener, UdpSocket};

        let (server, root, provider) = start_switch_repository(
            "e2e-switch-systemd",
            "1.2.3",
            "1.3.0",
            b"webserver 1.3.0 payload",
        );
        let dir = std::env::temp_dir().join(format!(
            "lkit-pipeline-test-switch-systemd-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let tcp1 = TcpListener::bind("127.0.0.1:0").unwrap();
        let tcp2 = TcpListener::bind("127.0.0.1:0").unwrap();
        let tcp3 = TcpListener::bind("127.0.0.1:0").unwrap();
        let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
        let ports = vec![
            PortCheck {
                protocol: super::super::process::Protocol::Tcp,
                port: tcp1.local_addr().unwrap().port(),
            },
            PortCheck {
                protocol: super::super::process::Protocol::Tcp,
                port: tcp2.local_addr().unwrap().port(),
            },
            PortCheck {
                protocol: super::super::process::Protocol::Tcp,
                port: tcp3.local_addr().unwrap().port(),
            },
            PortCheck {
                protocol: super::super::process::Protocol::Udp,
                port: udp.local_addr().unwrap().port(),
            },
        ];
        let systemd = fake_systemd_stateful(&dir, std::process::id());
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        init_watcher(root.canonical.join("data"), stop.clone());

        let options = HealthOptions {
            docs: FakeDocs,
            ports: ports.clone(),
            startup_timeout: Duration::from_secs(15),
            stable_duration: Duration::from_millis(100),
        };
        first_install(
            &root,
            &provider,
            &TargetVersion::Version(version()),
            &credentials(),
            ManagerChoice::Systemd,
            &systemd,
            &options,
        )
        .await
        .unwrap();
        let state = super::super::state::load_state(&root).unwrap().unwrap();
        assert_eq!(state.active_version, "1.2.3");
        let retained_init = b"externally_modified = true\n";
        std::fs::write(
            root.canonical.join("data/landscape_init.toml"),
            retained_init,
        )
        .unwrap();

        let target = provider
            .release(&semver::Version::new(1, 3, 0), Architecture::X86_64)
            .await
            .unwrap();
        let outcome = switch_version(
            &root,
            &provider,
            &state,
            target,
            &SwitchArgs {
                allow_no_backup: false,
            },
            &systemd,
            &switch_options(&server.base, &options, true),
        )
        .await
        .unwrap();
        let SwitchOutcome::Committed { version, backup_id } = outcome else {
            panic!("expected committed switch, got {outcome:?}");
        };
        assert_eq!(version.to_string(), "1.3.0");
        assert!(backup_id.is_some());

        let state = super::super::state::load_state(&root).unwrap().unwrap();
        assert_eq!(state.active_version, "1.3.0");
        assert_eq!(state.service.manager, StateServiceManager::Systemd);
        assert!(state.service.verified);
        assert!(state.initialization.lock_present);
        assert_eq!(
            std::fs::read(root.canonical.join("data/landscape_init.toml")).unwrap(),
            retained_init
        );
        assert!(
            root.canonical
                .join("backups")
                .join(format!("{}.lkb", backup_id.as_ref().unwrap()))
                .is_file()
        );
        let tx = load_transaction_json(&root);
        assert_eq!(tx["phase"], "committed");
        assert_eq!(tx["operation"], "switch");
        assert!(tx["systemd_before"]["registration"]["kind"] == "symlink");
        assert!(tx["resolv_conf_backup"].is_string());

        drop(tcp1);
        drop(tcp2);
        drop(tcp3);
        drop(udp);
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = std::fs::remove_dir_all(&root.install_root);
        let _ = std::fs::remove_dir_all(&dir);
    }

    type StoppedServiceWorld = (
        TestServer,
        InstallRoot,
        ReleaseProvider,
        Systemd,
        std::path::PathBuf,
        HealthOptions<ToggleDocs>,
        Vec<std::net::TcpListener>,
        UdpSocket,
        std::sync::Arc<std::sync::atomic::AtomicBool>,
    );

    fn stopped_service_world(name: &str) -> StoppedServiceWorld {
        use std::net::{TcpListener, UdpSocket};

        let (server, root, provider) =
            start_switch_repository(name, "1.2.3", "1.3.0", b"webserver 1.3.0 payload");
        let dir =
            std::env::temp_dir().join(format!("lkit-pipeline-host-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let tcp1 = TcpListener::bind("127.0.0.1:0").unwrap();
        let tcp2 = TcpListener::bind("127.0.0.1:0").unwrap();
        let tcp3 = TcpListener::bind("127.0.0.1:0").unwrap();
        let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
        let ports = vec![
            PortCheck {
                protocol: super::super::process::Protocol::Tcp,
                port: tcp1.local_addr().unwrap().port(),
            },
            PortCheck {
                protocol: super::super::process::Protocol::Tcp,
                port: tcp2.local_addr().unwrap().port(),
            },
            PortCheck {
                protocol: super::super::process::Protocol::Tcp,
                port: tcp3.local_addr().unwrap().port(),
            },
            PortCheck {
                protocol: super::super::process::Protocol::Udp,
                port: udp.local_addr().unwrap().port(),
            },
        ];
        let systemd = fake_systemd_stateful(&dir, std::process::id());
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        init_watcher(root.canonical.join("data"), stop.clone());
        let docs = ToggleDocs {
            ok: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
        };
        let options = HealthOptions {
            docs,
            ports: ports.clone(),
            startup_timeout: Duration::from_secs(15),
            stable_duration: Duration::from_millis(100),
        };
        (
            server,
            root,
            provider,
            systemd,
            dir,
            options,
            vec![tcp1, tcp2, tcp3],
            udp,
            stop,
        )
    }

    #[tokio::test]
    async fn refuses_switch_when_stopped_service_without_allow_no_backup() {
        use std::os::unix::fs::PermissionsExt;

        let (server, root, provider, systemd, dir, options, _tcp, _udp, stop) =
            stopped_service_world("e2e-switch-stopped-refuse");
        first_install(
            &root,
            &provider,
            &TargetVersion::Version(version()),
            &credentials(),
            ManagerChoice::Systemd,
            &systemd,
            &options,
        )
        .await
        .unwrap();
        // 模拟 systemctl stop:unit 仍注册,但 ActiveState 为 inactive。
        std::fs::write(dir.join("state"), b"inactive").unwrap();
        let state = super::super::state::load_state(&root).unwrap().unwrap();
        let target = provider
            .release(&semver::Version::new(1, 3, 0), Architecture::X86_64)
            .await
            .unwrap();
        let result = switch_version(
            &root,
            &provider,
            &state,
            target,
            &SwitchArgs {
                allow_no_backup: false,
            },
            &systemd,
            &switch_options(&server.base, &options, true),
        )
        .await;
        assert!(matches!(result, Err(InstallError::ServiceNotRunning(_))));
        assert!(
            super::super::transaction::find_unfinished(&root)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            std::fs::read_link(root.canonical.join("current")).unwrap(),
            std::path::PathBuf::from("releases/1.2.3")
        );
        assert_eq!(state.active_version, "1.2.3");
        assert!(
            std::fs::metadata(dir.join("state"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777
                > 0
        );

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = std::fs::remove_dir_all(&root.install_root);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn switches_stopped_service_without_backup_when_allowed() {
        let (server, root, provider, systemd, dir, options, _tcp, _udp, stop) =
            stopped_service_world("e2e-switch-stopped-ok");
        first_install(
            &root,
            &provider,
            &TargetVersion::Version(version()),
            &credentials(),
            ManagerChoice::Systemd,
            &systemd,
            &options,
        )
        .await
        .unwrap();
        std::fs::write(dir.join("state"), b"inactive").unwrap();
        let state = super::super::state::load_state(&root).unwrap().unwrap();
        let target = provider
            .release(&semver::Version::new(1, 3, 0), Architecture::X86_64)
            .await
            .unwrap();
        let outcome = switch_version(
            &root,
            &provider,
            &state,
            target,
            &SwitchArgs {
                allow_no_backup: true,
            },
            &systemd,
            &switch_options(&server.base, &options, true),
        )
        .await
        .unwrap();
        let SwitchOutcome::Committed { version, backup_id } = outcome else {
            panic!("expected committed switch, got {outcome:?}");
        };
        assert_eq!(version.to_string(), "1.3.0");
        assert!(backup_id.is_none());

        let state = super::super::state::load_state(&root).unwrap().unwrap();
        assert_eq!(state.active_version, "1.3.0");
        assert_eq!(state.service.manager, StateServiceManager::Systemd);
        assert!(state.service.verified);
        let tx = load_transaction_json(&root);
        assert_eq!(tx["phase"], "committed");
        assert_eq!(tx["operation"], "switch");
        assert!(tx["no_backup"] == true);
        assert!(tx["backup"].is_null());
        let lkb_count = std::fs::read_dir(root.canonical.join("backups"))
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "lkb"))
                    .count()
            })
            .unwrap_or(0);
        assert_eq!(lkb_count, 0, "no .lkb backup must be created");
        assert!(
            !server
                .request_paths()
                .contains(&"/api/v1/system/config/export".to_string()),
            "the stopped service must not be queried for a config snapshot"
        );
        assert!(
            super::super::transaction::find_unfinished(&root)
                .unwrap()
                .is_none()
        );

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = std::fs::remove_dir_all(&root.install_root);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn rolls_back_stopped_service_switch_without_backup_on_health_failure() {
        let (server, root, provider, systemd, dir, _options, _tcp, _udp, stop) =
            stopped_service_world("e2e-switch-stopped-rollback");
        // 目标版本验证阶段 /api/docs 失败,恢复后无备份回滚的健康检查才能通过。
        let docs_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let docs = ToggleDocs {
            ok: docs_flag.clone(),
        };
        let options = HealthOptions {
            docs,
            ports: _options.ports.clone(),
            startup_timeout: Duration::from_secs(3),
            stable_duration: Duration::from_millis(100),
        };
        first_install(
            &root,
            &provider,
            &TargetVersion::Version(version()),
            &credentials(),
            ManagerChoice::Systemd,
            &systemd,
            &options,
        )
        .await
        .unwrap();
        std::fs::write(dir.join("state"), b"inactive").unwrap();
        let state = super::super::state::load_state(&root).unwrap().unwrap();
        // 目标版本启动验证期间 /api/docs 一直失败:启动轮询超时触发无备份回滚;
        // 之后恢复 /api/docs,回滚的健康检查才能通过。
        docs_flag.store(false, std::sync::atomic::Ordering::Relaxed);
        let docs_flag = docs_flag.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(4500));
            docs_flag.store(true, std::sync::atomic::Ordering::Relaxed);
        });

        let target = provider
            .release(&semver::Version::new(1, 3, 0), Architecture::X86_64)
            .await
            .unwrap();
        let outcome = switch_version(
            &root,
            &provider,
            &state,
            target,
            &SwitchArgs {
                allow_no_backup: true,
            },
            &systemd,
            &switch_options(&server.base, &options, true),
        )
        .await
        .unwrap();
        let SwitchOutcome::RolledBack { version, backup_id } = outcome else {
            panic!("expected rolled back switch, got {outcome:?}");
        };
        assert_eq!(version.to_string(), "1.2.3");
        assert!(backup_id.is_none());

        let state = super::super::state::load_state(&root).unwrap().unwrap();
        assert_eq!(state.active_version, "1.2.3");
        assert_eq!(state.service.manager, StateServiceManager::Systemd);
        assert!(state.service.verified);
        assert_eq!(
            std::fs::read_link(root.canonical.join("current")).unwrap(),
            std::path::PathBuf::from("releases/1.2.3")
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("state")).unwrap().trim(),
            "inactive",
            "the previous version stays stopped, matching the pre-switch state"
        );
        let tx = load_transaction_json(&root);
        assert_eq!(tx["phase"], "rolled_back");
        assert_eq!(tx["operation"], "switch");
        assert!(tx["no_backup"] == true);
        assert!(tx["backup"].is_null());
        assert!(
            super::super::transaction::find_unfinished(&root)
                .unwrap()
                .is_none()
        );

        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = std::fs::remove_dir_all(&root.install_root);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn switches_and_rolls_back_via_lkb_on_health_failure() {
        use std::net::{TcpListener, UdpSocket};

        let (server, root, provider) =
            start_switch_repository("e2e-rollback", "1.2.3", "1.3.0", b"webserver 1.3.0 payload");
        let dir = std::env::temp_dir().join(format!(
            "lkit-pipeline-test-rollback-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let tcp1 = TcpListener::bind("127.0.0.1:0").unwrap();
        let tcp2 = TcpListener::bind("127.0.0.1:0").unwrap();
        let tcp3 = TcpListener::bind("127.0.0.1:0").unwrap();
        let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
        let ports = vec![
            PortCheck {
                protocol: super::super::process::Protocol::Tcp,
                port: tcp1.local_addr().unwrap().port(),
            },
            PortCheck {
                protocol: super::super::process::Protocol::Tcp,
                port: tcp2.local_addr().unwrap().port(),
            },
            PortCheck {
                protocol: super::super::process::Protocol::Tcp,
                port: tcp3.local_addr().unwrap().port(),
            },
            PortCheck {
                protocol: super::super::process::Protocol::Udp,
                port: udp.local_addr().unwrap().port(),
            },
        ];
        let systemd = fake_systemd_stateful(&dir, std::process::id());
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        init_watcher(root.canonical.join("data"), stop.clone());

        let docs_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let docs = ToggleDocs {
            ok: docs_flag.clone(),
        };
        let options = HealthOptions {
            docs,
            ports: ports.clone(),
            startup_timeout: Duration::from_secs(5),
            stable_duration: Duration::from_millis(100),
        };
        first_install(
            &root,
            &provider,
            &TargetVersion::Version(version()),
            &credentials(),
            ManagerChoice::Systemd,
            &systemd,
            &options,
        )
        .await
        .unwrap();
        let state = super::super::state::load_state(&root).unwrap().unwrap();
        assert_eq!(state.active_version, "1.2.3");

        // 让目标版本稳定观察失败(约 1s),随后恢复 /api/docs 以便回滚健康检查通过。
        let docs_flag = docs_flag.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            docs_flag.store(false, std::sync::atomic::Ordering::Relaxed);
            std::thread::sleep(Duration::from_millis(1300));
            docs_flag.store(true, std::sync::atomic::Ordering::Relaxed);
        });

        let target = provider
            .release(&semver::Version::new(1, 3, 0), Architecture::X86_64)
            .await
            .unwrap();
        let outcome = switch_version(
            &root,
            &provider,
            &state,
            target,
            &SwitchArgs {
                allow_no_backup: false,
            },
            &systemd,
            &switch_options(&server.base, &options, true),
        )
        .await
        .unwrap();
        let SwitchOutcome::RolledBack { version, backup_id } = outcome else {
            panic!("expected rolled back switch, got {outcome:?}");
        };
        assert_eq!(version.to_string(), "1.2.3");
        assert!(backup_id.is_some());

        let state = super::super::state::load_state(&root).unwrap().unwrap();
        assert_eq!(state.active_version, "1.2.3");
        assert_eq!(state.service.manager, StateServiceManager::Systemd);
        assert!(state.service.verified);
        assert_eq!(
            std::fs::read_link(root.canonical.join("current")).unwrap(),
            std::path::PathBuf::from("releases/1.2.3")
        );
        let init_config =
            std::fs::read_to_string(root.canonical.join("data/landscape_init.toml")).unwrap();
        assert!(
            init_config.contains("admin_pass = \"Secret123\""),
            "restored init config: {init_config}"
        );

        let tx = load_transaction_json(&root);
        assert_eq!(tx["phase"], "rolled_back");
        assert_eq!(tx["operation"], "switch");
        let tx_dir = root
            .canonical
            .join("transactions")
            .join(tx["transaction_id"].as_str().unwrap());
        assert!(tx_dir.join("failed-data").is_dir());
        assert!(tx_dir.join("replaced-release").is_dir());
        assert!(tx_dir.join("restore").is_dir());
        assert!(
            root.canonical
                .join("backups")
                .join(tx["transaction_id"].as_str().unwrap())
                .join("host/resolv.conf")
                .is_dir()
        );

        drop(tcp1);
        drop(tcp2);
        drop(tcp3);
        drop(udp);
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = std::fs::remove_dir_all(&root.install_root);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn builds_reference_command() {
        let root = InstallRoot {
            install_root: "/root/.lkit/landscape".into(),
            canonical: "/root/.lkit/landscape".into(),
        };
        assert_eq!(
            reference_command(&root),
            "'/root/.lkit/landscape/current/landscape-webserver' --config-dir '/root/.lkit/landscape/data' --web '/root/.lkit/landscape/current/static'"
        );
    }

    #[test]
    fn builds_minimal_init_config() {
        let config = build_init_config(
            &version(),
            &Credentials {
                admin_user: "admin".into(),
                password: "Secret123".into(),
            },
            None,
        )
        .unwrap();
        assert_eq!(
            config,
            "version = \"1.2.3\"\n\n[config.auth]\nadmin_user = \"admin\"\nadmin_pass = \"Secret123\"\n"
        );
    }

    fn initialization_state(status: InitStatus) -> InstallState {
        InstallState {
            schema_version: STATE_SCHEMA_VERSION,
            layout_version: STATE_LAYOUT_VERSION,
            install_root: "/tmp/lkit-init-check".into(),
            canonical_install_root: "/tmp/lkit-init-check".into(),
            active_version: "1.2.3".into(),
            repository: RepositorySource {
                kind: StateRepositoryKind::Http,
                location: "https://repo.example.test/".into(),
            },
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
