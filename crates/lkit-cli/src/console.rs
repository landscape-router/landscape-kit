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

use crate::check;
use crate::check::model::{CheckReport, Status};
use crate::commands::install::Install;
use crate::commands::{Commands, ServiceManagerArg};
use crate::deployment::{plan, root, state};
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
        return Err("a terminal is required; use an lkit subcommand for command mode".into());
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
            Self::Overview => "Overview",
            Self::Install => "Install",
            Self::Versions => "Versions",
            Self::Configuration => "Configuration",
            Self::Services => "Services",
            Self::Network => "Network",
            Self::Diagnostics => "Diagnostics",
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
        std::thread::spawn(move || {
            let _ = sender.send(check::run_all());
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
                self.state = PreflightState::Failed("check worker stopped unexpectedly".into());
            }
        }
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
            "Enter Exit  Esc Cancel"
        } else if self.exit_state == ExitState::Armed {
            "Press Esc again for exit confirmation  Any other key cancels"
        } else if self.preflight.expanded && self.menu() == Menu::Install {
            "Up/Down Scroll  PgUp/PgDn Page  R Re-run  Esc Close"
        } else if self.install.editing && self.menu() == Menu::Install && self.focus == Focus::Panel
        {
            "Type Edit  Backspace Delete  Enter/Esc Finish"
        } else {
            match (self.focus, self.menu()) {
                (Focus::Navigation, _) => {
                    "Up/Down Menu  Right/Enter Open  Tab Switch  Esc Esc Confirm"
                }
                (Focus::Panel, Menu::Install) if self.install.checks_selected => {
                    "Enter Details  R Re-run  Down Settings  Left Menu  Esc Esc Confirm"
                }
                (Focus::Panel, Menu::Install) => {
                    "Up/Down Field  Left Menu  Right Change  Enter Select  Tab Menu  Esc Esc Confirm"
                }
                (Focus::Panel, _) => "Left Menu  Tab Switch  Esc Esc Confirm",
            }
        }
    }
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
            Self::Mirror => "HTTP mirror",
            Self::Custom => "Custom HTTP",
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
            Self::Auto => "Auto",
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
                "Version",
                "Release to install. Use latest for the newest stable release or enter an exact stable version such as 1.2.3.",
            ),
            1 => (
                "Repository",
                "Release source. Choose GitHub, the default HTTP mirror, or a custom protocol v1 HTTP repository.",
            ),
            2 => (
                "Repository URL",
                "Base URL of the custom protocol v1 repository. Remote repositories require HTTPS; loopback HTTP is allowed.",
            ),
            3 => (
                "Install root",
                "Absolute directory that stores releases, configuration, state, transactions, and backups.",
            ),
            4 => (
                "Admin user",
                "Username for the initial Landscape administrator account.",
            ),
            5 => (
                "Password",
                "Password for the initial administrator. It remains masked and is validated before installation starts.",
            ),
            6 => (
                "Confirm password",
                "Enter the administrator password again to prevent typing mistakes.",
            ),
            7 => (
                "Service manager",
                "Choose automatic detection, explicit systemd supervision, or no service manager.",
            ),
            8 => (
                "Network takeover",
                "Allow Landscape to reconfigure host interfaces and network services during installation.",
            ),
            9 => (
                "Start installation",
                "Validate the form, leave the console, and start the installation using these settings.",
            ),
            _ => ("Install", "Configure the Landscape installation."),
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
            return Err("password confirmation does not match".into());
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
            Self::RootRequired => ("Root required", Color::Yellow),
            Self::NotInstalled => ("Not installed", Color::Yellow),
            Self::Installed { .. } => ("Installed", Color::Green),
            Self::Unavailable(_) => ("Attention required", Color::Red),
        }
    }
}

fn render(frame: &mut Frame<'_>, app: &ConsoleApp) {
    if frame.area().width < 72 || frame.area().height < 18 {
        frame.render_widget(
            Paragraph::new("Terminal too small")
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
    let notice_color = if app.notice == "Ready" {
        Color::DarkGray
    } else {
        Color::Red
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(app.notice.as_str(), Style::default().fg(notice_color)),
            Line::styled(app.hints(), Style::default().fg(Color::DarkGray)),
        ])
        .block(Block::default().borders(Borders::TOP)),
        status,
    );
    if app.exit_state == ExitState::Confirming {
        render_exit_confirmation(frame);
    }
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
                "Exit Landscape Kit?",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Line::raw(""),
            Line::raw("Press Enter to exit."),
            Line::styled("Press Esc to cancel.", Style::default().fg(Color::DarkGray)),
        ])
        .alignment(Alignment::Center)
        .block(Block::bordered().title("Confirm exit")),
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
            .block(Block::bordered().title("Navigation"))
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
                        "Not available in this release",
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
                "Root privileges are required",
                Style::default().fg(Color::Yellow),
            ),
            Line::raw(""),
            Line::raw(format!("Install root  {}", app.install.install_dir)),
        ],
        Snapshot::NotInstalled => vec![
            Line::styled(
                "Landscape is not installed",
                Style::default().fg(Color::Yellow),
            ),
            Line::raw(""),
            Line::raw(format!("Install root  {}", app.install.install_dir)),
        ],
        Snapshot::Installed {
            version,
            manager,
            initialized,
        } => vec![
            Line::styled("Landscape is installed", Style::default().fg(Color::Green)),
            Line::raw(""),
            Line::raw(format!("Version       {version}")),
            Line::raw(format!("Service       {manager}")),
            Line::raw(format!(
                "Initialization {}",
                if *initialized { "complete" } else { "pending" }
            )),
            Line::raw(format!("Install root  {}", app.install.install_dir)),
        ],
        Snapshot::Unavailable(error) => vec![
            Line::styled(
                "Installation state needs attention",
                Style::default().fg(Color::Red),
            ),
            Line::raw(""),
            Line::raw(error),
        ],
    };
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title("Overview"))
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
            "NOT RUN",
            "Waiting to check this host".into(),
            Color::DarkGray,
        ),
        PreflightState::Running(_) => ("RUNNING", "Checking this host...".into(), Color::Cyan),
        PreflightState::Complete(report) => (
            report.summary.label(),
            format!(
                "{} pass / {} warn / {} error / {} unknown",
                report.counts.pass,
                report.counts.warning,
                report.counts.error,
                report.counts.unknown
            ),
            check_status_color(report.summary),
        ),
        PreflightState::Failed(error) => ("FAILED", error.clone(), Color::Red),
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
        .block(Block::bordered().title("Environment checks")),
        area,
    );
}

fn render_preflight_details(frame: &mut Frame<'_>, preflight: &Preflight, area: Rect) {
    frame.render_widget(
        Paragraph::new(preflight_detail_lines(preflight))
            .block(Block::bordered().title("Environment checks"))
            .wrap(Wrap { trim: true })
            .scroll((preflight.scroll, 0)),
        area,
    );
}

fn preflight_detail_lines(preflight: &Preflight) -> Vec<Line<'static>> {
    let PreflightState::Complete(report) = &preflight.state else {
        return vec![match &preflight.state {
            PreflightState::NotRun => Line::raw("Checks have not run yet."),
            PreflightState::Running(_) => {
                Line::styled("Checking this host...", Style::default().fg(Color::Cyan))
            }
            PreflightState::Failed(error) => {
                Line::styled(error.clone(), Style::default().fg(Color::Red))
            }
            PreflightState::Complete(_) => unreachable!(),
        }];
    };
    let mut lines = vec![
        Line::styled(
            format!(
                "{} pass / {} warn / {} error / {} unknown",
                report.counts.pass,
                report.counts.warning,
                report.counts.error,
                report.counts.unknown
            ),
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
        "[ Start installation ]".into(),
    ];
    let labels = [
        "Version",
        "Repository",
        "Repository URL",
        "Install root",
        "Admin user",
        "Password",
        "Confirm password",
        "Service manager",
        "Network takeover",
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
                Span::styled(format!("{label:<19}"), Style::default().fg(Color::DarkGray)),
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
        Paragraph::new(lines).block(Block::bordered().title("Install")),
        area,
    );
}

fn render_install_help(frame: &mut Frame<'_>, app: &ConsoleApp, area: Rect) {
    let (title, description) = if app.install.checks_selected {
        (
            "Environment checks",
            "Read-only deployment checks for platform, kernel, resources, dependencies, ports, services, and DNS.",
        )
    } else {
        app.install.selected_help()
    };
    frame.render_widget(
        Paragraph::new(description)
            .block(Block::bordered().title(format!("About: {title}")))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn mask(value: &str) -> String {
    "*".repeat(value.chars().count())
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::check::model::{CheckGroup, CheckResult, StatusCounts};

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
