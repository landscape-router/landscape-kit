use std::sync::mpsc::{self, Receiver, TryRecvError};

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::network_wizard::Snapshot;
use super::render::{display_pad, panel_block, register_dialog_hits};
use super::widgets::{Focus, Hit, block_row_of};
use super::{ConsoleAction, ConsoleApp, Notice};
use crate::commands::Commands;
use crate::commands::update::{ResolvedUpdate, resolve_update_target};
use crate::deployment::config::{RepositorySource, RepositorySourceKind};
use crate::deployment::{plan, state};

/// Update 面板后台解析：与命令模式 `lkit update` 相同的状态发现、
/// 来源解析与目标版本解析/比较（复用 `resolve_update_target`），网络只读，零副作用。
fn resolve_update_from_console(
    repository: &plan::RepositoryChoice,
    version: &str,
) -> Result<ResolvedUpdate, String> {
    let root = state::discover_landscape_root()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| crate::tr!(crate::keys::MANAGE_COMMAND_REQUIRES_EXISTING_INSTALLATION))?;
    let state = state::load_state(&root)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| crate::tr!(crate::keys::MANAGE_COMMAND_REQUIRES_EXISTING_INSTALLATION))?;
    let target = plan::TargetVersion::parse(version).map_err(|error| error.to_string())?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("build update resolve runtime: {error}"))?;
    runtime
        .block_on(resolve_update_target(&state, repository, &target))
        .map_err(|error| error.to_string())
}
/// Update 面板的仓库来源选择,选项顺序与命令模式 `lkit update` 的渠道列表一致。
/// Current 只在 `config.toml` 存在且有效时提供。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UpdateRepositoryMode {
    Current,
    Github,
    Mirror,
    Custom,
}

/// Update 面板表单字段:声明顺序即表单次序,`Start` 恒为最后一个字段。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UpdateField {
    Version,
    Repository,
    RepositoryUrl,
    Start,
}

impl UpdateField {
    pub(crate) const ALL: [Self; 4] = [
        Self::Version,
        Self::Repository,
        Self::RepositoryUrl,
        Self::Start,
    ];

    /// 该字段是否在表单中可见(仓库 URL 仅在 Custom 模式下显示)。
    fn visible(self, repository: UpdateRepositoryMode) -> bool {
        match self {
            Self::RepositoryUrl => repository == UpdateRepositoryMode::Custom,
            _ => true,
        }
    }

    /// 当前模式下可见字段的有序列表(与渲染次序一致)。
    fn visible_fields(repository: UpdateRepositoryMode) -> Vec<Self> {
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
            Self::Start => String::new(),
        }
    }

    fn value(self, panel: &UpdatePanel) -> String {
        match self {
            Self::Version => panel.version.clone(),
            Self::Repository => panel.repository.label(panel.current_source.as_ref()),
            Self::RepositoryUrl => panel.repository_url.clone(),
            Self::Start => crate::tr!(crate::keys::CONSOLE_UPDATE_BUTTON),
        }
    }

    fn editable(self) -> bool {
        matches!(self, Self::Version | Self::RepositoryUrl)
    }
}

impl UpdateRepositoryMode {
    fn label(self, source: Option<&RepositorySource>) -> String {
        match self {
            Self::Current => {
                let source =
                    source.expect("the current source is selected without a config source");
                crate::tr!(
                    crate::keys::UPDATE_REPOSITORY_CURRENT,
                    kind = match source.kind {
                        RepositorySourceKind::Github => "github",
                        RepositorySourceKind::Http => "http",
                    },
                    location = source.location
                )
            }
            Self::Github => crate::tr!(crate::keys::UPDATE_REPOSITORY_GITHUB),
            Self::Mirror => crate::tr!(crate::keys::UPDATE_REPOSITORY_MIRROR),
            Self::Custom => crate::tr!(crate::keys::UPDATE_REPOSITORY_CUSTOM),
        }
    }
}

/// 卸载面板：版本/服务摘要 + 数据损失与保留物说明 + 确认层。
/// 确认层打开时检测网络接管特征并展示警告;Enter 分发结构化请求。
#[derive(Default)]
pub(crate) struct UninstallPanel {
    pub(crate) confirming: bool,
    pub(crate) masked: bool,
}

/// Update 面板：当前版本 + 目标版本/仓库来源表单、后台目标解析与确认层。
/// 解析与比较规则与命令模式 `lkit update` 一致（共享 `resolve_update_target`），
/// 已是最新与降级在面板内提示,只有升级才打开确认层。
pub(crate) struct UpdatePanel {
    pub(crate) version: String,
    pub(crate) repository: UpdateRepositoryMode,
    pub(crate) repository_url: String,
    pub(crate) selected: UpdateField,
    pub(crate) editing: bool,
    pub(crate) current_source: Option<RepositorySource>,
    pub(crate) config_error: Option<String>,
    pub(crate) resolving: Option<Receiver<Result<ResolvedUpdate, String>>>,
    pub(crate) confirming: Option<ResolvedUpdate>,
}

impl Default for UpdatePanel {
    fn default() -> Self {
        Self {
            version: "latest".into(),
            repository: UpdateRepositoryMode::Github,
            repository_url: plan::DEFAULT_HTTP_MIRROR.into(),
            selected: UpdateField::Version,
            editing: false,
            current_source: None,
            config_error: None,
            resolving: None,
            confirming: None,
        }
    }
}

impl UpdatePanel {
    /// 读取 `config.toml`（与 `lkit update` 相同的解析与校验）：有效时提供
    /// “当前来源”选项并默认选中,文件缺失时只留显式选项,损坏时显示错误提示。
    /// 每次进入 Update 菜单时重新读取,不缓存旧配置。
    pub(crate) fn load_config(&mut self) {
        let loaded =
            crate::deployment::config::load_repository().map_err(|error| error.to_string());
        match loaded {
            Ok(Some(source)) => {
                if self.current_source.is_none() {
                    self.repository = UpdateRepositoryMode::Current;
                }
                self.current_source = Some(source);
                self.config_error = None;
            }
            Ok(None) => {
                if self.repository == UpdateRepositoryMode::Current {
                    self.repository = UpdateRepositoryMode::Github;
                }
                self.current_source = None;
                self.config_error = None;
            }
            Err(error) => {
                if self.repository == UpdateRepositoryMode::Current {
                    self.repository = UpdateRepositoryMode::Github;
                }
                self.current_source = None;
                self.config_error = Some(error);
            }
        }
    }

    pub(crate) fn repository_options(&self) -> Vec<UpdateRepositoryMode> {
        let mut options = Vec::new();
        if self.current_source.is_some() {
            options.push(UpdateRepositoryMode::Current);
        }
        options.extend([
            UpdateRepositoryMode::Github,
            UpdateRepositoryMode::Mirror,
            UpdateRepositoryMode::Custom,
        ]);
        options
    }

    pub(crate) fn change(&mut self, forward: bool) {
        let options = self.repository_options();
        let position = options.iter().position(|mode| *mode == self.repository);
        let next = match position {
            // 当前选项不可用时(如配置来源失效),按它曾经排在最前的语义处理。
            None => {
                if forward {
                    0
                } else {
                    options.len() - 1
                }
            }
            Some(position) => {
                if forward {
                    (position + 1) % options.len()
                } else {
                    (position + options.len() - 1) % options.len()
                }
            }
        };
        self.repository = options[next];
    }

    pub(crate) fn editable_value_mut(&mut self) -> Option<&mut String> {
        match self.selected {
            UpdateField::Version => Some(&mut self.version),
            UpdateField::RepositoryUrl if self.repository == UpdateRepositoryMode::Custom => {
                Some(&mut self.repository_url)
            }
            _ => None,
        }
    }

    /// 消费后台解析结果,按与命令模式相同的规则分支。
    pub(crate) fn apply_resolution(&mut self, notice: &mut Notice, resolved: ResolvedUpdate) {
        match resolved.current.cmp(&resolved.target) {
            std::cmp::Ordering::Equal => {
                *notice = Notice::Info(crate::tr!(
                    crate::keys::UPDATE_ALREADY_UP_TO_DATE,
                    version = resolved.current
                ));
            }
            std::cmp::Ordering::Greater => {
                *notice = Notice::Error(crate::tr!(
                    crate::keys::SWITCH_DOWNGRADE_NOT_SUPPORTED,
                    from_version = resolved.current,
                    version = resolved.target
                ));
            }
            std::cmp::Ordering::Less => self.confirming = Some(resolved),
        }
    }

    pub(crate) fn poll(&mut self, notice: &mut Notice) {
        let result = match &self.resolving {
            Some(receiver) => receiver.try_recv(),
            None => return,
        };
        match result {
            Ok(Ok(resolved)) => {
                self.resolving = None;
                self.apply_resolution(notice, resolved);
            }
            Ok(Err(error)) => {
                self.resolving = None;
                *notice = Notice::Error(error);
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.resolving = None;
                *notice = Notice::Error(crate::tr!(
                    crate::keys::CONSOLE_UPDATE_RESOLVE_WORKER_STOPPED
                ));
            }
        }
    }
}

impl ConsoleApp {
    /// Update 面板按键：确认层、解析中、编辑与表单导航。返回 `None` 表示按键
    /// 未消费（如 Esc 返回菜单选择），回落到主处理流程。
    pub(crate) fn handle_update_key(&mut self, key: KeyEvent) -> Option<Option<ConsoleAction>> {
        if self.update.confirming.is_some() {
            match key.code {
                KeyCode::Enter => {
                    let action = self.update_action();
                    self.update.confirming = None;
                    return Some(Some(action));
                }
                KeyCode::Esc => {
                    self.update.confirming = None;
                    return Some(None);
                }
                _ => return Some(None),
            }
        }
        if self.update.resolving.is_some() {
            return Some(None);
        }
        if self.update.editing {
            match key.code {
                KeyCode::Enter | KeyCode::Esc => self.update.editing = false,
                KeyCode::Backspace => {
                    self.update.editable_value_mut().map(String::pop);
                }
                KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    if let Some(value) = self.update.editable_value_mut()
                        && value.chars().count() < 1024
                    {
                        value.push(character);
                    }
                }
                _ => {}
            }
            return Some(None);
        }
        match key.code {
            KeyCode::Up => {
                let fields = UpdateField::visible_fields(self.update.repository);
                let index = fields
                    .iter()
                    .position(|field| *field == self.update.selected)
                    .unwrap_or(0);
                if index > 0 {
                    self.update.selected = fields[index - 1];
                }
            }
            KeyCode::Down => {
                let fields = UpdateField::visible_fields(self.update.repository);
                let index = fields
                    .iter()
                    .position(|field| *field == self.update.selected)
                    .unwrap_or(0);
                self.update.selected = fields[(index + 1).min(fields.len() - 1)];
            }
            KeyCode::Right if self.update.selected == UpdateField::Repository => {
                self.update.change(true)
            }
            KeyCode::Left if self.update.selected == UpdateField::Repository => {
                self.update.change(false)
            }
            KeyCode::Enter | KeyCode::Char(' ') => match self.update.selected {
                UpdateField::Version | UpdateField::RepositoryUrl => self.update.editing = true,
                UpdateField::Repository => self.update.change(true),
                UpdateField::Start => {
                    if let Err(error) = self.start_update_resolution() {
                        self.notice = Notice::Error(error);
                    }
                }
            },
            _ => return None,
        }
        Some(None)
    }

    /// 校验表单并启动后台目标解析（与命令模式相同的版本、来源与 URL 校验）。
    fn start_update_resolution(&mut self) -> Result<(), String> {
        if self.update.resolving.is_some() {
            return Ok(());
        }
        plan::TargetVersion::parse(self.update.version.trim())
            .map_err(|error| error.to_string())?;
        if self.update.repository == UpdateRepositoryMode::Custom {
            plan::RepositoryChoice::Http(self.update.repository_url.trim().to_string())
                .resolve()
                .map_err(|error| error.to_string())?;
        }
        if self.update.repository == UpdateRepositoryMode::Current
            && self.update.current_source.is_none()
        {
            return Err(crate::tr!(
                crate::keys::CONSOLE_UPDATE_REPOSITORY_UNAVAILABLE
            ));
        }
        let (sender, receiver) = mpsc::channel();
        let repository = match self.update.repository {
            UpdateRepositoryMode::Current => self
                .update
                .current_source
                .as_ref()
                .expect("the current source is selected without a config source")
                .to_choice(),
            UpdateRepositoryMode::Github => plan::RepositoryChoice::Github(
                crate::release::repository::github::DEFAULT_REPOSITORY.into(),
            ),
            UpdateRepositoryMode::Mirror => plan::RepositoryChoice::Mirror,
            UpdateRepositoryMode::Custom => {
                plan::RepositoryChoice::Http(self.update.repository_url.trim().to_string())
            }
        };
        let version = self.update.version.trim().to_string();
        let language = crate::i18n::current();
        std::thread::spawn(move || {
            let result = crate::i18n::with_language(language, || {
                resolve_update_from_console(&repository, &version)
            });
            let _ = sender.send(result);
        });
        self.update.resolving = Some(receiver);
        Ok(())
    }

    /// 卸载面板键处理：面板 Enter 打开确认层并检测网络接管警告；
    /// 确认层 Enter 分发带 `--console-confirmed` 的结构化 `Uninstall` 请求，
    /// Esc 取消确认层留在面板。
    pub(crate) fn handle_uninstall_key(&mut self, key: KeyEvent) -> Option<Option<ConsoleAction>> {
        if self.uninstall.confirming {
            match key.code {
                KeyCode::Enter => {
                    self.uninstall.confirming = false;
                    return Some(Some(self.uninstall_action()));
                }
                KeyCode::Esc => self.uninstall.confirming = false,
                _ => {}
            }
            return Some(None);
        }
        match key.code {
            KeyCode::Enter | KeyCode::Char(' ') => {
                self.uninstall.confirming = true;
                self.uninstall.masked = crate::workflows::uninstall::host_network_services_masked(
                    &crate::service::systemd::Systemd::host(),
                );
            }
            _ => {}
        }
        None
    }

    /// 确认层 Enter：构建带 `--console-confirmed` 与 `--yes` 的结构化 `Uninstall` 请求。
    fn uninstall_action(&self) -> ConsoleAction {
        let command = Commands::Uninstall(crate::commands::uninstall::Uninstall {
            yes: true,
            allow_no_backup: false,
            keep_data: false,
            console_confirmed: true,
            #[cfg(feature = "test-support")]
            test_runtime: None,
        });
        let args = vec![
            "uninstall".into(),
            "--yes".into(),
            "--console-confirmed".into(),
        ];
        ConsoleAction::Command { command, args }
    }

    /// 确认层 Enter：构建带 `--console-confirmed` 的结构化 `Update` 请求。
    /// Current 来源不传 `--repository`，由命令按 `config.toml` > 官方 GitHub 解析。
    pub(crate) fn update_action(&self) -> ConsoleAction {
        let repository = match self.update.repository {
            UpdateRepositoryMode::Current => None,
            UpdateRepositoryMode::Github => Some(Some("github".into())),
            UpdateRepositoryMode::Mirror => Some(None),
            UpdateRepositoryMode::Custom => {
                Some(Some(self.update.repository_url.trim().to_string()))
            }
        };
        let version = self.update.version.trim().to_string();
        let command = Commands::Update(crate::commands::update::Update {
            version: Some(version.clone()),
            repository: repository.clone(),
            accept_service_change: false,
            allow_no_backup: false,
            console_confirmed: true,
            #[cfg(feature = "test-support")]
            test_runtime: None,
        });
        let mut args = vec![
            "update".into(),
            "--console-confirmed".into(),
            "--version".into(),
            version,
        ];
        match &repository {
            None => {}
            Some(Some(value)) if value == "github" => {
                args.extend(["--repository".into(), "github".into()])
            }
            Some(None) => args.push("--repository".into()),
            Some(Some(url)) => args.extend(["--repository".into(), url.clone()]),
        }
        ConsoleAction::Command { command, args }
    }
}

pub(crate) fn render_update(frame: &mut Frame<'_>, app: &mut ConsoleApp, area: Rect) {
    let focused = app.focus == Focus::Panel;
    if !matches!(app.snapshot, Snapshot::Installed { .. }) {
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled(
                    crate::tr!(crate::keys::CONSOLE_LANDSCAPE_NOT_INSTALLED),
                    Style::default().fg(Color::Yellow),
                ),
                Line::raw(""),
                Line::styled(
                    crate::tr!(crate::keys::CONSOLE_UPDATE_UNAVAILABLE),
                    Style::default().fg(Color::DarkGray),
                ),
            ])
            .block(panel_block(
                &crate::tr!(crate::keys::CONSOLE_UPDATE_MENU),
                focused,
            ))
            .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }
    let mut lines = Vec::new();
    let content_width = area.width.saturating_sub(2);
    if let Snapshot::Installed { version, .. } = &app.snapshot {
        lines.push(Line::styled(
            format!(
                "{}  {}",
                crate::tr!(crate::keys::CONSOLE_UPDATE_CURRENT_VERSION_LABEL),
                version
            ),
            Style::default().fg(Color::Green),
        ));
        lines.push(Line::raw(""));
    }
    if let Some(error) = &app.update.config_error {
        lines.push(Line::styled(error.clone(), Style::default().fg(Color::Red)));
        lines.push(Line::raw(""));
    }
    for field in UpdateField::ALL {
        if !field.visible(app.update.repository) {
            continue;
        }
        let (label, value) = (field.label(), field.value(&app.update));
        let editable = field.editable();
        app.hits.block_row(
            area,
            block_row_of(&lines, lines.len(), content_width),
            Hit::UpdateField(field),
        );
        let selected = focused && app.update.selected == field;
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
        } else if field == UpdateField::Start {
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        let marker = if selected && app.update.editing && editable {
            "_"
        } else {
            ""
        };
        let line = Line::from(vec![
            Span::styled(if selected { "> " } else { "  " }, selected_style),
            Span::styled(
                display_pad(&label, 17),
                if selected {
                    selected_style
                } else {
                    Style::default().fg(Color::DarkGray)
                },
            ),
            Span::styled(format!("{value}{marker}"), value_style),
        ]);
        if selected {
            lines.push(line.style(Style::default().bg(Color::Cyan)));
        } else {
            lines.push(line);
        }
    }
    if app.update.resolving.is_some() {
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            crate::tr!(crate::keys::CONSOLE_UPDATE_RESOLVING),
            Style::default().fg(Color::Cyan),
        ));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .block(panel_block(
                &crate::tr!(crate::keys::CONSOLE_UPDATE_MENU),
                focused,
            )),
        area,
    );
}

pub(crate) fn render_update_confirmation(frame: &mut Frame<'_>, app: &mut ConsoleApp) {
    let Some(resolved) = &app.update.confirming else {
        return;
    };
    let screen = frame.area();
    let width = 76.min(screen.width.saturating_sub(2));
    let height = 11.min(screen.height.saturating_sub(2));
    let area = Rect::new(
        screen.x + screen.width.saturating_sub(width) / 2,
        screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    );
    register_dialog_hits(&mut app.hits, screen, area);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_UPDATE_CONFIRM_QUESTION),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Line::raw(""),
            Line::raw(crate::tr!(
                crate::keys::CONSOLE_UPDATE_CONFIRM_PLAN,
                current = resolved.current,
                target = resolved.target
            )),
            Line::raw(crate::tr!(crate::keys::CONSOLE_UPDATE_CONFIRM_NOTE)),
            Line::raw(""),
            Line::raw(crate::tr!(crate::keys::CONSOLE_UPDATE_CONFIRM_PRESS_ENTER)),
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_PRESS_ESC_TO_CANCEL),
                Style::default().fg(Color::DarkGray),
            ),
        ])
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true })
        .block(Block::bordered().title(crate::tr!(crate::keys::CONSOLE_UPDATE_CONFIRM_TITLE))),
        area,
    );
}
pub(crate) fn render_uninstall(frame: &mut Frame<'_>, app: &mut ConsoleApp, area: Rect) {
    let focused = app.focus == Focus::Panel;
    if !matches!(app.snapshot, Snapshot::Installed { .. }) {
        let message = match &app.snapshot {
            Snapshot::NotInstalled => {
                crate::tr!(crate::keys::CONSOLE_LANDSCAPE_NOT_INSTALLED)
            }
            Snapshot::RootRequired => crate::tr!(crate::keys::CONSOLE_ROOT_PRIVILEGES_REQUIRED),
            Snapshot::Unavailable(error) => error.clone(),
            _ => unreachable!(),
        };
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled(message, Style::default().fg(Color::Yellow)),
                Line::raw(""),
                Line::styled(
                    crate::tr!(crate::keys::CONSOLE_UNINSTALL_UNAVAILABLE),
                    Style::default().fg(Color::DarkGray),
                ),
            ])
            .block(panel_block(
                &crate::tr!(crate::keys::CONSOLE_UNINSTALL_MENU),
                focused,
            ))
            .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }
    let mut lines = Vec::new();
    if let Snapshot::Installed {
        version, manager, ..
    } = &app.snapshot
    {
        lines.push(Line::styled(
            format!(
                "{}  {}",
                crate::tr!(crate::keys::CONSOLE_UNINSTALL_VERSION_LABEL),
                version
            ),
            Style::default().fg(Color::Green),
        ));
        lines.push(Line::raw(format!(
            "{}  {}",
            crate::tr!(crate::keys::CONSOLE_UNINSTALL_SERVICE_LABEL),
            manager
        )));
        lines.push(Line::raw(""));
    }
    lines.push(Line::styled(
        crate::tr!(crate::keys::CONSOLE_UNINSTALL_DATA_LOSS),
        Style::default().fg(Color::Yellow),
    ));
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        crate::tr!(crate::keys::CONSOLE_UNINSTALL_RETAINED),
        Style::default().fg(Color::DarkGray),
    ));
    if app.uninstall.masked {
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            crate::tr!(crate::keys::CONSOLE_UNINSTALL_HOST_NETWORK_WARNING),
            Style::default().fg(Color::Red),
        ));
    }
    lines.push(Line::raw(""));
    let action_row = lines.len();
    lines.push(Line::styled(
        format!(
            "{}{}",
            if focused { "> " } else { "  " },
            crate::tr!(crate::keys::CONSOLE_UNINSTALL_ACTION)
        ),
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
    ));
    app.hits.block_row(
        area,
        block_row_of(&lines, action_row, area.width.saturating_sub(2)),
        Hit::UninstallAction,
    );
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .block(panel_block(
                &crate::tr!(crate::keys::CONSOLE_UNINSTALL_MENU),
                focused,
            )),
        area,
    );
}

pub(crate) fn render_uninstall_confirmation(frame: &mut Frame<'_>, app: &mut ConsoleApp) {
    let screen = frame.area();
    let width = 76.min(screen.width.saturating_sub(2));
    let height = 13.min(screen.height.saturating_sub(2));
    let area = Rect::new(
        screen.x + screen.width.saturating_sub(width) / 2,
        screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    );
    register_dialog_hits(&mut app.hits, screen, area);
    frame.render_widget(Clear, area);
    let mut lines = vec![
        Line::styled(
            crate::tr!(crate::keys::CONSOLE_UNINSTALL_CONFIRM_QUESTION),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
    ];
    if let Snapshot::Installed { version, .. } = &app.snapshot {
        lines.push(Line::raw(crate::tr!(
            crate::keys::CONSOLE_UNINSTALL_CONFIRM_PLAN,
            version = version
        )));
    }
    lines.push(Line::styled(
        crate::tr!(crate::keys::CONSOLE_UNINSTALL_DATA_LOSS),
        Style::default().fg(Color::Yellow),
    ));
    if app.uninstall.masked {
        lines.push(Line::styled(
            crate::tr!(crate::keys::CONSOLE_UNINSTALL_HOST_NETWORK_WARNING),
            Style::default().fg(Color::Red),
        ));
    }
    lines.push(Line::raw(""));
    lines.push(Line::raw(crate::tr!(
        crate::keys::CONSOLE_UNINSTALL_CONFIRM_PRESS_ENTER
    )));
    lines.push(Line::styled(
        crate::tr!(crate::keys::CONSOLE_PRESS_ESC_TO_CANCEL),
        Style::default().fg(Color::DarkGray),
    ));
    frame.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true })
            .block(
                Block::bordered().title(crate::tr!(crate::keys::CONSOLE_UNINSTALL_CONFIRM_TITLE)),
            ),
        area,
    );
}
