use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};

use super::render::panel_block;
use super::widgets::{Focus, Hit, block_row_of};
use super::{ConsoleAction, ConsoleApp};
use crate::commands::Commands;
use crate::network::config::NetworkPlan;

/// reinit 面板步骤：概览（开始）→ 网络向导 → 凭据与计划 → 确认层。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReinitStep {
    Overview,
    Credentials,
}

/// 重新初始化面板：展示适用性说明与版本摘要，向导收集网络计划，凭据表单收集
/// 新 admin 用户与密码（两次输入确认，与 install 面板一致），确认层说明清空范围与
/// 确认窗口后分发结构化 `Reinit` 请求。
pub(crate) struct ReinitPanel {
    pub(crate) step: ReinitStep,
    pub(crate) wizard: bool,
    pub(crate) plan: Option<NetworkPlan>,
    pub(crate) admin_user: String,
    pub(crate) password: String,
    pub(crate) password_confirmation: String,
    pub(crate) selected: usize,
    pub(crate) editing: bool,
    pub(crate) confirming: bool,
}

impl Default for ReinitPanel {
    fn default() -> Self {
        Self {
            step: ReinitStep::Overview,
            wizard: false,
            plan: None,
            admin_user: "admin".into(),
            password: String::new(),
            password_confirmation: String::new(),
            selected: 0,
            editing: false,
            confirming: false,
        }
    }
}

/// 面板适用性：已安装 + systemd 服务管理器 + 宿主网络服务已被接管。
/// 测试环境通过 `LKIT_TEST_REINIT_ELIGIBLE` 跳过 systemd 探测。
pub(crate) fn reinit_eligible(app: &ConsoleApp) -> bool {
    let installed_systemd = matches!(
        &app.snapshot,
        super::network_wizard::Snapshot::Installed { manager, .. } if *manager == "systemd"
    );
    if !installed_systemd {
        return false;
    }
    if std::env::var_os("LKIT_TEST_REINIT_ELIGIBLE").is_some() {
        return true;
    }
    crate::workflows::uninstall::host_network_services_masked(
        &crate::service::systemd::Systemd::host(),
    )
}

impl ReinitPanel {
    fn editable_value_mut(&mut self) -> Option<&mut String> {
        match self.selected {
            0 => Some(&mut self.admin_user),
            1 => Some(&mut self.password),
            2 => Some(&mut self.password_confirmation),
            _ => None,
        }
    }

    /// 凭据校验：admin 用户合法、两次密码输入一致且满足复杂度（与 install 面板相同）。
    fn validate_credentials(&self) -> Result<(), String> {
        crate::deployment::plan::validate_admin_user(&self.admin_user)
            .map_err(|error| error.to_string())?;
        if self.password != self.password_confirmation {
            return Err(crate::tr!(
                crate::keys::CONSOLE_PASSWORD_CONFIRMATION_MISMATCH
            ));
        }
        crate::interaction::credentials::validate_password(&self.password)
            .map_err(|error| error.to_string())
    }
}

impl ConsoleApp {
    /// reinit 面板键处理：概览 Enter 打开网络向导；凭据步骤编辑 admin 用户、密码与
    /// 密码确认，动作行校验后打开确认层；确认层 Enter 分发结构化 `Reinit` 请求。
    pub(crate) fn handle_reinit_key(&mut self, key: KeyEvent) -> Option<Option<ConsoleAction>> {
        if self.reinit.confirming {
            match key.code {
                KeyCode::Enter => {
                    self.reinit.confirming = false;
                    return Some(Some(self.reinit_action()));
                }
                KeyCode::Esc => self.reinit.confirming = false,
                _ => {}
            }
            return Some(None);
        }
        if self.reinit.editing {
            match key.code {
                KeyCode::Enter | KeyCode::Esc => self.reinit.editing = false,
                KeyCode::Backspace => {
                    self.reinit.editable_value_mut().map(String::pop);
                }
                KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    if let Some(value) = self.reinit.editable_value_mut()
                        && value.chars().count() < 1024
                    {
                        value.push(character);
                    }
                }
                _ => {}
            }
            return Some(None);
        }
        match self.reinit.step {
            ReinitStep::Overview => match key.code {
                KeyCode::Enter | KeyCode::Char(' ') => {
                    match super::network_wizard::NetworkWizard::discover() {
                        Ok(wizard) => {
                            self.network_wizard = Some(wizard);
                            self.reinit.wizard = true;
                            self.notice =
                                crate::tr!(crate::keys::CONSOLE_CONFIGURE_NETWORK_TAKEOVER);
                        }
                        Err(error) => self.notice = error,
                    }
                }
                _ => return None,
            },
            ReinitStep::Credentials => match key.code {
                KeyCode::Up => {
                    self.reinit.selected = 0;
                }
                KeyCode::Down => {
                    self.reinit.selected = (self.reinit.selected + 1).min(3);
                }
                KeyCode::Enter | KeyCode::Char(' ') => match self.reinit.selected {
                    0..=2 => self.reinit.editing = true,
                    3 => {
                        if self.reinit.plan.is_none() {
                            self.notice = crate::tr!(crate::keys::CONSOLE_REINIT_PLAN_MISSING);
                            return Some(None);
                        }
                        if let Err(error) = self.reinit.validate_credentials() {
                            self.notice = error;
                            return Some(None);
                        }
                        self.reinit.confirming = true;
                    }
                    _ => {}
                },
                _ => return None,
            },
        }
        Some(None)
    }

    /// 确认层 Enter：构建带 `--console-confirmed` 与 `--yes` 的结构化 `Reinit` 请求。
    fn reinit_action(&self) -> ConsoleAction {
        let command = Commands::Reinit(crate::commands::reinit::Reinit {
            admin_user: Some(self.reinit.admin_user.clone()),
            password_file: None,
            interactive_password: Some(self.reinit.password.clone()),
            allow_no_backup: false,
            yes: true,
            console_confirmed: true,
            network_plan: self.reinit.plan.clone(),
            network_plan_file: None,
            #[cfg(feature = "test-support")]
            test_runtime: None,
        });
        let args = vec![
            "reinit".into(),
            "--yes".into(),
            "--console-confirmed".into(),
            "--admin-user".into(),
            self.reinit.admin_user.clone(),
        ];
        ConsoleAction::Command { command, args }
    }
}

pub(crate) fn render_reinit(frame: &mut Frame<'_>, app: &mut ConsoleApp, area: Rect) {
    let focused = app.focus == Focus::Panel;
    let eligible = reinit_eligible(app);
    let mut lines = Vec::new();
    if let super::network_wizard::Snapshot::Installed {
        version,
        manager,
        initialized,
    } = &app.snapshot
    {
        lines.push(Line::styled(
            crate::tr!(
                crate::keys::CONSOLE_REINIT_VERSION_LABEL,
                version = version,
                manager = manager,
                initialized = if *initialized { "complete" } else { "pending" }
            ),
            Style::default().fg(Color::Green),
        ));
    }
    lines.push(Line::raw(""));
    if !eligible {
        lines.push(Line::styled(
            crate::tr!(crate::keys::CONSOLE_REINIT_UNAVAILABLE),
            Style::default().fg(Color::Yellow),
        ));
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            crate::tr!(crate::keys::CONSOLE_REINIT_UNAVAILABLE_HINT),
            Style::default().fg(Color::DarkGray),
        ));
    } else if app.reinit.step == ReinitStep::Overview {
        lines.push(Line::styled(
            crate::tr!(crate::keys::CONSOLE_REINIT_SUMMARY),
            Style::default(),
        ));
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            crate::tr!(crate::keys::CONSOLE_REINIT_WIPE_SCOPE),
            Style::default().fg(Color::Yellow),
        ));
        lines.push(Line::raw(""));
        let begin_row = lines.len();
        let begin = crate::tr!(crate::keys::CONSOLE_REINIT_BEGIN);
        if focused {
            app.hits
                .add(block_row_area(area, &lines, begin_row), Hit::ReinitAction);
        }
        lines.push(Line::styled(
            format!("{} {begin}", if focused { ">" } else { " " }),
            Style::default().add_modifier(if focused {
                Modifier::REVERSED | Modifier::BOLD
            } else {
                Modifier::BOLD
            }),
        ));
    } else {
        let rows = [
            crate::tr!(crate::keys::CONSOLE_ADMIN_USER_LABEL),
            crate::tr!(crate::keys::CONSOLE_PASSWORD_LABEL),
            crate::tr!(crate::keys::CONSOLE_CONFIRM_PASSWORD_LABEL),
        ];
        for (index, label) in rows.iter().enumerate() {
            let row_index = lines.len();
            if focused {
                app.hits.add(
                    block_row_area(area, &lines, row_index),
                    Hit::ReinitField(index),
                );
            }
            let value = match index {
                0 => app.reinit.admin_user.clone(),
                1 => super::render::mask(&app.reinit.password),
                _ => super::render::mask(&app.reinit.password_confirmation),
            };
            let cursor = if focused && app.reinit.selected == index {
                if app.reinit.editing {
                    format!("{value}▍")
                } else {
                    format!("> {label}: {value}")
                }
            } else {
                format!("  {label}: {value}")
            };
            lines.push(Line::styled(
                cursor,
                Style::default().add_modifier(if focused && app.reinit.selected == index {
                    Modifier::REVERSED
                } else {
                    Modifier::empty()
                }),
            ));
        }
        lines.push(Line::raw(""));
        if let Some(plan) = app.reinit.plan.as_ref() {
            let lan = plan.lan();
            let lan = if lan.is_empty() {
                crate::tr!(crate::keys::CONSOLE_REINIT_LAN_NONE)
            } else {
                lan.join(", ")
            };
            lines.push(Line::styled(
                crate::tr!(
                    crate::keys::CONSOLE_REINIT_PLAN_SUMMARY,
                    wan = plan.wan(),
                    lan = lan
                ),
                Style::default().fg(Color::Cyan),
            ));
            lines.push(Line::raw(""));
        }
        let execute_row = lines.len();
        let execute = crate::tr!(crate::keys::CONSOLE_REINIT_EXECUTE);
        if focused {
            app.hits
                .add(block_row_area(area, &lines, execute_row), Hit::ReinitAction);
        }
        lines.push(Line::styled(
            format!(
                "{} {execute}",
                if focused && app.reinit.selected == 3 {
                    ">"
                } else {
                    " "
                }
            ),
            Style::default().add_modifier(if focused && app.reinit.selected == 3 {
                Modifier::REVERSED | Modifier::BOLD
            } else {
                Modifier::BOLD
            }),
        ));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel_block(
                &crate::tr!(crate::keys::CONSOLE_REINIT_MENU),
                focused,
            ))
            .wrap(Wrap { trim: true }),
        area,
    );
}

/// 计算 `lines` 中 `row_index` 行在带边框面板内容区内的实际行偏移（模拟换行），
/// 并返回整行的可点击区域。内容行本身不换行，前面的长文本行可能换行。
fn block_row_area(area: Rect, lines: &[Line], row_index: usize) -> Rect {
    let width = area.width.saturating_sub(2);
    let row = block_row_of(lines, row_index, width);
    Rect::new(
        area.x.saturating_add(1),
        area.y.saturating_add(1).saturating_add(row),
        area.width.saturating_sub(2),
        1,
    )
}

/// 确认层：清空范围、保护备份与确认窗口说明。
pub(crate) fn render_reinit_confirmation(frame: &mut Frame<'_>, app: &mut ConsoleApp) {
    let screen = frame.area();
    let width = 76.min(screen.width.saturating_sub(2));
    let height = 12.min(screen.height.saturating_sub(2));
    let area = Rect::new(
        screen.x + screen.width.saturating_sub(width) / 2,
        screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    );
    super::render::register_dialog_hits(&mut app.hits, screen, area);
    frame.render_widget(Clear, area);
    let lines = vec![
        Line::styled(
            crate::tr!(crate::keys::CONSOLE_REINIT_CONFIRM_WIPE),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
        Line::styled(
            crate::tr!(crate::keys::CONSOLE_REINIT_CONFIRM_WINDOW),
            Style::default().fg(Color::Yellow),
        ),
        Line::raw(""),
        Line::raw(crate::tr!(crate::keys::CONSOLE_REINIT_CONFIRM_BACKUP)),
        Line::raw(""),
        Line::raw(crate::tr!(crate::keys::CONSOLE_REINIT_CONFIRM_PROMPT)),
        Line::styled(
            crate::tr!(crate::keys::CONSOLE_PRESS_ESC_TO_CANCEL),
            Style::default().fg(Color::DarkGray),
        ),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title(crate::tr!(crate::keys::CONSOLE_REINIT_CONFIRM_TITLE)))
            .wrap(Wrap { trim: true }),
        area,
    );
    app.hits.add(area, Hit::DialogConfirm);
}
