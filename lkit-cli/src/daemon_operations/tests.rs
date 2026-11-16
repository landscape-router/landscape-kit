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
