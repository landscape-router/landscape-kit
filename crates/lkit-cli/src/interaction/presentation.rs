use std::fs::{File, OpenOptions};
use std::io::{IsTerminal, Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Gauge, Paragraph};
use ratatui::{Frame, Terminal, TerminalOptions, Viewport};
use serde::{Deserialize, Serialize};

pub(crate) const PRESENTATION_EVENTS_ENV: &str = "LKIT_INTERNAL_PRESENTATION_EVENTS";
pub(crate) const OPERATIONS_DIR: &str = "/run/lkit/operations";
const PROGRESS_REFRESH: Duration = Duration::from_millis(100);
const PROGRESS_HEIGHT: u16 = 2;
static NEXT_PROGRESS_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) fn warning(id: &str, reason: &str, suggestion: &str) {
    eprintln!("install: warning [{id}]");
    eprintln!("  {reason}");
    if !suggestion.is_empty() {
        eprintln!("  建议：{suggestion}");
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct DownloadState {
    id: u64,
    label: String,
    total: u64,
    position: u64,
    elapsed_millis: u64,
    status: DownloadStatus,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum DownloadStatus {
    Downloading,
    Complete,
    Retrying,
    Failed,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum PresentationEvent {
    Download { state: DownloadState },
}

enum ProgressOutput {
    Events(File),
    Interactive(InteractiveDownload),
    Hidden,
}

pub(crate) struct DownloadProgress {
    state: DownloadState,
    output: ProgressOutput,
    started: Instant,
    last_refresh: Instant,
}

impl DownloadProgress {
    pub(crate) fn new(label: &str, total: u64) -> Self {
        let now = Instant::now();
        let state = DownloadState {
            id: NEXT_PROGRESS_ID.fetch_add(1, Ordering::Relaxed),
            label: label.to_string(),
            total,
            position: 0,
            elapsed_millis: 0,
            status: DownloadStatus::Downloading,
        };
        let output = event_file()
            .map(ProgressOutput::Events)
            .or_else(|| {
                std::io::stderr()
                    .is_terminal()
                    .then(|| InteractiveDownload::new().ok())
                    .flatten()
                    .map(ProgressOutput::Interactive)
            })
            .unwrap_or(ProgressOutput::Hidden);
        let mut progress = Self {
            state,
            output,
            started: now,
            last_refresh: now.checked_sub(PROGRESS_REFRESH).unwrap_or(now),
        };
        progress.refresh(true);
        progress
    }

    pub(crate) fn set_position(&mut self, position: u64) {
        self.state.position = position.min(self.state.total);
        self.refresh(position >= self.state.total);
    }

    pub(crate) fn finish(mut self) {
        self.state.position = self.state.total;
        self.state.status = DownloadStatus::Complete;
        self.refresh(true);
        self.finish_interactive();
    }

    pub(crate) fn abandon_retrying(mut self) {
        self.state.status = DownloadStatus::Retrying;
        self.refresh(true);
        self.finish_interactive();
    }

    pub(crate) fn abandon_failed(mut self) {
        self.state.status = DownloadStatus::Failed;
        self.refresh(true);
        self.finish_interactive();
    }

    fn refresh(&mut self, force: bool) {
        let now = Instant::now();
        if !force && now.duration_since(self.last_refresh) < PROGRESS_REFRESH {
            return;
        }
        self.state.elapsed_millis = now.duration_since(self.started).as_millis() as u64;
        match &mut self.output {
            ProgressOutput::Events(file) => write_event(file, &self.state),
            ProgressOutput::Interactive(renderer) => {
                let _ = renderer.render(&self.state);
            }
            ProgressOutput::Hidden => {}
        }
        self.last_refresh = now;
    }

    fn finish_interactive(&mut self) {
        let output = std::mem::replace(&mut self.output, ProgressOutput::Hidden);
        if let ProgressOutput::Interactive(renderer) = output {
            renderer.finish();
        }
    }
}

fn event_file() -> Option<File> {
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

fn write_event(file: &mut File, state: &DownloadState) {
    let event = PresentationEvent::Download {
        state: state.clone(),
    };
    let mut line = match serde_json::to_vec(&event) {
        Ok(line) => line,
        Err(_) => return,
    };
    line.push(b'\n');
    let _ = file.write_all(&line).and_then(|()| file.flush());
}

struct InteractiveDownload {
    terminal: Terminal<CrosstermBackend<std::io::Stderr>>,
}

impl InteractiveDownload {
    fn new() -> std::io::Result<Self> {
        let backend = CrosstermBackend::new(std::io::stderr());
        let terminal = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Inline(PROGRESS_HEIGHT),
            },
        )?;
        Ok(Self { terminal })
    }

    fn render(&mut self, state: &DownloadState) -> std::io::Result<()> {
        self.terminal.draw(|frame| render_download(frame, state))?;
        Ok(())
    }

    fn finish(mut self) {
        let _ = self.terminal.show_cursor();
        drop(self.terminal);
        eprintln!();
    }
}

fn render_download(frame: &mut Frame<'_>, state: &DownloadState) {
    let [title_area, gauge_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(frame.area());
    let (verb, color) = match state.status {
        DownloadStatus::Downloading => ("Downloading", Color::Cyan),
        DownloadStatus::Complete => ("Downloaded", Color::Green),
        DownloadStatus::Retrying => ("Retrying", Color::Yellow),
        DownloadStatus::Failed => ("Download failed", Color::Red),
    };
    let title = Line::from(vec![
        Span::styled(
            verb,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!("  {}", state.label)),
    ]);
    frame.render_widget(Paragraph::new(title), title_area);

    let ratio = if state.total == 0 {
        0.0
    } else {
        state.position as f64 / state.total as f64
    };
    let percent = (ratio * 100.0).round() as u64;
    let speed = speed_bytes(state);
    let eta = eta(state, speed);
    let label = format!(
        "{percent:>3}%  {} / {}  {}/s  ETA {eta}",
        human_bytes(state.position),
        human_bytes(state.total),
        human_bytes(speed),
    );
    let gauge = Gauge::default()
        .ratio(ratio.clamp(0.0, 1.0))
        .label(label)
        .gauge_style(Style::default().fg(color))
        .use_unicode(false);
    frame.render_widget(gauge, gauge_area);
}

fn speed_bytes(state: &DownloadState) -> u64 {
    if state.elapsed_millis == 0 {
        return 0;
    }
    state
        .position
        .saturating_mul(1_000)
        .checked_div(state.elapsed_millis)
        .unwrap_or(0)
}

fn eta(state: &DownloadState, speed: u64) -> String {
    if state.position >= state.total {
        return "0s".into();
    }
    if speed == 0 {
        return "--".into();
    }
    let seconds = state.total.saturating_sub(state.position).div_ceil(speed);
    if seconds < 60 {
        format!("{seconds}s")
    } else {
        format!("{}m{:02}s", seconds / 60, seconds % 60)
    }
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub(crate) struct WorkerPresentation {
    events: Option<File>,
    pending: String,
    current: Option<DownloadState>,
    renderer: Option<InteractiveDownload>,
}

impl WorkerPresentation {
    pub(crate) fn new() -> Self {
        Self {
            events: None,
            pending: String::new(),
            current: None,
            renderer: None,
        }
    }

    pub(crate) fn drain(&mut self, path: &Path) -> Result<(), String> {
        if self.events.is_none() {
            self.events = File::open(path).ok();
        }
        let Some(events) = self.events.as_mut() else {
            return Ok(());
        };
        events
            .read_to_string(&mut self.pending)
            .map_err(|error| format!("read presentation events {}: {error}", path.display()))?;
        while let Some(newline) = self.pending.find('\n') {
            let line = self.pending[..newline].to_string();
            self.pending.drain(..=newline);
            if line.is_empty() {
                continue;
            }
            let event: PresentationEvent = serde_json::from_str(&line)
                .map_err(|error| format!("parse presentation event: {error}"))?;
            self.apply(event);
        }
        Ok(())
    }

    pub(crate) fn before_log(&mut self) {
        if let Some(renderer) = self.renderer.take() {
            renderer.finish();
        }
    }

    pub(crate) fn finish(&mut self) {
        self.before_log();
        self.current = None;
    }

    fn apply(&mut self, event: PresentationEvent) {
        let PresentationEvent::Download { state } = event;
        let finished = !matches!(state.status, DownloadStatus::Downloading);
        self.current = Some(state);
        if std::io::stderr().is_terminal() {
            if self.renderer.is_none() {
                self.renderer = InteractiveDownload::new().ok();
            }
            if let (Some(renderer), Some(state)) = (&mut self.renderer, &self.current) {
                let _ = renderer.render(state);
            }
        }
        if finished {
            self.before_log();
            self.current = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;

    #[test]
    fn renders_download_with_test_backend_without_a_terminal() {
        let backend = TestBackend::new(80, PROGRESS_HEIGHT);
        let mut terminal = Terminal::new(backend).unwrap();
        let state = DownloadState {
            id: 1,
            label: "Landscape webserver".into(),
            total: 8 * 1024 * 1024,
            position: 4 * 1024 * 1024,
            elapsed_millis: 2_000,
            status: DownloadStatus::Downloading,
        };
        terminal
            .draw(|frame| render_download(frame, &state))
            .unwrap();
        let lines: Vec<String> = terminal
            .backend()
            .buffer()
            .content
            .chunks(80)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect())
            .collect();
        assert!(lines[0].contains("Downloading  Landscape webserver"));
        assert!(lines[1].contains("50%"));
        assert!(lines[1].contains("4.0 MiB / 8.0 MiB"));
        assert!(lines[1].contains("2.0 MiB/s"));
        assert!(lines[1].contains("ETA 2s"));
    }

    #[test]
    fn formats_small_and_large_byte_counts() {
        assert_eq!(human_bytes(27), "27 B");
        assert_eq!(human_bytes(1536), "1.5 KiB");
        assert_eq!(human_bytes(2 * 1024 * 1024), "2.0 MiB");
    }
}
