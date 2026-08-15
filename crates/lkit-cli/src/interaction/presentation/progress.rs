use std::fs::File;
use std::io::IsTerminal;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Gauge, Paragraph};
use ratatui::{Frame, Terminal, TerminalOptions, Viewport};

use super::events::{DownloadState, DownloadStatus, PresentationEvent, event_file, write_event};

const PROGRESS_REFRESH: Duration = Duration::from_millis(100);
const PROGRESS_HEIGHT: u16 = 2;
static NEXT_PROGRESS_ID: AtomicU64 = AtomicU64::new(1);

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
                (std::io::stderr().is_terminal()
                    && !crate::interaction::interactive::is_non_interactive())
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
            ProgressOutput::Events(file) => write_event(
                file,
                &PresentationEvent::Download {
                    state: self.state.clone(),
                },
            ),
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

pub(super) struct InteractiveDownload {
    terminal: Terminal<CrosstermBackend<std::io::Stderr>>,
}

impl InteractiveDownload {
    pub(super) fn new() -> std::io::Result<Self> {
        let backend = CrosstermBackend::new(std::io::stderr());
        let terminal = Terminal::with_options(
            backend,
            TerminalOptions {
                viewport: Viewport::Inline(PROGRESS_HEIGHT),
            },
        )?;
        Ok(Self { terminal })
    }

    pub(super) fn render(&mut self, state: &DownloadState) -> std::io::Result<()> {
        self.terminal.draw(|frame| render_download(frame, state))?;
        Ok(())
    }

    fn render_step(&mut self, state: &StepState) -> std::io::Result<()> {
        self.terminal.draw(|frame| render_step(frame, state))?;
        Ok(())
    }

    pub(super) fn finish(mut self) {
        let _ = self.terminal.show_cursor();
        drop(self.terminal);
        eprintln!();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StepStatus {
    Running,
    Complete,
    Failed,
}

struct StepState {
    label: String,
    total: u64,
    position: u64,
    status: StepStatus,
}

enum StepOutput {
    Interactive(InteractiveDownload),
    Hidden,
}

/// 无字节下载的按步骤操作（如 `backup create` 的按文件进度）在普通终端上
/// 渲染的内联进度条。事件文件由 daemon worker 专用，备份创建不委托，因此
/// 只支持 stderr 内联渲染。
pub(crate) struct StepProgress {
    state: StepState,
    started: Instant,
    last_refresh: Instant,
    output: StepOutput,
}

impl StepProgress {
    pub(crate) fn new(label: String, total: u64) -> Self {
        let now = Instant::now();
        let state = StepState {
            label,
            total,
            position: 0,
            status: StepStatus::Running,
        };
        let output = (std::io::stderr().is_terminal()
            && !crate::interaction::interactive::is_non_interactive())
        .then(|| InteractiveDownload::new().ok())
        .flatten()
        .map(StepOutput::Interactive)
        .unwrap_or(StepOutput::Hidden);
        let mut progress = Self {
            state,
            output,
            started: now,
            last_refresh: now.checked_sub(PROGRESS_REFRESH).unwrap_or(now),
        };
        progress.refresh(true);
        progress
    }

    pub(crate) fn set_state(&mut self, label: String, position: u64, total: u64) {
        self.state.label = label;
        self.state.total = total;
        self.state.position = position.min(total);
        self.refresh(false);
    }

    pub(crate) fn finish(mut self) {
        self.state.status = StepStatus::Complete;
        self.state.position = self.state.total;
        self.refresh(true);
        self.finish_interactive();
    }

    pub(crate) fn abandon_failed(mut self) {
        self.state.status = StepStatus::Failed;
        self.refresh(true);
        self.finish_interactive();
    }

    fn refresh(&mut self, force: bool) {
        let now = Instant::now();
        if !force && now.duration_since(self.last_refresh) < PROGRESS_REFRESH {
            return;
        }
        if let StepOutput::Interactive(renderer) = &mut self.output {
            let _ = renderer.render_step(&self.state);
        }
        self.last_refresh = now;
    }

    fn finish_interactive(&mut self) {
        let output = std::mem::replace(&mut self.output, StepOutput::Hidden);
        if let StepOutput::Interactive(renderer) = output {
            renderer.finish();
        }
    }
}

fn render_step(frame: &mut Frame<'_>, state: &StepState) {
    let [title_area, gauge_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(frame.area());
    let (verb, color) = match state.status {
        StepStatus::Running => (
            crate::tr!(crate::keys::PRESENTATION_BACKUP_ARCHIVING),
            Color::Cyan,
        ),
        StepStatus::Complete => (
            crate::tr!(crate::keys::PRESENTATION_BACKUP_PROGRESS_DONE),
            Color::Green,
        ),
        StepStatus::Failed => (
            crate::tr!(crate::keys::PRESENTATION_BACKUP_PROGRESS_FAILED),
            Color::Red,
        ),
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
    let label = format!("{percent:>3}%  {} / {} files", state.position, state.total);
    let gauge = Gauge::default()
        .ratio(ratio.clamp(0.0, 1.0))
        .label(label)
        .gauge_style(Style::default().fg(color))
        .use_unicode(false);
    frame.render_widget(gauge, gauge_area);
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

pub(super) fn human_bytes(bytes: u64) -> String {
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
    fn renders_step_progress_with_test_backend() {
        let backend = TestBackend::new(80, PROGRESS_HEIGHT);
        let mut terminal = Terminal::new(backend).unwrap();
        let state = StepState {
            label: "static.zip".into(),
            total: 6,
            position: 3,
            status: StepStatus::Running,
        };
        terminal.draw(|frame| render_step(frame, &state)).unwrap();
        let lines: Vec<String> = terminal
            .backend()
            .buffer()
            .content
            .chunks(80)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect())
            .collect();
        assert!(lines[0].contains("Archiving  static.zip"));
        assert!(lines[1].contains("50%"));
        assert!(lines[1].contains("3 / 6 files"));
    }

    #[test]
    fn formats_small_and_large_byte_counts() {
        assert_eq!(human_bytes(27), "27 B");
        assert_eq!(human_bytes(1536), "1.5 KiB");
        assert_eq!(human_bytes(2 * 1024 * 1024), "2.0 MiB");
    }
}
