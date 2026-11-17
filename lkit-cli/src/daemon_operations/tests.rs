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

fn network_tx(f: &Fixture) -> transaction::TransactionFile {
    let root = normalize_install_root(&f.dir.join("landscape")).unwrap();
    let mut tx =
        transaction::TransactionFile::new_install(&root, &semver::Version::new(1, 0, 0)).unwrap();
    tx.network_takeover = Some(transaction::NetworkTakeoverTransaction {
        boot_id: None,
        plan: crate::network::config::NetworkPlan {
            mode: crate::network::config::NetworkMode::WanDhcp { wan: "ens3".into() },
            selected_macs: vec![crate::network::config::SelectedInterface {
                name: "ens3".into(),
                mac: "02:00:00:00:00:03".into(),
            }],
        },
        host_services: vec![],
        confirmation_deadline: chrono::Utc::now() + chrono::Duration::minutes(5),
        rollback_service: "lkit-network-test-rollback.service".into(),
        rollback_timer: "lkit-network-test-rollback.timer".into(),
        boot_rollback_service: "lkit-network-test-boot.service".into(),
        recovery_binary: "service/lkit-network-recovery".into(),
        pending_state: format!(
            "transactions/{}/pending-install-state.json",
            tx.transaction_id
        ),
    });
    tx.phase = transaction::Phase::AwaitingNetworkConfirmation;
    tx
}

#[test]
fn network_queue_can_resolve_pending_business_operation_without_claiming_other_changes() {
    let f = Fixture::new();
    let ordinary = f.submit();
    claim().unwrap().unwrap();
    f.submit();
    let tx = network_tx(&f);
    let request = submit_network(
        tx.canonical_install_root.clone(),
        Kind::NetworkConfirm,
        tx.transaction_id.clone(),
    )
    .unwrap();
    assert!(claim().unwrap().is_none());
    let network = claim_kind(true).unwrap().unwrap();
    assert_eq!(network.id, request.id);
    assert!(claim_kind(true).unwrap().is_none());
    assert!(cancel(&network.id).is_err());
    assert_eq!(read(&ordinary.id).unwrap().status, Status::Running);
    assert!(claim().unwrap().is_none());
}

#[test]
fn network_receipts_use_the_requested_outcome_and_never_replay_unaccepted_confirmation() {
    for (kind, phase, expected) in [
        (
            Kind::NetworkConfirm,
            transaction::Phase::Committed,
            Status::Succeeded,
        ),
        (
            Kind::NetworkRollback,
            transaction::Phase::RolledBack,
            Status::Succeeded,
        ),
        (
            Kind::NetworkConfirm,
            transaction::Phase::RolledBack,
            Status::Failed,
        ),
        (
            Kind::NetworkRollback,
            transaction::Phase::Committed,
            Status::Failed,
        ),
        (
            Kind::NetworkConfirm,
            transaction::Phase::AwaitingNetworkConfirmation,
            Status::Interrupted,
        ),
        (
            Kind::NetworkConfirm,
            transaction::Phase::Finalizing,
            Status::Recovering,
        ),
        (
            Kind::NetworkRollback,
            transaction::Phase::RollingBack,
            Status::Recovering,
        ),
    ] {
        let f = Fixture::new();
        let mut tx = network_tx(&f);
        submit_network(
            tx.canonical_install_root.clone(),
            kind,
            tx.transaction_id.clone(),
        )
        .unwrap();
        let mut record = claim_kind(true).unwrap().unwrap();
        let mut wrong = tx.clone();
        wrong.transaction_id = uuid::Uuid::now_v7().to_string();
        assert!(associate(&mut record, &wrong).is_err());
        associate(&mut record, &tx).unwrap();
        tx.phase = phase;
        f.write_tx(&tx);
        reconcile(&mut record, None).unwrap();
        assert_eq!(
            record.status, expected,
            "{kind:?} / {phase:?}: {:?}",
            record.message
        );
        if expected == Status::Succeeded {
            assert_eq!(record.exit_code, Some(0));
        }
        assert!(claim_kind(true).unwrap().is_none());
    }
}

#[test]
fn network_payload_validation_and_queued_cancel_do_not_change_transaction() {
    let f = Fixture::new();
    let tx = network_tx(&f);
    f.write_tx(&tx);
    let before = std::fs::read(
        layout::territory_transactions_dir().join(format!("{}.json", tx.transaction_id)),
    )
    .unwrap();
    assert!(
        submit_network(
            tx.canonical_install_root.clone(),
            Kind::NetworkConfirm,
            "../bad".into()
        )
        .is_err()
    );
    let mut record = submit_network(
        tx.canonical_install_root.clone(),
        Kind::NetworkRollback,
        tx.transaction_id.clone(),
    )
    .unwrap();
    assert_eq!(cancel(&record.id).unwrap().status, Status::Cancelled);
    assert!(claim_kind(true).unwrap().is_none());
    record.schema_version = 5;
    save(&record).unwrap();
    assert!(read(&record.id).is_err());
    assert_eq!(
        std::fs::read(
            layout::territory_transactions_dir().join(format!("{}.json", tx.transaction_id))
        )
        .unwrap(),
        before
    );
}

fn reinit_request(f: &Fixture) -> ReinitRequest {
    ReinitRequest {
        from_version: "1.0.0".into(),
        state_identity: "a".repeat(64),
        admin_user: "admin".into(),
        network: network_tx(f).network_takeover.unwrap().plan,
        allow_no_backup: false,
    }
}

#[test]
fn reinit_password_is_private_consumed_once_and_removed_on_cancel_or_interruption() {
    let f = Fixture::new();
    let record = reinit::submit(
        f.dir.join("landscape").to_string_lossy().into_owned(),
        reinit_request(&f),
        "Secret123",
    )
    .unwrap();
    let orphan = reinit::input_path(&uuid::Uuid::now_v7().to_string()).unwrap();
    std::fs::write(&orphan, "orphan").unwrap();
    reinit::cleanup_orphan_inputs().unwrap();
    assert!(!orphan.exists());
    let secret = reinit::input_path(&record.id).unwrap();
    assert_eq!(
        std::fs::metadata(&secret).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(
        !std::fs::read_to_string(path(&record.id).unwrap())
            .unwrap()
            .contains("Secret123")
    );
    let mut record = claim().unwrap().unwrap();
    assert_eq!(
        reinit::take_password(&record, unsafe { libc::geteuid() }).unwrap(),
        "Secret123"
    );
    assert!(!secret.exists());
    assert!(reinit::take_password(&record, unsafe { libc::geteuid() }).is_err());
    reconcile(&mut record, None).unwrap();
    for cancel_it in [true, false] {
        let mut record = reinit::submit(
            f.dir.join("landscape").to_string_lossy().into_owned(),
            reinit_request(&f),
            "Secret123",
        )
        .unwrap();
        let secret = reinit::input_path(&record.id).unwrap();
        if cancel_it {
            cancel(&record.id).unwrap();
        } else {
            record = claim().unwrap().unwrap();
            reconcile(&mut record, None).unwrap();
        }
        assert!(!secret.exists());
    }
}

#[test]
fn reinit_waits_for_network_confirmation_and_updates_the_original_receipt_after_resolution() {
    for outcome in [
        transaction::Phase::Committed,
        transaction::Phase::RolledBack,
    ] {
        let f = Fixture::new();
        let request = reinit_request(&f);
        let root = normalize_install_root(&f.dir.join("landscape")).unwrap();
        reinit::submit(
            root.canonical.to_string_lossy().into_owned(),
            request,
            "Secret123",
        )
        .unwrap();
        let mut record = claim().unwrap().unwrap();
        let mut tx =
            transaction::TransactionFile::new_reinit(&root, &semver::Version::new(1, 0, 0))
                .unwrap();
        tx.schema_version = 11;
        tx.no_backup = true;
        tx.network_takeover = network_tx(&f).network_takeover;
        associate(&mut record, &tx).unwrap();
        tx.phase = transaction::Phase::AwaitingNetworkConfirmation;
        f.write_tx(&tx);
        record.language = Some(crate::i18n::Language::Zh);
        save(&record).unwrap();
        set_message(&record.id, "等待网络确认；请运行 lkit network confirm").unwrap();
        reconcile(&mut record, Some(Ok(()))).unwrap();
        assert_eq!(
            record.status,
            Status::AwaitingConfirmation,
            "{:?}",
            record.message
        );
        assert!(cancel(&record.id).is_err());
        f.submit();
        assert!(claim().unwrap().is_none());
        let confirm = submit_network(
            record.install_root.clone(),
            Kind::NetworkConfirm,
            tx.transaction_id.clone(),
        )
        .unwrap();
        assert_eq!(claim_kind(true).unwrap().unwrap().id, confirm.id);
        tx.phase = outcome;
        f.write_tx(&tx);
        reconcile(&mut record, None).unwrap();
        assert_eq!(
            record.exit_code,
            Some(if outcome == transaction::Phase::Committed {
                0
            } else {
                5
            })
        );
        assert!(record.status.terminal());
        assert!(
            !record
                .message
                .as_deref()
                .unwrap()
                .contains("network confirm")
        );
        assert_eq!(
            record.message.as_deref(),
            Some(if outcome == transaction::Phase::Committed {
                "操作已成功完成"
            } else {
                "操作已回滚，恢复已完成"
            })
        );
    }
}

fn first_install_request() -> InstallRequest {
    InstallRequest {
        target: "latest".into(),
        repository: crate::deployment::plan::RepositoryChoice::Http(
            "https://example.invalid/".into(),
        ),
        admin_user: "admin".into(),
        network: None,
    }
}
#[test]
fn install_download_cancellation_removes_only_staging_and_private_inputs() {
    let f = Fixture::new();
    let root = normalize_install_root(&f.dir.join("landscape")).unwrap();
    let cache = root.canonical.join("releases/1.0.0/static.zip");
    std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
    std::fs::write(&cache, b"cached").unwrap();
    let request = install::submit(
        root.canonical.to_string_lossy().into_owned(),
        first_install_request(),
        "Secret123",
    )
    .unwrap();
    let mut record = claim().unwrap().unwrap();
    resolve_switch_version(&record.id, "1.0.0").unwrap();
    let mut tx =
        transaction::TransactionFile::new_install(&root, &semver::Version::new(1, 0, 0)).unwrap();
    tx.schema_version = 12;
    associate(&mut record, &tx).unwrap();
    transaction::begin(&root, &tx).unwrap();
    let staged = layout::territory_transactions_dir()
        .join(&tx.transaction_id)
        .join("target/releases");
    std::fs::create_dir_all(&staged).unwrap();
    std::fs::write(staged.join("partial"), b"download").unwrap();
    set_stage(&record.id, Stage::Downloading).unwrap();
    cancel(&record.id).unwrap();
    reconcile(&mut record, Some(Err("cancelled".into()))).unwrap();
    assert_eq!(record.status, Status::Cancelled, "{:?}", record.message);
    assert!(!staged.exists());
    assert_eq!(std::fs::read(cache).unwrap(), b"cached");
    assert!(!reinit::input_path(&request.id).unwrap().exists());
}
#[test]
fn install_committed_receipt_does_not_replay_and_resolved_version_is_immutable() {
    let f = Fixture::new();
    let root = normalize_install_root(&f.dir.join("landscape")).unwrap();
    install::submit(
        root.canonical.to_string_lossy().into_owned(),
        first_install_request(),
        "Secret123",
    )
    .unwrap();
    let mut record = claim().unwrap().unwrap();
    resolve_switch_version(&record.id, "1.0.0").unwrap();
    assert!(resolve_switch_version(&record.id, "2.0.0").is_err());
    let mut tx =
        transaction::TransactionFile::new_install(&root, &semver::Version::new(1, 0, 0)).unwrap();
    tx.schema_version = 12;
    associate(&mut record, &tx).unwrap();
    tx.phase = transaction::Phase::Committed;
    f.write_tx(&tx);
    reconcile(&mut record, None).unwrap();
    assert_eq!(record.status, Status::Succeeded);
    assert!(!execute_next(|_| panic!("must not replay")).unwrap());
}

fn migration_request() -> MigrateRequest {
    MigrateRequest {
        source: SourceIdentity {
            source: "/legacy".into(),
            device: 1,
            inode: 2,
            pid: 123,
            started: 456,
            executable_sha256: "a".repeat(64),
            arguments_sha256: "b".repeat(64),
            unit: None,
            unit_sha256: None,
        },
        version: "1.0.0".into(),
        repository: crate::deployment::plan::RepositoryChoice::Http(
            "https://example.invalid/".into(),
        ),
    }
}
#[test]
fn migration_is_cancellable_only_while_queued_and_receipts_bind_the_target() {
    let f = Fixture::new();
    let root = normalize_install_root(&f.dir.join("landscape")).unwrap();
    let queued = migrate::submit(
        root.canonical.to_string_lossy().into_owned(),
        migration_request(),
    )
    .unwrap();
    cancel(&queued.id).unwrap();
    assert_eq!(read(&queued.id).unwrap().status, Status::Cancelled);
    migrate::submit(
        root.canonical.to_string_lossy().into_owned(),
        migration_request(),
    )
    .unwrap();
    let mut active = claim().unwrap().unwrap();
    assert!(cancel(&active.id).is_err());
    let mut tx =
        transaction::TransactionFile::new_migrate(&root, &semver::Version::new(2, 0, 0)).unwrap();
    tx.schema_version = 13;
    assert!(associate(&mut active, &tx).is_err());
    tx.target_version = Some("1.0.0".into());
    tx.target_release = Some("releases/1.0.0".into());
    associate(&mut active, &tx).unwrap();
    tx.backup = Some(transaction::BackupRef {
        backup_id: "backup".into(),
        path: "backups/test.lkb".into(),
        sha256: "c".repeat(64),
    });
    tx.phase = transaction::Phase::Committed;
    f.write_tx(&tx);
    reconcile(&mut active, None).unwrap();
    assert_eq!(active.status, Status::Succeeded);
    assert!(!execute_next(|_| panic!("migration must not replay")).unwrap());
}
#[test]
fn migration_rejects_unbound_unit_and_foreign_payloads() {
    let f = Fixture::new();
    let mut request = migration_request();
    request.source.unit = Some("legacy.service".into());
    assert!(migrate::submit(f.dir.to_string_lossy().into_owned(), request).is_err());
    let mut record =
        migrate::submit(f.dir.to_string_lossy().into_owned(), migration_request()).unwrap();
    record.install = Some(first_install_request());
    save(&record).unwrap();
    assert!(read(&record.id).is_err());
}

#[test]
fn operation_language_is_capability_gated_and_scoped_to_execution() {
    let f = Fixture::new();
    let old = crate::i18n::with_language(crate::i18n::Language::Zh, || f.submit());
    assert!(old.language.is_none());
    assert!(
        !std::fs::read_to_string(path(&old.id).unwrap())
            .unwrap()
            .contains("language")
    );
    cancel(&old.id).unwrap();
    let pid = std::process::id().to_string();
    std::fs::write(layout::territory_pidfile(), &pid).unwrap();
    std::fs::write(layout::territory_run_dir().join("operations-v3.pid"), &pid).unwrap();
    let queued = crate::i18n::with_language(crate::i18n::Language::Zh, || f.submit());
    assert_eq!(queued.language, Some(crate::i18n::Language::Zh));
    crate::i18n::with_language(crate::i18n::Language::En, || {
        execute_next(|record| {
            assert_eq!(crate::i18n::current(), crate::i18n::Language::Zh);
            set_message(
                &record.id,
                &crate::tr!(crate::keys::INSTALL_NATIVE_COMPLETE, version = "1.0.0"),
            )?;
            Ok(())
        })
        .unwrap();
        assert_eq!(crate::i18n::current(), crate::i18n::Language::En);
    });
    assert_eq!(read(&queued.id).unwrap().status, Status::Succeeded);
    assert!(
        read(&queued.id)
            .unwrap()
            .message
            .unwrap()
            .contains("已安装")
    );
    let mut value = serde_json::to_value(&queued).unwrap();
    value["language"] = "invalid".into();
    assert!(serde_json::from_value::<Record>(value).is_err());
}

#[test]
fn recovery_retry_replaces_obsolete_errors_only_when_recovery_finishes() {
    let f = Fixture::new();
    f.submit();
    let mut record = claim().unwrap().unwrap();
    record.language = Some(crate::i18n::Language::En);
    save(&record).unwrap();
    let mut tx = f.transaction(&mut record);
    tx.phase = transaction::Phase::RollingBack;
    f.write_tx(&tx);
    reconcile(&mut record, Some(Err("restore service failed".into()))).unwrap();
    assert_eq!(record.status, Status::Recovering);
    assert_eq!(record.message.as_deref(), Some("restore service failed"));
    reconcile(&mut record, None).unwrap();
    assert_eq!(record.message.as_deref(), Some("restore service failed"));
    tx.phase = transaction::Phase::RolledBack;
    f.write_tx(&tx);
    reconcile(&mut record, None).unwrap();
    assert_eq!(
        record.message.as_deref(),
        Some("Operation rolled back; recovery completed")
    );
    assert_eq!(record.status, Status::Failed);
}
