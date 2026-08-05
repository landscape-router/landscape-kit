use std::io::{IsTerminal, Stdout};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

use crossterm::cursor::{Hide, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use unicode_width::UnicodeWidthStr;

use crate::check;
use crate::check::model::{CheckReport, Status};
use crate::commands::install::Install;
use crate::commands::{Commands, ServiceManagerArg};
use crate::deployment::{plan, root, state};
use crate::i18n::Language;
use crate::interaction::credentials;

const FORM_FIELDS: usize = 10;

pub(crate) enum ConsoleAction {
    Quit,
    Command {
        command: Commands,
        args: Vec<String>,
    },
}

pub(crate) fn run() -> Result<ConsoleAction, String> {
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(crate::tr!(
            "a terminal is required; use an lkit subcommand for command mode",
            "需要终端；命令模式请使用 lkit 子命令"
        )
        .into());
    }
    let mut terminal = ConsoleTerminal::start()?;
    let mut app = ConsoleApp::new();
    loop {
        app.update_preflight();
        terminal
            .terminal
            .draw(|frame| render(frame, &app))
            .map_err(|error| format!("draw console: {error}"))?;
        if !event::poll(Duration::from_millis(100))
            .map_err(|error| format!("poll terminal event: {error}"))?
        {
            continue;
        }
        match event::read().map_err(|error| format!("read terminal event: {error}"))? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                if let Some(action) = app.handle_key(key) {
                    return Ok(action);
                }
            }
            Event::Paste(value) => app.handle_paste(&value),
            Event::Resize(_, _) | Event::FocusGained | Event::FocusLost | Event::Mouse(_) => {}
            Event::Key(_) => {}
        }
    }
}

struct ConsoleTerminal {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl ConsoleTerminal {
    fn start() -> Result<Self, String> {
        enable_raw_mode().map_err(|error| format!("enable raw mode: {error}"))?;
        let mut stdout = std::io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen, Hide) {
            let _ = disable_raw_mode();
            return Err(format!("enter alternate screen: {error}"));
        }
        let terminal = match Terminal::new(CrosstermBackend::new(stdout)) {
            Ok(terminal) => terminal,
            Err(error) => {
                let _ = disable_raw_mode();
                let _ = execute!(std::io::stdout(), LeaveAlternateScreen, Show);
                return Err(format!("initialize terminal: {error}"));
            }
        };
        Ok(Self { terminal })
    }
}

impl Drop for ConsoleTerminal {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen, Show);
        let _ = self.terminal.show_cursor();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Menu {
    Overview,
    Install,
    Versions,
    Configuration,
    Services,
    Network,
    Diagnostics,
}

impl Menu {
    const ALL: [Self; 7] = [
        Self::Overview,
        Self::Install,
        Self::Versions,
        Self::Configuration,
        Self::Services,
        Self::Network,
        Self::Diagnostics,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Overview => crate::tr!("Overview", "概览"),
            Self::Install => crate::tr!("Install", "安装"),
            Self::Versions => crate::tr!("Versions", "版本"),
            Self::Configuration => crate::tr!("Configuration", "配置"),
            Self::Services => crate::tr!("Services", "服务"),
            Self::Network => crate::tr!("Network", "网络"),
            Self::Diagnostics => crate::tr!("Diagnostics", "诊断"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Focus {
    Navigation,
    Panel,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExitState {
    Idle,
    Armed,
    Confirming,
}

enum PreflightState {
    NotRun,
    Running(Receiver<CheckReport>),
    Complete(CheckReport),
    Failed(String),
}

struct Preflight {
    state: PreflightState,
    expanded: bool,
    scroll: u16,
}

impl Default for Preflight {
    fn default() -> Self {
        Self {
            state: PreflightState::NotRun,
            expanded: false,
            scroll: 0,
        }
    }
}

impl Preflight {
    fn start(&mut self) {
        if matches!(&self.state, PreflightState::Running(_)) {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let language = crate::i18n::current();
        std::thread::spawn(move || {
            let report = crate::i18n::with_language(language, check::run_all);
            let _ = sender.send(report);
        });
        self.state = PreflightState::Running(receiver);
        self.scroll = 0;
    }

    fn poll(&mut self) {
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
                self.state = PreflightState::Failed(
                    crate::tr!("check worker stopped unexpectedly", "检查 worker 意外停止").into(),
                );
            }
        }
    }

    fn restart(&mut self) {
        self.state = PreflightState::NotRun;
        self.expanded = false;
        self.scroll = 0;
        self.start();
    }

    fn scroll_down(&mut self, amount: u16) {
        let max = preflight_detail_lines(self)
            .len()
            .saturating_sub(1)
            .min(u16::MAX as usize) as u16;
        self.scroll = self.scroll.saturating_add(amount).min(max);
    }
}

struct ConsoleApp {
    menu_index: usize,
    focus: Focus,
    install: InstallForm,
    snapshot: Snapshot,
    notice: String,
    exit_state: ExitState,
    preflight: Preflight,
}

impl ConsoleApp {
    fn new() -> Self {
        let install = InstallForm::default();
        let snapshot = Snapshot::load(&install.install_dir);
        Self {
            menu_index: 0,
            focus: Focus::Navigation,
            install,
            snapshot,
            notice: "Ready".into(),
            exit_state: ExitState::Idle,
            preflight: Preflight::default(),
        }
    }

    fn menu(&self) -> Menu {
        Menu::ALL[self.menu_index]
    }

    fn update_preflight(&mut self) {
        if self.menu() == Menu::Install && matches!(&self.preflight.state, PreflightState::NotRun) {
            self.preflight.start();
        }
        self.preflight.poll();
    }

    fn handle_key(&mut self, key: KeyEvent) -> Option<ConsoleAction> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Some(ConsoleAction::Quit);
        }
        if self.exit_state == ExitState::Confirming {
            match key.code {
                KeyCode::Enter => return Some(ConsoleAction::Quit),
                KeyCode::Esc => {
                    self.exit_state = ExitState::Idle;
                    self.notice = "Ready".into();
                }
                _ => {}
            }
            return None;
        }
        if !self.install.editing && language_toggle_key(&key) {
            self.toggle_language();
            return None;
        }
        if self.preflight.expanded && self.menu() == Menu::Install {
            match key.code {
                KeyCode::Esc => {
                    self.preflight.expanded = false;
                    self.preflight.scroll = 0;
                }
                KeyCode::Up => {
                    self.preflight.scroll = self.preflight.scroll.saturating_sub(1);
                }
                KeyCode::Down => self.preflight.scroll_down(1),
                KeyCode::PageUp => {
                    self.preflight.scroll = self.preflight.scroll.saturating_sub(8);
                }
                KeyCode::PageDown => self.preflight.scroll_down(8),
                KeyCode::Home => self.preflight.scroll = 0,
                KeyCode::Char('r' | 'R') => self.preflight.start(),
                _ => {}
            }
            return None;
        }
        if self.install.editing && self.menu() == Menu::Install && self.focus == Focus::Panel {
            return self.handle_editing_key(key);
        }
        if key.code == KeyCode::Esc {
            match self.exit_state {
                ExitState::Idle => {
                    self.exit_state = ExitState::Armed;
                    self.notice = "Exit armed - press Esc again for confirmation".into();
                }
                ExitState::Armed => {
                    self.exit_state = ExitState::Confirming;
                    self.notice = "Ready".into();
                }
                ExitState::Confirming => unreachable!(),
            }
            return None;
        }
        if self.exit_state == ExitState::Armed {
            self.exit_state = ExitState::Idle;
            self.notice = "Ready".into();
        }
        if self.focus == Focus::Panel
            && self.menu() == Menu::Install
            && matches!(key.code, KeyCode::Char('r' | 'R'))
        {
            self.preflight.start();
            return None;
        }
        match key.code {
            KeyCode::Tab | KeyCode::BackTab => {
                self.focus = match self.focus {
                    Focus::Navigation => Focus::Panel,
                    Focus::Panel => Focus::Navigation,
                };
            }
            KeyCode::Up => match self.focus {
                Focus::Navigation => {
                    self.menu_index = self.menu_index.saturating_sub(1);
                }
                Focus::Panel if self.menu() == Menu::Install => {
                    self.install.select_previous();
                }
                Focus::Panel => {}
            },
            KeyCode::Down => match self.focus {
                Focus::Navigation => {
                    self.menu_index = (self.menu_index + 1).min(Menu::ALL.len() - 1);
                }
                Focus::Panel if self.menu() == Menu::Install => {
                    self.install.select_next();
                }
                Focus::Panel => {}
            },
            KeyCode::Right if self.focus == Focus::Navigation => self.focus = Focus::Panel,
            KeyCode::Left if self.focus == Focus::Panel => self.focus = Focus::Navigation,
            KeyCode::Right
                if self.focus == Focus::Panel
                    && self.menu() == Menu::Install
                    && self.install.checks_selected =>
            {
                self.preflight.expanded = true;
            }
            KeyCode::Right if self.focus == Focus::Panel => self.install.change_choice(true),
            KeyCode::Enter | KeyCode::Char(' ') if self.focus == Focus::Panel => {
                if self.menu() == Menu::Install {
                    if self.install.checks_selected {
                        self.preflight.expanded = true;
                    } else {
                        match self.install.activate() {
                            Ok(Some(action)) => return Some(action),
                            Ok(None) => self.notice = "Ready".into(),
                            Err(error) => self.notice = error,
                        }
                    }
                }
            }
            KeyCode::Enter if self.focus == Focus::Navigation => self.focus = Focus::Panel,
            _ => {}
        }
        None
    }

    fn toggle_language(&mut self) {
        crate::i18n::configure(crate::i18n::current().toggled());
        self.exit_state = ExitState::Idle;
        self.notice = "Ready".into();
        self.snapshot = Snapshot::load(&self.install.install_dir);
        if !matches!(&self.preflight.state, PreflightState::NotRun) {
            self.preflight.restart();
        }
    }

    fn handle_editing_key(&mut self, key: KeyEvent) -> Option<ConsoleAction> {
        match key.code {
            KeyCode::Enter | KeyCode::Esc => self.install.editing = false,
            KeyCode::Backspace => {
                self.install.editable_value_mut().map(String::pop);
            }
            KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(value) = self.install.editable_value_mut()
                    && value.chars().count() < 1024
                {
                    value.push(character);
                }
            }
            _ => {}
        }
        None
    }

    fn handle_paste(&mut self, value: &str) {
        if self.exit_state == ExitState::Confirming {
            return;
        }
        if self.exit_state == ExitState::Armed {
            self.exit_state = ExitState::Idle;
            self.notice = "Ready".into();
        }
        if !self.install.editing || self.menu() != Menu::Install || self.focus != Focus::Panel {
            return;
        }
        if let Some(target) = self.install.editable_value_mut() {
            let remaining = 1024_usize.saturating_sub(target.chars().count());
            target.extend(
                value
                    .chars()
                    .filter(|character| !character.is_control())
                    .take(remaining),
            );
        }
    }

    fn hints(&self) -> &'static str {
        if self.exit_state == ExitState::Confirming {
            crate::tr!("Enter Exit  Esc Cancel", "Enter 退出  Esc 取消")
        } else if self.exit_state == ExitState::Armed {
            crate::tr!(
                "Press Esc again for exit confirmation  Any other key cancels",
                "再次按 Esc 确认退出  其他按键取消"
            )
        } else if self.preflight.expanded && self.menu() == Menu::Install {
            crate::tr!(
                "Up/Down Scroll  PgUp/PgDn Page  R Re-run  Esc Close",
                "上/下 滚动  PgUp/PgDn 翻页  R 重跑  Esc 关闭"
            )
        } else if self.install.editing && self.menu() == Menu::Install && self.focus == Focus::Panel
        {
            crate::tr!(
                "Type Edit  Backspace Delete  Enter/Esc Finish",
                "输入 编辑  Backspace 删除  Enter/Esc 完成"
            )
        } else {
            match (self.focus, self.menu()) {
                (Focus::Navigation, _) => {
                    crate::tr!(
                        "Up/Down Menu  Right/Enter Open  Tab Switch  Esc Esc Confirm",
                        "上/下 菜单  右/Enter 打开  Tab 切换  Esc Esc 确认"
                    )
                }
                (Focus::Panel, Menu::Install) if self.install.checks_selected => {
                    crate::tr!(
                        "Enter Details  R Re-run  Down Settings  Left Menu  Esc Esc Confirm",
                        "Enter 详情  R 重跑  下 设置  左 菜单  Esc Esc 确认"
                    )
                }
                (Focus::Panel, Menu::Install) => {
                    crate::tr!(
                        "Up/Down Field  Left Menu  Right Change  Enter Select  Tab Menu  Esc Esc Confirm",
                        "上/下 字段  左 菜单  右 更改  Enter 选择  Tab 菜单  Esc Esc 确认"
                    )
                }
                (Focus::Panel, _) => crate::tr!(
                    "Left Menu  Tab Switch  Esc Esc Confirm",
                    "左 菜单  Tab 切换  Esc Esc 确认"
                ),
            }
        }
    }

    fn language_switch_available(&self) -> bool {
        self.exit_state != ExitState::Confirming && !self.install.editing
    }
}

fn language_toggle_key(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('l' | 'L'))
        && !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RepositoryMode {
    Github,
    Mirror,
    Custom,
}

impl RepositoryMode {
    fn label(self) -> &'static str {
        match self {
            Self::Github => "GitHub",
            Self::Mirror => crate::tr!("HTTP mirror", "HTTP 镜像"),
            Self::Custom => crate::tr!("Custom HTTP", "自定义 HTTP"),
        }
    }

    fn change(&mut self, forward: bool) {
        *self = match (*self, forward) {
            (Self::Github, true) | (Self::Custom, false) => Self::Mirror,
            (Self::Mirror, true) => Self::Custom,
            (Self::Mirror, false) => Self::Github,
            (Self::Custom, true) => Self::Github,
            (Self::Github, false) => Self::Custom,
        };
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ManagerMode {
    Auto,
    Systemd,
    None,
}

impl ManagerMode {
    fn label(self) -> &'static str {
        match self {
            Self::Auto => crate::tr!("Auto", "自动"),
            Self::Systemd => "systemd",
            Self::None => "none",
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

struct InstallForm {
    version: String,
    repository: RepositoryMode,
    repository_url: String,
    install_dir: String,
    admin_user: String,
    password: String,
    password_confirmation: String,
    manager: ManagerMode,
    takeover_network: bool,
    selected: usize,
    checks_selected: bool,
    editing: bool,
}

impl Default for InstallForm {
    fn default() -> Self {
        Self {
            version: "latest".into(),
            repository: RepositoryMode::Github,
            repository_url: plan::DEFAULT_HTTP_MIRROR.into(),
            install_dir: std::env::var("LKIT_INSTALL_DIR")
                .unwrap_or_else(|_| plan::DEFAULT_INSTALL_ROOT.into()),
            admin_user: "admin".into(),
            password: String::new(),
            password_confirmation: String::new(),
            manager: ManagerMode::Auto,
            takeover_network: false,
            selected: 0,
            checks_selected: true,
            editing: false,
        }
    }
}

impl InstallForm {
    fn select_previous(&mut self) {
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

    fn select_next(&mut self) {
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

    fn selected_help(&self) -> (&'static str, &'static str) {
        match self.selected {
            0 => (
                crate::tr!("Version", "版本"),
                crate::tr!(
                    "Release to install. Use latest for the newest stable release or enter an exact stable version such as 1.2.3.",
                    "要安装的发布版本。使用 latest 获取最新稳定版，或输入 1.2.3 这样的精确稳定版本。"
                ),
            ),
            1 => (
                crate::tr!("Repository", "仓库"),
                crate::tr!(
                    "Release source. Choose GitHub, the default HTTP mirror, or a custom protocol v1 HTTP repository.",
                    "发布源。可选择 GitHub、默认 HTTP 镜像或自定义 protocol v1 HTTP 仓库。"
                ),
            ),
            2 => (
                crate::tr!("Repository URL", "仓库 URL"),
                crate::tr!(
                    "Base URL of the custom protocol v1 repository. Remote repositories require HTTPS; loopback HTTP is allowed.",
                    "自定义 protocol v1 仓库的基础 URL。远程仓库必须使用 HTTPS；回环地址允许 HTTP。"
                ),
            ),
            3 => (
                crate::tr!("Install root", "安装根目录"),
                crate::tr!(
                    "Absolute directory that stores releases, configuration, state, transactions, and backups.",
                    "保存发布版本、配置、状态、事务和备份的绝对目录。"
                ),
            ),
            4 => (
                crate::tr!("Admin user", "管理员用户"),
                crate::tr!(
                    "Username for the initial Landscape administrator account.",
                    "初始 Landscape 管理员账户的用户名。"
                ),
            ),
            5 => (
                crate::tr!("Password", "密码"),
                crate::tr!(
                    "Password for the initial administrator. It remains masked and is validated before installation starts.",
                    "初始管理员密码。密码始终隐藏，并在安装开始前进行验证。"
                ),
            ),
            6 => (
                crate::tr!("Confirm password", "确认密码"),
                crate::tr!(
                    "Enter the administrator password again to prevent typing mistakes.",
                    "再次输入管理员密码，避免输入错误。"
                ),
            ),
            7 => (
                crate::tr!("Service manager", "服务管理器"),
                crate::tr!(
                    "Choose automatic detection, explicit systemd supervision, or no service manager.",
                    "选择自动检测、明确使用 systemd，或不使用服务管理器。"
                ),
            ),
            8 => (
                crate::tr!("Network takeover", "网络接管"),
                crate::tr!(
                    "Allow Landscape to reconfigure host interfaces and network services during installation.",
                    "允许 Landscape 在安装期间重新配置主机网卡和网络服务。"
                ),
            ),
            9 => (
                crate::tr!("Start installation", "开始安装"),
                crate::tr!(
                    "Validate the form, leave the console, and start the installation using these settings.",
                    "验证表单、退出控制台，并使用这些设置开始安装。"
                ),
            ),
            _ => (
                crate::tr!("Install", "安装"),
                crate::tr!(
                    "Configure the Landscape installation.",
                    "配置 Landscape 安装。"
                ),
            ),
        }
    }

    fn editable_value_mut(&mut self) -> Option<&mut String> {
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

    fn change_choice(&mut self, forward: bool) {
        match self.selected {
            1 => self.repository.change(forward),
            7 => self.manager.change(forward),
            8 => self.takeover_network = !self.takeover_network,
            _ => {}
        }
    }

    fn activate(&mut self) -> Result<Option<ConsoleAction>, String> {
        match self.selected {
            0 | 3 | 4 | 5 | 6 => {
                self.editing = true;
                Ok(None)
            }
            2 if self.repository == RepositoryMode::Custom => {
                self.editing = true;
                Ok(None)
            }
            1 | 7 | 8 => {
                self.change_choice(true);
                Ok(None)
            }
            9 => self.command().map(Some),
            _ => Ok(None),
        }
    }

    fn command(&mut self) -> Result<ConsoleAction, String> {
        let version = self.version.trim();
        plan::TargetVersion::parse(version).map_err(|error| error.to_string())?;
        plan::validate_admin_user(&self.admin_user).map_err(|error| error.to_string())?;
        let requested_install_dir = PathBuf::from(&self.install_dir);
        let install_dir = plan::select_install_root(Some(&requested_install_dir), None)
            .map_err(|error| error.to_string())?;
        let repository = match self.repository {
            RepositoryMode::Github => None,
            RepositoryMode::Mirror => Some(None),
            RepositoryMode::Custom => {
                let url = self.repository_url.trim().to_string();
                plan::RepositoryChoice::Http(url.clone())
                    .resolve()
                    .map_err(|error| error.to_string())?;
                Some(Some(url))
            }
        };
        if self.password != self.password_confirmation {
            return Err(crate::tr!(
                "password confirmation does not match",
                "两次输入的密码不一致"
            )
            .into());
        }
        credentials::validate_password(&self.password).map_err(|error| error.to_string())?;
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
            #[cfg(feature = "test-support")]
            test_runtime: None,
        };
        let mut args = vec!["install".into(), "--version".into(), version.into()];
        match &repository {
            None => {}
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

enum Snapshot {
    RootRequired,
    NotInstalled,
    Installed {
        version: String,
        manager: &'static str,
        initialized: bool,
    },
    Unavailable(String),
}

impl Snapshot {
    fn load(install_dir: &str) -> Self {
        if unsafe { libc::geteuid() } != 0 {
            return Self::RootRequired;
        }
        let path = PathBuf::from(install_dir);
        let result = root::normalize_install_root(&path).and_then(|root| state::load_state(&root));
        match result {
            Ok(None) => Self::NotInstalled,
            Ok(Some(installed)) => Self::Installed {
                version: installed.active_version,
                manager: match installed.service.manager {
                    state::StateServiceManager::Systemd => "systemd",
                    state::StateServiceManager::None => "none",
                },
                initialized: installed.initialization.status == state::InitStatus::Complete,
            },
            Err(error) => Self::Unavailable(error.to_string()),
        }
    }

    fn badge(&self) -> (&'static str, Color) {
        match self {
            Self::RootRequired => (crate::tr!("Root required", "需要 root"), Color::Yellow),
            Self::NotInstalled => (crate::tr!("Not installed", "未安装"), Color::Yellow),
            Self::Installed { .. } => (crate::tr!("Installed", "已安装"), Color::Green),
            Self::Unavailable(_) => (crate::tr!("Attention required", "需要处理"), Color::Red),
        }
    }
}

fn render(frame: &mut Frame<'_>, app: &ConsoleApp) {
    if frame.area().width < 72 || frame.area().height < 18 {
        frame.render_widget(
            Paragraph::new(crate::tr!("Terminal too small", "终端尺寸过小"))
                .alignment(Alignment::Center)
                .block(Block::bordered().title("Landscape Kit")),
            frame.area(),
        );
        if app.exit_state == ExitState::Confirming {
            render_exit_confirmation(frame);
        }
        return;
    }
    let [header, body, status] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(8),
        Constraint::Length(3),
    ])
    .areas(frame.area());
    render_header(frame, app, header);
    let [navigation, panel] =
        Layout::horizontal([Constraint::Length(24), Constraint::Min(24)]).areas(body);
    render_navigation(frame, app, navigation);
    render_panel(frame, app, panel);
    render_status(frame, app, status);
    if app.exit_state == ExitState::Confirming {
        render_exit_confirmation(frame);
    }
}

fn render_status(frame: &mut Frame<'_>, app: &ConsoleApp, area: Rect) {
    frame.render_widget(Block::default().borders(Borders::TOP), area);
    let content = Rect::new(
        area.x,
        area.y.saturating_add(1),
        area.width,
        area.height.saturating_sub(1),
    );
    let [summary, hints] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(content);
    let language = language_status(crate::i18n::current(), app.language_switch_available());
    let language_width = (UnicodeWidthStr::width(language.as_str()) as u16)
        .saturating_add(2)
        .min(summary.width);
    let [notice, language_area] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(language_width)]).areas(summary);
    let notice_color = if app.notice == "Ready" {
        Color::DarkGray
    } else {
        Color::Red
    };
    frame.render_widget(
        Paragraph::new(if app.notice == "Ready" {
            crate::tr!("Ready", "就绪")
        } else {
            app.notice.as_str()
        })
        .style(Style::default().fg(notice_color)),
        notice,
    );
    frame.render_widget(
        Paragraph::new(language)
            .alignment(Alignment::Right)
            .style(Style::default().fg(Color::Cyan)),
        language_area,
    );
    frame.render_widget(
        Paragraph::new(app.hints()).style(Style::default().fg(Color::DarkGray)),
        hints,
    );
}

fn language_status(language: Language, switch_available: bool) -> String {
    match (language, switch_available) {
        (Language::En, true) => "L  Language: English (en)",
        (Language::En, false) => "Language: English (en)",
        (Language::Zh, true) => "L  语言：中文 (zh)",
        (Language::Zh, false) => "语言：中文 (zh)",
    }
    .into()
}

fn render_exit_confirmation(frame: &mut Frame<'_>) {
    let screen = frame.area();
    let width = 48.min(screen.width.saturating_sub(2));
    let height = 7.min(screen.height.saturating_sub(2));
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
                crate::tr!("Exit Landscape Kit?", "退出 Landscape Kit？"),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Line::raw(""),
            Line::raw(crate::tr!("Press Enter to exit.", "按 Enter 退出。")),
            Line::styled(
                crate::tr!("Press Esc to cancel.", "按 Esc 取消。"),
                Style::default().fg(Color::DarkGray),
            ),
        ])
        .alignment(Alignment::Center)
        .block(Block::bordered().title(crate::tr!("Confirm exit", "确认退出"))),
        area,
    );
}

fn render_header(frame: &mut Frame<'_>, app: &ConsoleApp, area: Rect) {
    let (badge, color) = app.snapshot.badge();
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "Landscape Kit",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(badge, Style::default().fg(color)),
        ]))
        .alignment(Alignment::Left)
        .block(Block::default().borders(Borders::BOTTOM)),
        area,
    );
}

fn render_navigation(frame: &mut Frame<'_>, app: &ConsoleApp, area: Rect) {
    let items: Vec<ListItem<'_>> = Menu::ALL
        .iter()
        .map(|menu| ListItem::new(menu.label()))
        .collect();
    let mut state = ListState::default().with_selected(Some(app.menu_index));
    let highlight = if app.focus == Focus::Navigation {
        Style::default().fg(Color::Black).bg(Color::Cyan)
    } else {
        Style::default().fg(Color::Cyan)
    };
    frame.render_stateful_widget(
        List::new(items)
            .block(Block::bordered().title(crate::tr!("Navigation", "导航")))
            .highlight_style(highlight)
            .highlight_symbol("> "),
        area,
        &mut state,
    );
}

fn render_panel(frame: &mut Frame<'_>, app: &ConsoleApp, area: Rect) {
    match app.menu() {
        Menu::Overview => render_overview(frame, app, area),
        Menu::Install => render_install(frame, app, area),
        menu => {
            frame.render_widget(
                Paragraph::new(vec![
                    Line::styled(menu.label(), Style::default().add_modifier(Modifier::BOLD)),
                    Line::raw(""),
                    Line::styled(
                        crate::tr!("Not available in this release", "当前版本暂不可用"),
                        Style::default().fg(Color::DarkGray),
                    ),
                ])
                .block(Block::bordered().title(menu.label()))
                .wrap(Wrap { trim: true }),
                area,
            );
        }
    }
}

fn render_overview(frame: &mut Frame<'_>, app: &ConsoleApp, area: Rect) {
    let lines = match &app.snapshot {
        Snapshot::RootRequired => vec![
            Line::styled(
                crate::tr!("Root privileges are required", "需要 root 权限"),
                Style::default().fg(Color::Yellow),
            ),
            Line::raw(""),
            Line::raw(crate::trf!(
                ("Install root  {}", app.install.install_dir),
                ("安装根目录  {}", app.install.install_dir)
            )),
        ],
        Snapshot::NotInstalled => vec![
            Line::styled(
                crate::tr!("Landscape is not installed", "Landscape 尚未安装"),
                Style::default().fg(Color::Yellow),
            ),
            Line::raw(""),
            Line::raw(crate::trf!(
                ("Install root  {}", app.install.install_dir),
                ("安装根目录  {}", app.install.install_dir)
            )),
        ],
        Snapshot::Installed {
            version,
            manager,
            initialized,
        } => vec![
            Line::styled(
                crate::tr!("Landscape is installed", "Landscape 已安装"),
                Style::default().fg(Color::Green),
            ),
            Line::raw(""),
            Line::raw(crate::trf!(
                ("Version       {version}"),
                ("版本         {version}")
            )),
            Line::raw(crate::trf!(
                ("Service       {manager}"),
                ("服务         {manager}")
            )),
            Line::raw(crate::trf!(
                (
                    "Initialization {}",
                    if *initialized { "complete" } else { "pending" }
                ),
                (
                    "初始化       {}",
                    if *initialized { "完成" } else { "等待中" }
                )
            )),
            Line::raw(crate::trf!(
                ("Install root  {}", app.install.install_dir),
                ("安装根目录  {}", app.install.install_dir)
            )),
        ],
        Snapshot::Unavailable(error) => vec![
            Line::styled(
                crate::tr!("Installation state needs attention", "安装状态需要处理"),
                Style::default().fg(Color::Red),
            ),
            Line::raw(""),
            Line::raw(error),
        ],
    };
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title(crate::tr!("Overview", "概览")))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_install(frame: &mut Frame<'_>, app: &ConsoleApp, area: Rect) {
    if app.preflight.expanded {
        render_preflight_details(frame, &app.preflight, area);
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

fn render_preflight_summary(frame: &mut Frame<'_>, app: &ConsoleApp, area: Rect) {
    let (status, detail, color) = match &app.preflight.state {
        PreflightState::NotRun => (
            crate::tr!("NOT RUN", "未运行"),
            crate::tr!("Waiting to check this host", "等待检查此主机").into(),
            Color::DarkGray,
        ),
        PreflightState::Running(_) => (
            crate::tr!("RUNNING", "运行中"),
            crate::tr!("Checking this host...", "正在检查此主机...").into(),
            Color::Cyan,
        ),
        PreflightState::Complete(report) => (
            report.summary.label(),
            preflight_counts(report),
            check_status_color(report.summary),
        ),
        PreflightState::Failed(error) => (crate::tr!("FAILED", "失败"), error.clone(), Color::Red),
    };
    let selected = app.focus == Focus::Panel && app.install.checks_selected;
    let style = if selected {
        Style::default().bg(Color::Rgb(30, 68, 78))
    } else {
        Style::default()
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!("{status:<9}"), Style::default().fg(color)),
            Span::raw(detail),
        ]))
        .style(style)
        .block(Block::bordered().title(crate::tr!("Environment checks", "环境检查"))),
        area,
    );
}

fn render_preflight_details(frame: &mut Frame<'_>, preflight: &Preflight, area: Rect) {
    frame.render_widget(
        Paragraph::new(preflight_detail_lines(preflight))
            .block(Block::bordered().title(crate::tr!("Environment checks", "环境检查")))
            .wrap(Wrap { trim: true })
            .scroll((preflight.scroll, 0)),
        area,
    );
}

fn preflight_detail_lines(preflight: &Preflight) -> Vec<Line<'static>> {
    let PreflightState::Complete(report) = &preflight.state else {
        return vec![match &preflight.state {
            PreflightState::NotRun => {
                Line::raw(crate::tr!("Checks have not run yet.", "检查尚未运行。"))
            }
            PreflightState::Running(_) => Line::styled(
                crate::tr!("Checking this host...", "正在检查此主机..."),
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
            group.title,
            Style::default().add_modifier(Modifier::BOLD),
        ));
        for result in &group.results {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{:<7}", result.status.label()),
                    Style::default().fg(check_status_color(result.status)),
                ),
                Span::styled(result.title, Style::default().fg(Color::White)),
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
    crate::trf!(
        (
            "{} pass / {} warn / {} error / {} unknown",
            report.counts.pass,
            report.counts.warning,
            report.counts.error,
            report.counts.unknown
        ),
        (
            "{} 通过 / {} 警告 / {} 错误 / {} 未知",
            report.counts.pass,
            report.counts.warning,
            report.counts.error,
            report.counts.unknown
        )
    )
}

fn render_install_form(frame: &mut Frame<'_>, app: &ConsoleApp, area: Rect) {
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
        if form.takeover_network { "[x]" } else { "[ ]" }.into(),
        crate::tr!("[ Start installation ]", "[ 开始安装 ]").into(),
    ];
    let labels = [
        crate::tr!("Version", "版本"),
        crate::tr!("Repository", "仓库"),
        crate::tr!("Repository URL", "仓库 URL"),
        crate::tr!("Install root", "安装根目录"),
        crate::tr!("Admin user", "管理员用户"),
        crate::tr!("Password", "密码"),
        crate::tr!("Confirm password", "确认密码"),
        crate::tr!("Service manager", "服务管理器"),
        crate::tr!("Network takeover", "网络接管"),
        "",
    ];
    let lines: Vec<Line<'_>> = labels
        .iter()
        .zip(values)
        .enumerate()
        .filter_map(|(index, (label, value))| {
            if index == 2 && form.repository != RepositoryMode::Custom {
                return None;
            }
            let selected =
                app.focus == Focus::Panel && !form.checks_selected && form.selected == index;
            let value_style = if index == 9 {
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            let line = Line::from(vec![
                Span::styled(display_pad(label, 19), Style::default().fg(Color::DarkGray)),
                Span::styled(value, value_style),
                Span::raw(if selected && form.editing { "_" } else { "" }),
            ]);
            if selected {
                Some(line.style(Style::default().bg(Color::Rgb(30, 68, 78))))
            } else {
                Some(line)
            }
        })
        .collect();
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(crate::tr!("Install", "安装"))),
        area,
    );
}

fn render_install_help(frame: &mut Frame<'_>, app: &ConsoleApp, area: Rect) {
    let (title, description) = if app.install.checks_selected {
        (
            crate::tr!("Environment checks", "环境检查"),
            crate::tr!(
                "Read-only deployment checks for platform, kernel, resources, dependencies, ports, services, and DNS.",
                "针对平台、内核、资源、依赖、端口、服务和 DNS 的只读部署检查。"
            ),
        )
    } else {
        app.install.selected_help()
    };
    frame.render_widget(
        Paragraph::new(description)
            .block(Block::bordered().title(crate::trf!(("About: {title}"), ("说明：{title}"))))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn mask(value: &str) -> String {
    "*".repeat(value.chars().count())
}

fn display_pad(value: &str, width: usize) -> String {
    format!(
        "{value}{}",
        " ".repeat(width.saturating_sub(UnicodeWidthStr::width(value)))
    )
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::check::model::{CheckGroup, CheckResult, StatusCounts};

    struct LanguageGuard(Language);

    impl LanguageGuard {
        fn set(language: Language) -> Self {
            let previous = crate::i18n::current();
            crate::i18n::configure(language);
            Self(previous)
        }
    }

    impl Drop for LanguageGuard {
        fn drop(&mut self) {
            crate::i18n::configure(self.0);
        }
    }

    fn terminal_content(terminal: &Terminal<TestBackend>) -> String {
        let buffer = terminal.backend().buffer();
        let width = buffer.area.width as usize;
        let mut content = String::new();
        for row in buffer.content.chunks(width) {
            let mut column = 0;
            while column < row.len() {
                let symbol = row[column].symbol();
                content.push_str(symbol);
                column += UnicodeWidthStr::width(symbol).max(1);
            }
            content.push('\n');
        }
        content
    }

    fn sample_preflight_report() -> CheckReport {
        CheckReport {
            groups: vec![CheckGroup {
                title: "Host platform",
                results: vec![
                    CheckResult::new("platform.linux", "Operating system").set(
                        Status::Pass,
                        "linux",
                        "Linux detected",
                    ),
                    CheckResult::new("platform.architecture", "CPU architecture")
                        .set(
                            Status::Warning,
                            "riscv64",
                            "Release availability is unknown",
                        )
                        .suggestion("Confirm that a compatible release asset exists"),
                ],
            }],
            summary: Status::Warning,
            counts: StatusCounts {
                pass: 1,
                warning: 1,
                error: 0,
                unknown: 0,
            },
        }
    }

    #[test]
    fn renders_sidebar_and_install_form() {
        let _language = LanguageGuard::set(Language::En);
        let backend = TestBackend::new(100, 28);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = ConsoleApp::new();
        app.menu_index = 1;
        app.focus = Focus::Panel;
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let content: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(content.contains("Landscape Kit"));
        assert!(content.contains("Navigation"));
        assert!(content.contains("Install root"));
        assert!(content.contains("Confirm password"));
        assert!(content.contains("Start installation"));
        assert!(!content.contains("Repository URL"));
        assert!(content.contains("Environment checks"));
        assert!(content.contains("NOT RUN"));
        assert!(content.contains("Enter Details"));
        assert!(content.contains("L  Language: English (en)"));
    }

    #[test]
    fn language_key_switches_the_tui_and_updates_the_footer() {
        let _language = LanguageGuard::set(Language::En);
        let backend = TestBackend::new(100, 28);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = ConsoleApp::new();

        terminal.draw(|frame| render(frame, &app)).unwrap();
        let english = terminal_content(&terminal);
        assert!(english.contains("Navigation"));
        assert!(english.contains("L  Language: English (en)"));

        app.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));
        assert_eq!(crate::i18n::current(), Language::Zh);
        let mut chinese_terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
        chinese_terminal.draw(|frame| render(frame, &app)).unwrap();
        let chinese = terminal_content(&chinese_terminal);
        assert!(chinese.contains("导航"));
        assert!(chinese.contains("L  语言：中文 (zh)"));
        assert!(!chinese.contains("Language: English (en)"));

        app.handle_key(KeyEvent::new(KeyCode::Char('L'), KeyModifiers::SHIFT));
        assert_eq!(crate::i18n::current(), Language::En);
    }

    #[test]
    fn language_key_remains_text_while_editing() {
        let _language = LanguageGuard::set(Language::En);
        let mut app = ConsoleApp::new();
        app.menu_index = 1;
        app.focus = Focus::Panel;
        app.install.checks_selected = false;
        app.install.selected = 0;
        app.install.editing = true;
        app.install.version.clear();

        app.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));

        assert_eq!(crate::i18n::current(), Language::En);
        assert_eq!(app.install.version, "l");
        assert!(!app.language_switch_available());
    }

    #[test]
    fn repository_url_only_appears_for_custom_repository() {
        let backend = TestBackend::new(100, 28);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = ConsoleApp::new();
        app.menu_index = 1;
        app.focus = Focus::Panel;
        app.install.checks_selected = false;
        app.install.repository = RepositoryMode::Custom;

        terminal.draw(|frame| render(frame, &app)).unwrap();

        let content: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(content.contains("Repository URL"));
    }

    #[test]
    fn renders_contextual_help_below_form_on_narrow_terminal() {
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = ConsoleApp::new();
        app.menu_index = 1;
        app.focus = Focus::Panel;
        app.install.checks_selected = false;

        terminal.draw(|frame| render(frame, &app)).unwrap();

        let content: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(content.contains("About: Version"));
        assert!(content.contains("Release to install"));
    }

    #[test]
    fn renders_preflight_summary_and_expanded_results() {
        let backend = TestBackend::new(100, 28);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = ConsoleApp::new();
        app.menu_index = 1;
        app.focus = Focus::Panel;
        app.preflight.state = PreflightState::Complete(sample_preflight_report());

        terminal.draw(|frame| render(frame, &app)).unwrap();
        let summary: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(summary.contains("1 pass / 1 warn / 0 error / 0 unknown"));

        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.preflight.expanded);
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let details: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(details.contains("Host platform"));
        assert!(details.contains("Operating system"));
        assert!(details.contains("Release availability is unknown"));
        assert!(details.contains("Confirm that a compatible release asset exists"));

        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.preflight.expanded);
        assert_eq!(app.exit_state, ExitState::Idle);
    }

    #[test]
    fn entering_install_starts_background_checks() {
        let mut app = ConsoleApp::new();
        app.menu_index = 1;

        app.update_preflight();

        assert!(!matches!(app.preflight.state, PreflightState::NotRun));
    }

    #[test]
    fn every_install_field_has_contextual_help() {
        let mut form = InstallForm::default();
        for selected in 0..FORM_FIELDS {
            form.selected = selected;
            let (title, description) = form.selected_help();
            assert!(!title.is_empty(), "field {selected} has no help title");
            assert!(
                description.len() > 20,
                "field {selected} has no useful help description"
            );
        }
    }

    #[test]
    fn field_navigation_moves_between_checks_and_settings() {
        let mut app = ConsoleApp::new();
        app.menu_index = 1;
        app.focus = Focus::Panel;

        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert!(!app.install.checks_selected);
        assert_eq!(app.install.selected, 0);

        app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert!(app.install.checks_selected);
    }

    #[test]
    fn field_navigation_skips_hidden_repository_url() {
        let mut app = ConsoleApp::new();
        app.menu_index = 1;
        app.focus = Focus::Panel;
        app.install.selected = 1;
        app.install.checks_selected = false;

        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.install.selected, 3);

        app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.install.selected, 1);

        app.install.repository = RepositoryMode::Custom;
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.install.selected, 2);
    }

    #[test]
    fn left_returns_from_install_panel_to_navigation() {
        let mut app = ConsoleApp::new();
        app.menu_index = 1;
        app.install.selected = 1;
        let repository = app.install.repository;

        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(app.focus, Focus::Panel);

        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(app.focus, Focus::Navigation);
        assert_eq!(app.install.repository, repository);
    }

    #[test]
    fn right_still_changes_install_choices() {
        let mut app = ConsoleApp::new();
        app.menu_index = 1;
        app.focus = Focus::Panel;
        app.install.selected = 1;
        app.install.checks_selected = false;

        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));

        assert_eq!(app.focus, Focus::Panel);
        assert_eq!(app.install.repository, RepositoryMode::Mirror);
    }

    #[test]
    fn double_escape_opens_confirmation_before_enter_exits() {
        let mut app = ConsoleApp::new();
        let escape = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);

        assert!(app.handle_key(escape).is_none());
        assert_eq!(app.exit_state, ExitState::Armed);

        let backend = TestBackend::new(100, 28);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let armed: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(armed.contains("Exit armed"));
        assert!(!armed.contains("Exit Landscape Kit?"));

        assert!(
            app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE))
                .is_none()
        );
        assert_eq!(app.exit_state, ExitState::Idle);

        assert!(app.handle_key(escape).is_none());
        assert!(app.handle_key(escape).is_none());
        assert_eq!(app.exit_state, ExitState::Confirming);

        terminal.draw(|frame| render(frame, &app)).unwrap();
        let confirmation: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(confirmation.contains("Exit Landscape Kit?"));
        assert!(confirmation.contains("Press Enter to exit"));

        assert!(matches!(
            app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Some(ConsoleAction::Quit)
        ));
    }

    #[test]
    fn renders_stable_small_terminal_state() {
        let backend = TestBackend::new(60, 14);
        let mut terminal = Terminal::new(backend).unwrap();
        let app = ConsoleApp::new();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let content: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(content.contains("Terminal too small"));
    }

    #[test]
    fn install_form_builds_cli_and_domain_request() {
        let mut form = InstallForm {
            version: "1.2.3".into(),
            repository: RepositoryMode::Custom,
            repository_url: "https://example.com/releases/".into(),
            install_dir: "/opt/landscape".into(),
            admin_user: "operator".into(),
            password: "Secret123".into(),
            password_confirmation: "Secret123".into(),
            manager: ManagerMode::None,
            takeover_network: false,
            selected: 9,
            checks_selected: false,
            editing: false,
        };
        let ConsoleAction::Command { command, args } = form.command().unwrap() else {
            panic!("expected install command");
        };
        let Commands::Install(install) = command else {
            panic!("expected install request");
        };
        assert_eq!(install.version.as_deref(), Some("1.2.3"));
        assert_eq!(
            install.repository,
            Some(Some("https://example.com/releases/".into()))
        );
        assert_eq!(install.service_manager, Some(ServiceManagerArg::None));
        assert!(!format!("{install:?}").contains("Secret123"));
        assert_eq!(install.interactive_password.as_deref(), Some("Secret123"));
        assert!(install.password_file.is_none());
        assert_eq!(args[0], "install");
        assert!(args.windows(2).any(|pair| pair == ["--version", "1.2.3"]));
        assert!(args.iter().all(|argument| !argument.contains("Secret123")));
    }

    #[test]
    fn install_form_rejects_invalid_version_before_launch() {
        let mut form = InstallForm {
            version: "nightly".into(),
            ..InstallForm::default()
        };
        assert!(form.command().is_err());
    }

    #[test]
    fn install_form_masks_and_confirms_password() {
        assert_eq!(mask("Secret123"), "*********");
        let mut form = InstallForm {
            password: "Secret123".into(),
            password_confirmation: "Different123".into(),
            ..InstallForm::default()
        };
        assert_eq!(
            form.command().err().unwrap(),
            "password confirmation does not match"
        );
    }
}
