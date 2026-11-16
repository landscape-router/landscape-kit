use ratatui::Frame;

pub(crate) mod screens;

pub(crate) use screens::{
    InstallScreen, OperationResult, OperationScreen, RestoreScreen, operation_screen,
};

mod model;
mod operation;
mod progress;
mod signals;

use self::model::DownloadStatus;
pub(crate) use self::model::{DownloadState, OperationPhase};
pub(crate) use self::operation::{OperationPresentation, PresentationAction};
use self::progress::human_bytes;
pub(crate) use self::progress::{DownloadProgress, StepProgress};
pub(crate) use self::signals::InterruptGuard;

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
