use std::fs::File;
use std::io::{IsTerminal, Read, Stdout};
use std::path::Path;
use std::time::Duration;

use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{
    self, Event as TerminalEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    Clear as ClearScreen, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
    enable_raw_mode,
};
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};

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
    /// 结果页时是否存在待确认的网络接管（install/reinit 成功后进入该状态）。
    takeover_pending: bool,
    /// 结果页「确认网络接管」确认层是否开启。
    confirming_takeover: bool,
    pub(super) result: Option<OperationResult>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PresentationAction {
    Stop,
    Close,
    /// 结果页确认层确认：调用方退出全屏页后内联执行 `lkit network confirm`。
    ConfirmTakeover,
}

/// 结果页等待关闭的结果：普通关闭或确认网络接管。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CloseOutcome {
    Closed,
    ConfirmTakeover,
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
            takeover_pending: false,
            confirming_takeover: false,
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
            if let Some(action) = self.handle_key(key) {
                return Ok(Some(action));
            }
        }
        self.render_screen();
        Ok(None)
    }

    /// 全屏页键处理：`l` 切换语言；结果页处理确认层/关闭，
    /// 进行中处理取消确认层/停止。
    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> Option<PresentationAction> {
        let ctrl_c =
            key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c');
        if matches!(key.code, KeyCode::Char('l' | 'L'))
            && !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            self.toggle_language();
            return None;
        }
        if self.result.is_some() {
            if self.confirming_takeover {
                match key.code {
                    KeyCode::Enter => {
                        return Some(PresentationAction::ConfirmTakeover);
                    }
                    KeyCode::Esc => {
                        self.confirming_takeover = false;
                        self.notice.clear();
                    }
                    _ => {}
                }
                return None;
            }
            if ctrl_c {
                return Some(PresentationAction::Close);
            }
            // 结果页存在待确认的网络接管时,Enter 打开确认层。
            if key.code == KeyCode::Enter && self.takeover_pending {
                self.confirming_takeover = true;
            }
            return None;
        }
        if self.confirming_stop {
            match key.code {
                KeyCode::Enter => return Some(PresentationAction::Stop),
                KeyCode::Esc => {
                    self.confirming_stop = false;
                    self.notice.clear();
                }
                _ => {}
            }
            return None;
        }
        if ctrl_c {
            if self.is_cancellable() {
                return Some(PresentationAction::Stop);
            }
            self.ignore_stop();
            return None;
        }
        if key.code == KeyCode::Esc {
            if self.is_cancellable() {
                self.confirming_stop = true;
                self.notice.clear();
            } else {
                self.ignore_stop();
            }
        }
        None
    }

    /// 切换语言并写回 `config.toml` 的 `[ui] language`,下次会话沿用。
    /// 测试环境不写盘,保证单元测试零系统副作用。
    fn toggle_language(&mut self) {
        let language = crate::i18n::current().toggled();
        crate::i18n::configure(language);
        #[cfg(not(test))]
        if let Err(error) = crate::deployment::config::write_language(language) {
            self.notice = crate::tr!(crate::keys::CONSOLE_LANGUAGE_SAVE_FAILED, error = error);
        }
    }

    pub(crate) fn show_result(&mut self, success: bool, takeover_pending: bool) {
        self.result = Some(if success {
            OperationResult::Success
        } else {
            OperationResult::Failed
        });
        self.current = None;
        self.confirming_stop = false;
        self.confirming_takeover = false;
        self.takeover_pending = takeover_pending;
        self.notice.clear();
        self.render_screen();
    }

    pub(crate) fn wait_for_close(
        &mut self,
        interrupt: &InterruptGuard,
    ) -> Result<CloseOutcome, String> {
        if self.screen.is_none() {
            return Ok(CloseOutcome::Closed);
        }
        loop {
            if interrupt.requested() {
                interrupt.clear_request();
                break;
            }
            match self.poll_action()? {
                Some(PresentationAction::Close) => break,
                Some(PresentationAction::ConfirmTakeover) => {
                    return Ok(CloseOutcome::ConfirmTakeover);
                }
                Some(PresentationAction::Stop) | None => {}
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(CloseOutcome::Closed)
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
        let confirming_takeover = self.confirming_takeover;
        let takeover_pending = self.takeover_pending;
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
                takeover_pending,
            );
            if confirming_takeover {
                render_takeover_confirmation(frame);
            }
        });
    }
}

struct FullScreenOperation {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

/// 结果页「确认网络接管」居中确认层：Enter 确认(退出全屏页内联执行
/// `lkit network confirm`),Esc 关闭。正文先说明断连后果,再给出兜底命令
/// ——确认失败或会话中断时用管理地址重连执行 `lkit network confirm`,
/// 期限过期未确认将自动回滚(`lkit network rollback`)。
fn render_takeover_confirmation(frame: &mut Frame<'_>) {
    let screen = frame.area();
    let width = 76.min(screen.width.saturating_sub(2));
    let height = 12.min(screen.height.saturating_sub(2));
    let area = Rect::new(
        screen.x + screen.width.saturating_sub(width) / 2,
        screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                crate::tr!(crate::keys::PRESENTATION_TAKEOVER_CONFIRM_QUESTION),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Line::raw(""),
            Line::styled(
                crate::tr!(crate::keys::PRESENTATION_TAKEOVER_CONFIRM_FALLBACK),
                Style::default().fg(Color::DarkGray),
            ),
            Line::raw(""),
            Line::raw(crate::tr!(
                crate::keys::PRESENTATION_TAKEOVER_CONFIRM_PRESS_ENTER
            )),
            Line::styled(
                crate::tr!(crate::keys::PRESENTATION_TAKEOVER_CONFIRM_PRESS_ESC),
                Style::default().fg(Color::DarkGray),
            ),
        ])
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true })
        .block(
            Block::bordered().title(crate::tr!(crate::keys::PRESENTATION_TAKEOVER_CONFIRM_TITLE)),
        ),
        area,
    );
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

    #[test]
    fn language_key_switches_inside_full_screen() {
        let previous = crate::i18n::current();
        crate::i18n::configure(crate::i18n::Language::En);
        let mut presentation = WorkerPresentation::new(false, Box::new(InstallScreen));

        presentation.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));

        assert_eq!(crate::i18n::current(), crate::i18n::Language::Zh);
        crate::i18n::configure(previous);
    }

    #[test]
    fn language_key_does_not_interfere_with_stop_or_close() {
        let previous = crate::i18n::current();
        crate::i18n::configure(crate::i18n::Language::En);
        let mut presentation = WorkerPresentation::new(false, Box::new(InstallScreen));
        presentation.apply(PresentationEvent::Phase {
            phase: OperationPhase::Downloading,
            step: None,
            total: None,
        });

        assert_eq!(
            presentation.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE)),
            None
        );
        assert_eq!(
            presentation.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            None
        );
        assert!(presentation.confirming_stop);
        assert_eq!(
            presentation.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Some(PresentationAction::Stop)
        );
        crate::i18n::configure(previous);
    }
}
