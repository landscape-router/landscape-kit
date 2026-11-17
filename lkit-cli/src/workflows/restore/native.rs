use super::*;
use crate::daemon_operations::{Record, RestoreRequest};
use crate::deployment::runtime::InstallRuntime;

pub(crate) async fn run_operation(
    runtime: &InstallRuntime,
    record: &Record,
) -> Result<u8, InstallError> {
    let request = record
        .restore
        .as_ref()
        .ok_or_else(|| InstallError::ParameterUsage("missing restore parameters".into()))?;
    let root =
        super::super::root::normalize_install_root(std::path::Path::new(&record.install_root))?;
    let installed = super::super::state::load_state(&root)?
        .ok_or_else(|| InstallError::Preflight("missing installation".into()))?;
    if installed.active_version != request.from_version
        || crate::daemon_operations::installation_identity(&installed)? != request.state_identity
    {
        return Err(InstallError::Preflight(
            "installation state changed since restore confirmation".into(),
        ));
    }
    let manager = runtime.service_manager.as_ref();
    if installed.service.manager != manager.kind()
        || !matches!(
            manager.probe(),
            super::super::manager::Availability::Available { .. }
        )
    {
        return Err(InstallError::Preflight(
            "matching service backend is unavailable".into(),
        ));
    }
    let origin = root.canonical.join("service/landscape-router.service");
    if !matches!(manager.query_registration(ManagedService::LandscapeRouter)?, super::super::manager::SystemRegistration::Symlink { target } if target == origin)
    {
        return Err(InstallError::Preflight(
            "service registration is not owned by this installation".into(),
        ));
    }
    validate_service_definition(
        manager,
        &root.canonical,
        &std::fs::read_to_string(origin).map_err(InstallError::Io)?,
        runtime.test_runtime_path.is_some(),
    )?;
    let args = request.workflow_args();
    let prepared = prepare_restore(&root, &installed, &args)?;
    if prepared.sha256 != request.backup_sha256
        || prepared.metadata.landscape_version != request.target_version
        || prepared.metadata.backup_id != request.backup_id
    {
        return Err(InstallError::InvalidBackup(
            "backup changed since restore confirmation".into(),
        ));
    }
    let health = runtime.health_options()?;
    let options = RestoreOptions {
        export_base_url: runtime.export_base_url.clone(),
        token: &|| {
            crate::backup::export::read_api_token(
                &root.canonical.join("data/landscape_api_token"),
                runtime.managed_uid,
            )
        },
        confirm: &|_| {
            Err(InstallError::Preflight(
                "daemon cannot read terminal input".into(),
            ))
        },
        health: &health,
    };
    match restore_prepared(
        &root,
        &installed,
        manager,
        &args,
        &options,
        prepared,
        Some(runtime),
    )
    .await?
    {
        RestoreOutcome::AwaitingConfirmation => {
            control::message(&crate::tr!(crate::keys::RESTORE_AWAITING_CONFIRMATION))?;
            Ok(0)
        }
        RestoreOutcome::Committed { version, backup_id } => {
            control::message(&crate::tr!(
                crate::keys::RESTORE_COMMITTED,
                version = version,
                backup_id = backup_id
            ))?;
            Ok(0)
        }
        RestoreOutcome::RolledBack { version } => {
            control::message(&crate::tr!(
                crate::keys::RESTORE_FAILED_ROLLED_BACK,
                version = version
            ))?;
            Ok(5)
        }
        RestoreOutcome::RollbackFailed { version, reason } => {
            control::message(&crate::tr!(
                crate::keys::RESTORE_FAILED_ROLLBACK_FAILED,
                version = version,
                reason = reason
            ))?;
            Ok(6)
        }
    }
}

fn validate_service_definition(
    manager: &dyn ServiceManager,
    root: &std::path::Path,
    definition: &str,
    test_runtime: bool,
) -> Result<(), InstallError> {
    manager.validate_definition(ManagedService::LandscapeRouter, definition, root)?;
    if !test_runtime && !definition.contains("network startup-check") {
        return Err(InstallError::Preflight("Landscape service definition lacks the recovery startup check; run `lkit repair service` before restore".into()));
    }
    Ok(())
}

impl RestoreRequest {
    pub(crate) fn workflow_args(&self) -> RestoreArgs {
        RestoreArgs {
            backup_id: None,
            file_path: Some(self.source.clone()),
            allow_no_backup: self.allow_no_backup,
            yes: true,
            console_confirmed: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_requires_recovery_startup_check_for_all_service_backends() {
        use crate::service::{openrc::Openrc, systemd::Systemd, sysvinit::Sysvinit};

        let temp = super::super::tests::temp_root("native-startup-check");
        let _guard = layout::test_territory(&temp.join("territory"));
        let root = temp.join("install");
        let managers: [Box<dyn ServiceManager>; 3] = [
            Box::new(Systemd {
                systemctl: temp.join("systemctl"),
                system_unit_dir: temp.join("systemd"),
                run_systemd_dir: temp.join("run/systemd"),
                pid1_is_systemd: true,
                resolv_conf: temp.join("resolv.conf"),
            }),
            Box::new(Openrc {
                rc_service: temp.join("rc-service"),
                rc_update: temp.join("rc-update"),
                init_d_dir: temp.join("init.d"),
                resolv_conf: temp.join("resolv.conf"),
            }),
            Box::new(Sysvinit {
                update_rc_d: temp.join("update-rc.d"),
                init_d_dir: temp.join("init.d"),
                rc_d_glob: temp.join("rc?.d"),
                resolv_conf: temp.join("resolv.conf"),
            }),
        ];
        for manager in managers {
            let definition = manager
                .render_definition(ManagedService::LandscapeRouter, &root)
                .unwrap();
            validate_service_definition(manager.as_ref(), &root, &definition, false).unwrap();
            let legacy = definition
                .lines()
                .filter(|line| !line.contains("network startup-check"))
                .collect::<Vec<_>>()
                .join("\n");
            // General definition validation permits old deployments for repair;
            // guarded restore requires the independent startup protection.
            manager
                .validate_definition(ManagedService::LandscapeRouter, &legacy, &root)
                .unwrap();
            let error =
                validate_service_definition(manager.as_ref(), &root, &legacy, false).unwrap_err();
            assert!(matches!(error, InstallError::Preflight(_)));
            assert!(
                error
                    .to_string()
                    .contains("run `lkit repair service` before restore"),
                "{:?}: {error}",
                manager.kind()
            );
            validate_service_definition(manager.as_ref(), &root, &legacy, true).unwrap();
            let foreign = legacy.replace("current/landscape-webserver", "foreign/webserver");
            assert!(
                validate_service_definition(manager.as_ref(), &root, &foreign, true).is_err(),
                "fixture bypass must retain executable ownership validation"
            );
        }
        assert!(!layout::territory_transactions_dir().exists());
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn verified_restore_hash_covers_the_exact_bytes_consumed() {
        use sha2::{Digest, Sha256};
        let temp = super::super::tests::temp_root("native-confirmation");
        let _guard = layout::test_territory(&temp.join("territory"));
        let root =
            super::super::super::root::normalize_install_root(&temp.join("install")).unwrap();
        let installed = super::super::tests::install_state(
            &root,
            "1.3.0",
            super::super::tests::PAYLOAD_1_3_0,
            super::super::tests::ZIP_1_3_0,
        );
        let (backup, bytes) = super::super::tests::create_target_backup(&root);
        let args = RestoreArgs {
            backup_id: Some(backup.backup_id),
            file_path: None,
            yes: true,
            console_confirmed: false,
            allow_no_backup: false,
        };
        let prepared = prepare_restore(&root, &installed, &args).unwrap();
        assert_eq!(prepared.bytes, bytes);
        assert_eq!(
            prepared.sha256,
            crate::release::artifacts::hex(&Sha256::digest(&bytes))
        );
        assert_eq!(prepared.metadata.landscape_version, "1.2.3");
        assert!(!layout::territory_transactions_dir().exists());
    }
}
