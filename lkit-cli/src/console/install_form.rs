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
use crate::commands::Commands;
use crate::commands::install::Install;
use crate::deployment::plan;
use crate::interaction::credentials;
use crate::network::config::NetworkPlan;

// TODO(network-takeover): 处理完不同发行版网络服务差异后恢复网络接管开关:
// 在 `InstallField::ALL` 中于 `PasswordConfirmation` 之后插入 `NetworkTakeover`
// 变体(表单高度 `FORM_FIELDS` 自动加一),并把 `InstallForm::default` 的
// `takeover_network` 改回 false。
pub(crate) const FORM_FIELDS: usize = InstallField::ALL.len();

/// 安装表单字段:声明顺序即表单次序,`StartInstallation` 恒为最后一个字段。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InstallField {
    Version,
    Repository,
    RepositoryUrl,
    InstallRoot,
    AdminUser,
    Password,
    PasswordConfirmation,
    // TODO(network-takeover): 恢复网络接管开关时在此插入 NetworkTakeover。
    StartInstallation,
}

impl InstallField {
    pub(crate) const ALL: [Self; 8] = [
        Self::Version,
        Self::Repository,
        Self::RepositoryUrl,
        Self::InstallRoot,
        Self::AdminUser,
        Self::Password,
        Self::PasswordConfirmation,
        Self::StartInstallation,
    ];

    /// 该字段是否在表单中可见(仓库 URL 仅在 Custom 模式下显示)。
    fn visible(self, repository: RepositoryMode) -> bool {
        match self {
            Self::RepositoryUrl => repository == RepositoryMode::Custom,
            _ => true,
        }
    }

    /// 当前模式下可见字段的有序列表(与渲染次序一致)。
    fn visible_fields(repository: RepositoryMode) -> Vec<Self> {
        Self::ALL
            .iter()
            .copied()
            .filter(|field| field.visible(repository))
            .collect()
    }

    fn label(self) -> String {
        match self {
            Self::Version => crate::tr!(crate::keys::CONSOLE_VERSION_LABEL),
            Self::Repository => crate::tr!(crate::keys::CONSOLE_REPOSITORY_LABEL),
            Self::RepositoryUrl => crate::tr!(crate::keys::CONSOLE_REPOSITORY_URL_LABEL),
            Self::InstallRoot => crate::tr!(crate::keys::CONSOLE_INSTALL_ROOT_LABEL),
            Self::AdminUser => crate::tr!(crate::keys::CONSOLE_ADMIN_USER_LABEL),
            Self::Password => crate::tr!(crate::keys::CONSOLE_PASSWORD_LABEL),
            Self::PasswordConfirmation => {
                crate::tr!(crate::keys::CONSOLE_CONFIRM_PASSWORD_LABEL)
            }
            Self::StartInstallation => String::new(),
        }
    }

    fn value(self, form: &InstallForm) -> String {
        match self {
            Self::Version => form.version.clone(),
            Self::Repository => form.repository.label(),
            Self::RepositoryUrl => form.repository_url.clone(),
            Self::InstallRoot => form.install_dir.clone(),
            Self::AdminUser => form.admin_user.clone(),
            Self::Password => mask(&form.password),
            Self::PasswordConfirmation => mask(&form.password_confirmation),
            Self::StartInstallation => {
                crate::tr!(crate::keys::CONSOLE_START_INSTALLATION_BUTTON)
            }
        }
    }
}

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

#[derive(Clone)]
pub(crate) struct InstallForm {
    pub(crate) version: String,
    pub(crate) repository: RepositoryMode,
    pub(crate) repository_url: String,
    pub(crate) install_dir: String,
    pub(crate) admin_user: String,
    pub(crate) password: String,
    pub(crate) password_confirmation: String,
    pub(crate) takeover_network: bool,
    pub(crate) selected: InstallField,
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
            // TODO(network-takeover): 开关暂隐藏且恒为 true,console 安装始终走网络接管;
            // 处理完不同发行版网络服务差异后恢复开关并把默认改回 false。
            takeover_network: true,
            selected: InstallField::Version,
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
        let fields = InstallField::visible_fields(self.repository);
        if self.selected == fields[0] {
            self.checks_selected = true;
            return;
        }
        let index = fields
            .iter()
            .position(|field| *field == self.selected)
            .unwrap_or(0);
        self.selected = fields[index.saturating_sub(1)];
    }

    pub(crate) fn select_next(&mut self) {
        if self.checks_selected {
            self.checks_selected = false;
            self.selected = InstallField::visible_fields(self.repository)[0];
            return;
        }
        let fields = InstallField::visible_fields(self.repository);
        let index = fields
            .iter()
            .position(|field| *field == self.selected)
            .unwrap_or(0);
        self.selected = fields[(index + 1).min(fields.len() - 1)];
    }

    pub(crate) fn selected_help(&self) -> (String, String) {
        match self.selected {
            InstallField::Version => (
                crate::tr!(crate::keys::CONSOLE_VERSION_LABEL),
                crate::tr!(crate::keys::CONSOLE_VERSION_HELP),
            ),
            InstallField::Repository => (
                crate::tr!(crate::keys::CONSOLE_REPOSITORY_LABEL),
                crate::tr!(crate::keys::CONSOLE_REPOSITORY_HELP),
            ),
            InstallField::RepositoryUrl => (
                crate::tr!(crate::keys::CONSOLE_REPOSITORY_URL_LABEL),
                crate::tr!(crate::keys::CONSOLE_REPOSITORY_URL_HELP),
            ),
            InstallField::InstallRoot => (
                crate::tr!(crate::keys::CONSOLE_INSTALL_ROOT_LABEL),
                crate::tr!(crate::keys::CONSOLE_INSTALL_ROOT_HELP),
            ),
            InstallField::AdminUser => (
                crate::tr!(crate::keys::CONSOLE_ADMIN_USER_LABEL),
                crate::tr!(crate::keys::CONSOLE_ADMIN_USER_HELP),
            ),
            InstallField::Password => (
                crate::tr!(crate::keys::CONSOLE_PASSWORD_LABEL),
                crate::tr!(crate::keys::CONSOLE_PASSWORD_HELP),
            ),
            InstallField::PasswordConfirmation => (
                crate::tr!(crate::keys::CONSOLE_CONFIRM_PASSWORD_LABEL),
                crate::tr!(crate::keys::CONSOLE_CONFIRM_PASSWORD_HELP),
            ),
            // TODO(network-takeover): 恢复网络接管开关时放开下面被注释的接管 arm。
            // InstallField::NetworkTakeover => (
            //     crate::tr!(crate::keys::CONSOLE_NETWORK_TAKEOVER_LABEL),
            //     crate::tr!(crate::keys::CONSOLE_NETWORK_TAKEOVER_HELP),
            // ),
            InstallField::StartInstallation => (
                crate::tr!(crate::keys::CONSOLE_START_INSTALLATION_LABEL),
                crate::tr!(crate::keys::CONSOLE_START_INSTALLATION_HELP),
            ),
        }
    }

    pub(crate) fn editable_value_mut(&mut self) -> Option<&mut String> {
        match self.selected {
            InstallField::Version => Some(&mut self.version),
            InstallField::RepositoryUrl if self.repository == RepositoryMode::Custom => {
                Some(&mut self.repository_url)
            }
            InstallField::InstallRoot => Some(&mut self.install_dir),
            InstallField::AdminUser => Some(&mut self.admin_user),
            InstallField::Password => Some(&mut self.password),
            InstallField::PasswordConfirmation => Some(&mut self.password_confirmation),
            _ => None,
        }
    }

    pub(crate) fn change_choice(&mut self, forward: bool) {
        if self.selected == InstallField::Repository {
            self.repository.change(forward);
            // TODO(network-takeover): 恢复网络接管开关时放开:
            // InstallField::NetworkTakeover => self.takeover_network = !self.takeover_network,
        }
    }

    pub(crate) fn activate(&mut self) -> Result<Option<ConsoleAction>, String> {
        match self.selected {
            InstallField::Version
            | InstallField::InstallRoot
            | InstallField::AdminUser
            | InstallField::Password
            | InstallField::PasswordConfirmation => {
                self.editing = true;
                Ok(None)
            }
            InstallField::RepositoryUrl if self.repository == RepositoryMode::Custom => {
                self.editing = true;
                Ok(None)
            }
            InstallField::Repository => {
                self.change_choice(true);
                Ok(None)
            }
            // TODO(network-takeover): 恢复网络接管开关时在此加入接管切换 arm。
            InstallField::StartInstallation => self.command().map(Some),
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
            return Err(crate::tr!(
                crate::keys::CONSOLE_PASSWORD_CONFIRMATION_MISMATCH
            ));
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
        // 提前检查委托前置条件:root 下 daemon 未运行或无法 spawn worker 时
        // 留在 TUI 内提示,而不是退出控制台后在 delegate() 才失败(用户已
        // 完成全部安装参数)。
        match crate::daemon_worker::delegation_block() {
            Some(crate::daemon_worker::DelegationBlock::DaemonNotRunning) => {
                return Err(crate::tr!(crate::keys::CONSOLE_DAEMON_NOT_RUNNING_NOTICE));
            }
            Some(crate::daemon_worker::DelegationBlock::WorkerSpawnUnavailable) => {
                return Err(crate::tr!(
                    crate::keys::CONSOLE_DAEMON_SPAWN_UNAVAILABLE_NOTICE
                ));
            }
            None => {}
        }
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
    let mut form_rows: Vec<(InstallField, Line)> = Vec::new();
    for field in InstallField::ALL {
        if !field.visible(form.repository) {
            continue;
        }
        let selected = app.focus == Focus::Panel && !form.checks_selected && form.selected == field;
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
        } else if field == InstallField::StartInstallation {
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        let line = Line::from(vec![
            Span::styled(if selected { "> " } else { "  " }, selected_style),
            Span::styled(
                display_pad(&field.label(), 17),
                if selected {
                    selected_style
                } else {
                    Style::default().fg(Color::DarkGray)
                },
            ),
            Span::styled(field.value(form), value_style),
            Span::styled(
                if selected && form.editing { "_" } else { "" },
                selected_style,
            ),
        ]);
        form_rows.push((
            field,
            if selected {
                line.style(Style::default().bg(Color::Cyan))
            } else {
                line
            },
        ));
    }
    let content_width = area.width.saturating_sub(2);
    let lines: Vec<Line> = form_rows.iter().map(|(_, line)| line.clone()).collect();
    for (row, (field, _)) in form_rows.iter().enumerate() {
        app.hits.block_row(
            area,
            block_row_of(&lines, row, content_width),
            Hit::InstallField(*field),
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
