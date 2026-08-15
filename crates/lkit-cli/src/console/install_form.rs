use std::path::PathBuf;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};

use super::preflight::{render_preflight_details, render_preflight_summary};
use super::render::{display_pad, mask, panel_block};
use super::widgets::{Focus, Hit, block_row_of};
use super::{ConsoleAction, ConsoleApp};
use crate::commands::install::Install;
use crate::commands::{Commands, ServiceManagerArg};
use crate::deployment::plan;
use crate::interaction::credentials;
use crate::network::config::NetworkPlan;

// TODO(network-takeover): 处理完不同发行版网络服务差异后恢复网络接管开关:
// `FORM_FIELDS` 改回 10,恢复下方被注释的字段 8 代码,并把
// `InstallForm::default` 的 `takeover_network` 改回 false。
pub(crate) const FORM_FIELDS: usize = 9;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RepositoryMode {
    Default,
    Github,
    Mirror,
    Custom,
}

impl RepositoryMode {
    fn label(self) -> String {
        match self {
            Self::Default => crate::tr!(crate::keys::CONSOLE_REPOSITORY_DEFAULT),
            Self::Github => "GitHub".into(),
            Self::Mirror => crate::tr!(crate::keys::CONSOLE_REPOSITORY_MIRROR),
            Self::Custom => crate::tr!(crate::keys::CONSOLE_REPOSITORY_CUSTOM),
        }
    }

    fn change(&mut self, forward: bool) {
        *self = match (*self, forward) {
            (Self::Default, true) => Self::Github,
            (Self::Github, true) => Self::Mirror,
            (Self::Mirror, true) => Self::Custom,
            (Self::Custom, true) => Self::Default,
            (Self::Default, false) => Self::Custom,
            (Self::Custom, false) => Self::Mirror,
            (Self::Mirror, false) => Self::Github,
            (Self::Github, false) => Self::Default,
        };
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ManagerMode {
    Auto,
    Systemd,
    None,
}

impl ManagerMode {
    fn label(self) -> String {
        match self {
            Self::Auto => crate::tr!(crate::keys::CONSOLE_MANAGER_AUTO),
            Self::Systemd => "systemd".into(),
            Self::None => "none".into(),
        }
    }

    fn change(&mut self, forward: bool) {
        *self = match (*self, forward) {
            (Self::Auto, true) | (Self::None, false) => Self::Systemd,
            (Self::Systemd, true) => Self::None,
            (Self::Systemd, false) => Self::Auto,
            (Self::None, true) => Self::Auto,
            (Self::Auto, false) => Self::None,
        };
    }

    fn cli(self) -> Option<ServiceManagerArg> {
        match self {
            Self::Auto => None,
            Self::Systemd => Some(ServiceManagerArg::Systemd),
            Self::None => Some(ServiceManagerArg::None),
        }
    }
}

#[derive(Clone)]
pub(crate) struct InstallForm {
    pub(crate) version: String,
    pub(crate) repository: RepositoryMode,
    pub(crate) repository_url: String,
    pub(crate) install_dir: String,
    pub(crate) admin_user: String,
    pub(crate) password: String,
    pub(crate) password_confirmation: String,
    pub(crate) manager: ManagerMode,
    pub(crate) takeover_network: bool,
    pub(crate) selected: usize,
    pub(crate) checks_selected: bool,
    pub(crate) editing: bool,
}

impl Default for InstallForm {
    fn default() -> Self {
        Self {
            version: "latest".into(),
            repository: RepositoryMode::Default,
            repository_url: plan::DEFAULT_HTTP_MIRROR.into(),
            install_dir: std::env::var("LKIT_INSTALL_DIR")
                .unwrap_or_else(|_| plan::DEFAULT_INSTALL_ROOT.into()),
            admin_user: "admin".into(),
            password: String::new(),
            password_confirmation: String::new(),
            manager: ManagerMode::Auto,
            // TODO(network-takeover): 开关暂隐藏且恒为 true,console 安装始终走网络接管;
            // 处理完不同发行版网络服务差异后恢复开关并把默认改回 false。
            takeover_network: true,
            selected: 0,
            checks_selected: true,
            editing: false,
        }
    }
}

impl InstallForm {
    pub(crate) fn select_previous(&mut self) {
        if self.checks_selected {
            return;
        }
        if self.selected == 0 {
            self.checks_selected = true;
            return;
        }
        self.selected = self.selected.saturating_sub(1);
        if self.selected == 2 && self.repository != RepositoryMode::Custom {
            self.selected = 1;
        }
    }

    pub(crate) fn select_next(&mut self) {
        if self.checks_selected {
            self.checks_selected = false;
            self.selected = 0;
            return;
        }
        self.selected = (self.selected + 1).min(FORM_FIELDS - 1);
        if self.selected == 2 && self.repository != RepositoryMode::Custom {
            self.selected = 3;
        }
    }

    pub(crate) fn selected_help(&self) -> (String, String) {
        match self.selected {
            0 => (
                crate::tr!(crate::keys::CONSOLE_VERSION_LABEL),
                crate::tr!(crate::keys::CONSOLE_VERSION_HELP),
            ),
            1 => (
                crate::tr!(crate::keys::CONSOLE_REPOSITORY_LABEL),
                crate::tr!(crate::keys::CONSOLE_REPOSITORY_HELP),
            ),
            2 => (
                crate::tr!(crate::keys::CONSOLE_REPOSITORY_URL_LABEL),
                crate::tr!(crate::keys::CONSOLE_REPOSITORY_URL_HELP),
            ),
            3 => (
                crate::tr!(crate::keys::CONSOLE_INSTALL_ROOT_LABEL),
                crate::tr!(crate::keys::CONSOLE_INSTALL_ROOT_HELP),
            ),
            4 => (
                crate::tr!(crate::keys::CONSOLE_ADMIN_USER_LABEL),
                crate::tr!(crate::keys::CONSOLE_ADMIN_USER_HELP),
            ),
            5 => (
                crate::tr!(crate::keys::CONSOLE_PASSWORD_LABEL),
                crate::tr!(crate::keys::CONSOLE_PASSWORD_HELP),
            ),
            6 => (
                crate::tr!(crate::keys::CONSOLE_CONFIRM_PASSWORD_LABEL),
                crate::tr!(crate::keys::CONSOLE_CONFIRM_PASSWORD_HELP),
            ),
            7 => (
                crate::tr!(crate::keys::CONSOLE_SERVICE_MANAGER_LABEL),
                crate::tr!(crate::keys::CONSOLE_SERVICE_MANAGER_HELP),
            ),
            // TODO(network-takeover): 恢复网络接管开关时,该字段回到索引 8,
            // 开始安装回到索引 9,并放开下面被注释的接管 arm。
            // 8 => (
            //     crate::tr!(crate::keys::CONSOLE_NETWORK_TAKEOVER_LABEL),
            //     crate::tr!(crate::keys::CONSOLE_NETWORK_TAKEOVER_HELP),
            // ),
            8 => (
                crate::tr!(crate::keys::CONSOLE_START_INSTALLATION_LABEL),
                crate::tr!(crate::keys::CONSOLE_START_INSTALLATION_HELP),
            ),
            _ => (
                crate::tr!(crate::keys::CONSOLE_INSTALL_MENU),
                crate::tr!(crate::keys::CONSOLE_INSTALL_HELP_FALLBACK_DESC),
            ),
        }
    }

    pub(crate) fn editable_value_mut(&mut self) -> Option<&mut String> {
        match self.selected {
            0 => Some(&mut self.version),
            2 if self.repository == RepositoryMode::Custom => Some(&mut self.repository_url),
            3 => Some(&mut self.install_dir),
            4 => Some(&mut self.admin_user),
            5 => Some(&mut self.password),
            6 => Some(&mut self.password_confirmation),
            _ => None,
        }
    }

    pub(crate) fn change_choice(&mut self, forward: bool) {
        match self.selected {
            1 => self.repository.change(forward),
            7 => self.manager.change(forward),
            // TODO(network-takeover): 恢复网络接管开关时放开:
            // 8 => self.takeover_network = !self.takeover_network,
            _ => {}
        }
    }

    pub(crate) fn activate(&mut self) -> Result<Option<ConsoleAction>, String> {
        match self.selected {
            0 | 3 | 4 | 5 | 6 => {
                self.editing = true;
                Ok(None)
            }
            2 if self.repository == RepositoryMode::Custom => {
                self.editing = true;
                Ok(None)
            }
            // TODO(network-takeover): 恢复网络接管开关时改回 `1 | 7 | 8`。
            1 | 7 => {
                self.change_choice(true);
                Ok(None)
            }
            // TODO(network-takeover): 恢复网络接管开关时改回 `9`。
            8 => self.command().map(Some),
            _ => Ok(None),
        }
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        let version = self.version.trim();
        plan::TargetVersion::parse(version).map_err(|error| error.to_string())?;
        plan::validate_admin_user(&self.admin_user).map_err(|error| error.to_string())?;
        if self.repository == RepositoryMode::Custom {
            plan::RepositoryChoice::Http(self.repository_url.trim().to_string())
                .resolve()
                .map_err(|error| error.to_string())?;
        }
        if self.password != self.password_confirmation {
            return Err(crate::tr!(crate::keys::CONSOLE_PASSWORD_CONFIRMATION_MISMATCH).into());
        }
        credentials::validate_password(&self.password).map_err(|error| error.to_string())?;
        Ok(())
    }

    pub(crate) fn command(&mut self) -> Result<ConsoleAction, String> {
        self.command_with_network_plan(None)
    }

    pub(crate) fn command_with_network_plan(
        &mut self,
        network_plan: Option<NetworkPlan>,
    ) -> Result<ConsoleAction, String> {
        self.validate()?;
        let version = self.version.trim();
        let requested_install_dir = PathBuf::from(&self.install_dir);
        let install_dir = plan::select_install_root(Some(&requested_install_dir), None)
            .map_err(|error| error.to_string())?;
        let repository = match self.repository {
            RepositoryMode::Default => None,
            RepositoryMode::Github => Some(Some("github".into())),
            RepositoryMode::Mirror => Some(None),
            RepositoryMode::Custom => Some(Some(self.repository_url.trim().to_string())),
        };
        let password = std::mem::take(&mut self.password);
        self.password_confirmation.clear();
        let install = Install {
            version: Some(version.to_string()),
            repository: repository.clone(),
            install_dir: Some(install_dir.clone()),
            admin_user: Some(self.admin_user.clone()),
            password_file: None,
            interactive_password: Some(password),
            service_manager: self.manager.cli(),
            force: false,
            takeover_network: self.takeover_network,
            network_plan,
            network_plan_file: None,
            #[cfg(feature = "test-support")]
            test_runtime: None,
        };
        let mut args = vec!["install".into(), "--version".into(), version.into()];
        match &repository {
            None => {}
            Some(Some(value)) if value == "github" => {
                args.extend(["--repository".into(), "github".into()])
            }
            Some(None) => args.push("--repository".into()),
            Some(Some(url)) => args.extend(["--repository".into(), url.clone()]),
        }
        args.extend([
            "--install-dir".into(),
            install_dir.display().to_string(),
            "--admin-user".into(),
            self.admin_user.clone(),
        ]);
        if let Some(manager) = self.manager.cli() {
            args.extend([
                "--service-manager".into(),
                match manager {
                    ServiceManagerArg::Systemd => "systemd".into(),
                    ServiceManagerArg::None => "none".into(),
                },
            ]);
        }
        if self.takeover_network {
            args.push("--takeover-network".into());
        }
        Ok(ConsoleAction::Command {
            command: Commands::Install(install),
            args,
        })
    }
}
pub(crate) fn render_install(frame: &mut Frame<'_>, app: &mut ConsoleApp, area: Rect) {
    if app.preflight.expanded {
        render_preflight_details(frame, &app.preflight, app.focus == Focus::Panel, area);
        return;
    }
    let [checks_area, content_area] =
        Layout::vertical([Constraint::Length(3), Constraint::Min(8)]).areas(area);
    render_preflight_summary(frame, app, checks_area);
    let form_height = if app.install.repository == RepositoryMode::Custom {
        FORM_FIELDS as u16 + 2
    } else {
        FORM_FIELDS as u16 + 1
    };
    if content_area.width >= 72 {
        let [form_area, help_area] =
            Layout::horizontal([Constraint::Min(40), Constraint::Length(32)]).areas(content_area);
        render_install_form(frame, app, form_area);
        render_install_help(frame, app, help_area);
    } else if content_area.height >= form_height + 3 {
        let [form_area, help_area] =
            Layout::vertical([Constraint::Length(form_height), Constraint::Min(3)])
                .areas(content_area);
        render_install_form(frame, app, form_area);
        render_install_help(frame, app, help_area);
    } else {
        render_install_form(frame, app, content_area);
    }
}
pub(crate) fn render_install_form(frame: &mut Frame<'_>, app: &mut ConsoleApp, area: Rect) {
    let form = &app.install;
    let values = [
        form.version.clone(),
        form.repository.label().into(),
        form.repository_url.clone(),
        form.install_dir.clone(),
        form.admin_user.clone(),
        mask(&form.password),
        mask(&form.password_confirmation),
        form.manager.label().into(),
        // TODO(network-takeover): 恢复网络接管开关时放开该行。
        // if form.takeover_network { "[x]" } else { "[ ]" }.into(),
        crate::tr!(crate::keys::CONSOLE_START_INSTALLATION_BUTTON).into(),
    ];
    let labels = [
        crate::tr!(crate::keys::CONSOLE_VERSION_LABEL),
        crate::tr!(crate::keys::CONSOLE_REPOSITORY_LABEL),
        crate::tr!(crate::keys::CONSOLE_REPOSITORY_URL_LABEL),
        crate::tr!(crate::keys::CONSOLE_INSTALL_ROOT_LABEL),
        crate::tr!(crate::keys::CONSOLE_ADMIN_USER_LABEL),
        crate::tr!(crate::keys::CONSOLE_PASSWORD_LABEL),
        crate::tr!(crate::keys::CONSOLE_CONFIRM_PASSWORD_LABEL),
        crate::tr!(crate::keys::CONSOLE_SERVICE_MANAGER_LABEL),
        // TODO(network-takeover): 恢复网络接管开关时放开该行。
        // crate::tr!(crate::keys::CONSOLE_NETWORK_TAKEOVER_LABEL),
        String::new(),
    ];
    let mut form_rows: Vec<(usize, Line)> = Vec::new();
    for (index, (label, value)) in labels.iter().zip(values).enumerate() {
        if index == 2 && form.repository != RepositoryMode::Custom {
            continue;
        }
        let selected = app.focus == Focus::Panel && !form.checks_selected && form.selected == index;
        let selected_style = if selected {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let value_style = if selected {
            selected_style
        } else if index == 8 {
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        let line = Line::from(vec![
            Span::styled(if selected { "> " } else { "  " }, selected_style),
            Span::styled(
                display_pad(label, 17),
                if selected {
                    selected_style
                } else {
                    Style::default().fg(Color::DarkGray)
                },
            ),
            Span::styled(value, value_style),
            Span::styled(
                if selected && form.editing { "_" } else { "" },
                selected_style,
            ),
        ]);
        form_rows.push((
            index,
            if selected {
                line.style(Style::default().bg(Color::Cyan))
            } else {
                line
            },
        ));
    }
    let content_width = area.width.saturating_sub(2);
    let lines: Vec<Line> = form_rows.iter().map(|(_, line)| line.clone()).collect();
    for (row, (index, _)) in form_rows.iter().enumerate() {
        app.hits.block_row(
            area,
            block_row_of(&lines, row, content_width),
            Hit::InstallField(*index),
        );
    }
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .block(panel_block(
                &crate::tr!(crate::keys::CONSOLE_INSTALL_MENU),
                app.focus == Focus::Panel && !form.checks_selected,
            )),
        area,
    );
}
pub(crate) fn render_install_help(frame: &mut Frame<'_>, app: &ConsoleApp, area: Rect) {
    let (title, description) = if app.install.checks_selected {
        (
            crate::tr!(crate::keys::CONSOLE_ENVIRONMENT_CHECKS),
            crate::tr!(crate::keys::CONSOLE_ENVIRONMENT_CHECKS_HELP),
        )
    } else {
        app.install.selected_help()
    };
    frame.render_widget(
        Paragraph::new(description)
            .block(
                Block::bordered()
                    .title(crate::tr!(crate::keys::CONSOLE_ABOUT_PREFIX, title = title)),
            )
            .wrap(Wrap { trim: true }),
        area,
    );
}
