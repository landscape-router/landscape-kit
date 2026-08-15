use chrono::Utc;

use super::artifacts::build_release;
#[cfg(test)]
use super::artifacts::hex;
pub(crate) use super::artifacts::{
    BuiltRelease, STATIC_DIR, WEBSERVER_BINARY, fetch_static_asset, fetch_webserver_asset,
    hash_file, hash_str,
};
use super::credentials::Credentials;
use super::health::{self, DocsProbe, HealthOptions};
use super::manager::{ManagedService, ServiceManager};
use super::plan::{InstallError, TargetVersion};
pub(crate) use super::preflight::run_preflight;
use super::repository::{Architecture, ProviderKind, Release, ReleaseProvider};
use super::resolv;
use super::root::InstallRoot;
use super::state::{InitStatus, InitializationState, ServiceState, StateServiceManager};
pub(crate) use super::switch::{SwitchArgs, SwitchOptions, SwitchOutcome, switch_version};
use super::transaction::Phase;
use crate::deployment::runtime::InstallRuntime;

mod init_config;
mod manager;
mod state;
mod unit;

pub(crate) use super::manager::capture_before;
use init_config::write_init_config;
pub(crate) use init_config::{activate_current, build_init_config, parse_stable_version};
pub(crate) use manager::require_manager;
use state::{UnitActivation, build_state};
pub(crate) use state::{
    architecture_from_state, build_switched_state, check_initialization, verify_current_backend,
    verify_unit_ownership,
};
pub(crate) use unit::write_unit_origin;
pub(crate) struct FirstInstallOutcome {
    pub release: Release,
    pub pending_network_confirmation: bool,
    pub pending_network_address: Option<std::net::Ipv4Addr>,
}

pub(crate) async fn first_install<P: DocsProbe>(
    root: &InstallRoot,
    provider: &ReleaseProvider,
    target: &TargetVersion,
    credentials: &Credentials,
    manager: &dyn ServiceManager,
    health_options: &HealthOptions<P>,
) -> Result<FirstInstallOutcome, InstallError> {
    first_install_impl(
        root,
        provider,
        target,
        credentials,
        manager,
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
    manager: &dyn ServiceManager,
    health_options: &HealthOptions<P>,
    network: &crate::network::config::NetworkPlan,
    runtime: &InstallRuntime,
) -> Result<FirstInstallOutcome, InstallError> {
    first_install_impl(
        root,
        provider,
        target,
        credentials,
        manager,
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
    manager: &dyn ServiceManager,
    health_options: &HealthOptions<P>,
    network: Option<&crate::network::config::NetworkPlan>,
    runtime: Option<&InstallRuntime>,
) -> Result<FirstInstallOutcome, InstallError> {
    let architecture = Architecture::host().ok_or_else(|| {
        InstallError::UnsupportedPlatform(
            crate::tr!(crate::keys::INSTALL_ONLY_X86_64_AND_AARCH64_SUPPORTED).into(),
        )
    })?;
    let release = match target {
        TargetVersion::Latest => provider
            .latest(architecture)
            .await?
            .ok_or(InstallError::NoStableVersion)?,
        TargetVersion::Version(version) => provider.release(version, architecture).await?,
    };
    require_manager(manager)?;
    if network.is_some() {
        ensure_network_takeover_data_empty(root)?;
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
        crate::interaction::presentation::operation_phase(
            crate::interaction::presentation::OperationPhase::Downloading,
        );
        let built = build_release(root, &release).await?;
        crate::interaction::presentation::operation_phase(
            crate::interaction::presentation::OperationPhase::Applying,
        );
        let init_config = build_init_config(&release.version, credentials, network)?;
        write_init_config(root, &init_config)?;
        let before = capture_before(manager, ManagedService::LandscapeRouter)?;
        let backup_dir = root
            .canonical
            .join("backups")
            .join(&transaction.transaction_id)
            .join("host/resolv.conf");
        let _ = resolv::backup(manager.resolv_conf(), &backup_dir)?;
        let unit_sha = write_unit_origin(
            root,
            manager,
            ManagedService::LandscapeRouter,
            &manager.render_definition(ManagedService::LandscapeRouter, &root.canonical)?,
        )?;
        transaction.systemd_before = Some(before);
        transaction.resolv_conf_backup = Some(format!(
            "backups/{}/host/resolv.conf",
            transaction.transaction_id
        ));
        if let Some(takeover) = transaction.network_takeover.as_mut() {
            let runtime = runtime.ok_or_else(|| {
                InstallError::CorruptedTransaction("network takeover runtime disappeared".into())
            })?;
            crate::network::takeover::refresh_confirmation_deadline(takeover, runtime)?;
        }
        super::transaction::persist(root, &transaction)?;
        super::transaction::mark_phase(root, &transaction, Phase::Prepared)?;
        if let Some(takeover) = transaction.network_takeover.as_ref() {
            let runtime = runtime.ok_or_else(|| {
                InstallError::CorruptedTransaction("network takeover runtime disappeared".into())
            })?;
            crate::network::takeover::arm_recovery(root, takeover, runtime)?;
            super::transaction::mark_phase(root, &transaction, Phase::Stopping)?;
            crate::network::takeover::stop_host_services(takeover, manager)?;
            crate::network::takeover::clear_selected_lan_addresses(
                &takeover.plan,
                &runtime.ip_command,
            )?;
        }
        let activation = UnitActivation {
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
        };
        super::transaction::mark_phase(root, &transaction, Phase::Activating)?;
        activate_current(root, &release.version)?;
        manager.register(
            ManagedService::LandscapeRouter,
            &root.canonical.join("service/landscape-router.service"),
        )?;
        manager.enable(ManagedService::LandscapeRouter)?;
        manager.start(ManagedService::LandscapeRouter)?;
        super::transaction::mark_phase(root, &transaction, Phase::Verifying)?;
        let pid = manager.main_pid(ManagedService::LandscapeRouter)?;
        if pid == 0 {
            return Err(InstallError::Systemd(
                "service did not produce a main pid after start".into(),
            ));
        }
        let options = health::StartupOptions {
            ports: &health_options.ports,
            expected_pid: pid,
            docs: &health_options.docs,
            unit_state: Some(&(|| manager.active_state(ManagedService::LandscapeRouter).ok())),
            init_required: true,
            data_dir: &root.canonical.join("data"),
            startup_timeout: health_options.startup_timeout,
            stable_duration: health_options.stable_duration,
        };
        health::wait_for_startup(&options).await?;
        health::observe_stable(&options).await?;
        let state = build_state(root, &release, architecture, &built, &activation);
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
            pending_network_confirmation: pending_network,
            pending_network_address: pending_network
                .then(|| {
                    network
                        .expect("pending network has a plan")
                        .management_address()
                        .map(|address| address.address)
                })
                .flatten(),
        }),
        Err(error) => {
            if let Some(takeover) = transaction.network_takeover.as_ref() {
                let _ = super::transaction::mark_phase(root, &transaction, Phase::RollingBack);
                let cleanup =
                    super::transaction::cleanup_failed_first_install(root, &transaction, manager)
                        .and_then(|()| {
                            crate::network::takeover::cleanup_failed_takeover(
                                root, takeover, manager,
                            )
                        });
                if let Err(cleanup_error) = cleanup {
                    eprintln!(
                        "install: {}",
                        crate::tr!(
                            crate::keys::INSTALL_CLEANUP_FAILED_NETWORK,
                            cleanup_error = cleanup_error
                        )
                    );
                    return Err(error);
                }
            } else if let Err(cleanup_error) =
                super::transaction::cleanup_failed_first_install(root, &transaction, manager)
            {
                eprintln!(
                    "install: {}",
                    crate::tr!(
                        crate::keys::INSTALL_CLEANUP_FAILED_FIRST_INSTALL,
                        cleanup_error = cleanup_error
                    )
                );
            }
            let _ = super::transaction::mark_phase(root, &transaction, Phase::Failed);
            Err(error)
        }
    }
}

fn ensure_network_takeover_data_empty(root: &InstallRoot) -> Result<(), InstallError> {
    let data = root.canonical.join("data");
    let metadata = match std::fs::symlink_metadata(&data) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(InstallError::Io(error)),
    };
    if metadata.file_type().is_symlink() || !metadata.file_type().is_dir() {
        return Err(InstallError::DangerousDirectory(format!(
            "network takeover requires {} to be a real empty directory",
            data.display()
        )));
    }
    if std::fs::read_dir(&data)
        .map_err(InstallError::Io)?
        .next()
        .is_some()
    {
        return Err(InstallError::ParameterUsage(
            "network takeover requires an empty data directory".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
#[cfg(test)]
mod first_install_tests;
#[cfg(test)]
mod switch_tests;
