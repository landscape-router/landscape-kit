mod backup;
mod install_form;
mod mirror;
mod network_wizard;
mod preflight;
mod reinit;
mod render;
mod update;
mod widgets;

use self::mirror::MirrorPanel;
use self::render::render;
use backup::{BackupListState, BackupPanel};
use install_form::InstallForm;
use network_wizard::{NetworkWizard, Snapshot, WizardStep};
use preflight::{GateState, Preflight, PreflightState};
use reinit::{ReinitPanel, ReinitStep};
use update::{UninstallPanel, UpdatePanel};
use widgets::{Clicks, Focus, Hit, Menu};

use std::io::{IsTerminal, Stdout};
use std::path::PathBuf;
use std::time::Duration;

use crossterm::cursor::{Hide, MoveTo, Show};
use crossterm::event::{
    self,
    Event,
    KeyCode,
    KeyEvent,
    KeyEventKind,
    KeyModifiers,
    // 鼠标事件导入暂时不使用
    // DisableMouseCapture, EnableMouseCapture, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    Clear as ClearScreen, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
    enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::commands::Commands;

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
            .draw(|frame| render(frame, &mut app))
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
            // 鼠标点击暂时禁用:忽略鼠标事件,终端不再捕获鼠标
            Event::Mouse(_) => {}
            // Event::Mouse(mouse) => {
            //     if let Some(action) = app.handle_mouse(mouse) {
            //         return Ok(action);
            //     }
            // }
            Event::Resize(_, _) | Event::FocusGained | Event::FocusLost => {}
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
            // EnableMouseCapture, // 鼠标捕获暂时禁用
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
            // DisableMouseCapture, // 鼠标捕获暂时禁用
            ClearScreen(ClearType::All),
            MoveTo(0, 0),
            LeaveAlternateScreen
        );
        let _ = self.terminal.show_cursor();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExitState {
    Idle,
    Armed,
    Confirming,
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
    mirror: MirrorPanel,
    reinit: ReinitPanel,
    uninstall: UninstallPanel,
    takeover_choice: usize,
    hits: Clicks,
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
            mirror: MirrorPanel::default(),
            reinit: ReinitPanel::default(),
            uninstall: UninstallPanel::default(),
            takeover_choice: 0,
            hits: Clicks::default(),
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
            Menu::Update | Menu::Uninstall => matches!(self.snapshot, Snapshot::Installed { .. }),
            Menu::Reinit => reinit::reinit_eligible(self),
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
        if self.menu() == Menu::Mirror {
            self.mirror.ensure_detected();
        }
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
        if self.menu() == Menu::Uninstall
            && self.focus == Focus::Panel
            && let Some(action) = self.handle_uninstall_key(key)
        {
            return action;
        }
        if self.menu() == Menu::Mirror
            && self.focus == Focus::Panel
            && let Some(action) = self.handle_mirror_key(key)
        {
            return action;
        }
        if self.menu() == Menu::Reinit
            && self.focus == Focus::Panel
            && let Some(action) = self.handle_reinit_key(key)
        {
            return action;
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
                    } else if self.install.selected == 8 {
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

    // 鼠标点击暂时禁用,方法保留供测试直接调用
    #[allow(dead_code)]
    /// 鼠标事件处理:左键命中渲染时收集的可点击区域,按对应键盘语义执行;
    /// 右键视为 Esc;滚轮滚动当前可滚动视图。
    fn handle_mouse(&mut self, mouse: crossterm::event::MouseEvent) -> Option<ConsoleAction> {
        match mouse.kind {
            crossterm::event::MouseEventKind::ScrollUp => return self.handle_scroll(false),
            crossterm::event::MouseEventKind::ScrollDown => return self.handle_scroll(true),
            crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Right) => {
                self.focus = Focus::Panel;
                return self.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
            }
            crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left) => {}
            _ => return None,
        }
        if self.exit_state == ExitState::Armed {
            self.exit_state = ExitState::Idle;
            self.notice = "Ready".into();
        }
        let Some(hit) = self.hits.hit_at(mouse.column, mouse.row) else {
            return None;
        };
        match hit {
            Hit::Nothing => None,
            Hit::Outside => {
                self.focus = Focus::Panel;
                self.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
            }
            Hit::DialogConfirm => {
                self.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            }
            Hit::Navigation => {
                self.focus = Focus::Navigation;
                None
            }
            Hit::Panel => {
                self.focus = Focus::Panel;
                None
            }
            Hit::Menu(index) => {
                if !self.menu_available(Menu::ALL[index]) {
                    return None;
                }
                self.menu_index = index;
                self.focus = Focus::Panel;
                None
            }
            Hit::InstallChecks => {
                self.focus = Focus::Panel;
                self.install.checks_selected = true;
                self.install.editing = false;
                self.install.selected = 0;
                None
            }
            Hit::InstallField(index) => {
                self.focus = Focus::Panel;
                self.install.checks_selected = false;
                self.install.editing = false;
                self.install.selected = index;
                self.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            }
            Hit::UpdateField(index) => {
                self.focus = Focus::Panel;
                self.update.editing = false;
                self.update.selected = index;
                self.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            }
            Hit::MirrorField(index) => {
                self.focus = Focus::Panel;
                self.mirror.selected = index;
                self.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            }
            Hit::MirrorRestore => {
                self.focus = Focus::Panel;
                self.mirror.selected = 4;
                self.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            }
            Hit::MirrorSecurityToggle => {
                self.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE))
            }
            Hit::UninstallAction => {
                self.focus = Focus::Panel;
                self.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            }
            Hit::ReinitField(index) => {
                self.focus = Focus::Panel;
                if self.reinit.step == ReinitStep::Credentials && index < 3 {
                    self.reinit.selected = index;
                    self.reinit.editing = true;
                }
                None
            }
            Hit::ReinitAction => {
                self.focus = Focus::Panel;
                self.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            }
            Hit::BackupRow(index) => {
                self.focus = Focus::Panel;
                self.backup.selected = index;
                self.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            }
            Hit::WizardWan(index) => {
                let Some(wizard) = self.network_wizard.as_mut() else {
                    return None;
                };
                if wizard.step != WizardStep::Wan || wizard.cancel_confirming {
                    return None;
                }
                wizard.set_wan(index);
                self.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            }
            Hit::WizardTab(mode) => {
                let Some(wizard) = self.network_wizard.as_mut() else {
                    return None;
                };
                if wizard.step != WizardStep::WanConfig || wizard.cancel_confirming {
                    return None;
                }
                wizard.wan_mode = mode;
                wizard.focus = 0;
                wizard.editing = false;
                None
            }
            Hit::WizardField(focus) => {
                let Some(wizard) = self.network_wizard.as_mut() else {
                    return None;
                };
                if wizard.cancel_confirming || !wizard.is_field_focus(focus) {
                    return None;
                }
                wizard.focus = focus;
                wizard.editing = true;
                None
            }
            Hit::WizardLan(index) => {
                let Some(wizard) = self.network_wizard.as_mut() else {
                    return None;
                };
                if wizard.step != WizardStep::Lan || wizard.cancel_confirming {
                    return None;
                }
                if index >= wizard.lan_selected.len() {
                    return None;
                }
                wizard.lan_cursor = index;
                wizard.lan_selected[index] = !wizard.lan_selected[index];
                None
            }
            Hit::WizardContinue => {
                let Some(wizard) = self.network_wizard.as_mut() else {
                    return None;
                };
                if wizard.cancel_confirming {
                    return None;
                }
                match wizard.step {
                    WizardStep::WanConfig | WizardStep::LanDhcp => {
                        wizard.focus = wizard.focus_max();
                        wizard.editing = false;
                    }
                    _ => {}
                }
                self.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            }
            Hit::TakeoverChoice(index) => {
                if index == 0 {
                    self.takeover_choice = 0;
                    return None;
                }
                if !self.takeover_confirm_allowed() {
                    return None;
                }
                self.takeover_choice = 1;
                self.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            }
        }
    }

    // 鼠标点击暂时禁用,方法保留供测试直接调用
    #[allow(dead_code)]
    fn handle_scroll(&mut self, down: bool) -> Option<ConsoleAction> {
        if self.preflight.expanded && self.menu() == Menu::Install {
            if down {
                self.preflight.scroll_down(1);
            } else {
                self.preflight.scroll = self.preflight.scroll.saturating_sub(1);
            }
            return None;
        }
        if self.backup.details.is_some() {
            if down {
                self.backup.details_scroll = self.backup.details_scroll.saturating_add(1);
            } else {
                self.backup.details_scroll = self.backup.details_scroll.saturating_sub(1);
            }
        }
        None
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
        } else if self.menu() == Menu::Uninstall && self.focus == Focus::Panel {
            if self.uninstall.confirming {
                crate::tr!(crate::keys::CONSOLE_UNINSTALL_HINT_CONFIRM)
            } else {
                crate::tr!(crate::keys::CONSOLE_UNINSTALL_HINT_PANEL)
            }
        } else if self.menu() == Menu::Mirror && self.focus == Focus::Panel {
            if self.mirror.confirming.is_some() {
                crate::tr!(crate::keys::CONSOLE_MIRROR_HINT_CONFIRM)
            } else {
                crate::tr!(crate::keys::CONSOLE_MIRROR_HINT_PANEL)
            }
        } else if self.menu() == Menu::Reinit && self.focus == Focus::Panel {
            if self.reinit.confirming {
                crate::tr!(crate::keys::CONSOLE_REINIT_HINT_CONFIRM)
            } else if self.reinit.editing {
                crate::tr!(crate::keys::CONSOLE_HINT_CTRL_C_EXIT_EDIT)
            } else {
                crate::tr!(crate::keys::CONSOLE_REINIT_HINT_PANEL)
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
}

fn language_toggle_key(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('l' | 'L'))
        && !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
}

#[cfg(test)]
mod tests;
