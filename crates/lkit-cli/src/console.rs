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
use crate::network::config::{
    DEFAULT_MANAGEMENT_CIDR, Ipv4Cidr, NetworkMode, NetworkPlan, SelectedInterface, WanIpv4Config,
};
use crate::network::discovery::{self, DefaultRoute, Interface};

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

/// 环境检查门禁：Pass 和 warning 放行；NotRun/Running 静默等待；
/// Error、unknown 和 worker 失败通过居中弹窗阻断。
enum GateState {
    None,
    Waiting,
    Dialog,
}

impl ConsoleApp {
    fn preflight_gate(&self) -> GateState {
        match &self.preflight.state {
            PreflightState::NotRun | PreflightState::Running(_) => GateState::Waiting,
            PreflightState::Failed(_) => GateState::Dialog,
            PreflightState::Complete(report) => match report.summary {
                Status::Pass | Status::Warning => GateState::None,
                Status::Error | Status::Unknown => GateState::Dialog,
            },
        }
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
    preflight_dialog: bool,
    network_wizard: Option<NetworkWizard>,
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
            preflight_dialog: false,
            network_wizard: None,
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
        if self.network_wizard.is_some() {
            return self.handle_network_wizard_key(key);
        }
        if self.preflight_dialog {
            match key.code {
                KeyCode::Enter => {
                    if matches!(&self.preflight.state, PreflightState::Complete(_)) {
                        self.preflight.expanded = true;
                        self.preflight.scroll = 0;
                    }
                    self.preflight_dialog = false;
                }
                KeyCode::Esc => self.preflight_dialog = false,
                KeyCode::Char('r' | 'R') => {
                    self.preflight_dialog = false;
                    self.preflight.restart();
                }
                _ => {}
            }
            return None;
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
                Focus::Panel if self.menu() == Menu::Install && self.install.checks_selected => {
                    match self.preflight_gate() {
                        GateState::None => self.install.select_next(),
                        GateState::Waiting => {}
                        GateState::Dialog => self.preflight_dialog = true,
                    }
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
                    } else if self.install.selected == 9 {
                        match self.preflight_gate() {
                            GateState::None => {
                                if self.install.takeover_network {
                                    match self.install.validate() {
                                        Ok(()) => match NetworkWizard::discover() {
                                            Ok(wizard) => {
                                                self.network_wizard = Some(wizard);
                                                self.notice = crate::tr!(
                                                    "Configure network takeover",
                                                    "配置网络接管"
                                                )
                                                .into();
                                            }
                                            Err(error) => self.notice = error,
                                        },
                                        Err(error) => self.notice = error,
                                    }
                                } else {
                                    match self.install.activate() {
                                        Ok(Some(action)) => return Some(action),
                                        Ok(None) => self.notice = "Ready".into(),
                                        Err(error) => self.notice = error,
                                    }
                                }
                            }
                            GateState::Waiting => {
                                self.notice = crate::tr!(
                                    "environment checks have not completed yet",
                                    "环境检查尚未完成"
                                )
                                .into();
                            }
                            GateState::Dialog => self.preflight_dialog = true,
                        }
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
        if let Some(wizard) = self.network_wizard.as_mut() {
            if wizard.editing {
                if let Some(target) = wizard.value_mut() {
                    let remaining = 128_usize.saturating_sub(target.chars().count());
                    target.extend(
                        value
                            .chars()
                            .filter(|character| !character.is_control())
                            .take(remaining),
                    );
                }
            }
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
            crate::tr!(
                "Ctrl+C Exit  Enter Confirm  Esc Cancel",
                "Ctrl+C 退出  Enter 确认  Esc 取消"
            )
        } else if self.exit_state == ExitState::Armed {
            crate::tr!(
                "Ctrl+C Exit  Press Esc again for exit confirmation  Any other key cancels",
                "Ctrl+C 退出  再次按 Esc 打开退出确认  其他按键取消"
            )
        } else if self.preflight.expanded && self.menu() == Menu::Install {
            crate::tr!(
                "Ctrl+C Exit  Up/Down Scroll  PgUp/PgDn Page  R Re-run  Esc Close",
                "Ctrl+C 退出  上/下 滚动  PgUp/PgDn 翻页  R 重跑  Esc 关闭"
            )
        } else if self.preflight_dialog {
            crate::tr!(
                "Enter Details  Esc Close  R Re-run",
                "Enter 详情  Esc 关闭  R 重跑"
            )
        } else if self.install.editing && self.menu() == Menu::Install && self.focus == Focus::Panel
        {
            crate::tr!(
                "Ctrl+C Exit  Type Edit  Backspace Delete  Enter/Esc Finish",
                "Ctrl+C 退出  输入 编辑  Backspace 删除  Enter/Esc 完成"
            )
        } else {
            match (self.focus, self.menu()) {
                (Focus::Navigation, _) => {
                    crate::tr!(
                        "Ctrl+C Exit  Up/Down Menu  Right/Enter Open  Tab Switch  Esc Esc Exit prompt",
                        "Ctrl+C 退出  上/下 菜单  右/Enter 打开  Tab 切换  Esc Esc 退出确认"
                    )
                }
                (Focus::Panel, Menu::Install) if self.install.checks_selected => {
                    crate::tr!(
                        "Ctrl+C Exit  Enter Details  R Re-run  Down Settings  Left Menu  Esc Esc Exit prompt",
                        "Ctrl+C 退出  Enter 详情  R 重跑  下 设置  左 菜单  Esc Esc 退出确认"
                    )
                }
                (Focus::Panel, Menu::Install) => {
                    crate::tr!(
                        "Ctrl+C Exit  Up/Down Field  Left Menu  Right Change  Enter Select  Tab Menu  Esc Esc Exit prompt",
                        "Ctrl+C 退出  上/下 字段  左 菜单  右 更改  Enter 选择  Tab 菜单  Esc Esc 退出确认"
                    )
                }
                (Focus::Panel, _) => crate::tr!(
                    "Ctrl+C Exit  Left Menu  Tab Switch  Esc Esc Exit prompt",
                    "Ctrl+C 退出  左 菜单  Tab 切换  Esc Esc 退出确认"
                ),
            }
        }
    }

    fn language_switch_available(&self) -> bool {
        self.exit_state != ExitState::Confirming && !self.install.editing
    }

    fn handle_network_wizard_key(&mut self, key: KeyEvent) -> Option<ConsoleAction> {
        let Some(wizard) = self.network_wizard.as_mut() else {
            return None;
        };
        if wizard.cancel_confirming {
            match key.code {
                KeyCode::Enter => {
                    self.network_wizard = None;
                    self.notice = "Ready".into();
                }
                KeyCode::Esc => wizard.cancel_confirming = false,
                _ => {}
            }
            return None;
        }
        if key.code == KeyCode::Esc {
            if wizard.step == WizardStep::Wan {
                wizard.cancel_confirming = true;
            } else {
                wizard.back();
            }
            return None;
        }
        if matches!(
            wizard.step,
            WizardStep::WanStatic
                | WizardStep::Management
                | WizardStep::DhcpStart
                | WizardStep::DhcpEnd
        ) && wizard.editing
        {
            match key.code {
                KeyCode::Up | KeyCode::Down if wizard.step == WizardStep::WanStatic => {
                    wizard.wan_static_field = (wizard.wan_static_field + 1) % 2;
                }
                KeyCode::Backspace => {
                    wizard.value_mut().map(String::pop);
                }
                KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    if let Some(value) = wizard.value_mut()
                        && value.chars().count() < 128
                    {
                        value.push(character);
                    }
                }
                KeyCode::Enter => {
                    wizard.editing = false;
                    if let Err(error) = wizard.advance_after_edit() {
                        self.notice = error;
                        wizard.editing = true;
                    }
                }
                _ => {}
            }
            return None;
        }
        match wizard.step {
            WizardStep::Wan => match key.code {
                KeyCode::Up => wizard.set_wan(wizard.wan.saturating_sub(1)),
                KeyCode::Down => wizard.set_wan((wizard.wan + 1).min(wizard.interfaces.len() - 1)),
                KeyCode::Enter => {
                    wizard.apply_wan_selection();
                    wizard.step = WizardStep::WanMode;
                }
                _ => {}
            },
            WizardStep::WanMode => match key.code {
                KeyCode::Left | KeyCode::Right => wizard.wan_mode = wizard.wan_mode.toggle(),
                KeyCode::Enter => {
                    wizard.step = if wizard.wan_mode == WanMode::Static {
                        WizardStep::WanStatic
                    } else {
                        WizardStep::Lan
                    };
                    wizard.editing = matches!(wizard.step, WizardStep::WanStatic);
                    wizard.wan_static_field = 0;
                }
                _ => {}
            },
            WizardStep::Lan => match key.code {
                KeyCode::Up => wizard.lan_cursor = wizard.lan_cursor.saturating_sub(1),
                KeyCode::Down => {
                    if !wizard.lan_candidates.is_empty() {
                        wizard.lan_cursor =
                            (wizard.lan_cursor + 1).min(wizard.lan_candidates.len() - 1);
                    }
                }
                KeyCode::Char(' ') => {
                    if let Some(selected) = wizard.lan_selected.get_mut(wizard.lan_cursor) {
                        *selected = !*selected;
                    }
                }
                KeyCode::Enter => {
                    wizard.step = if wizard.lan_selected.iter().any(|selected| *selected) {
                        WizardStep::Management
                    } else {
                        WizardStep::Confirm
                    };
                    wizard.editing = matches!(wizard.step, WizardStep::Management);
                }
                _ => {}
            },
            WizardStep::Confirm => match key.code {
                KeyCode::Enter => {
                    let plan = match wizard.plan() {
                        Ok(plan) => plan,
                        Err(error) => {
                            self.notice = error;
                            return None;
                        }
                    };
                    match self.install.command_with_network_plan(Some(plan)) {
                        Ok(action) => {
                            self.network_wizard = None;
                            return Some(action);
                        }
                        Err(error) => self.notice = error,
                    }
                }
                _ => {}
            },
            _ => {}
        }
        None
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

    fn validate(&self) -> Result<(), String> {
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
                "password confirmation does not match",
                "两次输入的密码不一致"
            )
            .into());
        }
        credentials::validate_password(&self.password).map_err(|error| error.to_string())?;
        Ok(())
    }

    fn command(&mut self) -> Result<ConsoleAction, String> {
        self.command_with_network_plan(None)
    }

    fn command_with_network_plan(
        &mut self,
        network_plan: Option<NetworkPlan>,
    ) -> Result<ConsoleAction, String> {
        self.validate()?;
        let version = self.version.trim();
        let requested_install_dir = PathBuf::from(&self.install_dir);
        let install_dir = plan::select_install_root(Some(&requested_install_dir), None)
            .map_err(|error| error.to_string())?;
        let repository = match self.repository {
            RepositoryMode::Github => None,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WizardStep {
    Wan,
    WanMode,
    WanStatic,
    Lan,
    Management,
    DhcpStart,
    DhcpEnd,
    Confirm,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WanMode {
    Static,
    Dhcp,
}

impl WanMode {
    fn toggle(self) -> Self {
        match self {
            Self::Static => Self::Dhcp,
            Self::Dhcp => Self::Static,
        }
    }
}

struct NetworkWizard {
    interfaces: Vec<Interface>,
    routes: Vec<DefaultRoute>,
    wan: usize,
    step: WizardStep,
    wan_mode: WanMode,
    address: String,
    gateway: String,
    wan_static_field: usize,
    lan_candidates: Vec<Interface>,
    lan_cursor: usize,
    lan_selected: Vec<bool>,
    management: String,
    dhcp_start: String,
    dhcp_end: String,
    editing: bool,
    cancel_confirming: bool,
}

impl NetworkWizard {
    fn discover() -> Result<Self, String> {
        discovery::ensure_management_bridge_absent(std::path::Path::new("/sys/class/net"))
            .map_err(|error| error.to_string())?;
        let (interfaces, routes) = discovery::discover(
            std::path::Path::new("/sys/class/net"),
            std::path::Path::new("/usr/sbin/ip"),
        )
        .map_err(|error| error.to_string())?;
        let mut wizard = Self {
            interfaces,
            routes,
            wan: 0,
            step: WizardStep::Wan,
            wan_mode: WanMode::Static,
            address: String::new(),
            gateway: String::new(),
            wan_static_field: 0,
            lan_candidates: Vec::new(),
            lan_cursor: 0,
            lan_selected: Vec::new(),
            management: DEFAULT_MANAGEMENT_CIDR.into(),
            dhcp_start: String::new(),
            dhcp_end: String::new(),
            editing: false,
            cancel_confirming: false,
        };
        wizard.set_wan(0);
        Ok(wizard)
    }

    fn selected_wan(&self) -> &Interface {
        &self.interfaces[self.wan]
    }

    fn set_wan(&mut self, wan: usize) {
        self.wan = wan;
        self.lan_candidates = self
            .interfaces
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != self.wan)
            .map(|(_, iface)| iface.clone())
            .collect();
        self.lan_selected = vec![false; self.lan_candidates.len()];
        self.lan_cursor = 0;
        self.wan_static_field = 0;
        self.cancel_confirming = false;
    }

    /// 进入 WAN 模式选择前按与 CLI 相同的发现规则计算默认模式与预填值。
    fn apply_wan_selection(&mut self) {
        self.cancel_confirming = false;
        let wan = self.selected_wan();
        match discovery::discovered_static(wan, &self.routes) {
            Some((address, gateway)) => {
                self.wan_mode = WanMode::Static;
                self.address = address.to_string();
                self.gateway = gateway.to_string();
            }
            None => {
                self.wan_mode = WanMode::Dhcp;
                self.address.clear();
                self.gateway.clear();
            }
        }
    }

    /// 非首页步骤 Esc 返回上一步，保留已填写的值。
    fn back(&mut self) {
        self.editing = false;
        self.cancel_confirming = false;
        match self.step {
            WizardStep::WanMode => self.step = WizardStep::Wan,
            WizardStep::WanStatic => self.step = WizardStep::WanMode,
            WizardStep::Lan => {
                self.step = if self.wan_mode == WanMode::Static {
                    WizardStep::WanStatic
                } else {
                    WizardStep::WanMode
                };
            }
            WizardStep::Management => self.step = WizardStep::Lan,
            WizardStep::DhcpStart => self.step = WizardStep::Management,
            WizardStep::DhcpEnd => self.step = WizardStep::DhcpStart,
            WizardStep::Confirm => {
                self.step = if self.lan_selected.iter().any(|selected| *selected) {
                    WizardStep::DhcpEnd
                } else {
                    WizardStep::Lan
                };
            }
            WizardStep::Wan => {}
        }
    }

    fn value_mut(&mut self) -> Option<&mut String> {
        match self.step {
            WizardStep::WanStatic if self.wan_static_field == 0 => Some(&mut self.address),
            WizardStep::WanStatic => Some(&mut self.gateway),
            WizardStep::Management => Some(&mut self.management),
            WizardStep::DhcpStart => Some(&mut self.dhcp_start),
            WizardStep::DhcpEnd => Some(&mut self.dhcp_end),
            _ => None,
        }
    }

    fn advance_after_edit(&mut self) -> Result<(), String> {
        match self.step {
            WizardStep::WanStatic if self.wan_static_field == 0 => {
                self.address
                    .trim()
                    .parse::<Ipv4Cidr>()
                    .map_err(|error| error.to_string())?;
                self.wan_static_field = 1;
                self.editing = true;
            }
            WizardStep::WanStatic => {
                self.gateway
                    .trim()
                    .parse::<std::net::Ipv4Addr>()
                    .map_err(|_| crate::tr!("invalid WAN gateway", "WAN 网关无效"))?;
                self.address
                    .trim()
                    .parse::<Ipv4Cidr>()
                    .map_err(|error| error.to_string())?;
                self.editing = false;
                self.step = WizardStep::Lan;
            }
            WizardStep::Management => {
                self.management
                    .trim()
                    .parse::<Ipv4Cidr>()
                    .map_err(|error| error.to_string())?;
                let management: Ipv4Cidr = self.management.trim().parse().unwrap();
                let (start, end) = management
                    .default_pool()
                    .map_err(|error| error.to_string())?;
                self.dhcp_start = start.to_string();
                self.dhcp_end = end.to_string();
                self.step = WizardStep::DhcpStart;
                self.editing = true;
            }
            WizardStep::DhcpStart => {
                self.dhcp_start
                    .trim()
                    .parse::<std::net::Ipv4Addr>()
                    .map_err(|_| {
                        crate::tr!(
                            "invalid LAN DHCP range start",
                            "LAN DHCP 地址池起始地址无效"
                        )
                    })?;
                self.step = WizardStep::DhcpEnd;
                self.editing = true;
            }
            WizardStep::DhcpEnd => {
                self.dhcp_end
                    .trim()
                    .parse::<std::net::Ipv4Addr>()
                    .map_err(|_| {
                        crate::tr!("invalid LAN DHCP range end", "LAN DHCP 地址池结束地址无效")
                    })?;
                self.step = WizardStep::Confirm;
                self.editing = false;
            }
            _ => {}
        }
        Ok(())
    }

    fn plan(&self) -> Result<NetworkPlan, String> {
        let wan = self.selected_wan();
        let selected_macs = std::iter::once(SelectedInterface {
            name: wan.name.clone(),
            mac: wan.mac.clone(),
        })
        .chain(
            self.lan_candidates
                .iter()
                .zip(&self.lan_selected)
                .filter(|(_, selected)| **selected)
                .map(|(iface, _)| SelectedInterface {
                    name: iface.name.clone(),
                    mac: iface.mac.clone(),
                }),
        )
        .collect();
        let lan = self
            .lan_candidates
            .iter()
            .zip(&self.lan_selected)
            .filter(|(_, selected)| **selected)
            .map(|(iface, _)| iface.name.clone())
            .collect::<Vec<_>>();
        let mode = if lan.is_empty() {
            match self.wan_mode {
                WanMode::Dhcp => NetworkMode::WanDhcp {
                    wan: wan.name.clone(),
                },
                WanMode::Static => NetworkMode::WanOnly {
                    wan: wan.name.clone(),
                    address: self.address.trim().parse().map_err(
                        |error: crate::deployment::plan::InstallError| error.to_string(),
                    )?,
                    gateway: self
                        .gateway
                        .trim()
                        .parse()
                        .map_err(|_| crate::tr!("invalid WAN gateway", "WAN 网关无效"))?,
                },
            }
        } else {
            let management = self
                .management
                .trim()
                .parse()
                .map_err(|error: crate::deployment::plan::InstallError| error.to_string())?;
            let dhcp_start = self.dhcp_start.trim().parse().map_err(|_| {
                crate::tr!(
                    "invalid LAN DHCP range start",
                    "LAN DHCP 地址池起始地址无效"
                )
            })?;
            let dhcp_end = self.dhcp_end.trim().parse().map_err(|_| {
                crate::tr!("invalid LAN DHCP range end", "LAN DHCP 地址池结束地址无效")
            })?;
            NetworkMode::RoutedLan {
                wan: wan.name.clone(),
                wan_ipv4: Some(match self.wan_mode {
                    WanMode::Static => WanIpv4Config::Static {
                        address: self.address.trim().parse().map_err(
                            |error: crate::deployment::plan::InstallError| error.to_string(),
                        )?,
                        gateway: self
                            .gateway
                            .trim()
                            .parse()
                            .map_err(|_| crate::tr!("invalid WAN gateway", "WAN 网关无效"))?,
                    },
                    WanMode::Dhcp => WanIpv4Config::Dhcp,
                }),
                lan,
                management,
                dhcp_start,
                dhcp_end,
            }
        };
        let plan = NetworkPlan {
            mode,
            selected_macs,
        };
        plan.validate().map_err(|error| error.to_string())?;
        Ok(plan)
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
    if let Some(wizard) = &app.network_wizard {
        render_network_wizard(frame, wizard);
        return;
    }
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
    if app.preflight_dialog {
        render_preflight_dialog(frame, app);
    }
}

fn render_preflight_dialog(frame: &mut Frame<'_>, app: &ConsoleApp) {
    let lines: Vec<Line<'_>> = match &app.preflight.state {
        PreflightState::Failed(error) => vec![
            Line::styled(
                crate::tr!("Environment checks could not complete", "环境检查无法完成"),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Line::raw(""),
            Line::raw(error.clone()),
        ],
        PreflightState::Complete(report) => {
            let mut lines = vec![
                Line::styled(
                    crate::tr!("Environment checks block installation", "环境检查阻止安装"),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Line::raw(""),
            ];
            let items = blocking_items(report);
            if items.is_empty() {
                lines.push(Line::raw(crate::tr!(
                    "Checks did not pass.",
                    "检查未通过。"
                )));
            } else {
                for item in items {
                    lines.push(Line::raw(format!("- {item}")));
                }
            }
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                crate::tr!(
                    "Enter view details  Esc close  R re-run",
                    "Enter 查看详情  Esc 关闭  R 重跑"
                ),
                Style::default().fg(Color::DarkGray),
            ));
            lines
        }
        _ => return,
    };
    let screen = frame.area();
    let width = 64.min(screen.width.saturating_sub(2));
    let height = (lines.len() as u16 + 2).min(screen.height.saturating_sub(2));
    let area = Rect::new(
        screen.x + screen.width.saturating_sub(width) / 2,
        screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .block(Block::bordered().title(crate::tr!("Install blocked", "安装被阻止"))),
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

fn render_network_wizard(frame: &mut Frame<'_>, wizard: &NetworkWizard) {
    let area = frame.area();
    let [title, body, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(8),
        Constraint::Length(2),
    ])
    .areas(area);
    frame.render_widget(
        Paragraph::new(crate::tr!(
            "Landscape network takeover",
            "Landscape 网络接管"
        ))
        .style(Style::default().add_modifier(Modifier::BOLD))
        .block(Block::default().borders(Borders::BOTTOM)),
        title,
    );
    let mut lines = Vec::new();
    match wizard.step {
        WizardStep::Wan => {
            lines.push(Line::styled(
                crate::tr!("Select the WAN interface", "选择 WAN 网卡"),
                Style::default().add_modifier(Modifier::BOLD),
            ));
            lines.push(Line::raw(""));
            for (index, iface) in wizard.interfaces.iter().enumerate() {
                let selected = index == wizard.wan;
                let address = iface
                    .addresses
                    .first()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| crate::tr!("no IPv4", "无 IPv4").into());
                let gateway = wizard
                    .routes
                    .iter()
                    .find(|route| route.iface == iface.name)
                    .map(|route| route.gateway.to_string())
                    .unwrap_or_else(|| crate::tr!("not found", "未发现").into());
                lines.push(Line::styled(
                    format!(
                        "{}{}  {}  {}  {}  gw {}",
                        if selected { "> " } else { "  " },
                        index + 1,
                        iface.name,
                        iface.mac,
                        address,
                        gateway
                    ),
                    if selected {
                        Style::default().fg(Color::Black).bg(Color::Cyan)
                    } else {
                        Style::default()
                    },
                ));
            }
        }
        WizardStep::WanMode => {
            lines.push(Line::styled(
                crate::tr!("WAN IPv4 mode", "WAN IPv4 模式"),
                Style::default().add_modifier(Modifier::BOLD),
            ));
            lines.push(Line::raw(""));
            lines.push(Line::raw(format!(
                "{} Static   {} DHCP",
                if wizard.wan_mode == WanMode::Static {
                    "(*)"
                } else {
                    "( )"
                },
                if wizard.wan_mode == WanMode::Dhcp {
                    "(*)"
                } else {
                    "( )"
                },
            )));
        }
        WizardStep::WanStatic => {
            lines.push(Line::styled(
                crate::tr!("WAN static IPv4 configuration", "WAN 静态 IPv4 配置"),
                Style::default().add_modifier(Modifier::BOLD),
            ));
            lines.push(Line::raw(""));
            let fields = [
                (
                    crate::tr!("IPv4 address/CIDR", "IPv4 地址/CIDR"),
                    &wizard.address,
                ),
                (crate::tr!("Default gateway", "默认网关"), &wizard.gateway),
            ];
            for (index, (label, value)) in fields.iter().enumerate() {
                let focused = index == wizard.wan_static_field;
                let style = if focused {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                let marker = if focused && wizard.editing { "_" } else { "" };
                lines.push(Line::from(vec![
                    Span::styled(if focused { "> " } else { "  " }, style),
                    Span::styled(display_pad(label, 20), style),
                    Span::styled(format!("{value}{marker}"), style),
                ]));
            }
        }
        WizardStep::Lan => {
            lines.push(Line::styled(
                crate::tr!(
                    "Select LAN interfaces (Space toggles; empty means WAN-only)",
                    "选择 LAN 网卡（空格切换；留空表示仅 WAN）"
                ),
                Style::default().add_modifier(Modifier::BOLD),
            ));
            lines.push(Line::raw(""));
            if wizard.lan_candidates.is_empty() {
                lines.push(Line::raw(crate::tr!("No other interfaces", "没有其他网卡")));
            }
            for (index, iface) in wizard.lan_candidates.iter().enumerate() {
                let cursor = index == wizard.lan_cursor;
                lines.push(Line::styled(
                    format!(
                        "{}[{}] {}  {}  {}",
                        if cursor { "> " } else { "  " },
                        if wizard.lan_selected[index] { "x" } else { " " },
                        iface.name,
                        iface.mac,
                        if iface.operstate == "up" {
                            crate::tr!("link up", "链路已启用")
                        } else {
                            crate::tr!("link down", "链路未启用")
                        }
                    ),
                    if cursor {
                        Style::default().fg(Color::Black).bg(Color::Cyan)
                    } else {
                        Style::default()
                    },
                ));
            }
        }
        WizardStep::Management | WizardStep::DhcpStart | WizardStep::DhcpEnd => {
            let (label, value) = match wizard.step {
                WizardStep::Management => (
                    crate::tr!("LAN management IPv4 address", "LAN 管理 IPv4 地址"),
                    &wizard.management,
                ),
                WizardStep::DhcpStart => (
                    crate::tr!("LAN DHCP range start", "LAN DHCP 地址池起始地址"),
                    &wizard.dhcp_start,
                ),
                WizardStep::DhcpEnd => (
                    crate::tr!("LAN DHCP range end", "LAN DHCP 地址池结束地址"),
                    &wizard.dhcp_end,
                ),
                _ => unreachable!(),
            };
            lines.push(Line::styled(
                label,
                Style::default().add_modifier(Modifier::BOLD),
            ));
            lines.push(Line::raw(""));
            lines.push(Line::raw(format!(
                "{}{}_",
                crate::tr!("Value: ", "值："),
                value
            )));
        }
        WizardStep::Confirm => {
            let wan = wizard.selected_wan();
            lines.push(Line::styled(
                crate::tr!("Confirm network takeover plan", "确认网络接管计划"),
                Style::default().add_modifier(Modifier::BOLD),
            ));
            lines.push(Line::raw(""));
            lines.push(Line::raw(crate::trf!(
                ("WAN interface  {}  MAC {}", wan.name, wan.mac),
                ("WAN 网卡  {}  MAC {}", wan.name, wan.mac)
            )));
            lines.push(Line::raw(match wizard.wan_mode {
                WanMode::Static => crate::trf!(
                    (
                        "WAN mode       Static  {}  gw {}",
                        wizard.address,
                        wizard.gateway
                    ),
                    (
                        "WAN 模式       Static  {}  网关 {}",
                        wizard.address,
                        wizard.gateway
                    )
                ),
                WanMode::Dhcp => crate::tr!("WAN mode       DHCP", "WAN 模式       DHCP").into(),
            }));
            let lan: Vec<&str> = wizard
                .lan_candidates
                .iter()
                .zip(&wizard.lan_selected)
                .filter(|(_, selected)| **selected)
                .map(|(iface, _)| iface.name.as_str())
                .collect();
            if lan.is_empty() {
                lines.push(Line::raw(crate::tr!(
                    "LAN mode       WAN-only (no bridge, no LAN DHCP)",
                    "LAN 模式       仅 WAN（不创建网桥，不启用 LAN DHCP）"
                )));
            } else {
                let names = lan.join(", ");
                lines.push(Line::raw(crate::trf!(
                    ("LAN interfaces {}", names),
                    ("LAN 网卡       {}", names)
                )));
                lines.push(Line::raw(crate::trf!(
                    ("Management      {}", wizard.management),
                    ("管理地址       {}", wizard.management)
                )));
                lines.push(Line::raw(crate::trf!(
                    (
                        "DHCP range      {} - {}",
                        wizard.dhcp_start,
                        wizard.dhcp_end
                    ),
                    (
                        "DHCP 范围       {} - {}",
                        wizard.dhcp_start,
                        wizard.dhcp_end
                    )
                )));
            }
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                crate::tr!(
                    "Selected LAN interfaces will have their IPv4/IPv6 addresses flushed; unselected interfaces remain unchanged.",
                    "所选 LAN 网卡将清理 IPv4/IPv6 地址；未选择的网卡保持不变。"
                ),
                Style::default().fg(Color::Yellow),
            ));
            lines.push(Line::styled(
                crate::tr!("Press Enter to start installation.", "按 Enter 开始安装。"),
                Style::default().add_modifier(Modifier::BOLD),
            ));
        }
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title(crate::tr!("Network", "网络")))
            .wrap(Wrap { trim: true }),
        body,
    );
    frame.render_widget(
        Paragraph::new(wizard_hints(wizard)).style(Style::default().fg(Color::DarkGray)),
        footer,
    );
    if wizard.cancel_confirming {
        render_wizard_cancel_confirmation(frame);
    }
}

fn wizard_hints(wizard: &NetworkWizard) -> &'static str {
    if wizard.cancel_confirming {
        return crate::tr!("Enter cancel wizard  Esc close", "Enter 取消向导  Esc 关闭");
    }
    match wizard.step {
        WizardStep::Wan => crate::tr!(
            "Up/Down select WAN  Enter confirm  Esc cancel wizard",
            "上/下 选择 WAN  Enter 确认  Esc 取消向导"
        ),
        WizardStep::WanMode => crate::tr!(
            "Left/Right choose mode  Enter confirm  Esc back",
            "左/右 选择模式  Enter 确认  Esc 返回"
        ),
        WizardStep::WanStatic => crate::tr!(
            "Up/Down field  Type edit  Backspace delete  Enter confirm  Esc back",
            "上/下 切换字段  输入编辑  Backspace 删除  Enter 确认  Esc 返回"
        ),
        WizardStep::Lan => crate::tr!(
            "Up/Down move  Space select  Enter continue  Esc back",
            "上/下 移动  空格选择  Enter 继续  Esc 返回"
        ),
        WizardStep::Management | WizardStep::DhcpStart | WizardStep::DhcpEnd => crate::tr!(
            "Type edit  Backspace delete  Enter confirm  Esc back",
            "输入编辑  Backspace 删除  Enter 确认  Esc 返回"
        ),
        WizardStep::Confirm => crate::tr!(
            "Enter start installation  Esc back",
            "Enter 开始安装  Esc 返回上一步"
        ),
    }
}

fn render_wizard_cancel_confirmation(frame: &mut Frame<'_>) {
    let screen = frame.area();
    let width = 52.min(screen.width.saturating_sub(2));
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
                crate::tr!("Cancel network wizard?", "取消网络向导？"),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Line::raw(""),
            Line::raw(crate::tr!(
                "Press Enter to cancel the wizard and return to the Install form.",
                "按 Enter 取消向导并返回 Install 表单。"
            )),
            Line::styled(
                crate::tr!(
                    "Press Esc to close and continue.",
                    "按 Esc 关闭并继续向导。"
                ),
                Style::default().fg(Color::DarkGray),
            ),
        ])
        .alignment(Alignment::Center)
        .block(Block::bordered().title(crate::tr!("Cancel wizard", "取消向导"))),
        area,
    );
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
    let focused = app.focus == Focus::Panel;
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
                .block(panel_block(menu.label(), focused))
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
            .block(panel_block(
                crate::tr!("Overview", "概览"),
                app.focus == Focus::Panel,
            ))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_install(frame: &mut Frame<'_>, app: &ConsoleApp, area: Rect) {
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
            Span::styled(format!("{status:<9}"), status_style),
            Span::raw(detail),
        ]))
        .style(style)
        .block(panel_block(
            crate::tr!("Environment checks", "环境检查"),
            selected,
        )),
        area,
    );
}

fn render_preflight_details(
    frame: &mut Frame<'_>,
    preflight: &Preflight,
    focused: bool,
    area: Rect,
) {
    frame.render_widget(
        Paragraph::new(preflight_detail_lines(preflight))
            .block(panel_block(
                crate::tr!("Environment checks", "环境检查"),
                focused,
            ))
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
            } else if index == 9 {
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
            if selected {
                Some(line.style(Style::default().bg(Color::Cyan)))
            } else {
                Some(line)
            }
        })
        .collect();
    frame.render_widget(
        Paragraph::new(lines).block(panel_block(
            crate::tr!("Install", "安装"),
            app.focus == Focus::Panel && !form.checks_selected,
        )),
        area,
    );
}

fn panel_block(title: &str, focused: bool) -> Block<'static> {
    let title = if focused {
        format!("> {title}")
    } else {
        title.to_string()
    };
    let border_style = if focused {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    Block::bordered().title(title).border_style(border_style)
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

    fn pass_preflight_report() -> CheckReport {
        CheckReport {
            groups: vec![CheckGroup {
                title: "Host platform",
                results: vec![CheckResult::new("platform.linux", "Operating system").set(
                    Status::Pass,
                    "linux",
                    "Linux detected",
                )],
            }],
            summary: Status::Pass,
            counts: StatusCounts {
                pass: 1,
                warning: 0,
                error: 0,
                unknown: 0,
            },
        }
    }

    fn error_preflight_report() -> CheckReport {
        CheckReport {
            groups: vec![CheckGroup {
                title: "Ports",
                results: vec![
                    CheckResult::new("ports.6443", "Port 6443")
                        .set(Status::Error, "6443", "already in use")
                        .suggestion("stop the conflicting process"),
                    CheckResult::new("ports.22", "Port 22").set(
                        Status::Unknown,
                        "",
                        "cannot probe",
                    ),
                ],
            }],
            summary: Status::Error,
            counts: StatusCounts {
                pass: 0,
                warning: 0,
                error: 1,
                unknown: 1,
            },
        }
    }

    fn sample_network_wizard() -> NetworkWizard {
        let mut wizard = NetworkWizard {
            interfaces: vec![
                Interface {
                    name: "ens32".into(),
                    mac: "00:0c:29:a4:09:08".into(),
                    operstate: "up".into(),
                    addresses: vec!["10.1.1.105/24".parse().unwrap()],
                },
                Interface {
                    name: "ens33".into(),
                    mac: "00:0c:29:a4:09:12".into(),
                    operstate: "down".into(),
                    addresses: Vec::new(),
                },
            ],
            routes: Vec::new(),
            wan: 0,
            step: WizardStep::Wan,
            wan_mode: WanMode::Static,
            address: String::new(),
            gateway: String::new(),
            wan_static_field: 0,
            lan_candidates: Vec::new(),
            lan_cursor: 0,
            lan_selected: Vec::new(),
            management: DEFAULT_MANAGEMENT_CIDR.into(),
            dhcp_start: String::new(),
            dhcp_end: String::new(),
            editing: false,
            cancel_confirming: false,
        };
        wizard.set_wan(0);
        wizard
    }

    fn routes_armed_wizard() -> NetworkWizard {
        let mut wizard = sample_network_wizard();
        wizard.routes = vec![DefaultRoute {
            iface: "ens32".into(),
            gateway: "10.1.1.1".parse().unwrap(),
        }];
        wizard
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
        assert!(content.contains("> Environment checks"));
        assert!(content.contains("NOT RUN"));
        assert!(content.contains("Enter Details"));
        assert!(content.contains("L  Language: English (en)"));
    }

    #[test]
    fn network_wizard_is_full_screen_and_supports_keyboard_selection() {
        let _language = LanguageGuard::set(Language::En);
        let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
        let mut app = ConsoleApp::new();
        app.network_wizard = Some(sample_network_wizard());

        terminal.draw(|frame| render(frame, &app)).unwrap();
        let content = terminal_content(&terminal);
        assert!(content.contains("Select the WAN interface"));
        assert!(content.contains("not found"));
        assert!(!content.contains("Navigation"));

        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(
            app.network_wizard.as_ref().unwrap().selected_wan().name,
            "ens33"
        );
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let wizard = app.network_wizard.as_ref().unwrap();
        assert_eq!(wizard.step, WizardStep::WanMode);
        assert_eq!(wizard.wan_mode, WanMode::Dhcp);
        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(
            app.network_wizard.as_ref().unwrap().wan_mode,
            WanMode::Static
        );
        app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(app.network_wizard.as_ref().unwrap().wan_mode, WanMode::Dhcp);
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.network_wizard.as_ref().unwrap().step, WizardStep::Lan);
        app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        assert!(app.network_wizard.as_ref().unwrap().lan_selected[0]);
    }

    #[test]
    fn renders_panel_focus_marker_on_overview() {
        let _language = LanguageGuard::set(Language::En);
        let backend = TestBackend::new(100, 28);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = ConsoleApp::new();
        app.focus = Focus::Panel;

        terminal.draw(|frame| render(frame, &app)).unwrap();

        assert!(terminal_content(&terminal).contains("> Overview"));

        app.menu_index = 2;
        terminal.draw(|frame| render(frame, &app)).unwrap();
        assert!(terminal_content(&terminal).contains("> Versions"));
    }

    #[test]
    fn renders_portable_markers_for_install_focus() {
        let _language = LanguageGuard::set(Language::En);
        let backend = TestBackend::new(100, 28);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = ConsoleApp::new();
        app.menu_index = 1;
        app.focus = Focus::Panel;
        app.install.checks_selected = false;
        app.install.selected = 0;

        terminal.draw(|frame| render(frame, &app)).unwrap();

        let buffer = terminal.backend().buffer();
        assert!(terminal_content(&terminal).contains("> Install"));
        assert!(terminal_content(&terminal).contains("> Version"));
        assert!(
            buffer
                .content
                .iter()
                .any(|cell| cell.symbol() == ">" && cell.bg == Color::Cyan)
        );
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
        assert!(english.contains("Ctrl+C Exit"));
        assert!(english.contains("L  Language: English (en)"));

        app.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));
        assert_eq!(crate::i18n::current(), Language::Zh);
        let mut chinese_terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
        chinese_terminal.draw(|frame| render(frame, &app)).unwrap();
        let chinese = terminal_content(&chinese_terminal);
        assert!(chinese.contains("导航"));
        assert!(chinese.contains("Ctrl+C 退出"));
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
        let _language = LanguageGuard::set(Language::En);
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
        assert!(details.contains("Ctrl+C Exit"));
        assert!(details.contains("Esc Close"));

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
        app.preflight.state = PreflightState::Complete(pass_preflight_report());

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

    #[test]
    fn network_wizard_builds_wan_only_dhcp_plan_without_lan() {
        let mut wizard = NetworkWizard {
            interfaces: vec![Interface {
                name: "ens32".into(),
                mac: "00:0c:29:a4:09:08".into(),
                operstate: "up".into(),
                addresses: Vec::new(),
            }],
            routes: Vec::new(),
            wan: 0,
            step: WizardStep::Lan,
            wan_mode: WanMode::Dhcp,
            address: String::new(),
            gateway: String::new(),
            wan_static_field: 0,
            lan_candidates: Vec::new(),
            lan_cursor: 0,
            lan_selected: Vec::new(),
            management: DEFAULT_MANAGEMENT_CIDR.into(),
            dhcp_start: String::new(),
            dhcp_end: String::new(),
            editing: false,
            cancel_confirming: false,
        };
        let plan = wizard.plan().unwrap();
        assert!(matches!(plan.mode, NetworkMode::WanDhcp { .. }));
        wizard.set_wan(0);
        assert!(wizard.lan_candidates.is_empty());
    }

    #[test]
    fn network_wizard_prefills_static_from_discovery() {
        let mut app = ConsoleApp::new();
        app.network_wizard = Some(routes_armed_wizard());

        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        let wizard = app.network_wizard.as_ref().unwrap();
        assert_eq!(wizard.step, WizardStep::WanMode);
        assert_eq!(wizard.wan_mode, WanMode::Static);
        assert_eq!(wizard.address, "10.1.1.105/24");
        assert_eq!(wizard.gateway, "10.1.1.1");
    }

    #[test]
    fn network_wizard_defaults_to_dhcp_without_complete_static_pair() {
        let mut app = ConsoleApp::new();
        app.network_wizard = Some(sample_network_wizard());

        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        let wizard = app.network_wizard.as_ref().unwrap();
        assert_eq!(wizard.wan_mode, WanMode::Dhcp);
        assert!(wizard.address.is_empty());
        assert!(wizard.gateway.is_empty());
    }

    #[test]
    fn network_wizard_static_page_edits_both_fields_and_validates() {
        let mut app = ConsoleApp::new();
        app.network_wizard = Some(routes_armed_wizard());
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        let wizard = app.network_wizard.as_ref().unwrap();
        assert_eq!(wizard.step, WizardStep::WanStatic);
        assert!(wizard.editing);
        assert_eq!(wizard.wan_static_field, 0);

        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.network_wizard.as_ref().unwrap().wan_static_field, 1);
        app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.network_wizard.as_ref().unwrap().wan_static_field, 0);

        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let wizard = app.network_wizard.as_ref().unwrap();
        assert_eq!(wizard.wan_static_field, 1);
        assert!(wizard.editing);

        app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let wizard = app.network_wizard.as_ref().unwrap();
        assert_eq!(wizard.step, WizardStep::WanStatic);
        assert!(wizard.editing);
        assert!(!app.notice.is_empty());

        app.network_wizard.as_mut().unwrap().gateway = "10.1.1.1".into();
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let wizard = app.network_wizard.as_ref().unwrap();
        assert_eq!(wizard.step, WizardStep::Lan);
        assert!(!wizard.editing);
    }

    #[test]
    fn network_wizard_confirm_requires_enter_to_start() {
        let mut app = ConsoleApp::new();
        app.install.password = "Secret123".into();
        app.install.password_confirmation = "Secret123".into();
        app.network_wizard = Some(sample_network_wizard());

        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            app.network_wizard.as_ref().unwrap().step,
            WizardStep::Confirm
        );

        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.network_wizard.as_ref().unwrap().step, WizardStep::Lan);

        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            app.network_wizard.as_ref().unwrap().step,
            WizardStep::Confirm
        );
        assert!(matches!(
            app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Some(ConsoleAction::Command { .. })
        ));
        assert!(app.network_wizard.is_none());
    }

    #[test]
    fn network_wizard_first_page_esc_opens_cancel_confirmation() {
        let mut app = ConsoleApp::new();
        app.network_wizard = Some(sample_network_wizard());

        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.network_wizard.as_ref().unwrap().cancel_confirming);

        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.network_wizard.as_ref().unwrap().cancel_confirming);

        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.network_wizard.is_none());

        app.network_wizard = Some(sample_network_wizard());
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        let wizard = app.network_wizard.as_ref().unwrap();
        assert_eq!(wizard.step, WizardStep::Wan);
        assert!(!wizard.cancel_confirming);
    }

    #[test]
    fn error_checks_block_form_entry_with_dialog() {
        let _language = LanguageGuard::set(Language::En);
        let mut app = ConsoleApp::new();
        app.menu_index = 1;
        app.focus = Focus::Panel;
        app.preflight.state = PreflightState::Complete(error_preflight_report());
        assert!(app.install.checks_selected);

        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert!(app.install.checks_selected);
        assert!(app.preflight_dialog);

        let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let content = terminal_content(&terminal);
        assert!(content.contains("Install blocked"));
        assert!(content.contains("Port 6443"));
        assert!(content.contains("stop the conflicting process"));

        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(!app.preflight_dialog);
        assert!(app.preflight.expanded);
        assert!(app.install.checks_selected);

        app.preflight_dialog = true;
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.preflight_dialog);

        app.preflight_dialog = true;
        app.handle_key(KeyEvent::new(KeyCode::Char('R'), KeyModifiers::SHIFT));
        assert!(!app.preflight_dialog);
        assert!(matches!(app.preflight.state, PreflightState::Running(_)));
    }

    #[test]
    fn running_checks_keep_focus_on_summary_without_dialog() {
        let mut app = ConsoleApp::new();
        app.menu_index = 1;
        app.focus = Focus::Panel;
        let (_, receiver) = std::sync::mpsc::channel();
        app.preflight.state = PreflightState::Running(receiver);

        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert!(app.install.checks_selected);
        assert!(!app.preflight_dialog);
    }

    #[test]
    fn warning_checks_allow_form_entry() {
        let mut app = ConsoleApp::new();
        app.menu_index = 1;
        app.focus = Focus::Panel;
        app.preflight.state = PreflightState::Complete(sample_preflight_report());

        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert!(!app.install.checks_selected);
        assert_eq!(app.install.selected, 0);
        assert!(!app.preflight_dialog);
    }

    #[test]
    fn start_installation_is_blocked_when_checks_fail() {
        let mut app = ConsoleApp::new();
        app.menu_index = 1;
        app.focus = Focus::Panel;
        app.preflight.state = PreflightState::Complete(error_preflight_report());
        app.install.checks_selected = false;
        app.install.selected = 9;

        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.preflight_dialog);
        assert!(app.network_wizard.is_none());

        app.preflight.state = PreflightState::Complete(pass_preflight_report());
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(!app.preflight_dialog);
    }

    #[test]
    fn wizard_render_shows_gateway_and_confirm_summary() {
        let _language = LanguageGuard::set(Language::En);
        let mut terminal = Terminal::new(TestBackend::new(120, 32)).unwrap();
        let mut app = ConsoleApp::new();
        app.install.password = "Secret123".into();
        app.install.password_confirmation = "Secret123".into();
        app.network_wizard = Some(routes_armed_wizard());

        terminal.draw(|frame| render(frame, &app)).unwrap();
        let content = terminal_content(&terminal);
        assert!(content.contains("10.1.1.105/24"));
        assert!(content.contains("gw 10.1.1.1"));

        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            app.network_wizard.as_ref().unwrap().step,
            WizardStep::Confirm
        );

        terminal.draw(|frame| render(frame, &app)).unwrap();
        let content = terminal_content(&terminal);
        assert!(content.contains("Confirm network takeover plan"));
        assert!(content.contains("ens32"));
        assert!(content.contains("00:0c:29:a4:09:08"));
        assert!(content.contains("10.1.1.105/24"));
        assert!(content.contains("WAN-only"));
        assert!(content.contains("will have their IPv4/IPv6 addresses flushed"));
    }
}
