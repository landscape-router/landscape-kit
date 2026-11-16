use super::*;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InstallRequest {
    pub target: String,
    pub repository: crate::deployment::plan::RepositoryChoice,
    pub admin_user: String,
    pub network: Option<crate::network::config::NetworkPlan>,
}
fn validate(args: &InstallRequest) -> Result<(), InstallError> {
    crate::deployment::plan::TargetVersion::parse(&args.target)?;
    args.repository.clone().resolve()?;
    crate::deployment::plan::validate_admin_user(&args.admin_user)?;
    if let Some(network) = &args.network {
        network.validate()?;
    }
    Ok(())
}
pub(super) fn validate_record(record: &Record) -> Result<(), InstallError> {
    if record.schema_version != 8
        || record.assets.is_some()
        || record.switch.is_some()
        || record.restore.is_some()
        || record.uninstall.is_some()
        || record.reinit.is_some()
        || record.network_transaction.is_some()
    {
        return Err(error("invalid install operation schema"));
    }
    validate(
        record
            .install
            .as_ref()
            .ok_or_else(|| error("missing install parameters"))?,
    )?;
    if let Some(version) = &record.resolved_version {
        crate::workflows::install::parse_stable_version(version)
            .map_err(|e| error(e.to_string()))?;
    }
    Ok(())
}
pub(super) fn submit(
    root: String,
    args: InstallRequest,
    password: &str,
) -> Result<Record, InstallError> {
    validate(&args)?;
    crate::interaction::credentials::validate_password(password)?;
    if password.len() > 4096 || password.chars().any(char::is_control) {
        return Err(error("invalid password input"));
    }
    let _guard = queue_lock()?;
    let record = Record {
        schema_version: 8,
        id: uuid::Uuid::now_v7().to_string(),
        kind: Kind::Install,
        install_root: root,
        status: Status::Queued,
        transaction_id: None,
        message: None,
        assets: None,
        switch: None,
        restore: None,
        uninstall: None,
        reinit: None,
        install: Some(args),
        migrate: None,
        network_transaction: None,
        resolved_version: None,
        stage: Stage::Checking,
        revision: 1,
        cancel_requested: false,
        cancel_cleaned: false,
        cancellable: true,
        downloaded_bytes: 0,
        total_bytes: None,
        exit_code: None,
    };
    crate::workflows::repair_service::atomic_write(
        &reinit::input_path(&record.id)?,
        password.as_bytes(),
        0o600,
    )?;
    if let Err(e) = save(&record) {
        reinit::remove_input(&record)?;
        return Err(e);
    }
    Ok(record)
}
pub(crate) async fn submit_install_and_wait(
    root: String,
    args: InstallRequest,
    password: &str,
    fullscreen: bool,
) -> Result<u8, InstallError> {
    crate::daemon::require_capability("install-v1.pid")?;
    wait_for_result(submit(root, args, password)?, "install-v1.pid", fullscreen).await
}
