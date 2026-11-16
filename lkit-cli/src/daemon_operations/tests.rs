use super::*;
use crate::deployment::root::normalize_install_root;

struct Fixture {
    _guard: layout::TerritoryOverride,
    dir: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("lkit-operations-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        Self {
            _guard: layout::test_territory(&dir.join("territory")),
            dir,
        }
    }
    fn submit(&self) -> Record {
        submit(self.dir.join("landscape").to_string_lossy().into_owned()).unwrap()
    }
    fn transaction(&self, record: &mut Record) -> transaction::TransactionFile {
        let root = normalize_install_root(&self.dir.join("landscape")).unwrap();
        let mut tx =
            transaction::TransactionFile::new_install(&root, &semver::Version::new(1, 0, 0))
                .unwrap();
        tx.schema_version = 6;
        tx.operation = transaction::Operation::RepairService;
        tx.target_release = None;
        tx.systemd_before = Some(crate::service::manager::ServiceBefore {
            active: false,
            enabled: false,
            registration: crate::service::manager::Registration {
                kind: crate::service::manager::RegistrationKind::Missing,
                target: None,
            },
        });
        associate(record, &tx).unwrap();
        tx
    }
    fn write_tx(&self, tx: &transaction::TransactionFile) {
        std::fs::create_dir_all(layout::territory_transactions_dir()).unwrap();
        std::fs::write(
            layout::territory_transactions_dir().join(format!("{}.json", tx.transaction_id)),
            serde_json::to_vec(tx).unwrap(),
        )
        .unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn fifo_cancellation_and_terminal_records_are_never_executed_twice() {
    let f = Fixture::new();
    let first = f.submit();
    let second = f.submit();
    let third = f.submit();
    assert_eq!(cancel(&second.id).unwrap().status, Status::Cancelled);
    let mut calls = vec![];
    for _ in 0..4 {
        execute_next(|r| {
            calls.push(r.id.clone());
            Ok(())
        })
        .unwrap();
    }
    assert_eq!(calls, vec![first.id.clone(), third.id.clone()]);
    assert_eq!(read(&first.id).unwrap().status, Status::Succeeded);
    assert_eq!(read(&second.id).unwrap().status, Status::Cancelled);
    assert!(cancel(&first.id).is_err());
    assert_eq!(
        std::fs::metadata(directory()).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(path(&first.id).unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}
#[test]
fn running_operation_blocks_claim_and_rejects_cancel_but_allows_queries() {
    let f = Fixture::new();
    let first = f.submit();
    f.submit();
    let record = claim().unwrap().unwrap();
    assert_eq!(record.id, first.id);
    assert!(claim().unwrap().is_none());
    assert!(cancel(&first.id).is_err());
    let _lock = lock::acquire_install_lock().unwrap();
    assert_eq!(list().unwrap().len(), 2);
    assert_eq!(read(&first.id).unwrap().status, Status::Running);
}
#[test]
fn crash_before_transaction_is_interrupted_without_replay() {
    let f = Fixture::new();
    f.submit();
    let mut record = claim().unwrap().unwrap();
    reconcile(&mut record, None).unwrap();
    assert_eq!(record.status, Status::Interrupted);
    assert!(!execute_next(|_| panic!("must not replay")).unwrap());
}
#[test]
fn association_is_durable_before_transaction_exists_and_missing_transaction_is_interrupted() {
    let f = Fixture::new();
    f.submit();
    let mut record = claim().unwrap().unwrap();
    let tx = f.transaction(&mut record);
    assert_eq!(
        read(&record.id).unwrap().transaction_id.as_deref(),
        Some(tx.transaction_id.as_str())
    );
    reconcile(&mut record, None).unwrap();
    assert_eq!(record.status, Status::Interrupted);
}
#[test]
fn crash_after_commit_or_rollback_uses_transaction_and_never_replays() {
    for (phase, expected) in [
        (transaction::Phase::Committed, Status::Succeeded),
        (transaction::Phase::RolledBack, Status::Failed),
        (transaction::Phase::Failed, Status::Failed),
    ] {
        let f = Fixture::new();
        f.submit();
        let mut record = claim().unwrap().unwrap();
        let mut tx = f.transaction(&mut record);
        tx.phase = phase;
        f.write_tx(&tx);
        reconcile(&mut record, None).unwrap();
        assert_eq!(record.status, expected, "{:?}", record.message);
        assert!(!execute_next(|_| panic!("must not replay")).unwrap());
    }
}
#[test]
fn unfinished_transaction_blocks_new_work_until_recovery_commits() {
    let f = Fixture::new();
    f.submit();
    let mut record = claim().unwrap().unwrap();
    f.submit();
    let mut tx = f.transaction(&mut record);
    f.write_tx(&tx);
    reconcile(&mut record, None).unwrap();
    assert_eq!(record.status, Status::Recovering);
    assert!(!execute_next(|_| panic!("must wait for recovery")).unwrap());
    tx.phase = transaction::Phase::Committed;
    f.write_tx(&tx);
    reconcile(&mut record, None).unwrap();
    assert_eq!(record.status, Status::Succeeded);
    let mut count = 0;
    assert!(
        execute_next(|_| {
            count += 1;
            Ok(())
        })
        .unwrap()
    );
    assert_eq!(count, 1);
}
#[test]
fn corrupt_or_foreign_transaction_cannot_be_reported_as_success() {
    let f = Fixture::new();
    f.submit();
    let mut record = claim().unwrap().unwrap();
    let mut tx = f.transaction(&mut record);
    tx.phase = transaction::Phase::Committed;
    tx.canonical_install_root = "/another/installation".into();
    f.write_tx(&tx);
    reconcile(&mut record, None).unwrap();
    assert_eq!(record.status, Status::Recovering);
    let path = layout::territory_transactions_dir().join(format!("{}.json", tx.transaction_id));
    std::fs::write(path, "broken").unwrap();
    reconcile(&mut record, None).unwrap();
    assert_eq!(record.status, Status::Recovering);
    assert!(record.message.as_deref().unwrap().contains("unreadable"));
    assert!(claim().unwrap().is_none());
}
#[test]
fn live_failure_is_terminal_and_committed_evidence_wins_over_returned_error() {
    let f = Fixture::new();
    let first = f.submit();
    execute_next(|_| Err(error("preflight rejected"))).unwrap();
    assert_eq!(read(&first.id).unwrap().status, Status::Failed);
    let second = f.submit();
    execute_next(|r| {
        let mut tx = f.transaction(r);
        tx.phase = transaction::Phase::Committed;
        f.write_tx(&tx);
        Err(error("late failure"))
    })
    .unwrap();
    assert_eq!(read(&second.id).unwrap().status, Status::Succeeded);
}
#[test]
fn rejects_traversal_and_corrupt_records_without_claiming_other_work() {
    let f = Fixture::new();
    let record = f.submit();
    f.submit();
    assert!(read("../state").is_err());
    assert!(cancel("../state").is_err());
    std::fs::write(path(&record.id).unwrap(), b"broken").unwrap();
    assert!(claim().is_err());
}

fn asset_request() -> AssetRequest {
    AssetRequest {
        version: "1.2.3".into(),
        state_identity: "a".repeat(64),
        repository: None,
        official: false,
    }
}
fn queued_asset(f: &Fixture, kind: Kind) -> Record {
    submit_request(
        f.dir.join("landscape").to_string_lossy().into_owned(),
        kind,
        Some(asset_request()),
    )
    .unwrap()
}
fn preparing_asset(f: &Fixture, record: &mut Record) -> transaction::TransactionFile {
    let root = normalize_install_root(&f.dir.join("landscape")).unwrap();
    let mut tx = transaction::TransactionFile::new_repair_static(&root).unwrap();
    tx.schema_version = 7;
    associate(record, &tx).unwrap();
    transaction::begin(&root, &tx).unwrap();
    tx
}
#[test]
fn typed_requests_validate_payloads_and_preserve_repository_choices() {
    use crate::deployment::plan::RepositoryChoice;
    let f = Fixture::new();
    assert!(submit_request(f.dir.to_string_lossy().into_owned(), Kind::Binary, None).is_err());
    assert!(validate_request(Kind::Service, Some(&asset_request())).is_err());
    let mut invalid = asset_request();
    invalid.official = true;
    assert!(validate_request(Kind::Binary, Some(&invalid)).is_err());
    for choice in [
        None,
        Some(RepositoryChoice::Mirror),
        Some(RepositoryChoice::Github(
            "landscape-router/landscape".into(),
        )),
        Some(RepositoryChoice::Http(
            "https://example.com/repository".into(),
        )),
    ] {
        let request = AssetRequest {
            repository: choice.clone(),
            ..asset_request()
        };
        let record = submit_request(
            f.dir.to_string_lossy().into_owned(),
            Kind::Static,
            Some(request),
        )
        .unwrap();
        assert_eq!(read(&record.id).unwrap().assets.unwrap().repository, choice);
    }
    assert!(
        serde_json::from_str::<AssetRequest>(
            r#"{"version":"1.2.3","repository":null,"official":false,"argv":["shell"]}"#
        )
        .is_err()
    );
}
#[test]
fn cancellation_wins_before_preparation_and_cannot_be_overwritten() {
    let f = Fixture::new();
    queued_asset(&f, Kind::Static);
    let mut record = claim().unwrap().unwrap();
    let tx = preparing_asset(&f, &mut record);
    set_stage(&record.id, Stage::Downloading).unwrap();
    let accepted = cancel(&record.id).unwrap();
    assert!(accepted.cancel_requested);
    assert_eq!(accepted.status, Status::Running);
    assert!(set_stage(&record.id, Stage::Preparing).is_err());
    let dir = layout::territory_transactions_dir().join(&tx.transaction_id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("partial"), "partial").unwrap();
    reconcile(&mut record, None).unwrap();
    assert_eq!(record.status, Status::Cancelled);
    assert_eq!(record.exit_code, Some(130));
    assert!(record.cancel_cleaned);
    assert!(!dir.exists());
    assert_eq!(cancel(&record.id).unwrap().status, Status::Cancelled);
    assert!(!execute_next(|_| panic!("cancelled request must not execute")).unwrap());
}
#[test]
fn preparation_wins_before_cancel_and_terminal_progress_is_immutable() {
    let f = Fixture::new();
    queued_asset(&f, Kind::Static);
    let mut record = claim().unwrap().unwrap();
    set_stage(&record.id, Stage::Downloading).unwrap();
    set_progress(&record.id, 5, 10).unwrap();
    let downloading = read(&record.id).unwrap();
    assert_eq!(downloading.downloaded_bytes, 5);
    assert!(downloading.cancellable);
    set_stage(&record.id, Stage::Preparing).unwrap();
    assert!(cancel(&record.id).is_err());
    reconcile(
        &mut record,
        Some(Err("download complete; preflight rejected".into())),
    )
    .unwrap();
    let before = std::fs::read(path(&record.id).unwrap()).unwrap();
    set_progress(&record.id, 10, 10).unwrap();
    assert!(set_stage(&record.id, Stage::Downloading).is_err());
    assert_eq!(std::fs::read(path(&record.id).unwrap()).unwrap(), before);
    assert!(record.revision > downloading.revision);
}
#[test]
fn interrupted_cancel_retries_cleanup_and_cannot_hide_modified_transactions() {
    let f = Fixture::new();
    queued_asset(&f, Kind::Static);
    let mut record = claim().unwrap().unwrap();
    let mut tx = preparing_asset(&f, &mut record);
    set_stage(&record.id, Stage::Downloading).unwrap();
    cancel(&record.id).unwrap();
    let dir = layout::territory_transactions_dir().join(&tx.transaction_id);
    std::fs::write(&dir, "cannot remove as directory").unwrap();
    reconcile(&mut record, None).unwrap();
    assert_eq!(record.status, Status::Recovering);
    assert!(!record.cancel_cleaned);
    assert!(claim().unwrap().is_none());
    std::fs::remove_file(&dir).unwrap();
    tx.phase = transaction::Phase::Activating;
    tx.static_backup = Some(transaction::StaticBackupRef {
        path: format!("transactions/{}/static-backup", tx.transaction_id),
        target: "releases/1.2.3/static".into(),
    });
    f.write_tx(&tx);
    reconcile(&mut record, None).unwrap();
    assert_eq!(record.status, Status::Recovering);
    tx.phase = transaction::Phase::Preparing;
    f.write_tx(&tx);
    reconcile(&mut record, None).unwrap();
    assert_eq!(record.status, Status::Cancelled);
}
#[tokio::test]
async fn cooperative_download_cancellation_drops_the_pending_future() {
    let f = Fixture::new();
    let record = queued_asset(&f, Kind::Static);
    claim().unwrap().unwrap();
    let _control = crate::workflows::operation_control::enter(&record.id);
    let (result, ()) = tokio::join!(
        crate::workflows::operation_control::download(std::future::pending::<
            Result<(), InstallError>,
        >()),
        async {
            tokio::task::yield_now().await;
            cancel(&record.id).unwrap();
        }
    );
    assert!(result.is_err());
    assert!(set_stage(&record.id, Stage::Preparing).is_err());
}

#[test]
fn committed_asset_transaction_is_reported_once_without_reexecuting() {
    let f = Fixture::new();
    queued_asset(&f, Kind::Static);
    let mut record = claim().unwrap().unwrap();
    let mut tx = preparing_asset(&f, &mut record);
    tx.static_backup = Some(transaction::StaticBackupRef {
        path: format!("transactions/{}/static-backup", tx.transaction_id),
        target: "releases/1.2.3/static".into(),
    });
    tx.phase = transaction::Phase::Committed;
    f.write_tx(&tx);
    reconcile(&mut record, None).unwrap();
    assert_eq!(record.status, Status::Succeeded);
    assert_eq!(record.exit_code, Some(0));
    assert!(!execute_next(|_| panic!("must not execute committed repair")).unwrap());
}

fn queued_switch(f: &Fixture, target: &str) -> Record {
    submit_operation(
        f.dir.join("landscape").to_string_lossy().into_owned(),
        Kind::Switch,
        None,
        Some(SwitchRequest {
            from_version: "1.0.0".into(),
            state_identity: "a".repeat(64),
            target: target.into(),
            repository: None,
            accept_service_change: false,
            allow_no_backup: false,
        }),
    )
    .unwrap()
}
#[test]
fn switch_request_binds_resolved_target_and_rejects_wrong_transactions() {
    let f = Fixture::new();
    let queued = queued_switch(&f, "latest");
    let mut record = claim().unwrap().unwrap();
    resolve_switch_version(&record.id, "2.0.0").unwrap();
    assert!(resolve_switch_version(&record.id, "3.0.0").is_err());
    let root = normalize_install_root(&f.dir.join("landscape")).unwrap();
    let mut tx = transaction::TransactionFile::new_switch(
        &root,
        &semver::Version::new(1, 0, 0),
        &semver::Version::new(2, 0, 0),
    )
    .unwrap();
    assert!(associate(&mut record, &tx).is_err());
    tx.schema_version = 8;
    tx.target_version = Some("3.0.0".into());
    assert!(associate(&mut record, &tx).is_err());
    tx.target_version = Some("2.0.0".into());
    associate(&mut record, &tx).unwrap();
    assert_eq!(read(&queued.id).unwrap().schema_version, 3);
    assert_eq!(
        read(&queued.id).unwrap().transaction_id,
        Some(tx.transaction_id)
    );
}
#[test]
fn switch_download_cancel_cleans_staging_without_removing_cached_release() {
    let f = Fixture::new();
    let queued = queued_switch(&f, "2.0.0");
    let mut record = claim().unwrap().unwrap();
    resolve_switch_version(&record.id, "2.0.0").unwrap();
    let root = normalize_install_root(&f.dir.join("landscape")).unwrap();
    let mut tx = transaction::TransactionFile::new_switch(
        &root,
        &semver::Version::new(1, 0, 0),
        &semver::Version::new(2, 0, 0),
    )
    .unwrap();
    tx.schema_version = 8;
    associate(&mut record, &tx).unwrap();
    transaction::begin(&root, &tx).unwrap();
    let staging = layout::territory_transactions_dir().join(&tx.transaction_id);
    std::fs::create_dir_all(&staging).unwrap();
    let cached = root.canonical.join("releases/2.0.0");
    std::fs::create_dir_all(&cached).unwrap();
    std::fs::write(cached.join("keep"), b"cache").unwrap();
    set_stage(&queued.id, Stage::Downloading).unwrap();
    cancel(&queued.id).unwrap();
    assert!(set_stage(&queued.id, Stage::Preparing).is_err());
    reconcile(&mut record, None).unwrap();
    assert_eq!(record.status, Status::Cancelled);
    assert_eq!(record.exit_code, Some(130));
    assert!(!staging.exists());
    assert_eq!(std::fs::read(cached.join("keep")).unwrap(), b"cache");
}
#[test]
fn switch_committed_evidence_recovers_success_without_reexecution() {
    let f = Fixture::new();
    let queued = queued_switch(&f, "2.0.0");
    let mut record = claim().unwrap().unwrap();
    resolve_switch_version(&record.id, "2.0.0").unwrap();
    let root = normalize_install_root(&f.dir.join("landscape")).unwrap();
    let mut tx = transaction::TransactionFile::new_switch(
        &root,
        &semver::Version::new(1, 0, 0),
        &semver::Version::new(2, 0, 0),
    )
    .unwrap();
    tx.schema_version = 8;
    tx.no_backup = true;
    tx.phase = transaction::Phase::Committed;
    associate(&mut record, &tx).unwrap();
    f.write_tx(&tx);
    reconcile(&mut record, None).unwrap();
    assert_eq!(read(&queued.id).unwrap().status, Status::Succeeded);
    assert_eq!(record.exit_code, Some(0));
    assert!(!execute_next(|_| panic!("committed switch replayed")).unwrap());
}

#[test]
fn restore_payload_rejects_unbound_inputs_and_queued_cancel_needs_no_backup_io() {
    let temp = std::env::temp_dir().join(format!("lkit-restore-request-{}", uuid::Uuid::now_v7()));
    let _guard = layout::test_territory(&temp);
    let request = RestoreRequest {
        from_version: "1.3.0".into(),
        state_identity: "a".repeat(64),
        source: temp.join("selected.lkb"),
        backup_id: "20260807-131500-ab12cd34".into(),
        backup_sha256: "b".repeat(64),
        target_version: "1.2.3".into(),
        allow_no_backup: false,
    };
    let mut invalid = request.clone();
    invalid.source = "relative.lkb".into();
    assert!(validate_restore(&invalid).is_err());
    invalid = request.clone();
    invalid.backup_sha256.clear();
    assert!(validate_restore(&invalid).is_err());
    let record = submit_restore(temp.join("install").display().to_string(), request).unwrap();
    assert_eq!(read(&record.id).unwrap().schema_version, 4);
    assert_eq!(cancel(&record.id).unwrap().status, Status::Cancelled);
    assert!(!temp.join("selected.lkb").exists());
    let _ = std::fs::remove_dir_all(temp);
}

#[test]
fn uninstall_receipt_binds_keep_data_and_survives_installation_state_removal() {
    let temp =
        std::env::temp_dir().join(format!("lkit-uninstall-request-{}", uuid::Uuid::now_v7()));
    let _guard = layout::test_territory(&temp);
    let root = crate::deployment::root::normalize_install_root(&temp.join("install")).unwrap();
    let request = UninstallRequest {
        from_version: "1.2.3".into(),
        state_identity: "a".repeat(64),
        allow_no_backup: true,
        keep_data: true,
    };
    let record = submit_uninstall(root.canonical.display().to_string(), request).unwrap();
    let mut claimed = claim().unwrap().unwrap();
    assert_eq!(record.id, claimed.id);
    let mut tx =
        transaction::TransactionFile::new_uninstall(&root, &semver::Version::new(1, 2, 3)).unwrap();
    tx.schema_version = 10;
    tx.no_backup = true;
    assert!(
        associate_id(&record.id, &tx).is_err(),
        "keep-data mismatch must fail before mutation"
    );
    tx.uninstall_keep_data = true;
    associate_id(&record.id, &tx).unwrap();
    transaction::begin(&root, &tx).unwrap();
    transaction::mark_phase(&root, &tx, transaction::Phase::Committed).unwrap();
    transaction::purge_root_except(&root, Some(&tx.transaction_id)).unwrap();
    reconcile(&mut claimed, None).unwrap();
    assert_eq!(read(&record.id).unwrap().status, Status::Succeeded);
    assert!(!layout::territory_state_path().exists());
    let _ = std::fs::remove_dir_all(temp);
}
