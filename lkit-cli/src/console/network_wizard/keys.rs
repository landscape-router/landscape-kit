use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::super::{ConsoleAction, ConsoleApp};
use super::{WanMode, WizardStep};

impl ConsoleApp {
    pub(crate) fn handle_network_wizard_key(&mut self, key: KeyEvent) -> Option<ConsoleAction> {
        let wizard = self.network_wizard.as_mut()?;
        if wizard.cancel_confirming {
            match key.code {
                KeyCode::Enter => {
                    self.network_wizard = None;
                    self.reinit.wizard = false;
                    self.reinit.step = super::super::reinit::ReinitStep::Overview;
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
        if wizard.editing {
            match key.code {
                KeyCode::Up | KeyCode::Down => {
                    wizard.move_focus(key.code == KeyCode::Up);
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
                    wizard.step = WizardStep::WanConfig;
                    wizard.focus = 0;
                }
                _ => {}
            },
            WizardStep::WanConfig => match key.code {
                KeyCode::Up | KeyCode::Down => wizard.move_focus(key.code == KeyCode::Up),
                KeyCode::Left | KeyCode::Right if wizard.focus == 0 => {
                    wizard.wan_mode = wizard.wan_mode.toggle();
                }
                KeyCode::Enter if wizard.focus == 0 => wizard.move_focus(false),
                KeyCode::Enter if wizard.focus == wizard.focus_max() => {
                    if wizard.wan_mode == WanMode::Static
                        && let Err(error) = wizard.validate_wan_static()
                    {
                        self.notice = error;
                        return None;
                    }
                    wizard.step = WizardStep::Lan;
                    wizard.focus = 0;
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
                    if wizard.lan_selected.iter().any(|selected| *selected) {
                        wizard.enter_lan_dhcp();
                    } else {
                        wizard.step = WizardStep::Confirm;
                        wizard.editing = false;
                    }
                }
                _ => {}
            },
            WizardStep::LanDhcp => match key.code {
                KeyCode::Up | KeyCode::Down => wizard.move_focus(key.code == KeyCode::Up),
                KeyCode::Enter if wizard.focus == wizard.focus_max() => {
                    if let Err(error) = wizard.validate_lan_dhcp() {
                        self.notice = error;
                        return None;
                    }
                    wizard.step = WizardStep::Confirm;
                    wizard.focus = 0;
                }
                _ => {}
            },
            WizardStep::Confirm => {
                if key.code == KeyCode::Enter {
                    let plan = match wizard.plan() {
                        Ok(plan) => plan,
                        Err(error) => {
                            self.notice = error;
                            return None;
                        }
                    };
                    if self.reinit.wizard {
                        self.network_wizard = None;
                        self.reinit.wizard = false;
                        self.reinit.plan = Some(plan);
                        self.reinit.step = super::super::reinit::ReinitStep::Credentials;
                        self.reinit.selected = super::super::reinit::ReinitField::AdminUser;
                        self.reinit.editing = false;
                        self.notice = crate::tr!(crate::keys::CONSOLE_REINIT_ENTER_CREDENTIALS);
                        return None;
                    }
                    match self.install.command_with_network_plan(Some(plan)) {
                        Ok(action) => {
                            self.network_wizard = None;
                            return Some(action);
                        }
                        Err(error) => self.notice = error,
                    }
                }
            }
        }
        None
    }
}
