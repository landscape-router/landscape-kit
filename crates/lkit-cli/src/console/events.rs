use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::install_form::InstallField;
use super::mirror::MirrorRow;
use super::network_wizard::{NetworkWizard, WizardStep};
use super::preflight::{GateState, PreflightState};
use super::reinit::{ReinitField, ReinitStep};
use super::widgets::{Focus, Hit, Menu};
use super::{ConsoleAction, ConsoleApp, ExitState};

impl ConsoleApp {
    /// 阻塞屏键处理：↑/↓ 或 Tab 选择，Enter 执行，Esc/Ctrl+C 等同"稍后"退出。
    pub(super) fn handle_takeover_pending_key(&mut self, key: KeyEvent) -> Option<ConsoleAction> {
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

    pub(super) fn handle_key(&mut self, key: KeyEvent) -> Option<ConsoleAction> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Some(ConsoleAction::Quit);
        }
        if self.language_switch_available() && language_toggle_key(&key) {
            self.toggle_language();
            return None;
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
        if self.software.install.is_some() {
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
        if self.menu() == Menu::Software
            && self.focus == Focus::Panel
            && let Some(action) = self.handle_software_key(key)
        {
            return action;
        }
        if self.menu() == Menu::Reinit
            && self.focus == Focus::Panel
            && let Some(action) = self.handle_reinit_key(key)
        {
            return action;
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
                    } else if self.install.selected == InstallField::StartInstallation {
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

    pub(super) fn handle_editing_key(&mut self, key: KeyEvent) -> Option<ConsoleAction> {
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

    // 鼠标点击暂时禁用,方法保留供测试直接调用
    #[allow(dead_code)]
    /// 鼠标事件处理:左键命中渲染时收集的可点击区域,按对应键盘语义执行;
    /// 右键视为 Esc;滚轮滚动当前可滚动视图。
    pub(super) fn handle_mouse(
        &mut self,
        mouse: crossterm::event::MouseEvent,
    ) -> Option<ConsoleAction> {
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
                self.install.selected = InstallField::Version;
                None
            }
            Hit::InstallField(field) => {
                self.focus = Focus::Panel;
                self.install.checks_selected = false;
                self.install.editing = false;
                self.install.selected = field;
                self.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            }
            Hit::UpdateField(field) => {
                self.focus = Focus::Panel;
                self.update.editing = false;
                self.update.selected = field;
                self.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            }
            Hit::MirrorField(mirror) => {
                self.focus = Focus::Panel;
                self.mirror.selected = MirrorRow::Mirror(mirror);
                self.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            }
            Hit::MirrorRestore => {
                self.focus = Focus::Panel;
                self.mirror.selected = MirrorRow::Restore;
                self.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            }
            Hit::MirrorSecurityToggle => {
                self.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE))
            }
            Hit::SoftwareField(software) => {
                self.focus = Focus::Panel;
                self.software.selected = Some(software);
                self.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            }
            Hit::SoftwareSourceToggle => {
                self.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE))
            }
            Hit::UninstallAction => {
                self.focus = Focus::Panel;
                self.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            }
            Hit::ReinitField(field) => {
                self.focus = Focus::Panel;
                if self.reinit.step == ReinitStep::Credentials && field != ReinitField::Start {
                    self.reinit.selected = field;
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
    pub(super) fn handle_scroll(&mut self, down: bool) -> Option<ConsoleAction> {
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
}

fn language_toggle_key(key: &KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('l' | 'L'))
        && !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
}
