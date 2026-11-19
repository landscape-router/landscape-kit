//! Shared daemon recovery decisions and the service startup barrier.
use crate::deployment::{
    layout,
    plan::InstallError,
    runtime::InstallRuntime,
    transaction::{self, Phase, TransactionFile},
};
use chrono::{DateTime, Utc};

#[derive(Debug, PartialEq)]
pub(crate) enum Action {
    Wait,
    Confirm,
    Rollback,
}

pub(crate) fn action(tx: &TransactionFile, now: DateTime<Utc>, boot_id: &str) -> Action {
    let Some((deadline, saved_boot)) = tx.confirmation_window() else {
        return Action::Wait;
    };
    match tx.phase {
        Phase::Finalizing => Action::Confirm,
        Phase::RollingBack => Action::Rollback,
        Phase::AwaitingNetworkConfirmation
            if now >= deadline || saved_boot.is_some_and(|saved| saved != boot_id) =>
        {
            Action::Rollback
        }
        Phase::AwaitingNetworkConfirmation => Action::Wait,
        _ if tx.operation == transaction::Operation::Reinit && !tx.phase.is_terminal() => {
            Action::Rollback
        }
        _ => Action::Wait,
    }
}

pub(crate) async fn recover(
    root: &crate::deployment::root::InstallRoot,
    tx: &TransactionFile,
    runtime: &InstallRuntime,
) -> Result<(), InstallError> {
    match action(tx, Utc::now(), &super::takeover::current_boot_id()?) {
        Action::Wait => Ok(()),
        Action::Confirm => super::takeover::confirm(root, runtime).await,
        Action::Rollback => {
            if !matches!(
                tx.phase,
                Phase::AwaitingNetworkConfirmation | Phase::Finalizing | Phase::RollingBack
            ) {
                transaction::mark_phase(root, tx, Phase::RollingBack)?;
            }
            super::takeover::rollback(root, runtime, true).await
        }
    }
}

pub(crate) fn startup_check() -> Result<(), InstallError> {
    startup_check_at(&super::takeover::current_boot_id()?, Utc::now())
}

#[cfg(test)]
fn startup_check_for(boot: &str) -> Result<(), InstallError> {
    startup_check_at(boot, Utc::now())
}

pub(crate) fn startup_check_at(boot: &str, now: DateTime<Utc>) -> Result<(), InstallError> {
    let entries = match std::fs::read_dir(layout::territory_transactions_dir()) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(InstallError::Io(error)),
    };
    for entry in entries {
        let path = entry.map_err(InstallError::Io)?.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let tx: TransactionFile =
            serde_json::from_slice(&std::fs::read(&path).map_err(InstallError::Io)?)
                .map_err(|error| InstallError::CorruptedTransaction(error.to_string()))?;
        transaction::validate_transaction(&tx)?;
        if tx.phase.is_terminal() {
            continue;
        }
        let Some((deadline, saved_boot)) = tx.confirmation_window() else {
            continue;
        };
        if tx.phase == Phase::RollingBack
            && matches!(
                tx.operation,
                transaction::Operation::Reinit | transaction::Operation::Restore
            )
        {
            let permit = layout::territory_transactions_dir()
                .join(&tx.transaction_id)
                .join("restored-boot-id");
            if std::fs::read_to_string(permit).ok().as_deref() == Some(boot) {
                continue;
            }
        }
        if tx.phase == Phase::RollingBack
            || (tx.phase == Phase::AwaitingNetworkConfirmation && now >= deadline)
            || (tx.phase != Phase::Finalizing && saved_boot.is_some_and(|saved| saved != boot))
        {
            return Err(InstallError::Preflight(
                "Landscape startup is blocked until lkit completes network recovery".into(),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::config::{NetworkMode, NetworkPlan, SelectedInterface};

    #[test]
    fn deadline_restart_and_confirmation_have_deterministic_precedence() {
        let dir = std::env::temp_dir().join(format!("lkit-recovery-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let root = crate::deployment::root::normalize_install_root(&dir).unwrap();
        let mut tx = TransactionFile::new_install(&root, &semver::Version::new(1, 2, 3)).unwrap();
        let now = Utc::now();
        let boot = uuid::Uuid::now_v7().to_string();
        tx.network_takeover = Some(transaction::NetworkTakeoverTransaction {
            boot_id: Some(boot.clone()),
            plan: NetworkPlan {
                mode: NetworkMode::WanDhcp { wan: "eth0".into() },
                selected_macs: vec![SelectedInterface {
                    name: "eth0".into(),
                    mac: "02:00:00:00:00:01".into(),
                }],
            },
            host_services: vec![],
            network_manager: None,
            management_bridge_was_absent: false,
            preserve_startup_links: false,
            confirmation_deadline: now,
            rollback_service: "lkit-test.service".into(),
            rollback_timer: "lkit-test.timer".into(),
            boot_rollback_service: "lkit-test-boot.service".into(),
            recovery_binary: "service/lkit-network-recovery".into(),
            pending_state: format!(
                "transactions/{}/pending-install-state.json",
                tx.transaction_id
            ),
        });
        tx.phase = Phase::AwaitingNetworkConfirmation;
        assert_eq!(
            action(&tx, now - chrono::Duration::seconds(1), &boot),
            Action::Wait
        );
        assert_eq!(action(&tx, now, &boot), Action::Rollback);
        assert_eq!(
            action(&tx, now - chrono::Duration::seconds(1), "another-boot"),
            Action::Rollback
        );
        tx.phase = Phase::Finalizing;
        assert_eq!(action(&tx, now, "another-boot"), Action::Confirm);
        tx.phase = Phase::RollingBack;
        assert_eq!(
            action(&tx, now - chrono::Duration::seconds(1), &boot),
            Action::Rollback
        );
        tx.phase = Phase::Committed;
        assert_eq!(action(&tx, now, "another-boot"), Action::Wait);
        tx.phase = Phase::AwaitingNetworkConfirmation;
        tx.network_takeover.as_mut().unwrap().boot_id = None;
        assert_eq!(
            action(&tx, now - chrono::Duration::seconds(1), &boot),
            Action::Wait
        );
        assert_eq!(action(&tx, now, &boot), Action::Rollback);
        let territory = dir.join("territory");
        let _territory = layout::test_territory(&territory);
        std::fs::create_dir_all(layout::territory_transactions_dir()).unwrap();
        let file = layout::territory_transactions_dir().join(format!("{}.json", tx.transaction_id));
        tx.network_takeover.as_mut().unwrap().boot_id = Some(boot.clone());
        std::fs::write(&file, serde_json::to_vec(&tx).unwrap()).unwrap();
        assert!(startup_check_at(&boot, now - chrono::Duration::seconds(1)).is_ok());
        assert!(startup_check_at(&boot, now).is_err());
        assert!(startup_check_at(&boot, now + chrono::Duration::seconds(1)).is_err());
        assert!(startup_check_for(&uuid::Uuid::now_v7().to_string()).is_err());
        tx.phase = Phase::Finalizing;
        std::fs::write(&file, serde_json::to_vec(&tx).unwrap()).unwrap();
        assert!(startup_check_for(&uuid::Uuid::now_v7().to_string()).is_ok());
        tx.phase = Phase::RollingBack;
        std::fs::write(&file, serde_json::to_vec(&tx).unwrap()).unwrap();
        assert!(startup_check_for(&boot).is_err());
        tx.phase = Phase::RolledBack;
        std::fs::write(&file, serde_json::to_vec(&tx).unwrap()).unwrap();
        assert!(startup_check_for(&boot).is_ok());
        tx.operation = transaction::Operation::Reinit;
        for phase in [
            Phase::Preparing,
            Phase::Prepared,
            Phase::Stopping,
            Phase::Activating,
            Phase::Verifying,
        ] {
            tx.phase = phase;
            assert_eq!(action(&tx, now, &boot), Action::Rollback);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn install_and_reinit_both_wait_until_expiry_or_a_host_reboot() {
        let dir =
            std::env::temp_dir().join(format!("lkit-recovery-matrix-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let root = crate::deployment::root::normalize_install_root(&dir).unwrap();
        let now = Utc::now();
        let boot = uuid::Uuid::now_v7().to_string();
        for operation in [
            transaction::Operation::Install,
            transaction::Operation::Reinit,
        ] {
            let mut tx = match operation {
                transaction::Operation::Install => {
                    TransactionFile::new_install(&root, &semver::Version::new(1, 2, 3)).unwrap()
                }
                _ => TransactionFile::new_reinit(&root, &semver::Version::new(1, 2, 3)).unwrap(),
            };
            tx.network_takeover = Some(transaction::NetworkTakeoverTransaction {
                boot_id: Some(boot.clone()),
                plan: NetworkPlan {
                    mode: NetworkMode::WanDhcp { wan: "eth0".into() },
                    selected_macs: vec![SelectedInterface {
                        name: "eth0".into(),
                        mac: "02:00:00:00:00:01".into(),
                    }],
                },
                host_services: vec![],
                network_manager: None,
                management_bridge_was_absent: false,
                preserve_startup_links: false,
                confirmation_deadline: now,
                rollback_service: "lkit-test.service".into(),
                rollback_timer: "lkit-test.timer".into(),
                boot_rollback_service: "lkit-test-boot.service".into(),
                recovery_binary: "service/lkit-network-recovery".into(),
                pending_state: format!(
                    "transactions/{}/pending-install-state.json",
                    tx.transaction_id
                ),
            });
            let network = tx.network_takeover.as_ref().unwrap();
            for seconds in [-1, 0, 1] {
                let result = super::super::takeover::check_confirmation_window(
                    network,
                    now + chrono::Duration::seconds(seconds),
                    Ok(boot.clone()),
                );
                assert_eq!(result.is_ok(), seconds < 0);
            }
            assert!(
                super::super::takeover::check_confirmation_window(
                    network,
                    now - chrono::Duration::seconds(1),
                    Ok("new-boot".into())
                )
                .is_err()
            );
            assert!(
                super::super::takeover::check_confirmation_window(
                    network,
                    now - chrono::Duration::seconds(1),
                    Err(InstallError::Preflight("unreadable boot".into()))
                )
                .is_err()
            );
            tx.phase = Phase::AwaitingNetworkConfirmation;
            for saved_boot in [Some(boot.clone()), None] {
                tx.network_takeover.as_mut().unwrap().boot_id = saved_boot;
                assert_eq!(
                    action(&tx, now - chrono::Duration::seconds(1), &boot),
                    Action::Wait,
                    "{operation:?} must retain its confirmation window"
                );
                assert_eq!(action(&tx, now, &boot), Action::Rollback);
                assert_eq!(
                    action(&tx, now + chrono::Duration::seconds(1), &boot),
                    Action::Rollback
                );
            }
            tx.network_takeover.as_mut().unwrap().boot_id = Some(boot.clone());
            assert_eq!(
                action(&tx, now - chrono::Duration::seconds(1), "new-boot"),
                Action::Rollback
            );
            for phase in [Phase::Committed, Phase::RolledBack, Phase::Failed] {
                tx.phase = phase;
                assert_eq!(action(&tx, now, "new-boot"), Action::Wait);
            }
            tx.phase = Phase::Finalizing;
            assert_eq!(action(&tx, now, "new-boot"), Action::Confirm);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
