use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReinitRequest {
    pub from_version: String,
    pub state_identity: String,
    pub admin_user: String,
    pub network: crate::network::config::NetworkPlan,
    pub allow_no_backup: bool,
}
fn validate(args: &ReinitRequest) -> Result<(), InstallError> {
    crate::workflows::install::parse_stable_version(&args.from_version)
        .map_err(|e| error(e.to_string()))?;
    if args.state_identity.len() != 64
        || !args.state_identity.bytes().all(|c| c.is_ascii_hexdigit())
    {
        return Err(error("invalid reinit installation identity"));
    }
    if args.admin_user.is_empty() || args.admin_user.chars().any(char::is_control) {
        return Err(error("invalid admin username"));
    }
    args.network.validate()
}
pub(super) fn validate_record(record: &Record) -> Result<(), InstallError> {
    if record.schema_version != 7
        || record.assets.is_some()
        || record.switch.is_some()
        || record.restore.is_some()
        || record.uninstall.is_some()
        || record.network_transaction.is_some()
        || record.resolved_version.is_some()
    {
        return Err(error("invalid reinit operation schema"));
    }
    validate(
        record
            .reinit
            .as_ref()
            .ok_or_else(|| error("missing reinit parameters"))?,
    )
}
pub(super) fn input_path(id: &str) -> Result<PathBuf, InstallError> {
    validate_id(id)?;
    Ok(directory().join(format!("{id}.password")))
}
pub(super) fn remove_input(record: &Record) -> Result<(), InstallError> {
    if !matches!(record.kind, Kind::Reinit | Kind::Install) {
        return Ok(());
    }
    match std::fs::remove_file(input_path(&record.id)?) {
        Ok(()) => std::fs::File::open(directory())
            .and_then(|f| f.sync_all())
            .map_err(InstallError::Io),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(InstallError::Io(e)),
    }
}
pub(crate) fn take_password(record: &Record, uid: u32) -> Result<String, InstallError> {
    let result = crate::interaction::credentials::read_password_file(&input_path(&record.id)?, uid);
    remove_input(record)?;
    let password = result?;
    crate::interaction::credentials::validate_password(&password)?;
    Ok(password)
}
pub(super) fn submit(
    root: String,
    args: ReinitRequest,
    password: &str,
) -> Result<Record, InstallError> {
    validate(&args)?;
    crate::interaction::credentials::validate_password(password)?;
    if password.len() > 4096 || password.chars().any(char::is_control) {
        return Err(error("invalid password input"));
    }
    let _guard = queue_lock()?;
    let record = Record {
        schema_version: 7,
        id: uuid::Uuid::now_v7().to_string(),
        kind: Kind::Reinit,
        install_root: root,
        status: Status::Queued,
        transaction_id: None,
        message: None,
        assets: None,
        switch: None,
        restore: None,
        uninstall: None,
        network_transaction: None,
        reinit: Some(args),
        install: None,
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
        &input_path(&record.id)?,
        password.as_bytes(),
        0o600,
    )?;
    if let Err(e) = save(&record) {
        remove_input(&record)?;
        return Err(e);
    }
    Ok(record)
}
pub(crate) async fn submit_reinit_and_wait(
    root: String,
    args: ReinitRequest,
    password: &str,
    fullscreen: bool,
) -> Result<u8, InstallError> {
    crate::daemon::require_capability("reinit-v1.pid")?;
    wait_for_result(submit(root, args, password)?, "reinit-v1.pid", fullscreen).await
}

pub(super) fn cleanup_orphan_inputs() -> Result<(), InstallError> {
    let _guard = queue_lock()?;
    for entry in std::fs::read_dir(directory()).map_err(InstallError::Io)? {
        let file = entry.map_err(InstallError::Io)?.path();
        if file.extension().is_none_or(|ext| ext != "password") {
            continue;
        }
        let Some(id) = file.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        validate_id(id)?;
        match std::fs::metadata(path(id)?) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                std::fs::remove_file(file).map_err(InstallError::Io)?;
            }
            Err(e) => return Err(InstallError::Io(e)),
        }
    }
    Ok(())
}
