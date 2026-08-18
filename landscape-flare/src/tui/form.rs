use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use landscape_terrain_proto::cli::parse_ethertype;
use landscape_terrain_proto::transport;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Field {
    Psk,
    User,
    ClientName,
    Device,
    Ethertype,
    Token,
    Connect,
}

impl Field {
    fn next(self) -> Self {
        match self {
            Self::Psk => Self::User,
            Self::User => Self::ClientName,
            Self::ClientName => Self::Device,
            Self::Device => Self::Ethertype,
            Self::Ethertype => Self::Token,
            Self::Token => Self::Connect,
            Self::Connect => Self::Psk,
        }
    }

    fn prev(self) -> Self {
        match self {
            Self::Psk => Self::Connect,
            Self::User => Self::Psk,
            Self::ClientName => Self::User,
            Self::Device => Self::ClientName,
            Self::Ethertype => Self::Device,
            Self::Token => Self::Ethertype,
            Self::Connect => Self::Token,
        }
    }
}

pub(super) struct FormState {
    pub(super) focus: Field,
    pub(super) psk: String,
    pub(super) show_psk: bool,
    pub(super) user: String,
    pub(super) client_name: String,
    pub(super) device: String,
    pub(super) device_index: usize,
    pub(super) device_selecting: bool,
    pub(super) devices: Vec<String>,
    pub(super) devices_err: Option<String>,
    pub(super) ethertype: String,
    pub(super) token: String,
    pub(super) error: Option<String>,
}

impl FormState {
    pub(super) fn new() -> Self {
        let (devices, devices_err) = match transport::list_interfaces() {
            Ok(mut devices) => {
                devices.sort();
                (devices, None)
            }
            Err(error) => (
                Vec::new(),
                Some(crate::tr!("tui.list_interfaces_failed", error = error)),
            ),
        };
        Self::from_devices(devices, devices_err)
    }

    pub(super) fn from_devices(devices: Vec<String>, devices_err: Option<String>) -> Self {
        Self {
            focus: Field::Psk,
            psk: String::new(),
            show_psk: false,
            user: "admin".into(),
            client_name: "pc".into(),
            device: String::new(),
            device_index: 0,
            device_selecting: false,
            devices,
            devices_err,
            ethertype: "0x88b6".into(),
            token: String::new(),
            error: None,
        }
    }

    pub(super) fn device_options(&self) -> Vec<String> {
        let mut options = vec![crate::tr!("tui.auto_device")];
        options.extend(self.devices.iter().cloned());
        options
    }

    pub(super) fn device_label(&self) -> String {
        if self.device.is_empty() {
            crate::tr!("tui.auto_device")
        } else {
            self.device.clone()
        }
    }

    pub(super) fn build(&self) -> Result<OwnedConfig, String> {
        let psk = self.psk.trim().to_string();
        if psk.is_empty() {
            return Err(crate::tr!("tui.psk_required"));
        }
        let ethertype = parse_ethertype(self.ethertype.trim())
            .map_err(|error| crate::tr!("tui.invalid_ethertype", error = error))?;
        let devs = if self.device.is_empty() {
            Vec::new()
        } else {
            vec![self.device.clone()]
        };
        Ok(OwnedConfig {
            devs,
            user: self.user.trim().to_string(),
            client_name: self.client_name.trim().to_string(),
            psk,
            ethertype,
            token: self.token.trim().to_string(),
        })
    }
}

#[derive(Debug)]
pub(super) struct OwnedConfig {
    pub(super) devs: Vec<String>,
    pub(super) user: String,
    pub(super) client_name: String,
    pub(super) psk: String,
    pub(super) ethertype: u16,
    pub(super) token: String,
}

#[derive(PartialEq, Debug)]
pub(super) enum FormAction {
    None,
    Connect,
    Quit,
}

pub(super) fn handle_key(form: &mut FormState, key: KeyEvent) -> FormAction {
    if key.kind != KeyEventKind::Press {
        return FormAction::None;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let editable = !ctrl && !key.modifiers.contains(KeyModifiers::ALT);

    match key.code {
        KeyCode::Char('c') if ctrl => FormAction::Quit,
        KeyCode::Char('q') if ctrl => FormAction::Quit,
        KeyCode::Char('l' | 'L')
            if !ctrl
                && !key.modifiers.contains(KeyModifiers::ALT)
                && matches!(form.focus, Field::Device | Field::Connect) =>
        {
            crate::i18n::toggle();
            FormAction::None
        }
        KeyCode::Esc if !form.device_selecting => FormAction::Quit,
        KeyCode::F(2) if form.focus == Field::Psk => {
            form.show_psk = !form.show_psk;
            FormAction::None
        }
        KeyCode::Tab => {
            form.device_selecting = false;
            form.focus = form.focus.next();
            FormAction::None
        }
        KeyCode::BackTab => {
            form.device_selecting = false;
            form.focus = form.focus.prev();
            FormAction::None
        }
        KeyCode::Down => {
            if form.focus == Field::Device && form.device_selecting {
                let options = form.device_options();
                form.device_index = (form.device_index + 1).min(options.len() - 1);
                form.device = if form.device_index == 0 {
                    String::new()
                } else {
                    options[form.device_index].clone()
                };
            } else {
                form.device_selecting = false;
                form.focus = form.focus.next();
            }
            FormAction::None
        }
        KeyCode::Up => {
            if form.focus == Field::Device && form.device_selecting {
                form.device_index = form.device_index.saturating_sub(1);
                let options = form.device_options();
                form.device = if form.device_index == 0 {
                    String::new()
                } else {
                    options[form.device_index].clone()
                };
            } else {
                form.device_selecting = false;
                form.focus = form.focus.prev();
            }
            FormAction::None
        }
        KeyCode::Enter => {
            if form.focus == Field::Connect {
                return FormAction::Connect;
            }
            if form.focus == Field::Device {
                form.device_selecting = !form.device_selecting;
            } else {
                form.focus = form.focus.next();
            }
            FormAction::None
        }
        KeyCode::Esc => {
            form.device_selecting = false;
            FormAction::None
        }
        KeyCode::Backspace => {
            match form.focus {
                Field::Psk => {
                    form.psk.pop();
                }
                Field::User => {
                    form.user.pop();
                }
                Field::ClientName => {
                    form.client_name.pop();
                }
                Field::Ethertype => {
                    form.ethertype.pop();
                }
                Field::Token => {
                    form.token.pop();
                }
                Field::Device | Field::Connect => {}
            }
            FormAction::None
        }
        KeyCode::Char('u') if ctrl => {
            match form.focus {
                Field::Psk => form.psk.clear(),
                Field::User => form.user.clear(),
                Field::ClientName => form.client_name.clear(),
                Field::Ethertype => form.ethertype.clear(),
                Field::Token => form.token.clear(),
                Field::Device | Field::Connect => {}
            }
            FormAction::None
        }
        KeyCode::Char(c) if editable => {
            match form.focus {
                Field::Psk => form.psk.push(c),
                Field::User => form.user.push(c),
                Field::ClientName => form.client_name.push(c),
                Field::Ethertype => form.ethertype.push(c),
                Field::Token => form.token.push(c),
                Field::Device | Field::Connect => {}
            }
            FormAction::None
        }
        _ => FormAction::None,
    }
}
