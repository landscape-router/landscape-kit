use std::sync::mpsc::{self, TryRecvError};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};

use super::ConsoleApp;
use super::render::{panel_block, register_dialog_hits, register_modal_hits};

/// daemon 部署后台线程的最终结果:成功返回与 CLI 相同的结果消息,失败返回错误文本。
pub(super) type DeployResult = Result<String, String>;

/// psk 弹窗(部署确认与查看/修改)中的导航单元:两个急救恢复码输入字段
/// 加一个动作行(部署/保存)。方向键或 Tab 在单元间移动,Enter 进入编辑
/// 或执行动作。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PskDialogField {
    Psk,
    Confirmation,
    Action,
}

impl PskDialogField {
    pub(super) fn previous(self) -> Self {
        match self {
            Self::Psk => Self::Action,
            Self::Confirmation => Self::Psk,
            Self::Action => Self::Confirmation,
        }
    }

    pub(super) fn next(self) -> Self {
        match self {
            Self::Psk => Self::Confirmation,
            Self::Confirmation => Self::Action,
            Self::Action => Self::Psk,
        }
    }
}

impl ConsoleApp {
    /// Overview 是否可执行 daemon 部署:daemon 未运行时显示「部署」动作行。
    /// 非 root 会话也能看到动作行,确认后得到与 CLI 相同的 root 权限错误提示。
    pub(super) fn daemon_deploy_available(&self) -> bool {
        !crate::daemon_worker::daemon_is_running()
    }

    /// 打开「部署 daemon」确认弹窗:预填既有急救恢复码(通常为空,留空自动
    /// 生成),复位二次确认与导航。Overview 动作行与安装阻断弹框共用。
    pub(super) fn open_deploy_dialog(&mut self) {
        self.deploy_psk = crate::deployment::config::load_flare()
            .and_then(|section| section.psk)
            .unwrap_or_default();
        self.deploy_psk_confirmation.clear();
        self.deploy_psk_field = PskDialogField::Psk;
        self.deploy_psk_editing = false;
        self.deploy_daemon_confirming = true;
    }

    /// 打开「查看/修改急救恢复码」弹窗:载入当前 `[flare]` 段 psk 明文
    /// (查看分发),内嵌 psk 与二次确认两个输入框,保存时校验一致后写回。
    /// daemon 运行时 Overview 动作行/Enter 触发。
    pub(super) fn open_show_psk(&mut self) {
        self.show_psk_value = crate::deployment::config::load_flare()
            .and_then(|section| section.psk)
            .unwrap_or_default();
        self.show_psk_confirmation.clear();
        self.show_psk_field = PskDialogField::Psk;
        self.show_psk_editing = false;
        self.show_psk = true;
    }

    /// 部署确认弹窗当前字段的值(编辑、粘贴与删除的目标;动作行无值)。
    pub(super) fn deploy_psk_value_mut(&mut self) -> Option<&mut String> {
        match self.deploy_psk_field {
            PskDialogField::Psk => Some(&mut self.deploy_psk),
            PskDialogField::Confirmation => Some(&mut self.deploy_psk_confirmation),
            PskDialogField::Action => None,
        }
    }

    /// 查看/修改弹窗当前字段的值(编辑、粘贴与删除的目标;动作行无值)。
    pub(super) fn show_psk_value_mut(&mut self) -> Option<&mut String> {
        match self.show_psk_field {
            PskDialogField::Psk => Some(&mut self.show_psk_value),
            PskDialogField::Confirmation => Some(&mut self.show_psk_confirmation),
            PskDialogField::Action => None,
        }
    }

    /// 保存查看/修改弹窗的修改:psk 非空且至少 12 字符、与二次确认一致后
    /// 写回 `config.toml` 的 `[flare]` 段(保留其它字段),daemon 下一周期拾取。
    pub(super) fn save_show_psk_dialog(&mut self) {
        let psk = self.show_psk_value.trim().to_string();
        if psk.is_empty() {
            self.notice = crate::tr!(crate::keys::CONSOLE_FLARE_PSK_REQUIRED);
            return;
        }
        if psk.len() < crate::deployment::config::FLARE_PSK_MIN_LENGTH {
            self.notice = crate::tr!(crate::keys::CONSOLE_FLARE_PSK_TOO_SHORT);
            return;
        }
        if psk != self.show_psk_confirmation.trim() {
            self.notice = crate::tr!(crate::keys::CONSOLE_DEPLOY_PSK_MISMATCH);
            return;
        }
        let mut section = crate::deployment::config::load_flare()
            .unwrap_or_else(crate::deployment::config::default_flare_section);
        section.psk = Some(psk);
        match crate::deployment::config::save_flare(&section) {
            Ok(()) => {
                self.show_psk = false;
                self.show_psk_editing = false;
                self.notice = crate::tr!(crate::keys::CONSOLE_FLARE_SAVED);
            }
            Err(error) => {
                self.notice = crate::tr!(crate::keys::CONSOLE_FLARE_SAVE_FAILED, error = error);
            }
        }
    }

    /// 在后台线程执行 `lkit self install`(与 CLI 相同的 root 检查、安装锁与
    /// systemd 语义),结果经 channel 回传,由 `poll_daemon_deploy` 展示。
    /// 控制台不另起 lkit 进程、不解析 CLI 文本输出。弹窗中填写的急救恢复码
    /// 一并传入:提供时在 daemon 启动前写回 `[flare]` 段,留空自动生成。
    pub(super) fn start_daemon_deploy(&mut self) -> Result<(), String> {
        if self.deploy_daemon.is_some() {
            return Ok(());
        }
        let (sender, receiver) = mpsc::channel();
        let language = crate::i18n::current();
        let psk = self.deploy_psk.trim().to_string();
        let psk = (!psk.is_empty()).then_some(psk);
        std::thread::spawn(move || {
            let result = crate::i18n::with_language(language, || {
                crate::commands::lkit_self::install_daemon(psk).map_err(|error| error.to_string())
            });
            let _ = sender.send(result);
        });
        self.deploy_daemon = Some(receiver);
        Ok(())
    }

    /// 部署完成或线程断开后收起进度,结果写入底栏;成功后 Overview 状态行
    /// 在下一帧自动变为运行中。
    pub(super) fn poll_daemon_deploy(&mut self) {
        let Some(receiver) = &self.deploy_daemon else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                self.deploy_daemon = None;
                self.deploy_daemon_confirming = false;
                self.notice = crate::tr!(crate::keys::CONSOLE_DEPLOY_DAEMON_WORKER_STOPPED);
                return;
            }
        };
        self.deploy_daemon = None;
        self.deploy_daemon_confirming = false;
        match result {
            Ok(message) => {
                self.notice = message;
                // 部署成功后预检报告已过期(daemon 检查此前报告 error 并挡住了
                // 安装表单),自动重跑,报告更新后表单门禁自然放行。
                self.preflight.restart();
            }
            Err(error) => self.notice = error,
        }
    }
}

pub(crate) fn render_daemon_deploy_confirmation(frame: &mut Frame<'_>, app: &mut ConsoleApp) {
    if !app.deploy_daemon_confirming {
        return;
    }
    let screen = frame.area();
    let width = 72.min(screen.width.saturating_sub(2));
    let height = 17.min(screen.height.saturating_sub(2));
    let area = Rect::new(
        screen.x + screen.width.saturating_sub(width) / 2,
        screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    );
    register_dialog_hits(&mut app.hits, screen, area);
    frame.render_widget(Clear, area);
    let psk_row = psk_edit_row(app, PskDialogField::Psk, true);
    let confirmation_row = psk_edit_row(app, PskDialogField::Confirmation, true);
    let start_row = dialog_action_row(
        app.deploy_psk_field == PskDialogField::Action,
        crate::tr!(crate::keys::CONSOLE_DEPLOY_DAEMON_START),
    );
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_DEPLOY_DAEMON_QUESTION),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Line::raw(""),
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_DEPLOY_FLARE_PURPOSE),
                Style::default().fg(Color::DarkGray),
            ),
            Line::raw(""),
            psk_row,
            confirmation_row,
            start_row,
            Line::raw(""),
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_DEPLOY_FLARE_HINT),
                Style::default().fg(Color::Green),
            ),
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_DEPLOY_DAEMON_PRESS_ESC),
                Style::default().fg(Color::DarkGray),
            ),
        ])
        .wrap(Wrap { trim: true })
        .block(Block::bordered().title(crate::tr!(crate::keys::CONSOLE_DEPLOY_DAEMON_TITLE))),
        area,
    );
}

/// psk 输入字段行:聚焦字段高亮并显示光标。`masked` 为 true 时未编辑显示掩码
/// (部署确认弹窗),false 时恒为明文(查看/修改弹窗);留空显示占位文本。
fn psk_edit_row(app: &ConsoleApp, field: PskDialogField, masked: bool) -> Line<'static> {
    let active = app.deploy_psk_field == field;
    let (label, value) = match field {
        PskDialogField::Psk => (
            crate::tr!(crate::keys::CONSOLE_FLARE_PSK_LABEL),
            &app.deploy_psk,
        ),
        PskDialogField::Confirmation => (
            crate::tr!(crate::keys::CONSOLE_CONFIRM_PSK_LABEL),
            &app.deploy_psk_confirmation,
        ),
        PskDialogField::Action => unreachable!(),
    };
    let editing = active && app.deploy_psk_editing;
    let value_display = if editing || !masked {
        value.clone()
    } else if value.is_empty() {
        crate::tr!(crate::keys::CONSOLE_DEPLOY_FLARE_EMPTY)
    } else {
        super::render::mask(value)
    };
    let cursor = if editing { "_" } else { "" };
    let selected_style = if active {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    Line::from(vec![
        Span::styled(if active { "> " } else { "  " }, selected_style),
        Span::styled(
            super::render::display_pad(&label, 17),
            if active {
                selected_style
            } else {
                Style::default().fg(Color::DarkGray)
            },
        ),
        Span::styled(value_display, selected_style),
        Span::styled(cursor, selected_style),
    ])
}

/// psk 弹窗的动作行(部署/保存):聚焦时反色,未聚焦时绿字加粗。
fn dialog_action_row(active: bool, label: String) -> Line<'static> {
    let selected_style = if active {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let action_style = if active {
        selected_style
    } else {
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD)
    };
    Line::from(vec![
        Span::styled(if active { "> " } else { "  " }, selected_style),
        Span::styled(label, action_style),
    ])
}

pub(crate) fn render_show_psk_dialog(frame: &mut Frame<'_>, app: &mut ConsoleApp) {
    if !app.show_psk {
        return;
    }
    let screen = frame.area();
    let width = 88.min(screen.width.saturating_sub(2));
    let height = 12.min(screen.height.saturating_sub(2));
    let area = Rect::new(
        screen.x + screen.width.saturating_sub(width) / 2,
        screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    );
    register_dialog_hits(&mut app.hits, screen, area);
    frame.render_widget(Clear, area);
    let psk_display = if app.show_psk_value.is_empty() {
        crate::tr!(crate::keys::CONSOLE_SHOW_PSK_EMPTY)
    } else {
        app.show_psk_value.clone()
    };
    let confirmation_display = app.show_psk_confirmation.clone();
    let psk_row = show_psk_row(app, PskDialogField::Psk, psk_display);
    let confirmation_row = show_psk_row(app, PskDialogField::Confirmation, confirmation_display);
    let save_row = dialog_action_row(
        app.show_psk_field == PskDialogField::Action,
        crate::tr!(crate::keys::CONSOLE_SHOW_PSK_SAVE),
    );
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_SHOW_PSK_PURPOSE),
                Style::default().fg(Color::DarkGray),
            ),
            Line::raw(""),
            psk_row,
            confirmation_row,
            save_row,
            Line::raw(""),
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_SHOW_PSK_HINT),
                Style::default().fg(Color::Green),
            ),
        ])
        .wrap(Wrap { trim: true })
        .block(Block::bordered().title(crate::tr!(crate::keys::CONSOLE_SHOW_PSK_TITLE))),
        area,
    );
}

/// 查看/修改弹窗的字段行:恒为明文(查看分发用途),聚焦字段高亮并显示光标。
fn show_psk_row(app: &ConsoleApp, field: PskDialogField, value: String) -> Line<'static> {
    let active = app.show_psk_field == field;
    let editing = active && app.show_psk_editing;
    let (label, value) = match field {
        PskDialogField::Psk => (crate::tr!(crate::keys::CONSOLE_FLARE_PSK_LABEL), value),
        PskDialogField::Confirmation => (crate::tr!(crate::keys::CONSOLE_CONFIRM_PSK_LABEL), value),
        PskDialogField::Action => unreachable!(),
    };
    let cursor = if editing { "_" } else { "" };
    let selected_style = if active {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    Line::from(vec![
        Span::styled(if active { "> " } else { "  " }, selected_style),
        Span::styled(
            super::render::display_pad(&label, 17),
            if active {
                selected_style
            } else {
                Style::default().fg(Color::DarkGray)
            },
        ),
        Span::styled(value, selected_style),
        Span::styled(cursor, selected_style),
    ])
}

pub(crate) fn render_daemon_deploy_progress(frame: &mut Frame<'_>, app: &mut ConsoleApp) {
    if app.deploy_daemon.is_none() {
        return;
    }
    let screen = frame.area();
    let width = 64.min(screen.width.saturating_sub(2));
    let height = 5.min(screen.height.saturating_sub(2));
    let area = Rect::new(
        screen.x + screen.width.saturating_sub(width) / 2,
        screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    );
    register_modal_hits(&mut app.hits, screen, area);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_DEPLOY_DAEMON_RUNNING),
                Style::default().fg(Color::Cyan),
            ),
            Line::raw(""),
        ])
        .wrap(Wrap { trim: true })
        .block(panel_block(
            &crate::tr!(crate::keys::CONSOLE_DEPLOY_DAEMON_TITLE),
            true,
        )),
        area,
    );
}
