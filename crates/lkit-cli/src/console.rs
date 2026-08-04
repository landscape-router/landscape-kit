use std::io::{IsTerminal, Stdout};
use std::path::PathBuf;

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
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{Frame, Terminal};

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
        terminal
            .terminal
            .draw(|frame| render(frame, &app))
            .map_err(|error| format!("draw console: {error}"))?;
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

struct ConsoleApp {
    menu_index: usize,
    focus: Focus,
    install: InstallForm,
    snapshot: Snapshot,
    notice: String,
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
        }
    }

    fn menu(&self) -> Menu {
        Menu::ALL[self.menu_index]
    }

    fn handle_key(&mut self, key: KeyEvent) -> Option<ConsoleAction> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Some(ConsoleAction::Quit);
        }
        if self.install.editing && self.menu() == Menu::Install && self.focus == Focus::Panel {
            return self.handle_editing_key(key);
        }
        match key.code {
            KeyCode::Esc => return Some(ConsoleAction::Quit),
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
                    self.install.selected = self.install.selected.saturating_sub(1);
                }
                Focus::Panel => {}
            },
            KeyCode::Down => match self.focus {
                Focus::Navigation => {
                    self.menu_index = (self.menu_index + 1).min(Menu::ALL.len() - 1);
                }
                Focus::Panel if self.menu() == Menu::Install => {
                    self.install.selected = (self.install.selected + 1).min(FORM_FIELDS - 1);
                }
                Focus::Panel => {}
            },
            KeyCode::Right if self.focus == Focus::Navigation => self.focus = Focus::Panel,
            KeyCode::Left if self.focus == Focus::Panel && self.menu() != Menu::Install => {
                self.focus = Focus::Navigation;
            }
            KeyCode::Left if self.focus == Focus::Panel => self.install.change_choice(false),
            KeyCode::Right if self.focus == Focus::Panel => self.install.change_choice(true),
            KeyCode::Enter | KeyCode::Char(' ') if self.focus == Focus::Panel => {
                if self.menu() == Menu::Install {
                    match self.install.activate() {
                        Ok(Some(action)) => return Some(action),
                        Ok(None) => self.notice = "Ready".into(),
                        Err(error) => self.notice = error,
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
        if self.install.editing && self.menu() == Menu::Install && self.focus == Focus::Panel {
            "Type Edit  Backspace Delete  Enter/Esc Finish"
        } else {
            match (self.focus, self.menu()) {
                (Focus::Navigation, _) => "Up/Down Menu  Right/Enter Open  Tab Switch  Esc Exit",
                (Focus::Panel, Menu::Install) => {
                    "Up/Down Field  Left/Right Change  Enter Edit/Select  Tab Menu  Esc Exit"
                }
                (Focus::Panel, _) => "Left Menu  Tab Switch  Esc Exit",
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
            editing: false,
        }
    }
}

impl InstallForm {
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
        .map(|(index, (label, value))| {
            let selected = app.focus == Focus::Panel && form.selected == index;
            let disabled = index == 2 && form.repository != RepositoryMode::Custom;
            let value_style = if disabled {
                Style::default().fg(Color::DarkGray)
            } else if index == 9 {
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
                line.style(Style::default().bg(Color::Rgb(30, 68, 78)))
            } else {
                line
            }
        })
        .collect();
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title("Install")),
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
        assert!(content.contains("Left/Right Change"));
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
