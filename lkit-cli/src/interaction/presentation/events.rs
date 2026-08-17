use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use serde::{Deserialize, Serialize};

pub(crate) const PRESENTATION_EVENTS_ENV: &str = "LKIT_INTERNAL_PRESENTATION_EVENTS";
pub(crate) const OPERATIONS_DIR: &str = "/run/lkit/operations";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct DownloadState {
    pub(super) id: u64,
    pub(super) label: String,
    pub(super) total: u64,
    pub(super) position: u64,
    pub(super) elapsed_millis: u64,
    pub(super) status: DownloadStatus,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum DownloadStatus {
    Downloading,
    Complete,
    Retrying,
    Failed,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub(super) enum PresentationEvent {
    Download {
        state: DownloadState,
    },
    Phase {
        phase: OperationPhase,
        #[serde(default)]
        step: Option<u8>,
        #[serde(default)]
        total: Option<u8>,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OperationPhase {
    Preparing,
    Downloading,
    Applying,
    Stopping,
    Activating,
    Verifying,
}

pub(super) fn event_file() -> Option<File> {
    let path = std::path::PathBuf::from(std::env::var_os(PRESENTATION_EVENTS_ENV)?);
    if path.parent() != Some(Path::new(OPERATIONS_DIR))
        || !path
            .file_name()?
            .to_string_lossy()
            .ends_with(".presentation.jsonl")
    {
        return None;
    }
    let file = OpenOptions::new()
        .append(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .ok()?;
    let metadata = file.metadata().ok()?;
    (metadata.is_file() && metadata.uid() == 0 && metadata.mode() & 0o077 == 0).then_some(file)
}

pub(super) fn write_event(file: &mut File, event: &PresentationEvent) {
    let mut line = match serde_json::to_vec(event) {
        Ok(line) => line,
        Err(_) => return,
    };
    line.push(b'\n');
    let _ = file.write_all(&line).and_then(|()| file.flush());
}

pub(crate) fn operation_phase(phase: OperationPhase) {
    operation_progress(phase, None);
}

/// 带步骤进度的阶段事件:step/total 供全屏页渲染步骤 Gauge(restore 等无字节下载的操作)。
pub(crate) fn operation_progress(phase: OperationPhase, progress: Option<(u8, u8)>) {
    let Some(mut file) = event_file() else {
        return;
    };
    write_event(
        &mut file,
        &PresentationEvent::Phase {
            phase,
            step: progress.map(|(step, _)| step),
            total: progress.map(|(_, total)| total),
        },
    );
}
