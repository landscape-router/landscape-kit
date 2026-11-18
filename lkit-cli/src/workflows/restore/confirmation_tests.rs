use super::*;
use crate::network::recovery::{Action, action, startup_check_at};
use std::path::PathBuf;

const ORIGINAL_BOOT: &str = "11111111-1111-4111-8111-111111111111";
const REBOOTED: &str = "22222222-2222-4222-8222-222222222222";

struct Fixture {
    root: InstallRoot,
    directory: PathBuf,
    tx: TransactionFile,
    _territory: layout::TerritoryOverride,
}

impl Fixture {
    fn new() -> Self {
        let directory =
            std::env::temp_dir().join(format!("lkit-restore-confirm-{}", uuid::Uuid::now_v7()));
        let path = directory.join("install");
        std::fs::create_dir_all(&path).unwrap();
        let territory = layout::test_territory(&directory.join("territory"));
        let root = InstallRoot {
            install_root: path.clone(),
            canonical: path,
        };
        let version = semver::Version::new(1, 2, 3);
        let mut tx = TransactionFile::new_restore(&root, &version, &version).unwrap();
        tx.schema_version = 14;
        tx.phase = Phase::AwaitingNetworkConfirmation;
        tx.no_backup = true;
        tx.restore_backup = Some(transaction::BackupRef {
            backup_id: "20261117-120000-12345678".into(),
            path: "backups/20261117-120000-12345678.lkb".into(),
            sha256: "a".repeat(64),
        });
        tx.restore_confirmation = Some(RestoreConfirmation {
            boot_id: ORIGINAL_BOOT.into(),
            confirmation_deadline: now() + chrono::Duration::minutes(10),
            pending_state: format!(
                "transactions/{}/pending-restore-state.json",
                tx.transaction_id
            ),
        });
        Self {
            root,
            directory,
            tx,
            _territory: territory,
        }
    }

    fn persist(&self) {
        transaction::persist(&self.root, &self.tx).unwrap();
    }

    fn permit(&self) -> PathBuf {
        layout::territory_transactions_dir()
            .join(&self.tx.transaction_id)
            .join("restored-boot-id")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

fn now() -> chrono::DateTime<Utc> {
    chrono::DateTime::parse_from_rfc3339("2026-11-17T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

#[test]
fn guarded_restore_schema_rejects_unbound_paths_boots_and_transaction_ids() {
    let fixture = Fixture::new();
    transaction::validate_transaction(&fixture.tx).unwrap();
    let guard = fixture.tx.restore_confirmation.as_ref().unwrap();
    assert_eq!(
        fixture.tx.confirmation_window(),
        Some((guard.confirmation_deadline, Some(ORIGINAL_BOOT)))
    );
    assert!(fixture.tx.is_native_restore());

    let mut invalid = Vec::new();
    let mut tx = fixture.tx.clone();
    tx.restore_confirmation.as_mut().unwrap().boot_id = "unknown-boot".into();
    invalid.push(tx);
    for path in [
        "../outside",
        "transactions/another/pending-restore-state.json",
        "/tmp/pending.json",
    ] {
        let mut tx = fixture.tx.clone();
        tx.restore_confirmation.as_mut().unwrap().pending_state = path.into();
        invalid.push(tx);
    }
    let mut tx = fixture.tx.clone();
    tx.transaction_id = "../outside".into();
    tx.log_path = format!("logs/{}.log", tx.transaction_id);
    tx.restore_confirmation.as_mut().unwrap().pending_state = format!(
        "transactions/{}/pending-restore-state.json",
        tx.transaction_id
    );
    invalid.push(tx);
    let mut tx = fixture.tx.clone();
    tx.schema_version = 9;
    invalid.push(tx);
    let mut tx = fixture.tx.clone();
    tx.restore_confirmation = None;
    invalid.push(tx);
    let mut tx = fixture.tx.clone();
    tx.operation = transaction::Operation::Reinit;
    invalid.push(tx);
    for tx in invalid {
        assert!(
            transaction::validate_transaction(&tx).is_err(),
            "unexpectedly accepted {tx:?}"
        );
    }

    // Missing guards remain valid for existing schema-v9 recovery records.
    let mut legacy = fixture.tx.clone();
    legacy.schema_version = 9;
    legacy.phase = Phase::Verifying;
    legacy.restore_confirmation = None;
    transaction::validate_transaction(&legacy).unwrap();
    assert_eq!(legacy.confirmation_window(), None);
    for phase in [Phase::AwaitingNetworkConfirmation, Phase::Finalizing] {
        legacy.phase = phase;
        assert!(transaction::validate_transaction(&legacy).is_err());
    }
}

#[test]
fn restore_deadline_and_reboot_rollback_but_accepted_confirmation_finishes() {
    let mut fixture = Fixture::new();
    let deadline = fixture
        .tx
        .restore_confirmation
        .as_ref()
        .unwrap()
        .confirmation_deadline;
    assert_eq!(action(&fixture.tx, now(), ORIGINAL_BOOT), Action::Wait);
    assert_eq!(
        action(&fixture.tx, deadline, ORIGINAL_BOOT),
        Action::Rollback
    );
    assert_eq!(action(&fixture.tx, now(), REBOOTED), Action::Rollback);

    fixture.tx.phase = Phase::Finalizing;
    assert_eq!(
        action(&fixture.tx, deadline + chrono::Duration::days(1), REBOOTED),
        Action::Confirm
    );
    fixture.tx.phase = Phase::RollingBack;
    assert_eq!(action(&fixture.tx, now(), ORIGINAL_BOOT), Action::Rollback);
    for phase in [Phase::Committed, Phase::RolledBack, Phase::Failed] {
        fixture.tx.phase = phase;
        assert_eq!(action(&fixture.tx, deadline, REBOOTED), Action::Wait);
    }
}

#[test]
fn restored_data_startup_requires_a_permit_for_the_current_boot() {
    let mut fixture = Fixture::new();
    fixture.persist();
    startup_check_at(ORIGINAL_BOOT, now()).unwrap();
    assert!(startup_check_at(REBOOTED, now()).is_err());
    let deadline = fixture
        .tx
        .restore_confirmation
        .as_ref()
        .unwrap()
        .confirmation_deadline;
    assert!(startup_check_at(ORIGINAL_BOOT, deadline).is_err());

    fixture.tx.phase = Phase::RollingBack;
    fixture.persist();
    assert!(startup_check_at(ORIGINAL_BOOT, now()).is_err());
    let permit = fixture.permit();
    std::fs::create_dir_all(permit.parent().unwrap()).unwrap();
    std::fs::write(&permit, ORIGINAL_BOOT).unwrap();
    assert!(startup_check_at(REBOOTED, now()).is_err());
    std::fs::write(&permit, REBOOTED).unwrap();
    startup_check_at(REBOOTED, deadline).unwrap();
    revoke_restored_start(&fixture.tx).unwrap();
    assert!(startup_check_at(REBOOTED, deadline).is_err());
    revoke_restored_start(&fixture.tx).unwrap();
    std::fs::write(&permit, REBOOTED).unwrap();

    // A permit does not override an expired window before rollback has restored data.
    fixture.tx.phase = Phase::AwaitingNetworkConfirmation;
    fixture.persist();
    assert!(startup_check_at(REBOOTED, deadline).is_err());

    // Finalizing persists the accepted decision and is completed after reboot.
    fixture.tx.phase = Phase::Finalizing;
    fixture.persist();
    std::fs::remove_file(permit).unwrap();
    startup_check_at(REBOOTED, deadline).unwrap();
}

#[test]
fn data_recovery_steps_preserve_original_data_when_rollback_is_interrupted() {
    let mut fixture = Fixture::new();
    std::fs::create_dir(fixture.root.canonical.join("releases")).unwrap();
    transaction::workspace::select(&mut fixture.tx);
    fixture.persist();
    let data = fixture.root.canonical.join("data");
    std::fs::create_dir(&data).unwrap();
    std::fs::write(data.join("old-marker"), b"original data").unwrap();
    std::fs::create_dir_all(layout::territory_transactions_dir().join(&fixture.tx.transaction_id))
        .unwrap();
    prepare_data_recovery(&fixture.root, &fixture.tx).unwrap();
    assert!(data_was_original(&fixture.root, &fixture.tx).unwrap());
    // Recovery before staging keeps the original data without requiring a workspace.
    sync_data_directories(&fixture.root, &fixture.tx).unwrap();

    // The normal restore stages the original directory before populating its target.
    let workspace = transaction::workspace::prepare(&fixture.root, &fixture.tx).unwrap();
    let previous = workspace.join("previous-data");
    std::fs::rename(&data, &previous).unwrap();
    std::fs::create_dir(&data).unwrap();
    record_data_saved(&fixture.root, &fixture.tx).unwrap();
    std::fs::write(data.join("target-config"), b"restored backup").unwrap();
    assert!(directory_exists(&previous).unwrap());
    assert!(!data_was_original(&fixture.root, &fixture.tx).unwrap());

    // Persist rollback progress before consuming previous-data. An interruption
    // before this rename still leaves the staged original available for retry.
    begin_data_restore(&fixture.tx).unwrap();
    assert!(directory_exists(&previous).unwrap());
    std::fs::remove_dir_all(&data).unwrap();
    std::fs::rename(&previous, &data).unwrap();
    sync_data_directories(&fixture.root, &fixture.tx).unwrap();

    // A retry after rename, before state/permit completion, keeps the restored data.
    for _ in 0..2 {
        assert!(data_was_original(&fixture.root, &fixture.tx).unwrap());
        assert!(!directory_exists(&previous).unwrap());
        assert_eq!(
            std::fs::read(data.join("old-marker")).unwrap(),
            b"original data"
        );
        assert!(!data.join("target-config").exists());
    }
}
