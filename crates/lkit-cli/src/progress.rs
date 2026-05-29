//! Download progress display using indicatif.

use indicatif::{ProgressBar, ProgressStyle};
use lkit_core::download::DownloadProgress;

/// CLI progress bar for artifact downloads.
pub struct CliProgress {
    pb: ProgressBar,
}

impl CliProgress {
    /// Create a new progress bar (hidden until `on_file_start`).
    pub fn new() -> Self {
        let pb = ProgressBar::new(0);
        pb.set_style(
            ProgressStyle::with_template(
                "  {msg:<24} {bytes:>10} / {total_bytes:<10} [{bar:30}] {bytes_per_sec}",
            )
            .expect("valid template")
            .progress_chars("=>-"),
        );
        Self { pb }
    }
}

impl Default for CliProgress {
    fn default() -> Self {
        Self::new()
    }
}

impl DownloadProgress for CliProgress {
    fn on_file_start(&self, name: &str, total_bytes: u64) {
        self.pb.reset();
        self.pb.set_length(total_bytes);
        self.pb.set_position(0);
        self.pb.set_message(name.to_string());
    }

    fn on_file_progress(&self, _name: &str, bytes_downloaded: u64) {
        self.pb.set_position(bytes_downloaded);
    }

    fn on_file_complete(&self, name: &str) {
        self.pb.finish_with_message(format!("{name} 完成"));
    }
}
