use ratatui::Frame;

pub(crate) mod screens;

pub(crate) use screens::{
    InstallScreen, OperationResult, OperationScreen, RestoreScreen, operation_screen,
};

mod events;
mod progress;
mod signals;
mod worker;

use self::events::DownloadStatus;
pub(crate) use self::events::{
    DownloadState, OPERATIONS_DIR, OperationPhase, PRESENTATION_EVENTS_ENV, operation_phase,
    operation_progress,
};
use self::progress::human_bytes;
pub(crate) use self::progress::{DownloadProgress, StepProgress};
pub(crate) use self::signals::{InterruptGuard, show_cancelled_screen};
pub(crate) use self::worker::{CloseOutcome, PresentationAction, WorkerPresentation};

pub(crate) fn warning(id: &str, reason: &str, suggestion: &str) {
    eprintln!(
        "install: {} [{id}]",
        crate::tr!(crate::keys::PRESENTATION_WARNING)
    );
    eprintln!("  {reason}");
    if !suggestion.is_empty() {
        eprintln!(
            "  {}{suggestion}",
            crate::tr!(crate::keys::PRESENTATION_SUGGESTION_PREFIX)
        );
    }
}
