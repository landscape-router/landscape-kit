use std::io::{IsTerminal, Stdout};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    Clear as ClearScreen, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
    enable_raw_mode,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Gauge, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use unicode_width::UnicodeWidthStr;

use crate::backup::lkb::{BackupMetadata, BackupProgress};
use crate::check;
use crate::check::model::{CheckReport, Status};
use crate::commands::backup::{architecture_key, scope_key};
use crate::commands::install::Install;
use crate::commands::update::{ResolvedUpdate, resolve_update_target};
use crate::commands::{Commands, ServiceManagerArg};
use crate::deployment::config::{RepositorySource, RepositorySourceKind};
use crate::deployment::{lock, plan, root, state};
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
        return Err(crate::tr!(crate::keys::CONSOLE_TERMINAL_REQUIRED).into());
    }
    let mut terminal = ConsoleTerminal::start()?;
    let mut app = ConsoleApp::new();
    loop {
        app.update();
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
        if let Err(error) = execute!(
            stdout,
            EnterAlternateScreen,
            Hide,
            ClearScreen(ClearType::All),
            MoveTo(0, 0)
        ) {
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
        let _ = execute!(
            self.terminal.backend_mut(),
            Show,
            ClearScreen(ClearType::All),
            MoveTo(0, 0),
            LeaveAlternateScreen
        );
        let _ = self.terminal.show_cursor();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Menu {
    Overview,
    Install,
    Backup,
    Update,
}

impl Menu {
    const ALL: [Self; 4] = [Self::Overview, Self::Install, Self::Backup, Self::Update];

    fn label(self) -> String {
        match self {
            Self::Overview => crate::tr!(crate::keys::CONSOLE_OVERVIEW),
            Self::Install => crate::tr!(crate::keys::CONSOLE_INSTALL_MENU),
            Self::Backup => crate::tr!(crate::keys::CONSOLE_BACKUP_MENU),
            Self::Update => crate::tr!(crate::keys::CONSOLE_UPDATE_MENU),
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
                    crate::tr!(crate::keys::CONSOLE_CHECK_WORKER_STOPPED).into(),
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

/// 备份菜单数据：条目与 CLI `backup list` 同源，metadata 为 `None` 表示损坏。
struct BackupEntry {
    metadata: Option<BackupMetadata>,
    path: PathBuf,
}

enum BackupListState {
    NotRun,
    Running(Receiver<Result<Vec<BackupEntry>, String>>),
    Complete(Vec<BackupEntry>),
    Failed(String),
}

enum BackupVerifyState {
    Idle,
    Running(Receiver<Result<String, String>>),
}

enum BackupCreateMessage {
    Progress(BackupProgress),
    Done(Result<BackupMetadata, String>),
}

/// 在 TUI 内执行备份创建：worker 线程跑完整创建流程并通过 channel 回传进度。
struct BackupCreateRun {
    receiver: Receiver<BackupCreateMessage>,
    progress: BackupProgress,
}

/// 备份面板：列表 + 详情 + 创建备注/进度 + 删除/恢复确认。
struct BackupPanel {
    state: BackupListState,
    selected: usize,
    editing: bool,
    remark: String,
    details: Option<usize>,
    details_scroll: u16,
    verify: BackupVerifyState,
    create: Option<BackupCreateRun>,
    restore_confirming: bool,
    delete_confirming: bool,
    delete_target: Option<String>,
}

impl Default for BackupPanel {
    fn default() -> Self {
        Self {
            state: BackupListState::NotRun,
            selected: 0,
            editing: false,
            remark: String::new(),
            details: None,
            details_scroll: 0,
            verify: BackupVerifyState::Idle,
            create: None,
            restore_confirming: false,
            delete_confirming: false,
            delete_target: None,
        }
    }
}

impl BackupPanel {
    fn start(&mut self, install_dir: &str) {
        if matches!(self.state, BackupListState::Running(_)) {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let language = crate::i18n::current();
        let install_dir = install_dir.to_string();
        std::thread::spawn(move || {
            let result = crate::i18n::with_language(language, || load_backups(&install_dir));
            let _ = sender.send(result);
        });
        self.state = BackupListState::Running(receiver);
        self.selected = 0;
        self.editing = false;
        self.remark.clear();
        self.details = None;
        self.details_scroll = 0;
        self.verify = BackupVerifyState::Idle;
        self.create = None;
        self.restore_confirming = false;
        self.delete_confirming = false;
        self.delete_target = None;
    }

    /// 在后台线程执行完整创建流程（与 CLI 共用 `create_manual_backup`），
    /// 进度经 channel 回传；结束后由 `poll` 刷新列表并显示结果。
    fn start_create(&mut self, install_dir: &str, remark: &str) {
        if self.create.is_some() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let install_dir = install_dir.to_string();
        let remark = remark.to_string();
        std::thread::spawn(move || {
            let result = (|| {
                let requested = PathBuf::from(&install_dir);
                let selected = plan::select_install_root(
                    Some(&requested),
                    std::env::var("LKIT_INSTALL_DIR").ok().as_deref(),
                )
                .map_err(|error| error.to_string())?;
                let root =
                    root::normalize_install_root(&selected).map_err(|error| error.to_string())?;
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|error| format!("build backup create runtime: {error}"))?;
                runtime
                    .block_on(crate::commands::backup::create_manual_backup(
                        &root,
                        &crate::deployment::runtime::InstallRuntime::production(),
                        &remark,
                        None,
                        |progress| {
                            let _ = sender.send(BackupCreateMessage::Progress(progress));
                        },
                    ))
                    .map_err(|error| error.to_string())
            })();
            let _ = sender.send(BackupCreateMessage::Done(result));
        });
        self.create = Some(BackupCreateRun {
            receiver,
            progress: BackupProgress::Exporting,
        });
    }

    fn poll(&mut self, notice: &mut String) {
        match &self.state {
            BackupListState::Running(receiver) => match receiver.try_recv() {
                Ok(Ok(entries)) => {
                    self.state = BackupListState::Complete(entries);
                    self.details = None;
                    self.details_scroll = 0;
                }
                Ok(Err(error)) => self.state = BackupListState::Failed(error),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.state = BackupListState::Failed(
                        crate::tr!(crate::keys::CONSOLE_CHECK_WORKER_STOPPED).into(),
                    );
                }
            },
            _ => {}
        }
        if let BackupVerifyState::Running(receiver) = &self.verify {
            match receiver.try_recv() {
                Ok(Ok(message)) => {
                    self.verify = BackupVerifyState::Idle;
                    *notice = message;
                }
                Ok(Err(error)) => {
                    self.verify = BackupVerifyState::Idle;
                    *notice = error;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.verify = BackupVerifyState::Idle;
                    *notice = crate::tr!(crate::keys::CONSOLE_BACKUP_VERIFY_WORKER_STOPPED).into();
                }
            }
        }
        loop {
            let message = match &self.create {
                Some(run) => run.receiver.try_recv(),
                None => break,
            };
            match message {
                Ok(BackupCreateMessage::Progress(progress)) => {
                    if let Some(run) = &mut self.create {
                        run.progress = progress;
                    }
                }
                Ok(BackupCreateMessage::Done(result)) => {
                    self.create = None;
                    match result {
                        Ok(metadata) => {
                            self.state = BackupListState::NotRun;
                            *notice = crate::tr!(
                                crate::keys::CONSOLE_BACKUP_CREATED,
                                backup_id = metadata.backup_id
                            )
                            .into();
                        }
                        Err(error) => *notice = error,
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.create = None;
                    *notice = crate::tr!(crate::keys::CONSOLE_BACKUP_CREATE_WORKER_STOPPED).into();
                }
            }
        }
    }

    fn rows(&self) -> &[BackupEntry] {
        match &self.state {
            BackupListState::Complete(rows) => rows,
            _ => &[],
        }
    }

    /// 当前选中的备份行；第 0 行是“创建备份”动作。
    fn selected_entry(&self) -> Option<&BackupEntry> {
        if self.selected == 0 {
            None
        } else {
            self.rows().get(self.selected - 1)
        }
    }

    fn details_entry(&self) -> Option<&BackupEntry> {
        self.details.and_then(|index| self.rows().get(index))
    }
}

/// 与 CLI `backup list` 相同的解析与完整校验。
fn load_backups(install_dir: &str) -> Result<Vec<BackupEntry>, String> {
    let requested = PathBuf::from(install_dir);
    let selected = plan::select_install_root(
        Some(&requested),
        std::env::var("LKIT_INSTALL_DIR").ok().as_deref(),
    )
    .map_err(|error| error.to_string())?;
    let normalized = root::normalize_install_root(&selected).map_err(|error| error.to_string())?;
    let rows = crate::commands::backup::list_backups_with(
        &normalized,
        crate::commands::backup::BackupListCheck::Metadata,
    )
    .map_err(|error| error.to_string())?;
    Ok(rows
        .into_iter()
        .map(|(metadata, path)| BackupEntry { metadata, path })
        .collect())
}

/// 与 CLI `backup delete` 相同的根目录解析、安装锁与文件删除。
fn delete_backup_via_console(install_dir: &str, backup_id: &str) -> Result<(), String> {
    let requested = PathBuf::from(install_dir);
    let selected = plan::select_install_root(
        Some(&requested),
        std::env::var("LKIT_INSTALL_DIR").ok().as_deref(),
    )
    .map_err(|error| error.to_string())?;
    let root = root::normalize_install_root(&selected).map_err(|error| error.to_string())?;
    let _lock = lock::acquire_install_lock(&root).map_err(|error| error.to_string())?;
    crate::commands::backup::delete_backup(&root, backup_id).map_err(|error| error.to_string())
}

/// Update 面板后台解析：与命令模式 `lkit update` 相同的根目录解析、状态读取、
/// 来源解析与目标版本解析/比较（复用 `resolve_update_target`），网络只读，零副作用。
fn resolve_update_from_console(
    install_dir: &str,
    repository: &plan::RepositoryChoice,
    version: &str,
) -> Result<ResolvedUpdate, String> {
    let requested = PathBuf::from(install_dir);
    let selected = plan::select_install_root(
        Some(&requested),
        std::env::var("LKIT_INSTALL_DIR").ok().as_deref(),
    )
    .map_err(|error| error.to_string())?;
    let root = root::normalize_install_root(&selected).map_err(|error| error.to_string())?;
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
    backup: BackupPanel,
    backup_menu_active: bool,
    update: UpdatePanel,
    update_menu_active: bool,
    takeover_choice: usize,
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
            backup: BackupPanel::default(),
            backup_menu_active: false,
            update: UpdatePanel::default(),
            update_menu_active: false,
            takeover_choice: 0,
        }
    }

    fn menu(&self) -> Menu {
        Menu::ALL[self.menu_index]
    }

    /// 已安装或存在等待确认的网络接管时，首次安装表单不可用，Install 菜单不可选中。
    fn install_available(&self) -> bool {
        !matches!(
            self.snapshot,
            Snapshot::Installed { .. } | Snapshot::AwaitingNetworkConfirmation { .. }
        )
    }

    /// 存在等待确认的网络接管时进入阻塞屏，不渲染菜单。
    fn takeover_pending(&self) -> bool {
        matches!(self.snapshot, Snapshot::AwaitingNetworkConfirmation { .. })
    }

    /// 回滚进行中（rolling_back）时确认不可用，只提供"稍后"。
    fn takeover_confirm_allowed(&self) -> bool {
        matches!(
            self.snapshot,
            Snapshot::AwaitingNetworkConfirmation { phase, .. } if phase != "rolling_back"
        )
    }

    /// 确认执行：退出 TUI 后按现状 CLI 语义内联运行 `lkit network confirm`。
    fn takeover_confirm_action(&self) -> ConsoleAction {
        ConsoleAction::Command {
            command: Commands::Network(crate::commands::network::Network {
                action: crate::commands::network::NetworkAction::Confirm,
                install_dir: Some(PathBuf::from(&self.install.install_dir)),
                #[cfg(feature = "test-support")]
                test_runtime: None,
            }),
            args: vec![
                "network".into(),
                "confirm".into(),
                "--install-dir".into(),
                self.install.install_dir.clone(),
            ],
        }
    }

    fn menu_available(&self, menu: Menu) -> bool {
        match menu {
            Menu::Install => self.install_available(),
            Menu::Update => matches!(self.snapshot, Snapshot::Installed { .. }),
            _ => true,
        }
    }

    fn select_next_menu(&mut self) {
        for index in (self.menu_index + 1)..Menu::ALL.len() {
            if self.menu_available(Menu::ALL[index]) {
                self.menu_index = index;
                return;
            }
        }
    }

    fn select_previous_menu(&mut self) {
        for index in (0..self.menu_index).rev() {
            if self.menu_available(Menu::ALL[index]) {
                self.menu_index = index;
                return;
            }
        }
    }

    fn update(&mut self) {
        if self.takeover_pending() {
            return;
        }
        if self.menu() == Menu::Install
            && self.install_available()
            && matches!(&self.preflight.state, PreflightState::NotRun)
        {
            self.preflight.start();
        }
        self.preflight.poll();
        if self.menu() == Menu::Backup {
            if !self.backup_menu_active {
                self.backup_menu_active = true;
                if !matches!(self.backup.state, BackupListState::NotRun) {
                    self.backup.state = BackupListState::NotRun;
                }
            }
            if matches!(&self.backup.state, BackupListState::NotRun) {
                self.backup.start(&self.install.install_dir);
            }
        } else {
            self.backup_menu_active = false;
        }
        self.backup.poll(&mut self.notice);
        if self.menu() == Menu::Update {
            if !self.update_menu_active {
                self.update_menu_active = true;
                self.update.load_config(&self.install.install_dir);
            }
        } else {
            self.update_menu_active = false;
        }
        self.update.poll(&mut self.notice);
    }

    /// 阻塞屏键处理：↑/↓ 或 Tab 选择，Enter 执行，Esc/Ctrl+C 等同"稍后"退出。
    fn handle_takeover_pending_key(&mut self, key: KeyEvent) -> Option<ConsoleAction> {
        if !self.install.editing && language_toggle_key(&key) {
            self.toggle_language();
            return None;
        }
        match key.code {
            KeyCode::Up => self.takeover_choice = 0,
            KeyCode::Down => {
                if self.takeover_confirm_allowed() {
                    self.takeover_choice = 1;
                }
            }
            KeyCode::Tab | KeyCode::BackTab => {
                if self.takeover_confirm_allowed() {
                    self.takeover_choice = 1 - self.takeover_choice;
                }
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                if self.takeover_choice == 1 && self.takeover_confirm_allowed() {
                    return Some(self.takeover_confirm_action());
                }
                return Some(ConsoleAction::Quit);
            }
            KeyCode::Esc => return Some(ConsoleAction::Quit),
            _ => {}
        }
        None
    }

    fn handle_key(&mut self, key: KeyEvent) -> Option<ConsoleAction> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Some(ConsoleAction::Quit);
        }
        if self.takeover_pending() {
            return self.handle_takeover_pending_key(key);
        }
        if self.network_wizard.is_some() {
            return self.handle_network_wizard_key(key);
        }
        if self.backup.create.is_some() {
            return None;
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
        if self.menu() == Menu::Backup && self.focus == Focus::Panel {
            match self.handle_backup_key(key) {
                Some(action) => return action,
                None => {}
            }
        }
        if self.menu() == Menu::Update
            && self.focus == Focus::Panel
            && let Some(action) = self.handle_update_key(key)
        {
            return action;
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
                Focus::Navigation => self.select_previous_menu(),
                Focus::Panel if self.menu() == Menu::Install => {
                    self.install.select_previous();
                }
                Focus::Panel => {}
            },
            KeyCode::Down => match self.focus {
                Focus::Navigation => self.select_next_menu(),
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
            KeyCode::Right
                if self.focus == Focus::Navigation && self.menu_available(self.menu()) =>
            {
                self.focus = Focus::Panel;
            }
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
                                                    crate::keys::CONSOLE_CONFIGURE_NETWORK_TAKEOVER
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
                                    crate::keys::CONSOLE_ENVIRONMENT_CHECKS_NOT_COMPLETED
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
            KeyCode::Enter
                if self.focus == Focus::Navigation && self.menu_available(self.menu()) =>
            {
                self.focus = Focus::Panel;
            }
            _ => {}
        }
        None
    }

    fn toggle_language(&mut self) {
        crate::i18n::configure(crate::i18n::current().toggled());
        self.exit_state = ExitState::Idle;
        self.notice = "Ready".into();
        self.snapshot = Snapshot::load(&self.install.install_dir);
        if !self.menu_available(self.menu()) {
            self.menu_index = 0;
            self.focus = Focus::Navigation;
        }
        if self.install_available() && !matches!(&self.preflight.state, PreflightState::NotRun) {
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

    fn hints(&self) -> String {
        if self.exit_state == ExitState::Confirming {
            crate::tr!(crate::keys::CONSOLE_HINT_CTRL_C_EXIT_ENTER_CONFIRM_ESC_CANCEL)
        } else if self.exit_state == ExitState::Armed {
            crate::tr!(crate::keys::CONSOLE_HINT_CTRL_C_EXIT_ESC_AGAIN)
        } else if self.backup.create.is_some() {
            crate::tr!(crate::keys::CONSOLE_BACKUP_HINT_CREATE_RUNNING)
        } else if self.preflight.expanded && self.menu() == Menu::Install {
            crate::tr!(crate::keys::CONSOLE_HINT_CTRL_C_EXIT_SCROLL)
        } else if self.preflight_dialog {
            crate::tr!(crate::keys::CONSOLE_HINT_ENTER_DETAILS_ESC_CLOSE_R)
        } else if self.install.editing && self.menu() == Menu::Install && self.focus == Focus::Panel
        {
            crate::tr!(crate::keys::CONSOLE_HINT_CTRL_C_EXIT_EDIT)
        } else if self.menu() == Menu::Update && self.focus == Focus::Panel {
            if self.update.confirming.is_some() {
                crate::tr!(crate::keys::CONSOLE_UPDATE_HINT_CONFIRM)
            } else if self.update.resolving.is_some() {
                crate::tr!(crate::keys::CONSOLE_UPDATE_HINT_RESOLVING)
            } else if self.update.editing {
                crate::tr!(crate::keys::CONSOLE_HINT_CTRL_C_EXIT_EDIT)
            } else {
                crate::tr!(crate::keys::CONSOLE_UPDATE_HINT_PANEL)
            }
        } else if self.menu() == Menu::Backup && self.focus == Focus::Panel {
            if self.backup.delete_confirming {
                crate::tr!(crate::keys::CONSOLE_BACKUP_HINT_DELETE_CONFIRM)
            } else if self.backup.restore_confirming {
                crate::tr!(crate::keys::CONSOLE_BACKUP_HINT_RESTORE_CONFIRM)
            } else if self.backup.editing {
                crate::tr!(crate::keys::CONSOLE_BACKUP_HINT_CREATE)
            } else if self.backup.details.is_some() {
                crate::tr!(crate::keys::CONSOLE_BACKUP_HINT_DETAILS)
            } else {
                crate::tr!(crate::keys::CONSOLE_BACKUP_HINT_LIST)
            }
        } else {
            match (self.focus, self.menu()) {
                (Focus::Navigation, _) => crate::tr!(crate::keys::CONSOLE_HINT_NAVIGATION),
                (Focus::Panel, Menu::Install) if self.install.checks_selected => {
                    crate::tr!(crate::keys::CONSOLE_HINT_CHECKS_SELECTED)
                }
                (Focus::Panel, Menu::Install) => {
                    crate::tr!(crate::keys::CONSOLE_HINT_INSTALL_PANEL)
                }
                (Focus::Panel, _) => crate::tr!(crate::keys::CONSOLE_HINT_PANEL),
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

    /// 返回 `None` 表示按键未消费（回落到主处理流程）；`Some(None)` 表示已消费；
    /// `Some(Some(action))` 表示触发备份创建或恢复。
    fn handle_backup_key(&mut self, key: KeyEvent) -> Option<Option<ConsoleAction>> {
        if self.backup.restore_confirming {
            match key.code {
                KeyCode::Enter => {
                    let entry = match self.backup.selected_entry() {
                        Some(entry) => entry,
                        None => {
                            self.backup.restore_confirming = false;
                            return Some(None);
                        }
                    };
                    let metadata = match &entry.metadata {
                        Some(metadata) => metadata,
                        None => {
                            self.backup.restore_confirming = false;
                            return Some(None);
                        }
                    };
                    let backup_id = metadata.backup_id.clone();
                    self.backup.restore_confirming = false;
                    return Some(Some(self.backup_restore_action(&backup_id)));
                }
                KeyCode::Esc => self.backup.restore_confirming = false,
                _ => {}
            }
            return Some(None);
        }
        if self.backup.delete_confirming {
            match key.code {
                KeyCode::Enter => {
                    let backup_id = self.backup.delete_target.take();
                    self.backup.delete_confirming = false;
                    if let Some(backup_id) = backup_id {
                        self.delete_backup(backup_id);
                    }
                }
                KeyCode::Esc => {
                    self.backup.delete_confirming = false;
                    self.backup.delete_target = None;
                }
                _ => {}
            }
            return Some(None);
        }
        if self.backup.editing {
            match key.code {
                KeyCode::Enter => {
                    let remark = self.backup.remark.clone();
                    match crate::backup::lkb::validate_remark(&remark) {
                        Ok(()) => {
                            self.backup.editing = false;
                            self.backup.remark.clear();
                            self.backup.start_create(&self.install.install_dir, &remark);
                        }
                        Err(error) => self.notice = error.to_string(),
                    }
                }
                KeyCode::Esc => {
                    self.backup.editing = false;
                    self.backup.remark.clear();
                }
                KeyCode::Backspace => {
                    self.backup.remark.pop();
                }
                KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    if self.backup.remark.chars().count() < 256 {
                        self.backup.remark.push(character);
                    }
                }
                _ => {}
            }
            return Some(None);
        }
        if self.backup.details.is_some() {
            match key.code {
                KeyCode::Esc => {
                    self.backup.details = None;
                    self.backup.details_scroll = 0;
                    self.backup.verify = BackupVerifyState::Idle;
                }
                KeyCode::Up => {
                    self.backup.details_scroll = self.backup.details_scroll.saturating_sub(1)
                }
                KeyCode::Down => {
                    self.backup.details_scroll = self.backup.details_scroll.saturating_add(1)
                }
                KeyCode::Char('v' | 'V') => self.start_backup_verify(),
                KeyCode::Char('r' | 'R') => {
                    if let Some(entry) = self.backup.details_entry()
                        && entry.metadata.is_some()
                    {
                        self.backup.restore_confirming = true;
                    }
                }
                KeyCode::Char('d' | 'D') => {
                    if let Some(entry) = self.backup.details_entry()
                        && let Some(metadata) = &entry.metadata
                    {
                        self.backup.delete_target = Some(metadata.backup_id.clone());
                        self.backup.delete_confirming = true;
                    }
                }
                _ => {}
            }
            return Some(None);
        }
        match key.code {
            KeyCode::Up => {
                self.backup.selected = self.backup.selected.saturating_sub(1);
            }
            KeyCode::Down => {
                let rows = self.backup.rows().len();
                self.backup.selected = (self.backup.selected + 1).min(rows);
            }
            KeyCode::Enter => {
                if self.backup.selected == 0 {
                    self.backup.editing = true;
                    self.backup.remark.clear();
                } else if let Some(entry) = self.backup.selected_entry() {
                    if entry.metadata.is_some() {
                        self.backup.details = Some(self.backup.selected - 1);
                        self.backup.details_scroll = 0;
                        self.backup.verify = BackupVerifyState::Idle;
                    } else {
                        self.notice = crate::tr!(
                            crate::keys::CONSOLE_BACKUP_INVALID,
                            id = entry
                                .path
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .trim_end_matches(".lkb")
                        )
                        .into();
                    }
                }
            }
            KeyCode::Char('r' | 'R') => {
                if let Some(entry) = self.backup.selected_entry()
                    && entry.metadata.is_some()
                {
                    self.backup.restore_confirming = true;
                } else {
                    self.notice = crate::tr!(crate::keys::CONSOLE_BACKUP_SELECT_TO_RESTORE).into();
                }
            }
            KeyCode::Char('d' | 'D') => {
                if let Some(entry) = self.backup.selected_entry()
                    && let Some(metadata) = &entry.metadata
                {
                    self.backup.delete_target = Some(metadata.backup_id.clone());
                    self.backup.delete_confirming = true;
                } else {
                    self.notice = crate::tr!(crate::keys::CONSOLE_BACKUP_SELECT_TO_DELETE);
                }
            }
            _ => return None,
        }
        Some(None)
    }

    /// Update 面板按键：确认层、解析中、编辑与表单导航。返回 `None` 表示按键
    /// 未消费（如 Left 返回侧栏、Esc 进入退出确认），回落到主处理流程。
    fn handle_update_key(&mut self, key: KeyEvent) -> Option<Option<ConsoleAction>> {
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
                self.update.selected = match self.update.selected {
                    0 => 0,
                    1 => 0,
                    2 => 1,
                    _ if self.update.repository == UpdateRepositoryMode::Custom => 2,
                    _ => 1,
                };
            }
            KeyCode::Down => {
                self.update.selected = match self.update.selected {
                    0 => 1,
                    1 if self.update.repository == UpdateRepositoryMode::Custom => 2,
                    _ => 3,
                };
            }
            KeyCode::Right if self.update.selected == 1 => self.update.change(true),
            KeyCode::Enter | KeyCode::Char(' ') => match self.update.selected {
                0 | 2 => self.update.editing = true,
                1 => self.update.change(true),
                3 => {
                    if let Err(error) = self.start_update_resolution() {
                        self.notice = error;
                    }
                }
                _ => {}
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
        let install_dir = self.install.install_dir.clone();
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
                resolve_update_from_console(&install_dir, &repository, &version)
            });
            let _ = sender.send(result);
        });
        self.update.resolving = Some(receiver);
        Ok(())
    }

    /// 确认层 Enter：构建带 `--console-confirmed` 的结构化 `Update` 请求。
    /// Current 来源不传 `--repository`，由命令按 `config.toml` > 官方 GitHub 解析。
    fn update_action(&self) -> ConsoleAction {
        let install_dir = PathBuf::from(&self.install.install_dir);
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
            install_dir: Some(install_dir.clone()),
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
            "--install-dir".into(),
            install_dir.display().to_string(),
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

    /// 同步删除备份（与 CLI 相同的根目录解析、安装锁与文件校验）并刷新列表。
    fn delete_backup(&mut self, backup_id: String) {
        let result = delete_backup_via_console(&self.install.install_dir, &backup_id);
        match result {
            Ok(()) => {
                self.backup.details = None;
                self.backup.details_scroll = 0;
                self.backup.state = BackupListState::NotRun;
                self.notice =
                    crate::tr!(crate::keys::CONSOLE_BACKUP_DELETED, backup_id = backup_id);
            }
            Err(error) => self.notice = format!("backup: {error}"),
        }
    }

    fn start_backup_verify(&mut self) {
        let Some(entry) = self.backup.details_entry() else {
            return;
        };
        if matches!(self.backup.verify, BackupVerifyState::Running(_)) {
            return;
        }
        let path = entry.path.clone();
        let (sender, receiver) = mpsc::channel();
        let language = crate::i18n::current();
        std::thread::spawn(move || {
            let result = crate::i18n::with_language(language, || {
                let bytes = std::fs::read(&path).map_err(|error| error.to_string())?;
                let metadata =
                    crate::backup::lkb::verify_lkb(&bytes).map_err(|error| error.to_string())?;
                let verify_dir = std::env::temp_dir()
                    .join(format!("lkit-backup-tui-verify-{}", uuid::Uuid::now_v7()));
                crate::backup::lkb::create_secure_dir(&verify_dir, 0o700)
                    .and_then(|()| crate::backup::lkb::extract_lkb(&bytes, &verify_dir))
                    .map(|_| {
                        crate::tr!(
                            crate::keys::CONSOLE_BACKUP_VERIFIED,
                            backup_id = metadata.backup_id
                        )
                    })
                    .map_err(|error| {
                        let _ = std::fs::remove_dir_all(&verify_dir);
                        error.to_string()
                    })
                    .map(|message| {
                        let _ = std::fs::remove_dir_all(&verify_dir);
                        message
                    })
            });
            let _ = sender.send(result);
        });
        self.backup.verify = BackupVerifyState::Running(receiver);
        self.notice = crate::tr!(crate::keys::CONSOLE_BACKUP_VERIFY_RUNNING).into();
    }

    fn backup_restore_action(&self, backup_id: &str) -> ConsoleAction {
        let install_dir = PathBuf::from(&self.install.install_dir);
        let command = Commands::Restore(crate::commands::restore::Restore {
            backup: Some(backup_id.to_string()),
            file: None,
            allow_no_backup: false,
            yes: true,
            console_confirmed: true,
            install_dir: Some(install_dir.clone()),
            #[cfg(feature = "test-support")]
            test_runtime: None,
        });
        let args = vec![
            "restore".into(),
            "--backup".into(),
            backup_id.to_string(),
            "--yes".into(),
            "--console-confirmed".into(),
            "--install-dir".into(),
            install_dir.display().to_string(),
        ];
        ConsoleAction::Command { command, args }
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
enum ManagerMode {
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
            repository: RepositoryMode::Default,
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

    fn selected_help(&self) -> (String, String) {
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
            8 => (
                crate::tr!(crate::keys::CONSOLE_NETWORK_TAKEOVER_LABEL),
                crate::tr!(crate::keys::CONSOLE_NETWORK_TAKEOVER_HELP),
            ),
            9 => (
                crate::tr!(crate::keys::CONSOLE_START_INSTALLATION_LABEL),
                crate::tr!(crate::keys::CONSOLE_START_INSTALLATION_HELP),
            ),
            _ => (
                crate::tr!(crate::keys::CONSOLE_INSTALL_MENU),
                crate::tr!(crate::keys::CONSOLE_INSTALL_HELP_FALLBACK_DESC),
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
            return Err(crate::tr!(crate::keys::CONSOLE_PASSWORD_CONFIRMATION_MISMATCH).into());
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

/// Update 面板的仓库来源选择,选项顺序与命令模式 `lkit update` 的渠道列表一致。
/// Current 只在 `config.toml` 存在且有效时提供。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum UpdateRepositoryMode {
    Current,
    Github,
    Mirror,
    Custom,
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

/// Update 面板：当前版本 + 目标版本/仓库来源表单、后台目标解析与确认层。
/// 解析与比较规则与命令模式 `lkit update` 一致（共享 `resolve_update_target`），
/// 已是最新与降级在面板内提示,只有升级才打开确认层。
struct UpdatePanel {
    version: String,
    repository: UpdateRepositoryMode,
    repository_url: String,
    selected: usize,
    editing: bool,
    current_source: Option<RepositorySource>,
    config_error: Option<String>,
    resolving: Option<Receiver<Result<ResolvedUpdate, String>>>,
    confirming: Option<ResolvedUpdate>,
}

impl Default for UpdatePanel {
    fn default() -> Self {
        Self {
            version: "latest".into(),
            repository: UpdateRepositoryMode::Github,
            repository_url: plan::DEFAULT_HTTP_MIRROR.into(),
            selected: 0,
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
    fn load_config(&mut self, install_dir: &str) {
        let requested = PathBuf::from(install_dir);
        let loaded = (|| -> Result<Option<RepositorySource>, String> {
            let selected = plan::select_install_root(
                Some(&requested),
                std::env::var("LKIT_INSTALL_DIR").ok().as_deref(),
            )
            .map_err(|error| error.to_string())?;
            let root =
                root::normalize_install_root(&selected).map_err(|error| error.to_string())?;
            crate::deployment::config::load_repository(&root).map_err(|error| error.to_string())
        })();
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

    fn repository_options(&self) -> Vec<UpdateRepositoryMode> {
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

    fn change(&mut self, forward: bool) {
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

    fn editable_value_mut(&mut self) -> Option<&mut String> {
        match self.selected {
            0 => Some(&mut self.version),
            2 if self.repository == UpdateRepositoryMode::Custom => Some(&mut self.repository_url),
            _ => None,
        }
    }

    /// 消费后台解析结果,按与命令模式相同的规则分支。
    fn apply_resolution(&mut self, notice: &mut String, resolved: ResolvedUpdate) {
        match resolved.current.cmp(&resolved.target) {
            std::cmp::Ordering::Equal => {
                *notice = crate::tr!(
                    crate::keys::UPDATE_ALREADY_UP_TO_DATE,
                    version = resolved.current
                );
            }
            std::cmp::Ordering::Greater => {
                *notice = crate::tr!(
                    crate::keys::SWITCH_DOWNGRADE_NOT_SUPPORTED,
                    from_version = resolved.current,
                    version = resolved.target
                );
            }
            std::cmp::Ordering::Less => self.confirming = Some(resolved),
        }
    }

    fn poll(&mut self, notice: &mut String) {
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
                *notice = error;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.resolving = None;
                *notice = crate::tr!(crate::keys::CONSOLE_UPDATE_RESOLVE_WORKER_STOPPED);
            }
        }
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
                    .map_err(|_| crate::tr!(crate::keys::CONSOLE_INVALID_WAN_GATEWAY))?;
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
                    .map_err(|_| crate::tr!(crate::keys::CONSOLE_INVALID_LAN_DHCP_RANGE_START))?;
                self.step = WizardStep::DhcpEnd;
                self.editing = true;
            }
            WizardStep::DhcpEnd => {
                self.dhcp_end
                    .trim()
                    .parse::<std::net::Ipv4Addr>()
                    .map_err(|_| crate::tr!(crate::keys::CONSOLE_INVALID_LAN_DHCP_RANGE_END))?;
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
        let mode =
            if lan.is_empty() {
                match self.wan_mode {
                    WanMode::Dhcp => NetworkMode::WanDhcp {
                        wan: wan.name.clone(),
                    },
                    WanMode::Static => {
                        NetworkMode::WanOnly {
                            wan: wan.name.clone(),
                            address: self.address.trim().parse().map_err(
                                |error: crate::deployment::plan::InstallError| error.to_string(),
                            )?,
                            gateway: self.gateway.trim().parse().map_err(|_| {
                                crate::tr!(crate::keys::CONSOLE_INVALID_WAN_GATEWAY)
                            })?,
                        }
                    }
                }
            } else {
                let management =
                    self.management.trim().parse().map_err(
                        |error: crate::deployment::plan::InstallError| error.to_string(),
                    )?;
                let dhcp_start =
                    self.dhcp_start.trim().parse().map_err(|_| {
                        crate::tr!(crate::keys::CONSOLE_INVALID_LAN_DHCP_RANGE_START)
                    })?;
                let dhcp_end = self
                    .dhcp_end
                    .trim()
                    .parse()
                    .map_err(|_| crate::tr!(crate::keys::CONSOLE_INVALID_LAN_DHCP_RANGE_END))?;
                NetworkMode::RoutedLan {
                    wan: wan.name.clone(),
                    wan_ipv4: Some(match self.wan_mode {
                        WanMode::Static => WanIpv4Config::Static {
                            address: self.address.trim().parse().map_err(
                                |error: crate::deployment::plan::InstallError| error.to_string(),
                            )?,
                            gateway: self.gateway.trim().parse().map_err(|_| {
                                crate::tr!(crate::keys::CONSOLE_INVALID_WAN_GATEWAY)
                            })?,
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
    AwaitingNetworkConfirmation {
        transaction_id: String,
        phase: &'static str,
        deadline: String,
        management_address: Option<String>,
    },
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
        let result = root::normalize_install_root(&path).and_then(|root| {
            if let Some(transaction) = crate::deployment::transaction::find_unfinished(&root)?
                && let Some(network) = transaction.network_takeover.as_ref()
                && matches!(
                    transaction.phase,
                    crate::deployment::transaction::Phase::AwaitingNetworkConfirmation
                        | crate::deployment::transaction::Phase::Finalizing
                        | crate::deployment::transaction::Phase::RollingBack
                )
            {
                return Ok(Self::AwaitingNetworkConfirmation {
                    transaction_id: transaction.transaction_id.clone(),
                    phase: transaction.phase.key(),
                    deadline: network.confirmation_deadline.to_rfc3339(),
                    management_address: network
                        .plan
                        .management_address()
                        .map(|address| address.to_string()),
                });
            }
            state::load_state(&root).map(|installed| match installed {
                None => Self::NotInstalled,
                Some(installed) => Self::Installed {
                    version: installed.active_version,
                    manager: match installed.service.manager {
                        state::StateServiceManager::Systemd => "systemd",
                        state::StateServiceManager::None => "none",
                    },
                    initialized: installed.initialization.status == state::InitStatus::Complete,
                },
            })
        });
        match result {
            Ok(snapshot) => snapshot,
            Err(error) => Self::Unavailable(error.to_string()),
        }
    }

    fn badge(&self) -> (String, Color) {
        match self {
            Self::RootRequired => (
                crate::tr!(crate::keys::CONSOLE_ROOT_REQUIRED_BADGE),
                Color::Yellow,
            ),
            Self::AwaitingNetworkConfirmation { .. } => (
                crate::tr!(crate::keys::CONSOLE_TAKEOVER_PENDING_BADGE),
                Color::Yellow,
            ),
            Self::NotInstalled => (
                crate::tr!(crate::keys::CONSOLE_NOT_INSTALLED_BADGE),
                Color::Yellow,
            ),
            Self::Installed { .. } => (
                crate::tr!(crate::keys::CONSOLE_INSTALLED_BADGE),
                Color::Green,
            ),
            Self::Unavailable(_) => (
                crate::tr!(crate::keys::CONSOLE_ATTENTION_REQUIRED_BADGE),
                Color::Red,
            ),
        }
    }
}

fn render(frame: &mut Frame<'_>, app: &ConsoleApp) {
    if frame.area().width < 72 || frame.area().height < 18 {
        frame.render_widget(
            Paragraph::new(crate::tr!(crate::keys::CONSOLE_TERMINAL_TOO_SMALL))
                .alignment(Alignment::Center)
                .block(Block::bordered().title("Landscape Kit")),
            frame.area(),
        );
        if app.exit_state == ExitState::Confirming {
            render_exit_confirmation(frame);
        }
        return;
    }
    if app.takeover_pending() {
        render_pending_takeover(frame, app);
        return;
    }
    if let Some(wizard) = &app.network_wizard {
        render_network_wizard(frame, wizard);
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
    if app.menu() == Menu::Backup && app.backup.restore_confirming {
        render_backup_restore_confirmation(frame, app);
    }
    if app.menu() == Menu::Backup && app.backup.delete_confirming {
        render_backup_delete_confirmation(frame, app);
    }
    if app.menu() == Menu::Backup && app.backup.editing {
        render_backup_create_dialog(frame, app);
    }
    if app.menu() == Menu::Backup && app.backup.create.is_some() {
        render_backup_create_progress(frame, app);
    }
    if app.menu() == Menu::Update && app.update.confirming.is_some() {
        render_update_confirmation(frame, app);
    }
}

fn render_preflight_dialog(frame: &mut Frame<'_>, app: &ConsoleApp) {
    let lines: Vec<Line<'_>> = match &app.preflight.state {
        PreflightState::Failed(error) => vec![
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_ENVIRONMENT_CHECKS_COULD_NOT_COMPLETE),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Line::raw(""),
            Line::raw(error.clone()),
        ],
        PreflightState::Complete(report) => {
            let mut lines = vec![
                Line::styled(
                    crate::tr!(crate::keys::CONSOLE_ENVIRONMENT_CHECKS_BLOCK_INSTALLATION),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Line::raw(""),
            ];
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
            lines.push(Line::styled(
                crate::tr!(crate::keys::CONSOLE_DIALOG_ENTER_DETAILS_ESC_CLOSE_R),
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

fn render_network_wizard(frame: &mut Frame<'_>, wizard: &NetworkWizard) {
    let area = frame.area();
    let [title, body, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(8),
        Constraint::Length(2),
    ])
    .areas(area);
    frame.render_widget(
        Paragraph::new(crate::tr!(crate::keys::CONSOLE_LANDSCAPE_NETWORK_TAKEOVER))
            .style(Style::default().add_modifier(Modifier::BOLD))
            .block(Block::default().borders(Borders::BOTTOM)),
        title,
    );
    let mut lines = Vec::new();
    match wizard.step {
        WizardStep::Wan => {
            lines.push(Line::styled(
                crate::tr!(crate::keys::CONSOLE_SELECT_WAN_INTERFACE),
                Style::default().add_modifier(Modifier::BOLD),
            ));
            lines.push(Line::raw(""));
            for (index, iface) in wizard.interfaces.iter().enumerate() {
                let selected = index == wizard.wan;
                let address = iface
                    .addresses
                    .first()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| crate::tr!(crate::keys::CONSOLE_NO_IPV4).into());
                let gateway = wizard
                    .routes
                    .iter()
                    .find(|route| route.iface == iface.name)
                    .map(|route| route.gateway.to_string())
                    .unwrap_or_else(|| crate::tr!(crate::keys::CONSOLE_GATEWAY_NOT_FOUND).into());
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
                crate::tr!(crate::keys::CONSOLE_WAN_IPV4_MODE),
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
                crate::tr!(crate::keys::CONSOLE_WAN_STATIC_IPV4_CONFIGURATION),
                Style::default().add_modifier(Modifier::BOLD),
            ));
            lines.push(Line::raw(""));
            let fields = [
                (
                    crate::tr!(crate::keys::CONSOLE_IPV4_ADDRESS_CIDR),
                    &wizard.address,
                ),
                (
                    crate::tr!(crate::keys::CONSOLE_DEFAULT_GATEWAY),
                    &wizard.gateway,
                ),
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
                crate::tr!(crate::keys::CONSOLE_SELECT_LAN_INTERFACES),
                Style::default().add_modifier(Modifier::BOLD),
            ));
            lines.push(Line::raw(""));
            if wizard.lan_candidates.is_empty() {
                lines.push(Line::raw(crate::tr!(
                    crate::keys::CONSOLE_NO_OTHER_INTERFACES
                )));
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
                            crate::tr!(crate::keys::CONSOLE_LINK_UP)
                        } else {
                            crate::tr!(crate::keys::CONSOLE_LINK_DOWN)
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
                    crate::tr!(crate::keys::CONSOLE_LAN_MANAGEMENT_IPV4_ADDRESS),
                    &wizard.management,
                ),
                WizardStep::DhcpStart => (
                    crate::tr!(crate::keys::CONSOLE_LAN_DHCP_RANGE_START),
                    &wizard.dhcp_start,
                ),
                WizardStep::DhcpEnd => (
                    crate::tr!(crate::keys::CONSOLE_LAN_DHCP_RANGE_END),
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
                crate::tr!(crate::keys::CONSOLE_VALUE_PREFIX),
                value
            )));
        }
        WizardStep::Confirm => {
            let wan = wizard.selected_wan();
            lines.push(Line::styled(
                crate::tr!(crate::keys::CONSOLE_CONFIRM_NETWORK_TAKEOVER_PLAN),
                Style::default().add_modifier(Modifier::BOLD),
            ));
            lines.push(Line::raw(""));
            lines.push(Line::raw(crate::tr!(
                crate::keys::CONSOLE_CONFIRM_WAN_INTERFACE,
                name = wan.name,
                mac = wan.mac
            )));
            lines.push(Line::raw(match wizard.wan_mode {
                WanMode::Static => crate::tr!(
                    crate::keys::CONSOLE_CONFIRM_WAN_MODE_STATIC,
                    address = wizard.address,
                    gateway = wizard.gateway
                ),
                WanMode::Dhcp => {
                    crate::tr!(crate::keys::CONSOLE_CONFIRM_WAN_MODE_DHCP)
                }
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
                    crate::keys::CONSOLE_CONFIRM_LAN_MODE_WAN_ONLY
                )));
            } else {
                let names = lan.join(", ");
                lines.push(Line::raw(crate::tr!(
                    crate::keys::CONSOLE_CONFIRM_LAN_INTERFACES,
                    names = names
                )));
                lines.push(Line::raw(crate::tr!(
                    crate::keys::CONSOLE_CONFIRM_MANAGEMENT,
                    management = wizard.management
                )));
                lines.push(Line::raw(crate::tr!(
                    crate::keys::CONSOLE_CONFIRM_DHCP_RANGE,
                    start = wizard.dhcp_start,
                    end = wizard.dhcp_end
                )));
            }
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                crate::tr!(crate::keys::CONSOLE_CONFIRM_LAN_FLUSH_NOTE),
                Style::default().fg(Color::Yellow),
            ));
            lines.push(Line::styled(
                crate::tr!(crate::keys::CONSOLE_PRESS_ENTER_TO_START_INSTALLATION),
                Style::default().add_modifier(Modifier::BOLD),
            ));
        }
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title(crate::tr!(crate::keys::CONSOLE_NETWORK_PANEL_TITLE)))
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

fn wizard_hints(wizard: &NetworkWizard) -> String {
    if wizard.cancel_confirming {
        return crate::tr!(crate::keys::CONSOLE_WIZARD_HINT_CANCEL);
    }
    match wizard.step {
        WizardStep::Wan => crate::tr!(crate::keys::CONSOLE_WIZARD_HINT_WAN),
        WizardStep::WanMode => crate::tr!(crate::keys::CONSOLE_WIZARD_HINT_MODE),
        WizardStep::WanStatic => crate::tr!(crate::keys::CONSOLE_WIZARD_HINT_STATIC),
        WizardStep::Lan => crate::tr!(crate::keys::CONSOLE_WIZARD_HINT_LAN),
        WizardStep::Management | WizardStep::DhcpStart | WizardStep::DhcpEnd => {
            crate::tr!(crate::keys::CONSOLE_WIZARD_HINT_EDIT)
        }
        WizardStep::Confirm => crate::tr!(crate::keys::CONSOLE_WIZARD_HINT_CONFIRM),
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
                crate::tr!(crate::keys::CONSOLE_CANCEL_NETWORK_WIZARD_QUESTION),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Line::raw(""),
            Line::raw(crate::tr!(
                crate::keys::CONSOLE_CANCEL_NETWORK_WIZARD_PRESS_ENTER
            )),
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_CANCEL_NETWORK_WIZARD_PRESS_ESC),
                Style::default().fg(Color::DarkGray),
            ),
        ])
        .alignment(Alignment::Center)
        .block(Block::bordered().title(crate::tr!(crate::keys::CONSOLE_CANCEL_WIZARD))),
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
            crate::tr!(crate::keys::CONSOLE_READY)
        } else {
            app.notice.clone()
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

fn render_pending_takeover(frame: &mut Frame<'_>, app: &ConsoleApp) {
    let Snapshot::AwaitingNetworkConfirmation {
        transaction_id,
        phase,
        deadline,
        management_address,
    } = &app.snapshot
    else {
        return;
    };
    let confirm_allowed = app.takeover_confirm_allowed();
    let screen = frame.area();
    let width = 76.min(screen.width.saturating_sub(2));
    let height = 15.min(screen.height.saturating_sub(2));
    let area = Rect::new(
        screen.x + screen.width.saturating_sub(width) / 2,
        screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    );
    let mut lines = vec![
        Line::styled(
            crate::tr!(crate::keys::CONSOLE_TAKEOVER_PENDING_TITLE),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
        Line::raw(crate::tr!(
            crate::keys::CONSOLE_TAKEOVER_PENDING_TRANSACTION,
            id = transaction_id
        )),
        Line::raw(crate::tr!(
            crate::keys::CONSOLE_TAKEOVER_PENDING_PHASE,
            phase = phase
        )),
        Line::raw(crate::tr!(
            crate::keys::CONSOLE_TAKEOVER_PENDING_ADDRESS,
            address = management_address
                .as_deref()
                .map(str::to_string)
                .unwrap_or_else(|| crate::tr!(crate::keys::TAKEOVER_DHCP_LEASE))
        )),
        Line::raw(crate::tr!(
            crate::keys::CONSOLE_TAKEOVER_PENDING_DEADLINE,
            deadline = deadline
        )),
        Line::raw(""),
        Line::raw(crate::tr!(crate::keys::CONSOLE_TAKEOVER_PENDING_HINT)),
        Line::raw(""),
    ];
    let later = crate::tr!(crate::keys::CONSOLE_TAKEOVER_PENDING_LATER);
    lines.push(Line::from(Span::styled(
        if app.takeover_choice == 0 {
            format!("> {later}")
        } else {
            format!("  {later}")
        },
        if app.takeover_choice == 0 {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        },
    )));
    let confirm = crate::tr!(crate::keys::CONSOLE_TAKEOVER_PENDING_CONFIRM);
    let confirm_line = if confirm_allowed {
        if app.takeover_choice == 1 {
            format!("> {confirm}")
        } else {
            format!("  {confirm}")
        }
    } else {
        format!(
            "  {} ({})",
            confirm,
            crate::tr!(crate::keys::CONSOLE_TAKEOVER_PENDING_ROLLING_BACK)
        )
    };
    lines.push(Line::from(Span::styled(
        confirm_line,
        if confirm_allowed && app.takeover_choice == 1 {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        },
    )));
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        crate::tr!(crate::keys::CONSOLE_TAKEOVER_PENDING_KEY_HINT),
        Style::default().fg(Color::DarkGray),
    ));
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).alignment(Alignment::Center).block(
            Block::bordered().title(crate::tr!(crate::keys::CONSOLE_TAKEOVER_PENDING_WINDOW)),
        ),
        area,
    );
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
                crate::tr!(crate::keys::CONSOLE_EXIT_LANDSCAPE_KIT_QUESTION),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Line::raw(""),
            Line::raw(crate::tr!(crate::keys::CONSOLE_PRESS_ENTER_TO_EXIT)),
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_PRESS_ESC_TO_CANCEL),
                Style::default().fg(Color::DarkGray),
            ),
        ])
        .alignment(Alignment::Center)
        .block(Block::bordered().title(crate::tr!(crate::keys::CONSOLE_CONFIRM_EXIT))),
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
        .map(|menu| {
            let style = if app.menu_available(*menu) {
                Style::default()
            } else {
                Style::default().fg(Color::DarkGray)
            };
            ListItem::new(Span::styled(menu.label(), style))
        })
        .collect();
    let mut state = ListState::default().with_selected(Some(app.menu_index));
    let highlight = if app.focus == Focus::Navigation {
        Style::default().fg(Color::Black).bg(Color::Cyan)
    } else {
        Style::default().fg(Color::Cyan)
    };
    frame.render_stateful_widget(
        List::new(items)
            .block(Block::bordered().title(crate::tr!(crate::keys::CONSOLE_NAVIGATION)))
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
        Menu::Install if app.install_available() => render_install(frame, app, area),
        Menu::Install => {
            frame.render_widget(
                Paragraph::new(vec![
                    Line::styled(
                        crate::tr!(crate::keys::CONSOLE_LANDSCAPE_IS_INSTALLED),
                        Style::default().fg(Color::Green),
                    ),
                    Line::raw(""),
                    Line::styled(
                        crate::tr!(crate::keys::CONSOLE_INSTALL_UNAVAILABLE),
                        Style::default().fg(Color::DarkGray),
                    ),
                ])
                .block(panel_block(
                    &crate::tr!(crate::keys::CONSOLE_INSTALL_MENU),
                    focused,
                ))
                .wrap(Wrap { trim: true }),
                area,
            );
        }
        Menu::Backup => render_backup(frame, app, area),
        Menu::Update => render_update(frame, app, area),
    }
}

fn render_overview(frame: &mut Frame<'_>, app: &ConsoleApp, area: Rect) {
    let lines = match &app.snapshot {
        Snapshot::RootRequired => vec![
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_ROOT_PRIVILEGES_REQUIRED),
                Style::default().fg(Color::Yellow),
            ),
            Line::raw(""),
            Line::raw(crate::tr!(
                crate::keys::CONSOLE_OVERVIEW_INSTALL_ROOT,
                root = app.install.install_dir
            )),
        ],
        Snapshot::AwaitingNetworkConfirmation { .. } => vec![Line::styled(
            crate::tr!(crate::keys::CONSOLE_TAKEOVER_PENDING_TITLE),
            Style::default().fg(Color::Yellow),
        )],
        Snapshot::NotInstalled => vec![
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_LANDSCAPE_NOT_INSTALLED),
                Style::default().fg(Color::Yellow),
            ),
            Line::raw(""),
            Line::raw(crate::tr!(
                crate::keys::CONSOLE_OVERVIEW_INSTALL_ROOT,
                root = app.install.install_dir
            )),
        ],
        Snapshot::Installed {
            version,
            manager,
            initialized,
        } => vec![
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_LANDSCAPE_IS_INSTALLED),
                Style::default().fg(Color::Green),
            ),
            Line::raw(""),
            Line::raw(crate::tr!(
                crate::keys::CONSOLE_OVERVIEW_VERSION,
                version = version
            )),
            Line::raw(crate::tr!(
                crate::keys::CONSOLE_OVERVIEW_SERVICE,
                manager = manager
            )),
            Line::raw(crate::tr!(if *initialized {
                crate::keys::CONSOLE_OVERVIEW_INITIALIZATION_COMPLETE
            } else {
                crate::keys::CONSOLE_OVERVIEW_INITIALIZATION_PENDING
            })),
            Line::raw(crate::tr!(
                crate::keys::CONSOLE_OVERVIEW_INSTALL_ROOT,
                root = app.install.install_dir
            )),
        ],
        Snapshot::Unavailable(error) => vec![
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_INSTALLATION_STATE_NEEDS_ATTENTION),
                Style::default().fg(Color::Red),
            ),
            Line::raw(""),
            Line::raw(error),
        ],
    };
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel_block(
                &crate::tr!(crate::keys::CONSOLE_OVERVIEW),
                app.focus == Focus::Panel,
            ))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_update(frame: &mut Frame<'_>, app: &ConsoleApp, area: Rect) {
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
    let rows: [(String, String, bool); 4] = [
        (
            crate::tr!(crate::keys::CONSOLE_VERSION_LABEL),
            app.update.version.clone(),
            true,
        ),
        (
            crate::tr!(crate::keys::CONSOLE_REPOSITORY_LABEL),
            app.update
                .repository
                .label(app.update.current_source.as_ref()),
            false,
        ),
        (
            crate::tr!(crate::keys::CONSOLE_REPOSITORY_URL_LABEL),
            app.update.repository_url.clone(),
            true,
        ),
        (
            String::new(),
            crate::tr!(crate::keys::CONSOLE_UPDATE_BUTTON),
            false,
        ),
    ];
    for (index, (label, value, editable)) in rows.iter().enumerate() {
        if index == 2 && app.update.repository != UpdateRepositoryMode::Custom {
            continue;
        }
        let selected = focused && app.update.selected == index;
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
        } else if index == 3 {
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        let marker = if selected && app.update.editing && *editable {
            "_"
        } else {
            ""
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
        Paragraph::new(lines).block(panel_block(
            &crate::tr!(crate::keys::CONSOLE_UPDATE_MENU),
            focused,
        )),
        area,
    );
}

fn render_update_confirmation(frame: &mut Frame<'_>, app: &ConsoleApp) {
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
        .block(Block::bordered().title(crate::tr!(crate::keys::CONSOLE_UPDATE_CONFIRM_TITLE))),
        area,
    );
}

fn render_backup(frame: &mut Frame<'_>, app: &ConsoleApp, area: Rect) {
    let focused = app.focus == Focus::Panel;
    if app.backup.details.is_some() {
        render_backup_details(frame, app, focused, area);
        return;
    }
    if matches!(app.snapshot, Snapshot::RootRequired) {
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled(
                    crate::tr!(crate::keys::CONSOLE_ROOT_PRIVILEGES_REQUIRED),
                    Style::default().fg(Color::Yellow),
                ),
                Line::raw(""),
                Line::styled(
                    crate::tr!(crate::keys::CONSOLE_BACKUP_REQUIRES_INSTALL),
                    Style::default().fg(Color::DarkGray),
                ),
            ])
            .block(panel_block(
                &crate::tr!(crate::keys::CONSOLE_BACKUP_MENU),
                focused,
            ))
            .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }
    if !matches!(app.snapshot, Snapshot::Installed { .. }) {
        let message = match &app.snapshot {
            Snapshot::NotInstalled => {
                crate::tr!(crate::keys::CONSOLE_LANDSCAPE_NOT_INSTALLED)
            }
            Snapshot::Unavailable(error) => error.clone(),
            _ => unreachable!(),
        };
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled(message, Style::default().fg(Color::Yellow)),
                Line::raw(""),
                Line::styled(
                    crate::tr!(crate::keys::CONSOLE_BACKUP_REQUIRES_INSTALL),
                    Style::default().fg(Color::DarkGray),
                ),
            ])
            .block(panel_block(
                &crate::tr!(crate::keys::CONSOLE_BACKUP_MENU),
                focused,
            ))
            .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }
    render_backup_list(frame, app, focused, area);
}

fn render_backup_list(frame: &mut Frame<'_>, app: &ConsoleApp, focused: bool, area: Rect) {
    let create_selected = app.focus == Focus::Panel && app.backup.selected == 0;
    let highlight = Style::default()
        .fg(Color::Black)
        .bg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let mut lines = vec![Line::styled(
        format!(
            "{}{}",
            if create_selected { "> " } else { "  " },
            crate::tr!(crate::keys::CONSOLE_BACKUP_CREATE)
        ),
        if create_selected {
            highlight
        } else {
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD)
        },
    )];
    match &app.backup.state {
        BackupListState::NotRun | BackupListState::Running(_) => {
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                crate::tr!(crate::keys::CONSOLE_BACKUP_LOADING),
                Style::default().fg(Color::DarkGray),
            ));
        }
        BackupListState::Failed(error) => {
            lines.push(Line::raw(""));
            lines.push(Line::styled(error.clone(), Style::default().fg(Color::Red)));
        }
        BackupListState::Complete(rows) => {
            if rows.is_empty() {
                lines.push(Line::raw(""));
                lines.push(Line::styled(
                    crate::tr!(crate::keys::CONSOLE_BACKUP_NONE_FOUND),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            for (index, entry) in rows.iter().enumerate() {
                let cursor = app.focus == Focus::Panel && app.backup.selected == index + 1;
                match &entry.metadata {
                    Some(metadata) => {
                        let text = format!(
                            "{}  {}  {}{}",
                            metadata.backup_id,
                            metadata.created_at,
                            metadata.landscape_version,
                            if metadata.remark.is_empty() {
                                String::new()
                            } else {
                                format!("  {}", metadata.remark)
                            }
                        );
                        lines.push(Line::styled(
                            format!("{}{}", if cursor { "> " } else { "  " }, text),
                            if cursor { highlight } else { Style::default() },
                        ));
                    }
                    None => {
                        let name = entry
                            .path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .trim_end_matches(".lkb")
                            .to_string();
                        lines.push(Line::styled(
                            format!(
                                "{}{}  {}",
                                if cursor { "> " } else { "  " },
                                name,
                                crate::tr!(crate::keys::CONSOLE_BACKUP_INVALID_BADGE)
                            ),
                            if cursor {
                                highlight
                            } else {
                                Style::default().fg(Color::Red)
                            },
                        ));
                    }
                }
            }
        }
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel_block(
                &crate::tr!(crate::keys::CONSOLE_BACKUP_MENU),
                focused,
            ))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn render_backup_details(frame: &mut Frame<'_>, app: &ConsoleApp, focused: bool, area: Rect) {
    let Some(entry) = app.backup.details_entry() else {
        return;
    };
    let Some(metadata) = &entry.metadata else {
        return;
    };
    let contents = format!(
        "binary={} static={} static_archive={} init_config={} geo_cache={}",
        metadata.contents.binary,
        metadata.contents.static_,
        metadata.contents.static_archive,
        metadata.contents.init_config,
        metadata.contents.geo_cache,
    );
    let lines = vec![
        Line::styled(
            crate::tr!(crate::keys::CONSOLE_BACKUP_DETAILS_TITLE),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
        Line::raw(format!(
            "{}  {}",
            crate::tr!(crate::keys::CONSOLE_BACKUP_ID_LABEL),
            metadata.backup_id
        )),
        Line::raw(format!(
            "{}  {}",
            crate::tr!(crate::keys::CONSOLE_BACKUP_CREATED_LABEL),
            metadata.created_at
        )),
        Line::raw(format!(
            "{}  {}",
            crate::tr!(crate::keys::CONSOLE_BACKUP_VERSION_LABEL),
            metadata.landscape_version
        )),
        Line::raw(format!(
            "{}  {}",
            crate::tr!(crate::keys::CONSOLE_BACKUP_LKIT_LABEL),
            metadata.lkit_version
        )),
        Line::raw(format!(
            "{}  {}",
            crate::tr!(crate::keys::CONSOLE_BACKUP_ARCH_LABEL),
            architecture_key(metadata.architecture)
        )),
        Line::raw(format!(
            "{}  {}",
            crate::tr!(crate::keys::CONSOLE_BACKUP_HOSTNAME_LABEL),
            metadata.hostname
        )),
        Line::raw(format!(
            "{}  {}",
            crate::tr!(crate::keys::CONSOLE_BACKUP_REMARK_LABEL),
            metadata.remark
        )),
        Line::raw(format!(
            "{}  {}",
            crate::tr!(crate::keys::CONSOLE_BACKUP_AUTO_LABEL),
            metadata.auto
        )),
        Line::raw(format!(
            "{}  {}",
            crate::tr!(crate::keys::CONSOLE_BACKUP_SCOPE_LABEL),
            scope_key(metadata.scope)
        )),
        Line::raw(format!(
            "{}  {}",
            crate::tr!(crate::keys::CONSOLE_BACKUP_CONTENTS_LABEL),
            contents
        )),
        Line::raw(""),
        Line::styled(
            crate::tr!(
                crate::keys::CONSOLE_BACKUP_DETAILS_RESTORE_HINT,
                id = metadata.backup_id
            ),
            Style::default().fg(Color::DarkGray),
        ),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel_block(
                &crate::tr!(crate::keys::CONSOLE_BACKUP_DETAILS_TITLE),
                focused,
            ))
            .wrap(Wrap { trim: true })
            .scroll((app.backup.details_scroll, 0)),
        area,
    );
}

fn render_backup_create_dialog(frame: &mut Frame<'_>, app: &ConsoleApp) {
    let screen = frame.area();
    let width = 68.min(screen.width.saturating_sub(2));
    let height = 9.min(screen.height.saturating_sub(2));
    let area = Rect::new(
        screen.x + screen.width.saturating_sub(width) / 2,
        screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    );
    let remark = app.backup.remark.clone();
    let remark_display = if remark.is_empty() {
        "_".to_string()
    } else {
        format!("{remark}_")
    };
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(vec![
            Line::raw(crate::tr!(crate::keys::CONSOLE_BACKUP_CREATE_SCOPE)),
            Line::raw(""),
            Line::from(vec![
                Span::styled(
                    format!("{}: ", crate::tr!(crate::keys::CONSOLE_BACKUP_REMARK_LABEL)),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    remark_display,
                    Style::default().add_modifier(Modifier::UNDERLINED),
                ),
            ]),
            Line::raw(""),
            Line::raw(crate::tr!(crate::keys::CONSOLE_BACKUP_CREATE_HINT)),
        ])
        .alignment(Alignment::Center)
        .block(Block::bordered().title(crate::tr!(crate::keys::CONSOLE_BACKUP_CREATE_TITLE))),
        area,
    );
}

/// 创建备份进行中的居中弹窗：阶段文案 + 文件数 Gauge。
fn render_backup_create_progress(frame: &mut Frame<'_>, app: &ConsoleApp) {
    let Some(run) = &app.backup.create else {
        return;
    };
    let screen = frame.area();
    let width = 76.min(screen.width.saturating_sub(2));
    let height = 7.min(screen.height.saturating_sub(2));
    let area = Rect::new(
        screen.x + screen.width.saturating_sub(width) / 2,
        screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    );
    let (stage_text, ratio) = match &run.progress {
        BackupProgress::Exporting => (
            crate::tr!(crate::keys::CONSOLE_BACKUP_CREATE_PROGRESS_EXPORT),
            0.0,
        ),
        BackupProgress::Archiving {
            done,
            total,
            current,
        } => {
            let ratio = if *total == 0 {
                0.0
            } else {
                *done as f64 / *total as f64
            };
            (
                crate::tr!(
                    crate::keys::CONSOLE_BACKUP_CREATE_PROGRESS_ARCHIVE,
                    done = *done,
                    total = *total,
                    current = current
                ),
                ratio,
            )
        }
        BackupProgress::Finalizing => (
            crate::tr!(crate::keys::CONSOLE_BACKUP_CREATE_PROGRESS_FINALIZE),
            1.0,
        ),
    };
    let percent = (ratio * 100.0).round() as u64;
    let inner = area.inner(Margin {
        vertical: 1,
        horizontal: 1,
    });
    let [stage_area, gauge_area, hint_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::bordered().title(crate::tr!(crate::keys::CONSOLE_BACKUP_CREATE_RUNNING)),
        area,
    );
    frame.render_widget(
        Paragraph::new(stage_text).wrap(Wrap { trim: true }),
        stage_area,
    );
    frame.render_widget(
        Gauge::default()
            .ratio(ratio.clamp(0.0, 1.0))
            .label(format!("{percent:>3}%"))
            .gauge_style(Style::default().fg(Color::Cyan))
            .use_unicode(false),
        gauge_area,
    );
    frame.render_widget(
        Paragraph::new(crate::tr!(crate::keys::CONSOLE_BACKUP_HINT_CREATE_RUNNING))
            .style(Style::default().fg(Color::DarkGray)),
        hint_area,
    );
}

fn render_backup_restore_confirmation(frame: &mut Frame<'_>, app: &ConsoleApp) {
    let Some(metadata) = app
        .backup
        .selected_entry()
        .and_then(|entry| entry.metadata.as_ref())
    else {
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
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_BACKUP_RESTORE_QUESTION),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Line::raw(""),
            Line::raw(crate::tr!(
                crate::keys::CONSOLE_BACKUP_RESTORE_PLAN,
                id = metadata.backup_id,
                version = metadata.landscape_version
            )),
            Line::raw(crate::tr!(
                crate::keys::CONSOLE_BACKUP_RESTORE_MINIMAL_SCOPE
            )),
            Line::raw(""),
            Line::raw(crate::tr!(crate::keys::CONSOLE_BACKUP_RESTORE_PRESS_ENTER)),
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_PRESS_ESC_TO_CANCEL),
                Style::default().fg(Color::DarkGray),
            ),
        ])
        .alignment(Alignment::Center)
        .block(Block::bordered().title(crate::tr!(crate::keys::CONSOLE_BACKUP_RESTORE_TITLE))),
        area,
    );
}

fn render_backup_delete_confirmation(frame: &mut Frame<'_>, app: &ConsoleApp) {
    let Some(metadata) = app.backup.delete_target.as_deref().and_then(|id| {
        app.backup
            .rows()
            .iter()
            .find_map(|entry| entry.metadata.as_ref().filter(|m| m.backup_id == id))
    }) else {
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
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_BACKUP_DELETE_QUESTION),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Line::raw(""),
            Line::raw(crate::tr!(
                crate::keys::CONSOLE_BACKUP_DELETE_PLAN,
                id = metadata.backup_id,
                version = metadata.landscape_version
            )),
            Line::raw(""),
            Line::raw(crate::tr!(crate::keys::CONSOLE_BACKUP_DELETE_PRESS_ENTER)),
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_PRESS_ESC_TO_CANCEL),
                Style::default().fg(Color::DarkGray),
            ),
        ])
        .alignment(Alignment::Center)
        .block(Block::bordered().title(crate::tr!(crate::keys::CONSOLE_BACKUP_DELETE_TITLE))),
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
            crate::tr!(crate::keys::CONSOLE_NOT_RUN),
            crate::tr!(crate::keys::CONSOLE_WAITING_TO_CHECK_HOST).into(),
            Color::DarkGray,
        ),
        PreflightState::Running(_) => (
            crate::tr!(crate::keys::CONSOLE_RUNNING),
            crate::tr!(crate::keys::CONSOLE_CHECKING_THIS_HOST).into(),
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
            Span::styled(format!("{status:<9}"), status_style),
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

fn render_preflight_details(
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
            .wrap(Wrap { trim: true })
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
        crate::tr!(crate::keys::CONSOLE_NETWORK_TAKEOVER_LABEL),
        String::new(),
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
            &crate::tr!(crate::keys::CONSOLE_INSTALL_MENU),
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
                title: "Host platform".to_string(),
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
                title: "Host platform".to_string(),
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
                title: "Ports".to_string(),
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
        assert!(terminal_content(&terminal).contains("> Backup"));
    }

    #[test]
    fn install_menu_is_skipped_when_landscape_is_installed() {
        let mut app = ConsoleApp::new();
        app.snapshot = installed_snapshot();
        assert_eq!(app.menu(), Menu::Overview);

        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.menu(), Menu::Backup);

        app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.menu(), Menu::Overview);
    }

    #[test]
    fn install_menu_stays_selectable_when_not_installed() {
        let mut app = ConsoleApp::new();
        app.snapshot = Snapshot::NotInstalled;

        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.menu(), Menu::Install);

        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.menu(), Menu::Backup);
    }

    #[test]
    fn installed_snapshot_renders_install_menu_disabled() {
        let _language = LanguageGuard::set(Language::En);
        let backend = TestBackend::new(100, 28);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = ConsoleApp::new();
        app.snapshot = installed_snapshot();

        terminal.draw(|frame| render(frame, &app)).unwrap();
        let buffer = terminal.backend().buffer();
        let width = buffer.area.width as usize;
        let mut found = false;
        for index in 0..buffer.content.len().saturating_sub(7) {
            if index % width >= 24 {
                continue;
            }
            let text: String = buffer.content[index..index + 7]
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            if text == "Install" && buffer.content[index + 7].symbol() != "e" {
                assert_eq!(buffer.content[index].fg, Color::DarkGray);
                found = true;
            }
        }
        assert!(found, "Install label rendered in sidebar");
    }

    #[test]
    fn installed_snapshot_renders_install_panel_unavailable() {
        let _language = LanguageGuard::set(Language::En);
        let backend = TestBackend::new(100, 28);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = ConsoleApp::new();
        app.menu_index = 1;
        app.focus = Focus::Panel;
        app.snapshot = installed_snapshot();

        terminal.draw(|frame| render(frame, &app)).unwrap();
        let content = terminal_content(&terminal);
        assert!(content.contains("Landscape is installed"));
        assert!(content.contains("unavailable"));
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

        app.update();

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
        assert_eq!(app.install.repository, RepositoryMode::Github);
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
    fn install_form_maps_repository_modes_to_cli_flags() {
        let base = InstallForm {
            password: "Secret123".into(),
            password_confirmation: "Secret123".into(),
            ..InstallForm::default()
        };
        for (mode, repository, expected) in [
            (RepositoryMode::Default, None, Vec::<&str>::new()),
            (
                RepositoryMode::Github,
                Some(Some("github".into())),
                vec!["--repository", "github"],
            ),
            (RepositoryMode::Mirror, Some(None), vec!["--repository"]),
        ] {
            let mut form = base.clone();
            form.repository = mode;
            let ConsoleAction::Command { command, args } = form.command().unwrap() else {
                panic!("expected install command");
            };
            let Commands::Install(install) = command else {
                panic!("expected install request");
            };
            assert_eq!(install.repository, repository);
            for pair in expected.chunks(2) {
                if pair.len() == 1 {
                    assert!(
                        args.iter().any(|argument| argument == pair[0]),
                        "{mode:?} must forward {:?}, got {args:?}",
                        pair[0]
                    );
                } else {
                    assert!(
                        args.windows(2).any(|window| window == pair),
                        "{mode:?} must forward {pair:?}, got {args:?}"
                    );
                }
            }
            if expected.is_empty() {
                assert!(
                    !args.iter().any(|argument| argument == "--repository"),
                    "{mode:?} must not forward --repository, got {args:?}"
                );
            }
        }
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

    fn installed_snapshot() -> Snapshot {
        Snapshot::Installed {
            version: "1.2.3".into(),
            manager: "systemd",
            initialized: true,
        }
    }

    fn pending_takeover_snapshot() -> Snapshot {
        Snapshot::AwaitingNetworkConfirmation {
            transaction_id: "tx-1".into(),
            phase: "awaiting_network_confirmation",
            deadline: "2026-08-07T10:00:00Z".into(),
            management_address: Some("192.168.10.1/24".into()),
        }
    }

    #[test]
    fn pending_takeover_snapshot_is_detected_from_transaction() {
        let temp =
            std::env::temp_dir().join(format!("lkit-console-pending-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();
        let root = crate::deployment::root::normalize_install_root(&temp).unwrap();
        let mut transaction = crate::deployment::transaction::TransactionFile::new_install(
            &root,
            &semver::Version::new(1, 0, 0),
        )
        .unwrap();
        transaction.phase = crate::deployment::transaction::Phase::AwaitingNetworkConfirmation;
        let id = transaction.transaction_id.clone();
        transaction.network_takeover =
            Some(crate::deployment::transaction::NetworkTakeoverTransaction {
                plan: NetworkPlan {
                    mode: NetworkMode::RoutedLan {
                        wan: "ens3".into(),
                        wan_ipv4: None,
                        lan: vec!["ens4".into()],
                        management: "192.168.10.1/24".parse().unwrap(),
                        dhcp_start: "192.168.10.100".parse().unwrap(),
                        dhcp_end: "192.168.10.254".parse().unwrap(),
                    },
                    selected_macs: vec![
                        SelectedInterface {
                            name: "ens3".into(),
                            mac: "02:00:00:00:00:03".into(),
                        },
                        SelectedInterface {
                            name: "ens4".into(),
                            mac: "02:00:00:00:00:04".into(),
                        },
                    ],
                },
                host_services: Vec::new(),
                confirmation_deadline: chrono::Utc::now() + chrono::Duration::minutes(10),
                rollback_service: format!("lkit-network-{id}-rollback.service"),
                rollback_timer: format!("lkit-network-{id}-rollback.timer"),
                boot_rollback_service: format!("lkit-network-{id}-boot-rollback.service"),
                recovery_binary: "service/lkit-network-recovery".into(),
                pending_state: format!("transactions/{id}/pending-install-state.json"),
            });
        crate::deployment::transaction::persist(&root, &transaction).unwrap();
        let snapshot = Snapshot::load(&temp.display().to_string());
        let _ = std::fs::remove_dir_all(&temp);
        match snapshot {
            // 以 root 运行测试时 Snapshot::load 返回 RootRequired，跳过检测断言。
            Snapshot::RootRequired => {}
            Snapshot::AwaitingNetworkConfirmation {
                transaction_id,
                phase,
                management_address,
                ..
            } => {
                assert_eq!(transaction_id, id);
                assert_eq!(phase, "awaiting_network_confirmation");
                assert_eq!(management_address.as_deref(), Some("192.168.10.1/24"));
            }
            _ => panic!("expected pending snapshot, got a different state"),
        }
    }

    #[test]
    fn pending_takeover_blocking_screen_renders_instead_of_menu() {
        let _language = LanguageGuard::set(Language::En);
        let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
        let mut app = ConsoleApp::new();
        app.snapshot = pending_takeover_snapshot();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let content = terminal_content(&terminal);
        assert!(content.contains("Network takeover awaiting confirmation"));
        assert!(content.contains("tx-1"));
        assert!(content.contains("awaiting_network_confirmation"));
        assert!(content.contains("192.168.10.1/24"));
        assert!(content.contains("2026-08-07T10:00:00Z"));
        assert!(content.contains("Later"));
        assert!(content.contains("Confirm now"));
        assert!(!content.contains("Navigation"));
    }

    #[test]
    fn pending_takeover_enter_confirm_executes_network_confirm() {
        let _language = LanguageGuard::set(Language::En);
        let mut app = ConsoleApp::new();
        app.snapshot = pending_takeover_snapshot();
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.takeover_choice, 1);
        let action = app
            .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .expect("enter on confirm must return an action");
        let ConsoleAction::Command { command, args } = action else {
            panic!("expected a command action");
        };
        assert!(matches!(
            command,
            Commands::Network(crate::commands::network::Network {
                action: crate::commands::network::NetworkAction::Confirm,
                ..
            })
        ));
        assert_eq!(args[0], "network");
        assert_eq!(args[1], "confirm");
        assert!(args.contains(&"--install-dir".to_string()));
    }

    #[test]
    fn pending_takeover_later_and_esc_quit_the_console() {
        let _language = LanguageGuard::set(Language::En);
        let mut app = ConsoleApp::new();
        app.snapshot = pending_takeover_snapshot();
        assert!(matches!(
            app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Some(ConsoleAction::Quit)
        ));
        assert!(matches!(
            app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            Some(ConsoleAction::Quit)
        ));
        assert!(
            app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE))
                .is_none()
        );
    }

    #[test]
    fn rolling_back_pending_disables_confirm() {
        let _language = LanguageGuard::set(Language::En);
        let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
        let mut app = ConsoleApp::new();
        app.snapshot = Snapshot::AwaitingNetworkConfirmation {
            transaction_id: "tx-1".into(),
            phase: "rolling_back",
            deadline: "2026-08-07T10:00:00Z".into(),
            management_address: None,
        };
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let content = terminal_content(&terminal);
        assert!(content.contains("rollback in progress"));
        assert!(content.contains("DHCP lease"));
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.takeover_choice, 0);
        assert!(matches!(
            app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Some(ConsoleAction::Quit)
        ));
    }

    #[test]
    fn pending_takeover_hides_install_menu() {
        let mut app = ConsoleApp::new();
        app.snapshot = pending_takeover_snapshot();
        assert!(!app.install_available());
        assert!(!app.menu_available(Menu::Install));
    }

    fn sample_backup_metadata() -> BackupMetadata {
        BackupMetadata {
            schema_version: 1,
            backup_id: "20260807-131500-ab12cd34".into(),
            created_at: chrono::DateTime::parse_from_rfc3339("2026-08-07T13:15:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
            landscape_version: "1.2.3".into(),
            lkit_version: "0.1.3".into(),
            architecture: crate::backup::lkb::BackupArchitecture::X86_64,
            hostname: "edge".into(),
            remark: "before upgrade".into(),
            auto: false,
            scope: crate::backup::lkb::BackupScope::Minimal,
            contents: crate::backup::lkb::BackupContents {
                binary: true,
                static_: true,
                static_archive: false,
                init_config: true,
                geo_cache: false,
            },
            checksum: "sha256:00".into(),
        }
    }

    fn sample_backup_entry() -> BackupEntry {
        BackupEntry {
            metadata: Some(sample_backup_metadata()),
            path: PathBuf::from("/opt/landscape/backups/20260807-131500-ab12cd34.lkb"),
        }
    }

    fn backup_ready_app() -> ConsoleApp {
        let mut app = ConsoleApp::new();
        app.menu_index = 2;
        app.focus = Focus::Panel;
        app.snapshot = installed_snapshot();
        app.backup.state = BackupListState::Complete(vec![sample_backup_entry()]);
        app
    }

    #[test]
    fn backup_menu_lists_backups_and_opens_details() {
        let _language = LanguageGuard::set(Language::En);
        let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
        let app = backup_ready_app();

        terminal.draw(|frame| render(frame, &app)).unwrap();
        let content = terminal_content(&terminal);
        assert!(content.contains("Backup"));
        assert!(content.contains("Create backup"));
        assert!(content.contains("20260807-131500-ab12cd34"));
        assert!(content.contains("before upgrade"));

        let mut app = backup_ready_app();
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.backup.details, Some(0));

        terminal.draw(|frame| render(frame, &app)).unwrap();
        let details = terminal_content(&terminal);
        assert!(details.contains("Backup details"));
        assert!(details.contains("x86_64"));
        assert!(details.contains("edge"));
        assert!(details.contains("Press R to restore"));

        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.backup.details, None);
    }

    #[test]
    fn backup_menu_without_installation_shows_requirements() {
        let _language = LanguageGuard::set(Language::En);
        let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
        let mut app = ConsoleApp::new();
        app.menu_index = 2;
        app.focus = Focus::Panel;
        app.snapshot = Snapshot::NotInstalled;

        terminal.draw(|frame| render(frame, &app)).unwrap();
        let content = terminal_content(&terminal);
        assert!(content.contains("Landscape is not installed"));
        assert!(content.contains("Backup and restore require an existing installation"));
    }

    #[test]
    fn backup_create_runs_in_console_with_progress_dialog() {
        let _language = LanguageGuard::set(Language::En);
        let mut app = backup_ready_app();
        app.install.install_dir = std::env::temp_dir()
            .join(format!("lkit-console-create-{}", std::process::id()))
            .display()
            .to_string();

        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.backup.editing);

        let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let content = terminal_content(&terminal);
        assert!(
            content.contains("Create backup"),
            "the backup create dialog must be visible while editing"
        );
        assert!(content.contains("Remark: _"));

        for character in "my-backup".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        }
        assert_eq!(app.backup.remark, "my-backup");

        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(!app.backup.editing);
        assert_eq!(app.backup.remark, "");
        assert!(
            app.backup.create.is_some(),
            "Enter must start the in-console backup create worker"
        );

        terminal.draw(|frame| render(frame, &app)).unwrap();
        let content = terminal_content(&terminal);
        assert!(
            content.contains("Creating backup"),
            "the progress dialog must be visible while the backup is created"
        );
        assert!(
            content.contains("Exporting configuration"),
            "the progress dialog must show the export stage"
        );

        let _ = std::fs::remove_dir_all(&app.install.install_dir);
    }

    #[test]
    fn backup_restore_flow_builds_restore_command() {
        let _language = LanguageGuard::set(Language::En);
        let mut app = backup_ready_app();

        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.backup.selected, 1);
        app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        assert!(app.backup.restore_confirming);

        let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let content = terminal_content(&terminal);
        assert!(content.contains("Restore this backup?"));
        assert!(content.contains("version 1.2.3"));
        assert!(content.contains("Press Enter to restore."));
        assert!(
            content.contains("SQLite data file"),
            "the restore confirmation must warn about the minimal scope"
        );

        let action = app
            .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        let ConsoleAction::Command { command, args } = action else {
            panic!("expected restore command");
        };
        let Commands::Restore(restore) = command else {
            panic!("expected restore request");
        };
        assert_eq!(restore.backup.as_deref(), Some("20260807-131500-ab12cd34"));
        assert!(restore.yes);
        assert!(
            restore.console_confirmed,
            "the console must mark the restore as confirmed so no TTY prompt appears"
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--backup", "20260807-131500-ab12cd34"])
        );
        assert!(args.iter().any(|argument| argument == "--yes"));
        assert!(
            args.iter()
                .any(|argument| argument == "--console-confirmed")
        );
    }

    #[test]
    fn backup_delete_confirms_and_removes_the_backup() {
        use std::os::unix::fs::PermissionsExt;
        let _language = LanguageGuard::set(Language::En);
        let dir = std::env::temp_dir().join(format!("lkit-console-delete-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let backups = dir.join("backups");
        std::fs::create_dir_all(&backups).unwrap();
        let path = backups.join("20260807-131500-ab12cd34.lkb");
        std::fs::write(&path, b"lkb bytes").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

        let mut app = backup_ready_app();
        app.install.install_dir = dir.display().to_string();
        app.backup.state =
            BackupListState::Complete(vec![sample_backup_entry(), sample_backup_entry()]);

        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        assert!(app.backup.delete_confirming);
        assert_eq!(
            app.backup.delete_target.as_deref(),
            Some("20260807-131500-ab12cd34")
        );

        let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let content = terminal_content(&terminal);
        assert!(content.contains("Confirm delete"));
        assert!(content.contains("Delete this backup?"));
        assert!(content.contains("version 1.2.3"));
        assert!(content.contains("Press Enter to delete."));

        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(!app.backup.delete_confirming);
        assert!(app.backup.delete_target.is_none());
        assert!(
            !path.exists(),
            "confirming the delete must remove the backup file"
        );
        assert!(app.notice.contains("deleted"));
        assert!(matches!(app.backup.state, BackupListState::NotRun));

        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn backup_delete_esc_cancels_confirmation() {
        let _language = LanguageGuard::set(Language::En);
        let mut app = backup_ready_app();
        app.backup.state = BackupListState::Complete(vec![sample_backup_entry()]);

        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        assert!(app.backup.delete_confirming);
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.backup.delete_confirming);
        assert!(app.backup.delete_target.is_none());
        assert_eq!(app.exit_state, ExitState::Idle);
    }

    #[test]
    fn backup_esc_cancels_restore_confirmation_and_details() {
        let _language = LanguageGuard::set(Language::En);
        let mut app = backup_ready_app();

        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
        assert!(app.backup.restore_confirming);
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.backup.restore_confirming);
        assert_eq!(app.exit_state, ExitState::Idle);

        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.backup.details, Some(0));
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.backup.details, None);
        assert_eq!(app.exit_state, ExitState::Idle);
    }

    fn update_ready_app() -> ConsoleApp {
        let mut app = ConsoleApp::new();
        app.menu_index = 3;
        app.focus = Focus::Panel;
        app.snapshot = installed_snapshot();
        app.update.current_source = Some(RepositorySource {
            kind: RepositorySourceKind::Http,
            location: "https://example.com/releases/".into(),
        });
        app.update.repository = UpdateRepositoryMode::Current;
        app
    }

    fn resolved(current: &str, target: &str) -> ResolvedUpdate {
        ResolvedUpdate {
            current: semver::Version::parse(current).unwrap(),
            target: semver::Version::parse(target).unwrap(),
        }
    }

    #[test]
    fn update_menu_is_only_available_when_installed() {
        let mut app = ConsoleApp::new();
        app.snapshot = Snapshot::NotInstalled;
        assert_eq!(app.menu(), Menu::Overview);

        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.menu(), Menu::Install);
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.menu(), Menu::Backup);
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(
            app.menu(),
            Menu::Backup,
            "Update must be skipped when Landscape is not installed"
        );

        let mut app = ConsoleApp::new();
        app.snapshot = installed_snapshot();
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.menu(), Menu::Backup);
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.menu(), Menu::Update);
    }

    #[test]
    fn update_panel_renders_current_version_and_form() {
        let _language = LanguageGuard::set(Language::En);
        let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
        let app = update_ready_app();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let content = terminal_content(&terminal);
        assert!(content.contains("Current version"));
        assert!(content.contains("1.2.3"));
        assert!(content.contains("latest"));
        assert!(content.contains("Current source (http: https://example.com/releases/)"));
        assert!(content.contains("[ Start update ]"));
    }

    #[test]
    fn update_menu_without_installation_shows_requirements() {
        let _language = LanguageGuard::set(Language::En);
        let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
        let mut app = ConsoleApp::new();
        app.menu_index = 3;
        app.focus = Focus::Panel;
        app.snapshot = Snapshot::NotInstalled;
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let content = terminal_content(&terminal);
        assert!(content.contains("Landscape is not installed"));
        assert!(content.contains("Update requires an existing installation"));
    }

    #[test]
    fn update_panel_navigation_edits_version_and_reaches_url_when_custom() {
        let mut app = update_ready_app();
        assert_eq!(app.update.selected, 0);

        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.update.selected, 1);
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(
            app.update.selected, 3,
            "the hidden URL row must be skipped for non-custom repositories"
        );
        app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.update.selected, 1);
        app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(app.update.selected, 0);

        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.update.editing);
        app.handle_key(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('.'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE));
        assert_eq!(app.update.version, "latest1.2");
        app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(app.update.version, "latest1.");
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(!app.update.editing);

        app.update.repository = UpdateRepositoryMode::Custom;
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.update.selected, 1);
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.update.selected, 2);
        assert_eq!(app.exit_state, ExitState::Idle);
    }

    #[test]
    fn update_repository_cycles_within_available_options() {
        let mut app = update_ready_app();
        app.update.selected = 1;
        app.update.repository = UpdateRepositoryMode::Current;

        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(app.update.repository, UpdateRepositoryMode::Github);
        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(app.update.repository, UpdateRepositoryMode::Mirror);
        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(app.update.repository, UpdateRepositoryMode::Custom);
        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(app.update.repository, UpdateRepositoryMode::Current);

        app.update.current_source = None;
        app.update.repository = UpdateRepositoryMode::Current;
        app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        assert_eq!(
            app.update.repository,
            UpdateRepositoryMode::Github,
            "Current must not be reachable without a config source"
        );
    }

    #[test]
    fn update_resolution_branches_like_the_update_command() {
        let _language = LanguageGuard::set(Language::En);
        let mut app = update_ready_app();
        let mut notice = String::new();

        app.update
            .apply_resolution(&mut notice, resolved("1.2.3", "1.2.3"));
        assert!(notice.contains("already up to date"));
        assert!(app.update.confirming.is_none());

        notice.clear();
        app.update
            .apply_resolution(&mut notice, resolved("1.2.4", "1.2.3"));
        assert!(notice.contains("downgrading"));
        assert!(app.update.confirming.is_none());

        notice.clear();
        app.update
            .apply_resolution(&mut notice, resolved("1.2.3", "1.2.4"));
        assert!(notice.is_empty(), "an upgrade must not set the notice");
        let confirming = app.update.confirming.as_ref().unwrap();
        assert_eq!(confirming.current.to_string(), "1.2.3");
        assert_eq!(confirming.target.to_string(), "1.2.4");
    }

    #[test]
    fn update_confirmation_builds_console_confirmed_command() {
        let _language = LanguageGuard::set(Language::En);
        let mut app = update_ready_app();
        app.update.confirming = Some(resolved("1.2.3", "1.2.4"));

        let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let content = terminal_content(&terminal);
        assert!(content.contains("Confirm update"));
        assert!(content.contains("Update Landscape?"));
        assert!(content.contains("1.2.3 -> target 1.2.4"));
        assert!(content.contains("Press Enter to update."));

        let action = app
            .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .unwrap();
        let ConsoleAction::Command { command, args } = action else {
            panic!("expected update command");
        };
        let Commands::Update(update) = command else {
            panic!("expected update request");
        };
        assert_eq!(update.version.as_deref(), Some("latest"));
        assert!(
            update.repository.is_none(),
            "Current source must not forward --repository"
        );
        assert!(
            update.console_confirmed,
            "the console must mark the update as confirmed so no TTY prompt appears"
        );
        assert!(
            args.iter()
                .any(|argument| argument == "--console-confirmed")
        );
        assert!(args.windows(2).any(|pair| pair == ["--version", "latest"]));
        assert!(!args.iter().any(|argument| argument == "--repository"));
    }

    #[test]
    fn update_repository_modes_map_to_cli_flags() {
        let mut app = update_ready_app();
        for (mode, repository, expected) in [
            (UpdateRepositoryMode::Current, None, Vec::<&str>::new()),
            (
                UpdateRepositoryMode::Github,
                Some(Some("github".into())),
                vec!["--repository", "github"],
            ),
            (
                UpdateRepositoryMode::Mirror,
                Some(None),
                vec!["--repository"],
            ),
            (
                UpdateRepositoryMode::Custom,
                Some(Some("https://example.com/releases/".into())),
                vec!["--repository", "https://example.com/releases/"],
            ),
        ] {
            app.update.repository = mode;
            app.update.repository_url = "https://example.com/releases/".into();
            let action = app.update_action();
            let ConsoleAction::Command { command, args } = action else {
                panic!("{mode:?} must build an update command");
            };
            let Commands::Update(update) = command else {
                panic!("{mode:?} must build an update request");
            };
            assert_eq!(update.repository, repository, "{mode:?}");
            assert!(update.console_confirmed, "{mode:?}");
            for pair in expected.chunks(2) {
                if pair.len() == 1 {
                    assert!(
                        args.iter().any(|argument| argument == pair[0]),
                        "{mode:?} must forward {:?}, got {args:?}",
                        pair[0]
                    );
                } else {
                    assert!(
                        args.windows(2).any(|window| window == pair),
                        "{mode:?} must forward {pair:?}, got {args:?}"
                    );
                }
            }
            if expected.is_empty() {
                assert!(
                    !args.iter().any(|argument| argument == "--repository"),
                    "{mode:?} must not forward --repository, got {args:?}"
                );
            }
        }
    }

    #[test]
    fn update_confirmation_esc_cancels_and_stays_in_panel() {
        let mut app = update_ready_app();
        app.update.confirming = Some(resolved("1.2.3", "1.2.4"));
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.update.confirming.is_none());
        assert_eq!(app.exit_state, ExitState::Idle);
    }

    #[test]
    fn start_update_validates_before_background_resolution() {
        let mut app = update_ready_app();
        app.update.version = "nightly".into();
        app.update.selected = 3;
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(
            app.update.resolving.is_none(),
            "an invalid version must not start the resolver"
        );
        assert!(!app.notice.is_empty());

        let mut app = update_ready_app();
        app.update.repository = UpdateRepositoryMode::Custom;
        app.update.repository_url = "not a url".into();
        app.update.selected = 3;
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(app.update.resolving.is_none());
        assert!(!app.notice.is_empty());

        let mut app = update_ready_app();
        app.install.install_dir = std::env::temp_dir()
            .join(format!("lkit-console-update-{}", std::process::id()))
            .display()
            .to_string();
        app.update.selected = 3;
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(
            app.update.resolving.is_some(),
            "a valid form must start the background resolver"
        );
        let _ = std::fs::remove_dir_all(&app.install.install_dir);
    }

    #[test]
    fn update_load_config_offers_current_source_and_reports_corruption() {
        let dir = std::env::temp_dir().join(format!("lkit-console-config-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let install_dir = dir.display().to_string();

        let mut app = update_ready_app();
        app.install.install_dir = install_dir.clone();
        app.update.current_source = None;

        app.update.load_config(&install_dir);
        assert!(app.update.current_source.is_none());
        assert!(app.update.config_error.is_none());
        assert_eq!(app.update.repository, UpdateRepositoryMode::Github);

        let preset = "schema_version = 1\n\n[repository]\nkind = \"http\"\nlocation = \"https://example.com/releases/\"\n";
        std::fs::write(dir.join("config.toml"), preset).unwrap();
        app.update.load_config(&install_dir);
        assert_eq!(
            app.update.repository,
            UpdateRepositoryMode::Current,
            "a valid config source must become the default option"
        );
        let source = app.update.current_source.as_ref().unwrap();
        assert_eq!(source.kind, RepositorySourceKind::Http);
        assert_eq!(source.location, "https://example.com/releases/");
        assert!(app.update.config_error.is_none());

        std::fs::write(dir.join("config.toml"), "not a config").unwrap();
        app.update.load_config(&install_dir);
        assert!(app.update.current_source.is_none());
        assert!(app.update.config_error.is_some());
        assert_eq!(app.update.repository, UpdateRepositoryMode::Github);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
