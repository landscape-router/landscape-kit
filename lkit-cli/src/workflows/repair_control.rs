//! Operation-scoped control for repair. The daemon runs each workflow on its own
//! blocking thread with a current-thread async runtime; no process environment is used.
use crate::deployment::{plan::InstallError, transaction::TransactionFile};
use std::{cell::RefCell, future::Future};

thread_local! { static OPERATION: RefCell<Option<String>> = const { RefCell::new(None) }; }

pub(crate) struct Guard;
pub(crate) fn enter(id: &str) -> Guard {
    OPERATION.with(|slot| {
        assert!(slot.borrow().is_none());
        *slot.borrow_mut() = Some(id.into());
    });
    Guard
}
impl Drop for Guard {
    fn drop(&mut self) {
        OPERATION.with(|slot| *slot.borrow_mut() = None);
    }
}
fn id() -> Option<String> {
    OPERATION.with(|slot| slot.borrow().clone())
}
pub(crate) fn associate(tx: &TransactionFile) -> Result<(), InstallError> {
    if let Some(id) = id() {
        crate::daemon_operations::associate_id(&id, tx)?;
    }
    Ok(())
}
pub(crate) fn phase(phase: crate::daemon_operations::Stage) -> Result<(), InstallError> {
    if let Some(id) = id() {
        crate::daemon_operations::set_stage(&id, phase)?;
        #[cfg(feature = "test-support")]
        if phase == crate::daemon_operations::Stage::Verifying
            && std::env::var_os("LKIT_TEST_PAUSE_REPAIR_VERIFY").is_some()
        {
            let gate =
                crate::deployment::layout::territory_run_dir().join("test-repair-verify-gate");
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            while gate.exists() {
                if std::time::Instant::now() >= deadline {
                    return Err(InstallError::Preflight(
                        "fixture repair pause timed out".into(),
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
    }
    Ok(())
}
pub(crate) fn progress(position: u64, total: u64) {
    if let Some(id) = id() {
        let _ = crate::daemon_operations::set_progress(&id, position, total);
    }
}
pub(crate) async fn download<T>(
    future: impl Future<Output = Result<T, InstallError>>,
) -> Result<T, InstallError> {
    let Some(id) = id() else {
        return future.await;
    };
    phase(crate::daemon_operations::Stage::Downloading)?;
    tokio::pin!(future);
    let mut ticks = tokio::time::interval(std::time::Duration::from_millis(100));
    loop {
        tokio::select! {
            biased;
            _ = ticks.tick() => {
                if crate::daemon_operations::read(&id)?.cancel_requested {
                    return Err(InstallError::UserRefused("download cancellation requested".into()));
                }
            }
            result = &mut future => return result,
        }
    }
}
