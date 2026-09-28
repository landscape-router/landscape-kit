use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};

use super::ConsoleApp;
use super::render::panel_block;
use super::widgets::{Focus, block_row_of};
use crate::check;
use crate::check::model::{CheckReport, Status};

pub(crate) enum PreflightState {
    NotRun,
    Running(Receiver<CheckReport>),
    Complete(CheckReport),
    Failed(String),
}

/// 环境检查的后台操作接缝:UI 只消费返回的 `Receiver`。
/// 生产实现真实跑全量检查;单测注入脚本化 `MockPreflightOps`。
pub(crate) trait PreflightOps {
    fn run(&self) -> Receiver<CheckReport>;
}

/// 生产实现:后台线程执行真实环境检查(`check::run_all`)。
pub(crate) struct RealPreflightOps;

impl PreflightOps for RealPreflightOps {
    fn run(&self) -> Receiver<CheckReport> {
        let (sender, receiver) = mpsc::channel();
        let language = crate::i18n::current();
        std::thread::spawn(move || {
            let report = crate::i18n::with_language(language, check::run_all);
            let _ = sender.send(report);
        });
        receiver
    }
}

fn default_preflight_ops() -> Arc<dyn PreflightOps> {
    Arc::new(RealPreflightOps)
}

/// 单测注入用的手动 mock:worker 停着不动,测试经 `run_sender` 注入。
#[cfg(test)]
pub(crate) struct MockPreflightOps {
    run_tx: std::sync::Mutex<Option<mpsc::Sender<CheckReport>>>,
}

#[cfg(test)]
impl MockPreflightOps {
    pub(crate) fn manual() -> Self {
        Self {
            run_tx: std::sync::Mutex::new(None),
        }
    }

    pub(crate) fn run_sender(&self) -> mpsc::Sender<CheckReport> {
        self.run_tx
            .lock()
            .unwrap()
            .take()
            .expect("run() must be called before grabbing the sender")
    }
}

#[cfg(test)]
impl PreflightOps for MockPreflightOps {
    fn run(&self) -> Receiver<CheckReport> {
        let (sender, receiver) = mpsc::channel();
        *self.run_tx.lock().unwrap() = Some(sender);
        receiver
    }
}

pub(crate) struct Preflight {
    /// 后台操作接缝,默认真实现;测试注入 mock。
    pub(crate) ops: Arc<dyn PreflightOps>,
    pub(crate) state: PreflightState,
    pub(crate) expanded: bool,
    pub(crate) scroll: u16,
}

impl Default for Preflight {
    fn default() -> Self {
        Self {
            ops: default_preflight_ops(),
            state: PreflightState::NotRun,
            expanded: false,
            scroll: 0,
        }
    }
}

impl Preflight {
    pub(crate) fn start(&mut self) {
        if matches!(&self.state, PreflightState::Running(_)) {
            return;
        }
        self.state = PreflightState::Running(self.ops.run());
        self.scroll = 0;
    }

    pub(crate) fn poll(&mut self) {
        let result = match &self.state {
            PreflightState::Running(receiver) => receiver.try_recv(),
            _ => return,
        };
        match result {
            Ok(report) => {
                self.state = PreflightState::Complete(report);
                self.scroll = 0;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.state =
                    PreflightState::Failed(crate::tr!(crate::keys::CONSOLE_CHECK_WORKER_STOPPED));
            }
        }
    }

    pub(crate) fn restart(&mut self) {
        self.state = PreflightState::NotRun;
        self.expanded = false;
        self.scroll = 0;
        self.start();
    }

    pub(crate) fn scroll_down(&mut self, amount: u16) {
        let max = preflight_detail_lines(self)
            .len()
            .saturating_sub(1)
            .min(u16::MAX as usize) as u16;
        self.scroll = self.scroll.saturating_add(amount).min(max);
    }
}
pub(crate) enum GateState {
    None,
    Waiting,
    Dialog,
}

impl ConsoleApp {
    pub(crate) fn preflight_gate(&self) -> GateState {
        match &self.preflight.state {
            PreflightState::NotRun | PreflightState::Running(_) => GateState::Waiting,
            PreflightState::Failed(_) => GateState::Dialog,
            PreflightState::Complete(report) => match report.summary {
                Status::Pass | Status::Warning => GateState::None,
                Status::Error | Status::Unknown => GateState::Dialog,
            },
        }
    }

    /// 预检报告是否因 daemon 未运行而被阻断:弹框内提供「部署 daemon」按钮,
    /// 而不是只提示命令行 `lkit self install`。
    pub(crate) fn preflight_daemon_blocked(&self) -> bool {
        matches!(&self.preflight.state, PreflightState::Complete(report) if daemon_check_blocks(report))
    }
}
pub(crate) fn render_preflight_dialog(frame: &mut Frame<'_>, app: &mut ConsoleApp) {
    let mut lines: Vec<Line<'_>> = match &app.preflight.state {
        PreflightState::Failed(error) => vec![
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_ENVIRONMENT_CHECKS_COULD_NOT_COMPLETE),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Line::raw(""),
            Line::raw(error.clone()),
        ],
        PreflightState::Complete(report) => {
            let mut lines = Vec::new();
            let items = blocking_items(report);
            if items.is_empty() {
                lines.push(Line::raw(crate::tr!(
                    crate::keys::CONSOLE_CHECKS_DID_NOT_PASS
                )));
            } else {
                for item in items {
                    lines.push(Line::raw(format!("- {item}")));
                }
            }
            lines.push(Line::raw(""));
            // daemon 未运行属于委托前置失败:弹框内直接提供部署按钮,
            // 而不是只提示命令行 `lkit self install`。
            let daemon_blocked = daemon_check_blocks(report);
            if daemon_blocked {
                // 按钮常显选中态(黑底青字+Bold):它是弹窗内唯一要突出的动作,
                // 弹窗没有焦点环,Enter/D 键都打开部署确认弹窗
                // (内嵌急救恢复码输入与二次确认)。
                lines.push(Line::styled(
                    crate::tr!(crate::keys::CONSOLE_OVERVIEW_DEPLOY_DAEMON),
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ));
                lines.push(Line::raw(""));
            }
            lines
        }
        _ => return,
    };
    // 按键说明跟随弹窗(与底栏同源),高度公式按内容行数自动适配。
    lines.push(Line::raw(""));
    lines.push(super::render::dialog_hint_line(app));
    let screen = frame.area();
    let width = 64.min(screen.width.saturating_sub(2));
    // 弹窗内容允许换行,高度按最后一行在内容宽度下的换行后行号计算,不截断。
    let content_width = width.saturating_sub(2);
    let height = (block_row_of(&lines, lines.len().saturating_sub(1), content_width) + 3)
        .min(screen.height.saturating_sub(2));
    let area = Rect::new(
        screen.x + screen.width.saturating_sub(width) / 2,
        screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    );
    super::render::begin_dialog(frame, area);
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(Block::bordered().title(crate::tr!(crate::keys::CONSOLE_INSTALL_BLOCKED))),
        area,
    );
}

fn blocking_items(report: &CheckReport) -> Vec<String> {
    report
        .groups
        .iter()
        .flat_map(|group| group.results.iter())
        .filter(|result| matches!(result.status, Status::Error | Status::Unknown))
        .take(4)
        .map(|result| {
            if result.suggestion.is_empty() {
                result.title.to_string()
            } else {
                format!("{} - {}", result.title, result.suggestion)
            }
        })
        .collect()
}

/// 预检报告是否被 lkit 常驻服务检查项阻断(daemon 未运行)。
fn daemon_check_blocks(report: &CheckReport) -> bool {
    report
        .groups
        .iter()
        .flat_map(|group| group.results.iter())
        .any(|result| {
            result.id == "service.lkit_daemon"
                && matches!(result.status, Status::Error | Status::Unknown)
        })
}
pub(crate) fn render_preflight_summary(frame: &mut Frame<'_>, app: &mut ConsoleApp, area: Rect) {
    let (status, detail, color) = match &app.preflight.state {
        PreflightState::NotRun => (
            crate::tr!(crate::keys::CONSOLE_NOT_RUN),
            crate::tr!(crate::keys::CONSOLE_WAITING_TO_CHECK_HOST),
            Color::DarkGray,
        ),
        PreflightState::Running(_) => (
            crate::tr!(crate::keys::CONSOLE_RUNNING),
            crate::tr!(crate::keys::CONSOLE_CHECKING_THIS_HOST),
            Color::Cyan,
        ),
        PreflightState::Complete(report) => (
            report.summary.label().to_string(),
            preflight_counts(report),
            check_status_color(report.summary),
        ),
        PreflightState::Failed(error) => (
            crate::tr!(crate::keys::CONSOLE_FAILED),
            error.clone(),
            Color::Red,
        ),
    };
    let selected = app.focus == Focus::Panel && app.install.checks_selected;
    let style = if selected {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let status_style = if selected {
        style
    } else {
        Style::default().fg(color)
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(if selected { "> " } else { "  " }, style),
            // 按显示宽补齐到 9 列,检查名与状态列对齐。
            Span::styled(super::render::display_pad(&status, 9), status_style),
            Span::raw(detail),
        ]))
        .style(style)
        .block(panel_block(
            &crate::tr!(crate::keys::CONSOLE_ENVIRONMENT_CHECKS),
            selected,
        )),
        area,
    );
}

pub(crate) fn render_preflight_details(
    frame: &mut Frame<'_>,
    preflight: &Preflight,
    focused: bool,
    area: Rect,
) {
    frame.render_widget(
        Paragraph::new(preflight_detail_lines(preflight))
            .block(panel_block(
                &crate::tr!(crate::keys::CONSOLE_ENVIRONMENT_CHECKS),
                focused,
            ))
            .wrap(Wrap { trim: false })
            .scroll((preflight.scroll, 0)),
        area,
    );
}

fn preflight_detail_lines(preflight: &Preflight) -> Vec<Line<'static>> {
    let PreflightState::Complete(report) = &preflight.state else {
        return vec![match &preflight.state {
            PreflightState::NotRun => {
                Line::raw(crate::tr!(crate::keys::CONSOLE_CHECKS_HAVE_NOT_RUN))
            }
            PreflightState::Running(_) => Line::styled(
                crate::tr!(crate::keys::CONSOLE_CHECKING_THIS_HOST),
                Style::default().fg(Color::Cyan),
            ),
            PreflightState::Failed(error) => {
                Line::styled(error.clone(), Style::default().fg(Color::Red))
            }
            PreflightState::Complete(_) => unreachable!(),
        }];
    };
    let mut lines = vec![
        Line::styled(
            preflight_counts(report),
            Style::default().fg(check_status_color(report.summary)),
        ),
        Line::raw(""),
    ];
    for group in &report.groups {
        lines.push(Line::styled(
            group.title.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        ));
        for result in &group.results {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{:<7}", result.status.label()),
                    Style::default().fg(check_status_color(result.status)),
                ),
                Span::styled(result.title.clone(), Style::default().fg(Color::White)),
                Span::raw(if result.value.is_empty() {
                    String::new()
                } else {
                    format!("  {}", result.value)
                }),
            ]));
            if result.status != Status::Pass && !result.reason.is_empty() {
                lines.push(Line::styled(
                    format!("        {}", result.reason),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            if result.status != Status::Pass && !result.suggestion.is_empty() {
                lines.push(Line::styled(
                    format!("        {}", result.suggestion),
                    Style::default().fg(Color::Yellow),
                ));
            }
        }
        lines.push(Line::raw(""));
    }
    lines
}

fn check_status_color(status: Status) -> Color {
    match status {
        Status::Pass => Color::Green,
        Status::Warning => Color::Yellow,
        Status::Error => Color::Red,
        Status::Unknown => Color::Magenta,
    }
}

fn preflight_counts(report: &CheckReport) -> String {
    crate::tr!(
        crate::keys::CONSOLE_PREFLIGHT_COUNTS,
        passed = report.counts.pass,
        warnings = report.counts.warning,
        errors = report.counts.error,
        unknown = report.counts.unknown
    )
}
