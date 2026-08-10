use std::fs::File;
use std::io::{IsTerminal, Read, Stdout};
use std::path::Path;
use std::time::Duration;

use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event as TerminalEvent, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    Clear as ClearScreen, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
    enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use super::events::{DownloadState, DownloadStatus, OperationPhase, PresentationEvent};
use super::progress::InteractiveDownload;
use super::screens::{InstallScreen, OperationResult, OperationScreen};
use super::signals::InterruptGuard;

pub(crate) struct WorkerPresentation {
    events: Option<File>,
    pending: String,
    current: Option<DownloadState>,
    renderer: Option<InteractiveDownload>,
    screen: Option<FullScreenOperation>,
    operation: Box<dyn OperationScreen>,
    phase: OperationPhase,
    progress: Option<(u8, u8)>,
    logs: Vec<String>,
    notice: String,
    confirming_stop: bool,
    pub(super) result: Option<OperationResult>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PresentationAction {
    Stop,
    Close,
}

impl WorkerPresentation {
    pub(crate) fn new(full_screen: bool, operation: Box<dyn OperationScreen>) -> Self {
        Self {
            events: None,
            pending: String::new(),
            current: None,
            renderer: None,
            screen: full_screen
                .then(FullScreenOperation::start)
                .and_then(Result::ok),
            operation,
            phase: OperationPhase::Preparing,
            progress: None,
            logs: Vec::new(),
            notice: String::new(),
            confirming_stop: false,
            result: None,
        }
    }

    pub(crate) fn operation(&self) -> &dyn OperationScreen {
        self.operation.as_ref()
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
        self.notice = crate::tr!(self.operation.stop_ignored_key());
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
            PresentationEvent::Phase { phase, step, total } => {
                self.phase = phase;
                self.progress = match (step, total) {
                    (Some(step), Some(total)) if total > 0 => Some((step, total)),
                    _ => None,
                };
                self.confirming_stop = false;
                self.notice.clear();
            }
        }
        self.render_screen();
    }

    pub(super) fn render_screen(&mut self) {
        let Some(screen) = self.screen.as_mut() else {
            return;
        };
        let operation = self.operation.as_ref();
        let phase = self.phase;
        let progress = self.progress;
        let current = self.current.as_ref();
        let logs = &self.logs;
        let notice = &self.notice;
        let confirming_stop = self.confirming_stop;
        let result = self.result;
        let _ = screen.terminal.draw(|frame| {
            operation.render(
                frame,
                phase,
                progress,
                current,
                logs,
                notice,
                confirming_stop,
                result,
            )
        });
    }
}

struct FullScreenOperation {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl FullScreenOperation {
    fn start() -> Result<Self, String> {
        enable_raw_mode().map_err(|error| format!("enable install screen raw mode: {error}"))?;
        let mut stdout = std::io::stdout();
        if let Err(error) = execute!(
            stdout,
            EnterAlternateScreen,
            Hide,
            ClearScreen(ClearType::All),
            MoveTo(0, 0)
        ) {
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
        let _ = execute!(
            self.terminal.backend_mut(),
            Show,
            ClearScreen(ClearType::All),
            MoveTo(0, 0),
            LeaveAlternateScreen
        );
        let _ = self.terminal.show_cursor();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_download_phase_is_cancellable() {
        let mut presentation = WorkerPresentation::new(false, Box::new(InstallScreen));
        assert!(!presentation.is_cancellable());
        presentation.apply(PresentationEvent::Phase {
            phase: OperationPhase::Downloading,
            step: None,
            total: None,
        });
        assert!(presentation.is_cancellable());
        presentation.apply(PresentationEvent::Phase {
            phase: OperationPhase::Applying,
            step: None,
            total: None,
        });
        assert!(!presentation.is_cancellable());
    }
}
