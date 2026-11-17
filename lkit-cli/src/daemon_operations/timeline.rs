//! Bounded, typed operation timeline. No free-form payloads or process output.
use super::*;
use crate::keys::operation as keys;
const LIMIT: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Step {
    CheckingInstallation,
    ResolvingVersion,
    CheckingCache,
    ReusingCache,
    DownloadingBinary,
    UnpackingBinary,
    DownloadingStatic,
    UnpackingStatic,
    PreparingFrontend,
    SavingRecovery,
    WritingConfiguration,
    PublishingRelease,
    TakingOverNetwork,
    ActivatingRelease,
    RegisteringService,
    StartingService,
    WaitingForHealth,
    ObservingStability,
    SavingPendingState,
    Committing,
    Restoring,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Event {
    Step(Step),
    Stage(Stage),
    Status(Status),
}
impl Event {
    fn key(self) -> &'static str {
        match self {
            Self::Step(Step::CheckingInstallation) => keys::STEP_CHECKING_INSTALLATION,
            Self::Step(Step::ResolvingVersion) => keys::STEP_RESOLVING_VERSION,
            Self::Step(Step::CheckingCache) => keys::STEP_CHECKING_CACHE,
            Self::Step(Step::ReusingCache) => keys::STEP_REUSING_CACHE,
            Self::Step(Step::DownloadingBinary) => keys::STEP_DOWNLOADING_BINARY,
            Self::Step(Step::UnpackingBinary) => keys::STEP_UNPACKING_BINARY,
            Self::Step(Step::DownloadingStatic) => keys::STEP_DOWNLOADING_STATIC,
            Self::Step(Step::UnpackingStatic) => keys::STEP_UNPACKING_STATIC,
            Self::Step(Step::PreparingFrontend) => keys::STEP_PREPARING_FRONTEND,
            Self::Step(Step::SavingRecovery) => keys::STEP_SAVING_RECOVERY,
            Self::Step(Step::WritingConfiguration) => keys::STEP_WRITING_CONFIGURATION,
            Self::Step(Step::PublishingRelease) => keys::STEP_PUBLISHING_RELEASE,
            Self::Step(Step::TakingOverNetwork) => keys::STEP_TAKING_OVER_NETWORK,
            Self::Step(Step::ActivatingRelease) => keys::STEP_ACTIVATING_RELEASE,
            Self::Step(Step::RegisteringService) => keys::STEP_REGISTERING_SERVICE,
            Self::Step(Step::StartingService) => keys::STEP_STARTING_SERVICE,
            Self::Step(Step::WaitingForHealth) => keys::STEP_WAITING_FOR_HEALTH,
            Self::Step(Step::ObservingStability) => keys::STEP_OBSERVING_STABILITY,
            Self::Step(Step::SavingPendingState) => keys::STEP_SAVING_PENDING_STATE,
            Self::Step(Step::Committing) => keys::STEP_COMMITTING,
            Self::Step(Step::Restoring) => keys::STEP_RESTORING,
            Self::Stage(Stage::Checking) => keys::STAGE_CHECKING,
            Self::Stage(Stage::Downloading) => keys::STAGE_DOWNLOADING,
            Self::Stage(Stage::Preparing) => keys::STAGE_PREPARING,
            Self::Stage(Stage::Stopping) => keys::STAGE_STOPPING,
            Self::Stage(Stage::WaitingForSourceStop) => keys::STAGE_WAITING_FOR_SOURCE_STOP,
            Self::Stage(Stage::Activating) => keys::STAGE_ACTIVATING,
            Self::Stage(Stage::Verifying) => keys::STAGE_VERIFYING,
            Self::Stage(Stage::RollingBack) => keys::STAGE_ROLLING_BACK,
            Self::Stage(Stage::Finished) => keys::STAGE_FINISHED,
            Self::Status(Status::Queued) => keys::STATUS_QUEUED,
            Self::Status(Status::Running) => keys::STATUS_RUNNING,
            Self::Status(Status::Recovering) => keys::STATUS_RECOVERING,
            Self::Status(Status::AwaitingConfirmation) => keys::STATUS_AWAITING_CONFIRMATION,
            Self::Status(Status::Succeeded) => keys::STATUS_SUCCEEDED,
            Self::Status(Status::Failed) => keys::STATUS_FAILED,
            Self::Status(Status::Interrupted) => keys::STATUS_INTERRUPTED,
            Self::Status(Status::Cancelled) => keys::STATUS_CANCELLED,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Entry {
    pub sequence: u64,
    pub at: chrono::DateTime<chrono::Utc>,
    pub event: Event,
}
impl Entry {
    pub(crate) fn display(&self) -> String {
        format!(
            "[{} UTC] {}",
            self.at.format("%H:%M:%S"),
            crate::tr!(self.event.key())
        )
    }
}
fn event_path(id: &str) -> Result<PathBuf, InstallError> {
    validate_id(id)?;
    Ok(directory().join(format!("{id}.events")))
}
pub(crate) fn read_events(id: &str) -> Result<Vec<Entry>, InstallError> {
    use std::io::Read;
    let file = match std::fs::File::open(event_path(id)?) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(InstallError::Io(e)),
    };
    let mut bytes = vec![];
    file.take(131073)
        .read_to_end(&mut bytes)
        .map_err(InstallError::Io)?;
    if bytes.len() > 131072 {
        return Err(error("operation timeline exceeds size limit"));
    }
    let entries: Vec<Entry> =
        serde_json::from_slice(&bytes).map_err(|_| error("invalid operation timeline"))?;
    if entries.len() > LIMIT
        || entries.iter().any(|e| e.sequence == 0)
        || entries.windows(2).any(|p| p[0].sequence >= p[1].sequence)
    {
        return Err(error("invalid operation timeline sequence"));
    }
    Ok(entries)
}
fn append(id: &str, event: Event) -> Result<(), InstallError> {
    let _guard = queue_lock()?;
    let mut entries = read_events(id)?;
    if entries.last().is_some_and(|entry| entry.event == event) {
        return Ok(());
    }
    let sequence = entries.last().map_or(Ok(1), |e| {
        e.sequence
            .checked_add(1)
            .ok_or_else(|| error("operation timeline sequence overflow"))
    })?;
    entries.push(Entry {
        sequence,
        at: chrono::Utc::now(),
        event,
    });
    if entries.len() > LIMIT {
        entries.remove(0);
    }
    crate::workflows::repair_service::atomic_write(
        &event_path(id)?,
        &serde_json::to_vec(&entries).map_err(InstallError::StateWrite)?,
        0o600,
    )
}
/// Diagnostic failures must never decide a transaction's outcome.
pub(crate) fn record(id: &str, event: Event) {
    if let Err(e) = append(id, event) {
        eprintln!("lkit daemon: cannot record operation step: {e}");
    }
}

pub(crate) fn debug_snapshot(record: &Record) -> String {
    format!(
        "[debug] operation={} transaction={} status={:?} stage={:?} revision={} downloaded={} total={:?} exit={:?}",
        record.id,
        record.transaction_id.as_deref().unwrap_or("-"),
        record.status,
        record.stage,
        record.revision,
        record.downloaded_bytes,
        record.total_bytes,
        record.exit_code
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture {
        _guard: layout::TerritoryOverride,
        dir: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("lkit-timeline-{}", uuid::Uuid::now_v7()));
            std::fs::create_dir_all(&dir).unwrap();
            Self {
                _guard: layout::test_territory(&dir),
                dir,
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
    #[test]
    fn timeline_is_private_bounded_ordered_and_separate_from_the_queue() {
        let f = Fixture::new();
        let request = super::super::submit(f.dir.join("landscape").display().to_string()).unwrap();
        assert!(read_events(&request.id).unwrap().is_empty());
        for n in 0..140 {
            let step = if n % 2 == 0 {
                Step::DownloadingBinary
            } else {
                Step::UnpackingBinary
            };
            append(&request.id, Event::Step(step)).unwrap();
            append(&request.id, Event::Step(step)).unwrap(); // adjacent duplicate suppressed
        }
        let entries = read_events(&request.id).unwrap();
        assert_eq!(entries.len(), LIMIT);
        assert_eq!(entries.first().unwrap().sequence, 13);
        assert_eq!(entries.last().unwrap().sequence, 140);
        assert_eq!(
            std::fs::metadata(event_path(&request.id).unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(directory()).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(super::super::list().unwrap().len(), 1);
        let entry = &entries[0];
        let zh = crate::i18n::with_language(crate::i18n::Language::Zh, || entry.display());
        let en = crate::i18n::with_language(crate::i18n::Language::En, || entry.display());
        assert!(zh.contains("下载并校验后端"));
        assert!(en.contains("Downloading and verifying"));
    }
    #[test]
    fn diagnostic_failure_and_private_payloads_cannot_change_business_records() {
        let f = Fixture::new();
        let mut request =
            super::super::submit(f.dir.join("landscape").display().to_string()).unwrap();
        let before = std::fs::read(path(&request.id).unwrap()).unwrap();
        let log = event_path(&request.id).unwrap();
        std::fs::write(&log, b"corrupted").unwrap();
        assert!(read_events(&request.id).is_err());
        record(&request.id, Event::Step(Step::StartingService));
        assert_eq!(std::fs::read(path(&request.id).unwrap()).unwrap(), before);
        request.message = Some("Secret123 token=private".into());
        assert!(!debug_snapshot(&request).contains("Secret123"));
        assert!(!debug_snapshot(&request).contains("private"));
        assert!(serde_json::from_str::<Event>(r#"{"step":"arbitrary command"}"#).is_err());
        assert!(read_events("../outside").is_err());
        std::fs::write(&log, vec![b' '; 131073]).unwrap();
        assert!(read_events(&request.id).is_err());
    }
}
