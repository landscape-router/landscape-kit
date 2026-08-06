use std::fs::{File, OpenOptions};
use std::io::{IsTerminal, Read, Stdout, Write};
use std::mem::MaybeUninit;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event as TerminalEvent, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Gauge, Paragraph, Wrap};
use ratatui::{Frame, Terminal, TerminalOptions, Viewport};
use serde::{Deserialize, Serialize};

pub(crate) const PRESENTATION_EVENTS_ENV: &str = "LKIT_INTERNAL_PRESENTATION_EVENTS";
pub(crate) const OPERATIONS_DIR: &str = "/run/lkit/operations";
const PROGRESS_REFRESH: Duration = Duration::from_millis(100);
const PROGRESS_HEIGHT: u16 = 2;
const SIGINT_MODE_NONE: u8 = 0;
const SIGINT_MODE_DIRECT: u8 = 1;
const SIGINT_MODE_DELEGATED: u8 = 2;
const SIGINT_MODE_CONSOLE: u8 = 3;
const TERMINAL_RESTORE: &[u8] = b"\x1b[0m\x1b[?25h\r\n";
const CONSOLE_RESTORE: &[u8] = b"\x1b[0m\x1b[?25h\x1b[?1049l\r\n";
static NEXT_PROGRESS_ID: AtomicU64 = AtomicU64::new(1);
static SIGINT_MODE: AtomicU8 = AtomicU8::new(SIGINT_MODE_NONE);
static SIGINT_RECEIVED: AtomicBool = AtomicBool::new(false);
static TERMINAL_FD: AtomicI32 = AtomicI32::new(-1);
static ORIGINAL_TERMIOS: SavedTermios = SavedTermios::new();

struct SavedTermios(std::cell::UnsafeCell<MaybeUninit<libc::termios>>);

// The value is initialized before the SIGINT action is installed and remains
// read-only until that action has been restored.
unsafe impl Sync for SavedTermios {}

impl SavedTermios {
    const fn new() -> Self {
        Self(std::cell::UnsafeCell::new(MaybeUninit::uninit()))
    }
}

pub(crate) fn warning(id: &str, reason: &str, suggestion: &str) {
    eprintln!("install: {} [{id}]", crate::tr!("warning", "警告"));
    eprintln!("  {reason}");
    if !suggestion.is_empty() {
        eprintln!("  {}{suggestion}", crate::tr!("Suggestion: ", "建议："));
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
    Phase { phase: OperationPhase },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OperationPhase {
    Preparing,
    Downloading,
    Applying,
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

fn write_event(file: &mut File, event: &PresentationEvent) {
    let mut line = match serde_json::to_vec(event) {
        Ok(line) => line,
        Err(_) => return,
    };
    line.push(b'\n');
    let _ = file.write_all(&line).and_then(|()| file.flush());
}

pub(crate) fn operation_phase(phase: OperationPhase) {
    let Some(mut file) = event_file() else {
        return;
    };
    write_event(&mut file, &PresentationEvent::Phase { phase });
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

pub(crate) struct InterruptGuard {
    previous: libc::sigaction,
    terminal: Option<TerminalState>,
}

impl InterruptGuard {
    pub(crate) fn install(delegated: bool) -> Result<Self, String> {
        let mode = if delegated {
            SIGINT_MODE_DELEGATED
        } else {
            SIGINT_MODE_DIRECT
        };
        Self::install_mode(mode)
    }

    pub(crate) fn install_console() -> Result<Self, String> {
        Self::install_mode(SIGINT_MODE_CONSOLE)
    }

    fn install_mode(mode: u8) -> Result<Self, String> {
        SIGINT_MODE
            .compare_exchange(SIGINT_MODE_NONE, mode, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| "a Ctrl+C handler is already active".to_string())?;
        SIGINT_RECEIVED.store(false, Ordering::SeqCst);
        let terminal = (!crate::interaction::interactive::is_non_interactive())
            .then(TerminalState::capture)
            .flatten();
        if let Some(terminal) = &terminal {
            unsafe {
                (*ORIGINAL_TERMIOS.0.get()).write(terminal.original);
            }
            TERMINAL_FD.store(terminal.file.as_raw_fd(), Ordering::SeqCst);
        }
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        action.sa_sigaction = handle_sigint as *const () as usize;
        action.sa_flags = 0;
        unsafe {
            libc::sigemptyset(&mut action.sa_mask);
        }
        let mut previous: libc::sigaction = unsafe { std::mem::zeroed() };
        if unsafe { libc::sigaction(libc::SIGINT, &action, &mut previous) } != 0 {
            TERMINAL_FD.store(-1, Ordering::SeqCst);
            SIGINT_MODE.store(SIGINT_MODE_NONE, Ordering::SeqCst);
            return Err(format!(
                "install terminal recovery handler: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(Self { previous, terminal })
    }

    pub(crate) fn requested(&self) -> bool {
        SIGINT_RECEIVED.load(Ordering::SeqCst)
    }

    pub(crate) fn clear_request(&self) {
        SIGINT_RECEIVED.store(false, Ordering::SeqCst);
    }
}

struct TerminalState {
    file: File,
    original: libc::termios,
}

impl TerminalState {
    fn capture() -> Option<Self> {
        let file = File::options()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOCTTY)
            .open("/dev/tty")
            .ok()
            .or_else(|| {
                [libc::STDERR_FILENO, libc::STDIN_FILENO, libc::STDOUT_FILENO]
                    .into_iter()
                    .find(|fd| unsafe { libc::isatty(*fd) } == 1)
                    .and_then(|fd| {
                        let duplicate = unsafe { libc::dup(fd) };
                        (duplicate >= 0).then(|| unsafe { File::from_raw_fd(duplicate) })
                    })
            })?;
        let mut original: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(file.as_raw_fd(), &mut original) } != 0 {
            return None;
        }
        Some(Self { file, original })
    }

    fn restore(&self) {
        unsafe {
            libc::tcsetattr(self.file.as_raw_fd(), libc::TCSANOW, &self.original);
        }
    }
}

impl Drop for InterruptGuard {
    fn drop(&mut self) {
        unsafe {
            libc::sigaction(libc::SIGINT, &self.previous, std::ptr::null_mut());
        }
        if let Some(terminal) = &self.terminal {
            terminal.restore();
        }
        TERMINAL_FD.store(-1, Ordering::SeqCst);
        SIGINT_MODE.store(SIGINT_MODE_NONE, Ordering::SeqCst);
        SIGINT_RECEIVED.store(false, Ordering::SeqCst);
    }
}

extern "C" fn handle_sigint(_signal: libc::c_int) {
    SIGINT_RECEIVED.store(true, Ordering::SeqCst);
    let terminal_fd = TERMINAL_FD.load(Ordering::SeqCst);
    let mode = SIGINT_MODE.load(Ordering::SeqCst);
    unsafe {
        if terminal_fd >= 0 {
            libc::tcsetattr(
                terminal_fd,
                libc::TCSANOW,
                (*ORIGINAL_TERMIOS.0.get()).as_ptr(),
            );
            let restore = if mode == SIGINT_MODE_CONSOLE {
                CONSOLE_RESTORE
            } else {
                TERMINAL_RESTORE
            };
            libc::write(terminal_fd, restore.as_ptr().cast(), restore.len());
        }
        if mode == SIGINT_MODE_DIRECT || mode == SIGINT_MODE_CONSOLE {
            libc::_exit(130);
        }
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

fn render_operation(
    frame: &mut Frame<'_>,
    phase: OperationPhase,
    current: Option<&DownloadState>,
    logs: &[String],
    notice: &str,
    confirming_stop: bool,
    result: Option<OperationResult>,
) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(8),
        Constraint::Length(3),
    ])
    .areas(frame.area());
    let title = match result {
        Some(OperationResult::Success) => crate::tr!("Installation complete", "安装完成"),
        Some(OperationResult::Failed) => crate::tr!("Installation failed", "安装失败"),
        Some(OperationResult::Cancelled) => crate::tr!("Installation cancelled", "安装已取消"),
        None => crate::tr!("Installing Landscape", "正在安装 Landscape"),
    };
    frame.render_widget(
        Paragraph::new(title)
            .style(Style::default().add_modifier(Modifier::BOLD))
            .block(Block::default().borders(Borders::BOTTOM)),
        header,
    );
    let [progress, log_area] =
        Layout::vertical([Constraint::Length(4), Constraint::Min(3)]).areas(body);
    if let Some(state) = current {
        let percent = if state.total == 0 {
            0.0
        } else {
            state.position as f64 / state.total as f64
        };
        let label = format!(
            "{}  {:>3}%  {} / {}",
            state.label,
            (percent * 100.0).round() as u64,
            human_bytes(state.position),
            human_bytes(state.total),
        );
        frame.render_widget(
            Gauge::default()
                .ratio(percent.clamp(0.0, 1.0))
                .label(label)
                .gauge_style(Style::default().fg(Color::Cyan))
                .use_unicode(false)
                .block(Block::bordered().title(crate::tr!("Download", "下载"))),
            progress,
        );
    } else {
        let status = match result {
            Some(OperationResult::Success) => crate::tr!(
                "The installation finished successfully.",
                "安装已成功完成。"
            ),
            Some(OperationResult::Failed) => {
                crate::tr!("The installation reported a failure.", "安装报告失败。")
            }
            Some(OperationResult::Cancelled) => crate::tr!(
                "The installation was stopped during download.",
                "安装已在下载阶段停止。"
            ),
            None => match phase {
                OperationPhase::Preparing => {
                    crate::tr!("Preparing installation...", "正在准备安装……")
                }
                OperationPhase::Downloading => {
                    crate::tr!("Waiting for download progress...", "等待下载进度……")
                }
                OperationPhase::Applying => crate::tr!(
                    "Applying configuration and starting services...",
                    "正在应用配置并启动服务……"
                ),
            },
        };
        frame.render_widget(
            Paragraph::new(status)
                .style(if matches!(result, Some(OperationResult::Success)) {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Green)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                })
                .block(Block::bordered().title(crate::tr!("Status", "状态"))),
            progress,
        );
    }
    let visible_logs = logs.iter().rev().take(8).rev().collect::<Vec<_>>();
    let log_lines: Vec<Line<'_>> = visible_logs
        .iter()
        .map(|line| {
            if is_confirmation_line(line) {
                Line::styled(
                    line.as_str(),
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Line::raw(line.as_str())
            }
        })
        .collect();
    frame.render_widget(
        Paragraph::new(log_lines)
            .block(Block::bordered().title(crate::tr!("Output", "输出")))
            .wrap(Wrap { trim: true }),
        log_area,
    );
    let hint = if result.is_some() {
        crate::tr!("Ctrl+C Close", "Ctrl+C 关闭")
    } else if confirming_stop {
        crate::tr!("Enter Stop  Esc Cancel", "Enter 停止  Esc 取消")
    } else if phase == OperationPhase::Downloading {
        crate::tr!("Ctrl+C Stop  Esc Stop options", "Ctrl+C 停止  Esc 停止选项")
    } else {
        crate::tr!(
            "Installation is in progress; stop requests are ignored",
            "安装正在进行；停止请求将被忽略"
        )
    };
    let footer_text = if notice.is_empty() {
        hint.to_string()
    } else {
        format!("{notice}  {hint}")
    };
    frame.render_widget(
        Paragraph::new(footer_text)
            .alignment(Alignment::Left)
            .style(Style::default().fg(Color::DarkGray))
            .block(Block::default().borders(Borders::TOP)),
        footer,
    );
    if confirming_stop {
        let width = 48.min(frame.area().width.saturating_sub(2));
        let height = 5.min(frame.area().height.saturating_sub(2));
        let area = Rect::new(
            frame.area().x + frame.area().width.saturating_sub(width) / 2,
            frame.area().y + frame.area().height.saturating_sub(height) / 2,
            width,
            height,
        );
        frame.render_widget(Clear, area);
        frame.render_widget(
            Paragraph::new(crate::tr!(
                "Stop the download? Press Enter to confirm or Esc to continue.",
                "停止下载？按 Enter 确认，按 Esc 继续。"
            ))
            .alignment(Alignment::Center)
            .block(Block::bordered().title(crate::tr!("Confirm stop", "确认停止"))),
            area,
        );
    }
}

/// 全屏结果页里把网络接管的确认与回滚后果提示行用醒目背景标出。
/// 中英文输出都包含 `lkit network confirm` 命令；等待确认、重新连接和
/// 未确认回滚的提示行分别包含 `confirm`/`确认`。
fn is_confirmation_line(line: &str) -> bool {
    line.contains("confirm") || line.contains("确认")
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
    screen: Option<FullScreenOperation>,
    phase: OperationPhase,
    logs: Vec<String>,
    notice: String,
    confirming_stop: bool,
    result: Option<OperationResult>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PresentationAction {
    Stop,
    Close,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OperationResult {
    Success,
    Failed,
    Cancelled,
}

impl WorkerPresentation {
    pub(crate) fn new(full_screen: bool) -> Self {
        Self {
            events: None,
            pending: String::new(),
            current: None,
            renderer: None,
            screen: full_screen
                .then(FullScreenOperation::start)
                .and_then(Result::ok),
            phase: OperationPhase::Preparing,
            logs: Vec::new(),
            notice: String::new(),
            confirming_stop: false,
            result: None,
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
        if self.screen.is_some() {
            return;
        }
        if let Some(renderer) = self.renderer.take() {
            renderer.finish();
        }
    }

    pub(crate) fn capture_log(&mut self, content: &str) -> bool {
        if self.screen.is_none() {
            return false;
        }
        self.logs.extend(
            content
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(str::to_string),
        );
        if self.logs.len() > 200 {
            self.logs.drain(..self.logs.len() - 200);
        }
        self.render_screen();
        true
    }

    pub(crate) fn is_cancellable(&self) -> bool {
        self.phase == OperationPhase::Downloading && self.result.is_none()
    }

    pub(crate) fn ignore_stop(&mut self) {
        self.notice = crate::tr!(
            "Installation is applying configuration; stopping is no longer available",
            "正在应用安装配置；当前阶段不能停止"
        )
        .into();
        self.confirming_stop = false;
        self.render_screen();
    }

    pub(crate) fn poll_action(&mut self) -> Result<Option<PresentationAction>, String> {
        if self.screen.is_none() {
            return Ok(None);
        }
        while event::poll(Duration::ZERO)
            .map_err(|error| format!("poll install screen: {error}"))?
        {
            let TerminalEvent::Key(key) =
                event::read().map_err(|error| format!("read install screen: {error}"))?
            else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            let ctrl_c =
                key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c');
            if self.result.is_some() {
                if ctrl_c {
                    return Ok(Some(PresentationAction::Close));
                }
                continue;
            }
            if self.confirming_stop {
                match key.code {
                    KeyCode::Enter => return Ok(Some(PresentationAction::Stop)),
                    KeyCode::Esc => {
                        self.confirming_stop = false;
                        self.notice.clear();
                    }
                    _ => {}
                }
                continue;
            }
            if ctrl_c {
                if self.is_cancellable() {
                    return Ok(Some(PresentationAction::Stop));
                }
                self.ignore_stop();
                continue;
            }
            if key.code == KeyCode::Esc {
                if self.is_cancellable() {
                    self.confirming_stop = true;
                    self.notice.clear();
                } else {
                    self.ignore_stop();
                }
            }
        }
        self.render_screen();
        Ok(None)
    }

    pub(crate) fn show_result(&mut self, success: bool) {
        self.result = Some(if success {
            OperationResult::Success
        } else {
            OperationResult::Failed
        });
        self.current = None;
        self.confirming_stop = false;
        self.notice.clear();
        self.render_screen();
    }

    pub(crate) fn wait_for_close(&mut self, interrupt: &InterruptGuard) -> Result<(), String> {
        if self.screen.is_none() {
            return Ok(());
        }
        loop {
            if interrupt.requested() {
                interrupt.clear_request();
                break;
            }
            if matches!(self.poll_action()?, Some(PresentationAction::Close)) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(())
    }

    pub(crate) fn finish(&mut self) {
        self.before_log();
        self.current = None;
        self.screen = None;
    }

    fn apply(&mut self, event: PresentationEvent) {
        match event {
            PresentationEvent::Download { state } => {
                let finished = !matches!(state.status, DownloadStatus::Downloading);
                self.current = Some(state);
                if self.screen.is_none()
                    && std::io::stderr().is_terminal()
                    && !crate::interaction::interactive::is_non_interactive()
                {
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
            PresentationEvent::Phase { phase } => {
                self.phase = phase;
                self.confirming_stop = false;
                self.notice.clear();
            }
        }
        self.render_screen();
    }

    fn render_screen(&mut self) {
        let Some(screen) = self.screen.as_mut() else {
            return;
        };
        let phase = self.phase;
        let current = self.current.as_ref();
        let logs = &self.logs;
        let notice = &self.notice;
        let confirming_stop = self.confirming_stop;
        let result = self.result;
        let _ = screen.terminal.draw(|frame| {
            render_operation(frame, phase, current, logs, notice, confirming_stop, result)
        });
    }
}

pub(crate) fn show_cancelled_screen(interrupt: &InterruptGuard) -> Result<(), String> {
    let mut presentation = WorkerPresentation::new(true);
    presentation.result = Some(OperationResult::Cancelled);
    presentation.render_screen();
    presentation.wait_for_close(interrupt)?;
    presentation.finish();
    Ok(())
}

struct FullScreenOperation {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl FullScreenOperation {
    fn start() -> Result<Self, String> {
        enable_raw_mode().map_err(|error| format!("enable install screen raw mode: {error}"))?;
        let mut stdout = std::io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen, Hide) {
            let _ = disable_raw_mode();
            return Err(format!("enter install screen: {error}"));
        }
        let terminal = match Terminal::new(CrosstermBackend::new(stdout)) {
            Ok(terminal) => terminal,
            Err(error) => {
                let _ = disable_raw_mode();
                let _ = execute!(std::io::stdout(), LeaveAlternateScreen, Show);
                return Err(format!("initialize install screen: {error}"));
            }
        };
        Ok(Self { terminal })
    }
}

impl Drop for FullScreenOperation {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen, Show);
        let _ = self.terminal.show_cursor();
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
    fn renders_full_screen_operation_without_sidebar() {
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let state = DownloadState {
            id: 1,
            label: "Landscape webserver".into(),
            total: 8,
            position: 4,
            elapsed_millis: 1_000,
            status: DownloadStatus::Downloading,
        };
        terminal
            .draw(|frame| {
                render_operation(
                    frame,
                    OperationPhase::Downloading,
                    Some(&state),
                    &[],
                    "",
                    false,
                    None,
                )
            })
            .unwrap();
        let content: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(content.contains("Installing Landscape"));
        assert!(content.contains("Landscape webserver"));
        assert!(content.contains("Ctrl+C Stop"));
        assert!(!content.contains("Navigation"));
    }

    #[test]
    fn renders_completed_operation_result() {
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render_operation(
                    frame,
                    OperationPhase::Applying,
                    None,
                    &[],
                    "",
                    false,
                    Some(OperationResult::Success),
                )
            })
            .unwrap();
        let content: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(content.contains("Installation complete"));
        assert!(content.contains("The installation finished successfully."));
        assert!(content.contains("Ctrl+C Close"));
        let buffer = terminal.backend().buffer();
        assert!(
            buffer
                .content
                .iter()
                .any(|cell| cell.bg == Color::Green && !cell.symbol().is_empty()),
            "success status box should use a green background"
        );
    }

    #[test]
    fn highlights_network_confirmation_lines_in_the_output_panel() {
        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let rollback_line = "install: confirm the network takeover within 10 minutes or the installation will be rolled back automatically";
        terminal
            .draw(|frame| {
                render_operation(
                    frame,
                    OperationPhase::Applying,
                    None,
                    &[
                        "install: systemd unit landscape-router.service is registered".into(),
                        "install: network takeover is awaiting confirmation".into(),
                        "install: reconnect to 10.1.1.105 and run `lkit network confirm`".into(),
                        rollback_line.into(),
                    ],
                    "",
                    false,
                    Some(OperationResult::Success),
                )
            })
            .unwrap();
        let content: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(content.contains("network takeover is awaiting confirmation"));
        assert!(content.contains("run `lkit network confirm`"));
        assert!(content.contains("rolled back automatically"));
        let buffer = terminal.backend().buffer();
        let highlighted = buffer
            .content
            .iter()
            .filter(|cell| cell.bg == Color::Yellow && !cell.symbol().is_empty())
            .count();
        let confirmation_lines = "install: network takeover is awaiting confirmation".len()
            + "install: reconnect to 10.1.1.105 and run `lkit network confirm`".len()
            + rollback_line.len();
        assert_eq!(
            highlighted, confirmation_lines,
            "only the confirmation and rollback lines should have a yellow background"
        );
    }

    #[test]
    fn only_download_phase_is_cancellable() {
        let mut presentation = WorkerPresentation::new(false);
        assert!(!presentation.is_cancellable());
        presentation.apply(PresentationEvent::Phase {
            phase: OperationPhase::Downloading,
        });
        assert!(presentation.is_cancellable());
        presentation.apply(PresentationEvent::Phase {
            phase: OperationPhase::Applying,
        });
        assert!(!presentation.is_cancellable());
    }

    #[test]
    fn formats_small_and_large_byte_counts() {
        assert_eq!(human_bytes(27), "27 B");
        assert_eq!(human_bytes(1536), "1.5 KiB");
        assert_eq!(human_bytes(2 * 1024 * 1024), "2.0 MiB");
    }
}
