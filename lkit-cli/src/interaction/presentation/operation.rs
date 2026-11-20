use std::io::{IsTerminal, Stdout};
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

use super::model::{DownloadState, DownloadStatus, OperationPhase};
use super::screens::{InstallScreen, OperationResult, OperationScreen};

pub(crate) struct OperationPresentation {
    current: Option<DownloadState>,
    download_asset: Option<&'static str>,
    screen: Option<FullScreenOperation>,
    operation: Box<dyn OperationScreen>,
    phase: OperationPhase,
    durable_cancellable: Option<bool>,
    progress: Option<(u8, u8)>,
    logs: Vec<String>,
    log_scroll: super::screens::LogScroll,
    notice: String,
    confirming_stop: bool,
    /// 结果页时是否存在待确认的网络接管（install/reinit 成功后进入该状态）。
    takeover_pending: bool,
    observed_outcome: Option<(u8, bool)>,
    /// 结果页「确认网络接管」确认层是否开启。
    confirming_takeover: bool,
    pub(super) result: Option<OperationResult>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PresentationAction {
    Stop,
    Close,
    /// 结果页确认层确认：调用方提交结构化网络确认操作。
    ConfirmTakeover,
}

impl OperationPresentation {
    pub(crate) fn new(full_screen: bool, operation: Box<dyn OperationScreen>) -> Self {
        Self {
            current: None,
            download_asset: None,
            screen: full_screen
                .then(FullScreenOperation::start)
                .and_then(Result::ok),
            operation,
            phase: OperationPhase::Preparing,
            durable_cancellable: None,
            progress: None,
            logs: Vec::new(),
            log_scroll: Default::default(),
            notice: String::new(),
            confirming_stop: false,
            takeover_pending: false,
            observed_outcome: None,
            confirming_takeover: false,
            result: None,
        }
    }

    pub(crate) fn is_full_screen(&self) -> bool {
        self.screen.is_some()
    }

    /// Observe durable state directly, without worker stdout or event files.
    pub(crate) fn observe_operation(&mut self, record: &crate::daemon_operations::Record) {
        use crate::daemon_operations::{Stage, Status};
        let cancellable = record.status == Status::Queued
            || (record.status == Status::Running && record.cancellable && !record.cancel_requested);
        if self.durable_cancellable != Some(cancellable) {
            self.confirming_stop = false;
        }
        self.durable_cancellable = Some(cancellable);
        let (phase, step) = match record.stage {
            Stage::Checking | Stage::Preparing => (OperationPhase::Preparing, 1),
            Stage::Downloading => (OperationPhase::Downloading, 1),
            Stage::Stopping | Stage::WaitingForSourceStop => (OperationPhase::Stopping, 2),
            Stage::Activating | Stage::RollingBack => (OperationPhase::Activating, 3),
            Stage::Verifying | Stage::Finished => (OperationPhase::Verifying, 4),
        };
        self.phase = phase;
        let total = if record.kind == crate::daemon_operations::Kind::Uninstall {
            3
        } else {
            4
        };
        self.progress = Some((step.min(total), total));
        self.current = (record.stage == Stage::Downloading).then(|| DownloadState {
            id: 0,
            label: self.download_label(),
            total: record.total_bytes.unwrap_or(0),
            position: record.downloaded_bytes,
            elapsed_millis: 0,
            status: DownloadStatus::Downloading,
        });
        let outcome = crate::daemon_operations::observed_outcome(record);
        if self.observed_outcome != outcome {
            self.observed_outcome = outcome;
            if let Some((code, pending)) = outcome {
                self.show_result(code, pending);
            } else {
                self.result = None;
                self.takeover_pending = false;
                self.confirming_takeover = false;
            }
        }
        self.notice = format!("{:?} / {:?}", record.status, record.stage);
        if let Some(message) = &record.message
            && self.logs.last() != Some(message)
        {
            self.capture_log(message);
        }
        self.render_screen();
    }

    fn download_label(&self) -> String {
        self.download_asset
            .map(str::to_owned)
            .unwrap_or_else(|| crate::tr!(crate::keys::PRESENTATION_DOWNLOAD))
    }

    pub(crate) fn capture_event(&mut self, entry: &crate::daemon_operations::timeline::Entry) {
        use crate::daemon_operations::timeline::{Event, Step};
        self.download_asset = match entry.event {
            Event::Step(Step::DownloadingBinary) => Some("Landscape webserver"),
            Event::Step(Step::DownloadingStatic) => Some("Landscape static assets"),
            _ => None,
        };
        let label = self.download_label();
        if let Some(current) = &mut self.current {
            current.label = label;
        }
        let line = entry.display();
        if matches!(entry.event, Event::Stage(_)) {
            self.capture_log(&format!("── {line} ──"));
        } else {
            self.capture_log(&line);
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
            let discard = self.logs.len() - 200;
            self.log_scroll.discard(&self.logs[..discard]);
            self.logs.drain(..discard);
        }
        self.render_screen();
        true
    }

    pub(crate) fn is_cancellable(&self) -> bool {
        if self.result.is_some() {
            return false;
        }
        self.durable_cancellable.unwrap_or(false)
    }

    pub(crate) fn cancel_requested(&mut self) {
        self.notice = crate::tr!(crate::keys::PRESENTATION_CANCELLING);
        self.confirming_stop = false;
        self.render_screen();
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
        if self.durable_cancellable.is_some() && ctrl_c {
            return Some(PresentationAction::Close);
        }
        if matches!(key.code, KeyCode::Char('l' | 'L'))
            && !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            self.toggle_language();
            return None;
        }
        if !self.confirming_stop
            && !self.confirming_takeover
            && self.log_scroll.handle_key(key.code)
        {
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

    pub(crate) fn show_result(&mut self, code: u8, takeover_pending: bool) {
        self.result = Some(match code {
            0 => OperationResult::Success,
            130 => OperationResult::Cancelled,
            _ => OperationResult::Failed,
        });
        self.current = None;
        self.confirming_stop = false;
        self.confirming_takeover = false;
        self.takeover_pending = takeover_pending && code == 0;
        self.notice.clear();
        self.render_screen();
    }

    pub(crate) fn finish(&mut self) {
        self.current = None;
        self.screen = None;
    }

    pub(super) fn render_screen(&mut self) {
        let Some(screen) = self.screen.as_mut() else {
            return;
        };
        let operation = self.operation.as_ref();
        let phase = self.phase;
        let progress = self.progress.filter(|_| self.result.is_none());
        let current = self.current.as_ref().filter(|_| self.result.is_none());
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
                &mut self.log_scroll,
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
        // 多行正文左对齐(逐行居中在折行时产生锯齿),与控制台弹窗规则一致。
        .wrap(Wrap { trim: false })
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
    fn durable_observer_detaches_and_obeys_persisted_cancellation_window() {
        use crate::daemon_operations::{Record, Stage, Status};
        let mut record: Record = serde_json::from_value(serde_json::json!({
            "schema_version":3, "id":"fixture", "kind":"switch", "install_root":"/fixture",
            "status":"queued", "transaction_id":null, "message":null
        }))
        .unwrap();
        let mut screen =
            OperationPresentation::new(false, Box::new(super::super::screens::UpdateScreen));
        screen.observe_operation(&record);
        assert!(screen.is_cancellable());
        assert_eq!(
            screen.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(PresentationAction::Close)
        );
        assert_eq!(record.status, Status::Queued);
        record.status = Status::Running;
        record.stage = Stage::Downloading;
        record.cancellable = true;
        screen.observe_operation(&record);
        assert_eq!(
            screen.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            None
        );
        assert_eq!(
            screen.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Some(PresentationAction::Stop)
        );
        record.stage = Stage::Preparing;
        record.cancellable = false;
        screen.observe_operation(&record);
        assert!(!screen.is_cancellable());
        assert_eq!(
            screen.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            None
        );
        assert_eq!(
            screen.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(PresentationAction::Close)
        );
    }

    #[test]
    fn download_gauge_follows_timeline_assets_on_attach_and_live_updates() {
        use crate::daemon_operations::{
            Record,
            timeline::{Entry, Event, Step},
        };
        let record: Record = serde_json::from_value(serde_json::json!({
            "schema_version":8, "id":"fixture-operation-id", "kind":"install", "install_root":"/fixture",
            "status":"running", "stage":"downloading", "transaction_id":null, "message":null
        })).unwrap();
        let mut screen = OperationPresentation::new(false, Box::new(InstallScreen));
        screen.observe_operation(&record);
        assert_ne!(screen.current.as_ref().unwrap().label, record.id);
        for (step, label) in [
            (Step::DownloadingBinary, "Landscape webserver"),
            (Step::DownloadingStatic, "Landscape static assets"),
        ] {
            screen.capture_event(&Entry {
                sequence: 1,
                at: chrono::Utc::now(),
                event: Event::Step(step),
            });
            assert_eq!(screen.current.as_ref().unwrap().label, label);
            screen.observe_operation(&record);
            assert_eq!(screen.current.as_ref().unwrap().label, label);
        }
    }

    #[test]
    fn pending_confirmation_tracks_external_resolution_and_invalidates_open_dialog() {
        use crate::daemon_operations::{Record, Status, observed_outcome};
        for (status, code) in [
            (Status::Succeeded, 0),
            (Status::Failed, 5),
            (Status::Recovering, 6),
        ] {
            let mut record: Record = serde_json::from_value(serde_json::json!({
                "schema_version":7, "id":"fixture", "kind":"reinit", "install_root":"/fixture",
                "status":"awaiting_confirmation", "transaction_id":null, "message":null, "exit_code":0
            })).unwrap();
            let mut screen = OperationPresentation::new(false, Box::new(InstallScreen));
            screen.observe_operation(&record);
            assert_eq!(observed_outcome(&record), Some((0, true)));
            screen.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
            assert!(screen.confirming_takeover);
            // Unrelated receipt revisions must not dismiss a still-valid dialog.
            record.revision += 1;
            screen.observe_operation(&record);
            assert!(screen.confirming_takeover);
            record.status = status;
            record.exit_code = Some(code);
            screen.observe_operation(&record);
            assert!(!screen.confirming_takeover);
            assert!(!screen.takeover_pending);
            assert_eq!(observed_outcome(&record), Some((code, false)));
            assert_eq!(
                screen.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
                None
            );
            assert_eq!(
                screen.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
                Some(PresentationAction::Close)
            );
        }
    }

    #[test]
    fn pending_confirmation_tracks_recovery_progress_and_final_failure() {
        use crate::daemon_operations::{Record, Status, observed_outcome};
        let mut record: Record = serde_json::from_value(serde_json::json!({
            "schema_version":8, "id":"fixture", "kind":"install", "install_root":"/fixture",
            "status":"awaiting_confirmation", "transaction_id":null, "message":null
        }))
        .unwrap();
        let mut screen = OperationPresentation::new(false, Box::new(InstallScreen));
        screen.observe_operation(&record);
        screen.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        record.status = Status::Recovering;
        screen.observe_operation(&record);
        assert_eq!(observed_outcome(&record), None);
        assert!(screen.result.is_none());
        assert!(!screen.confirming_takeover);
        assert!(!screen.takeover_pending);
        record.status = Status::Failed;
        record.exit_code = Some(1);
        screen.observe_operation(&record);
        assert!(matches!(screen.result, Some(OperationResult::Failed)));
        assert_eq!(observed_outcome(&record), Some((1, false)));
    }

    #[test]
    fn language_key_switches_inside_full_screen() {
        let previous = crate::i18n::current();
        crate::i18n::configure(crate::i18n::Language::En);
        let mut presentation = OperationPresentation::new(false, Box::new(InstallScreen));

        presentation.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));

        assert_eq!(crate::i18n::current(), crate::i18n::Language::Zh);
        crate::i18n::configure(previous);
    }

    #[test]
    fn language_key_does_not_interfere_with_stop_or_close() {
        let previous = crate::i18n::current();
        crate::i18n::configure(crate::i18n::Language::En);
        let mut presentation = OperationPresentation::new(false, Box::new(InstallScreen));
        presentation.durable_cancellable = Some(true);

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
