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
    manager.validate_definition(
        ManagedService::LandscapeRouter,
        &std::fs::read_to_string(origin).map_err(InstallError::Io)?,
        &root.canonical,
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
    match restore_prepared(&root, &installed, manager, &args, &options, prepared).await? {
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
