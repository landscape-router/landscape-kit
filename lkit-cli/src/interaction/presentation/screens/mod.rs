pub(crate) mod install;
pub(crate) mod migrate;
pub(crate) mod reinit;
pub(crate) mod repair;
pub(crate) mod restore;
pub(crate) mod switch;
pub(crate) mod uninstall;
pub(crate) mod update;

pub(crate) use install::InstallScreen;
pub(crate) use migrate::MigrateScreen;
pub(crate) use reinit::ReinitScreen;
pub(crate) use repair::RepairScreen;
pub(crate) use restore::RestoreScreen;
pub(crate) use switch::SwitchScreen;
pub(crate) use uninstall::UninstallScreen;
pub(crate) use update::UpdateScreen;

use super::DownloadState;
use super::OperationPhase;
use crate::interaction::presentation::Frame;

/// 操作结果：成功、失败或取消，用于结果页标题与状态框文案。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OperationResult {
    Success,
    Failed,
    Cancelled,
}

/// 每个委托操作一个完全独立的页面组件：各自的完整渲染实现（布局与文案
/// 互不共享），以及命令行结束提示所需的结果文案与前缀。将来某个操作需要
/// 不同布局时，只改它自己的文件，不影响其他操作。
pub(crate) trait OperationScreen {
    /// 结果页标题键（完成/失败/取消），供命令行结束提示复用。
    fn result_key(&self, result: OperationResult) -> &'static str;
    /// 停止请求被忽略时的底栏提示键。
    fn stop_ignored_key(&self) -> &'static str;
    /// 该操作是否可能等待网络确认（install/reinit/restore 为 true）。
    fn takeover_confirmable(&self) -> bool {
        false
    }
    /// 渲染整个操作页（完整独立实现）。
    #[allow(clippy::too_many_arguments)]
    fn render(
        &self,
        frame: &mut Frame<'_>,
        phase: OperationPhase,
        step_progress: Option<(u8, u8)>,
        current: Option<&DownloadState>,
        logs: &[String],
        scroll: &mut LogScroll,
        notice: &str,
        confirming_stop: bool,
        result: Option<OperationResult>,
        takeover_pending: bool,
    );
}

/// 根据操作名选择页面组件，缺省为安装组件。
pub(crate) fn operation_screen(command: &str) -> Box<dyn OperationScreen> {
    match command {
        "install" => Box::new(install::InstallScreen),
        "migrate" => Box::new(migrate::MigrateScreen),
        "switch" => Box::new(switch::SwitchScreen),
        "update" => Box::new(update::UpdateScreen),
        "repair" => Box::new(repair::RepairScreen),
        "restore" => Box::new(restore::RestoreScreen),
        "reinit" => Box::new(reinit::ReinitScreen),
        "uninstall" => Box::new(uninstall::UninstallScreen),
        _ => Box::new(install::InstallScreen),
    }
}

/// 步骤进度条使用的阶段文案（与下载字节进度无关的操作，如 restore）。
pub(crate) fn step_phase_text(phase: OperationPhase) -> String {
    match phase {
        OperationPhase::Preparing => crate::tr!(crate::keys::PRESENTATION_PREPARING),
        OperationPhase::Stopping => crate::tr!(crate::keys::PRESENTATION_STOPPING),
        OperationPhase::Activating => crate::tr!(crate::keys::PRESENTATION_ACTIVATING),
        OperationPhase::Verifying => crate::tr!(crate::keys::PRESENTATION_VERIFYING),
        OperationPhase::Downloading | OperationPhase::Applying => {
            crate::tr!(crate::keys::PRESENTATION_APPLYING_CONFIGURATION)
        }
    }
}

/// A missing top row follows live output. Offsets count wrapped terminal rows.
#[derive(Default)]
pub(crate) struct LogScroll {
    top: Option<u16>,
    bottom: u16,
    height: u16,
    width: u16,
}

impl LogScroll {
    pub(super) fn handle_key(&mut self, key: crossterm::event::KeyCode) -> bool {
        use crossterm::event::KeyCode;
        let top = self.top.unwrap_or(self.bottom);
        let page = self.height.max(1);
        let next = match key {
            KeyCode::Up => top.saturating_sub(1),
            KeyCode::Down => top.saturating_add(1),
            KeyCode::PageUp => top.saturating_sub(page),
            KeyCode::PageDown => top.saturating_add(page),
            KeyCode::Home => 0,
            KeyCode::End => self.bottom,
            _ => return false,
        };
        self.top = (next < self.bottom).then_some(next);
        true
    }

    pub(super) fn discard(&mut self, lines: &[String]) {
        if let Some(top) = &mut self.top {
            use ratatui::widgets::{Paragraph, Wrap};
            for line in lines {
                let rows = Paragraph::new(line.as_str())
                    .wrap(Wrap { trim: false })
                    .line_count(self.width);
                *top = top.saturating_sub(u16::try_from(rows).unwrap_or(u16::MAX));
            }
        }
    }
}

fn render_log_panel(
    frame: &mut Frame<'_>,
    area: ratatui::layout::Rect,
    logs: &[String],
    scroll: &mut LogScroll,
) {
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::Line;
    use ratatui::widgets::{Block, Paragraph, Wrap};

    let block = Block::bordered().title(crate::tr!(crate::keys::PRESENTATION_OUTPUT));
    let inner = block.inner(area);
    let height = usize::from(inner.height);
    let lines = logs
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
        .collect::<Vec<_>>();
    let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
    scroll.height = inner.height;
    scroll.width = inner.width;
    scroll.bottom =
        u16::try_from(paragraph.line_count(inner.width).saturating_sub(height)).unwrap_or(u16::MAX);
    scroll.top = scroll.top.filter(|top| *top < scroll.bottom);
    let block = if scroll.bottom > 0 {
        block.title_bottom(crate::tr!(crate::keys::PRESENTATION_LOG_SCROLL))
    } else {
        block
    };
    frame.render_widget(block, area);
    frame.render_widget(
        paragraph.scroll((scroll.top.unwrap_or(scroll.bottom), 0)),
        inner,
    );
}

/// 全屏结果页里把网络接管的确认与回滚后果提示行用醒目背景标出。
/// 中英文输出都包含 `lkit network confirm` 命令；等待确认、重新连接和
/// 未确认回滚的提示行分别包含 `confirm`/`确认`。
pub(crate) fn is_confirmation_line(line: &str) -> bool {
    line.contains("confirm") || line.contains("确认")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyCode;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn page_through_wrapped_logs_and_resume_following() {
        let mut terminal = Terminal::new(TestBackend::new(12, 5)).unwrap();
        let mut scroll = LogScroll::default();
        let mut logs: Vec<_> = ["first", "aaaaaaaaa bbbbbbbbb", "three", "four", "five"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        let mut draw = |logs: &[String], scroll: &mut LogScroll| {
            terminal
                .draw(|frame| render_log_panel(frame, frame.area(), logs, scroll))
                .unwrap();
            let buffer = terminal.backend().buffer();
            (1..4)
                .map(|y| {
                    (1..11)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>()
                        .trim()
                        .to_owned()
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(draw(&logs, &mut scroll), ["three", "four", "five"]);
        scroll.handle_key(KeyCode::PageUp);
        assert_eq!(
            draw(&logs, &mut scroll),
            ["first", "aaaaaaaaa", "bbbbbbbbb"]
        );
        logs.push("six".into());
        scroll.handle_key(KeyCode::Down);
        assert_eq!(
            draw(&logs, &mut scroll),
            ["aaaaaaaaa", "bbbbbbbbb", "three"]
        );
        scroll.handle_key(KeyCode::Home);
        assert_eq!(draw(&logs, &mut scroll)[0], "first");
        scroll.handle_key(KeyCode::PageDown);
        assert_eq!(draw(&logs, &mut scroll), ["three", "four", "five"]);
        scroll.handle_key(KeyCode::End);
        logs.push("seven".into());
        assert_eq!(draw(&logs, &mut scroll), ["five", "six", "seven"]);
    }
}
