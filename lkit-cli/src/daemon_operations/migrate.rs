use super::*;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SourceIdentity {
    pub source: PathBuf,
    pub device: u64,
    pub inode: u64,
    pub pid: u32,
    pub started: u64,
    pub executable_sha256: String,
    pub arguments_sha256: String,
    pub unit: Option<String>,
    pub unit_sha256: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MigrateRequest {
    pub source: SourceIdentity,
    pub version: String,
    pub repository: crate::deployment::plan::RepositoryChoice,
}
fn validate(args: &MigrateRequest) -> Result<(), InstallError> {
    crate::workflows::install::parse_stable_version(&args.version)
        .map_err(|e| error(e.to_string()))?;
    args.repository.clone().resolve()?;
    if !args.source.source.is_absolute()
        || args.source.pid == 0
        || args.source.started == 0
        || args.source.unit.is_some() != args.source.unit_sha256.is_some()
    {
        return Err(error("invalid migration source identity"));
    }
    for digest in [
        &args.source.executable_sha256,
        &args.source.arguments_sha256,
    ]
    .into_iter()
    .chain(args.source.unit_sha256.iter())
    {
        if digest.len() != 64 || !digest.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(error("invalid migration identity digest"));
        }
    }
    Ok(())
}
pub(super) fn validate_record(record: &Record) -> Result<(), InstallError> {
    if record.schema_version != 9
        || record.assets.is_some()
        || record.switch.is_some()
        || record.restore.is_some()
        || record.uninstall.is_some()
        || record.reinit.is_some()
        || record.install.is_some()
        || record.network_transaction.is_some()
        || record.resolved_version.is_some()
    {
        return Err(error("invalid migrate operation schema"));
    }
    validate(
        record
            .migrate
            .as_ref()
            .ok_or_else(|| error("missing migration parameters"))?,
    )
}
pub(super) fn submit(root: String, args: MigrateRequest) -> Result<Record, InstallError> {
    validate(&args)?;
    let _guard = queue_lock()?;
    let record = Record {
        schema_version: 9,
        id: uuid::Uuid::now_v7().to_string(),
        kind: Kind::Migrate,
        install_root: root,
        status: Status::Queued,
        transaction_id: None,
        message: None,
        assets: None,
        switch: None,
        restore: None,
        uninstall: None,
        reinit: None,
        install: None,
        migrate: Some(args),
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
    save(&record)?;
    Ok(record)
}
pub(crate) async fn submit_migrate_and_wait(
    root: String,
    args: MigrateRequest,
) -> Result<u8, InstallError> {
    crate::daemon::require_capability("migrate-v1.pid")?;
    wait_for_result(submit(root, args)?, "migrate-v1.pid", false).await
}
