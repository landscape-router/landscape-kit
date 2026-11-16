use serde::{Deserialize, Serialize};

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
