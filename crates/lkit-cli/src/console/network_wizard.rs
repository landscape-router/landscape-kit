use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use super::render::{display_pad, register_dialog_hits};
use super::widgets::{Clicks, Hit, block_row_of};
use super::{ConsoleAction, ConsoleApp};
use crate::deployment::{root, state};
use crate::network::config::{
    DEFAULT_MANAGEMENT_CIDR, Ipv4Cidr, NetworkMode, NetworkPlan, SelectedInterface, WanIpv4Config,
};
use crate::network::discovery::{self, DefaultRoute, Interface};

impl ConsoleApp {
    pub(crate) fn handle_network_wizard_key(&mut self, key: KeyEvent) -> Option<ConsoleAction> {
        let Some(wizard) = self.network_wizard.as_mut() else {
            return None;
        };
        if wizard.cancel_confirming {
            match key.code {
                KeyCode::Enter => {
                    self.network_wizard = None;
                    self.reinit.wizard = false;
                    self.reinit.step = super::reinit::ReinitStep::Overview;
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
            WizardStep::Confirm => match key.code {
                KeyCode::Enter => {
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
                        self.reinit.step = super::reinit::ReinitStep::Credentials;
                        self.reinit.selected = 0;
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
                _ => {}
            },
        }
        None
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WizardStep {
    Wan,
    WanConfig,
    Lan,
    LanDhcp,
    Confirm,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WanMode {
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

pub(crate) struct NetworkWizard {
    pub(crate) interfaces: Vec<Interface>,
    pub(crate) routes: Vec<DefaultRoute>,
    pub(crate) wan: usize,
    pub(crate) step: WizardStep,
    pub(crate) wan_mode: WanMode,
    pub(crate) address: String,
    pub(crate) gateway: String,
    pub(crate) focus: usize,
    pub(crate) lan_candidates: Vec<Interface>,
    pub(crate) lan_cursor: usize,
    pub(crate) lan_selected: Vec<bool>,
    pub(crate) management: String,
    pub(crate) dhcp_start: String,
    pub(crate) dhcp_end: String,
    pub(crate) editing: bool,
    pub(crate) cancel_confirming: bool,
}

impl NetworkWizard {
    pub(crate) fn discover() -> Result<Self, String> {
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
            focus: 0,
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

    pub(crate) fn selected_wan(&self) -> &Interface {
        &self.interfaces[self.wan]
    }

    pub(crate) fn set_wan(&mut self, wan: usize) {
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
        self.focus = 0;
        self.cancel_confirming = false;
    }

    /// 进入 WAN 模式选择前按与 CLI 相同的发现规则计算默认模式与预填值。
    pub(crate) fn apply_wan_selection(&mut self) {
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
    pub(crate) fn back(&mut self) {
        self.editing = false;
        self.cancel_confirming = false;
        match self.step {
            WizardStep::WanConfig => self.step = WizardStep::Wan,
            WizardStep::Lan => self.step = WizardStep::WanConfig,
            WizardStep::LanDhcp => self.step = WizardStep::Lan,
            WizardStep::Confirm => {
                self.step = if self.lan_selected.iter().any(|selected| *selected) {
                    WizardStep::LanDhcp
                } else {
                    WizardStep::Lan
                };
            }
            WizardStep::Wan => {}
        }
    }

    /// 页面内焦点位置的最大值。
    pub(crate) fn focus_max(&self) -> usize {
        match self.step {
            WizardStep::WanConfig if self.wan_mode == WanMode::Dhcp => 1,
            WizardStep::WanConfig | WizardStep::LanDhcp => 3,
            _ => 0,
        }
    }

    /// 焦点是否落在可编辑字段上。
    pub(crate) fn focus_is_field(&self) -> bool {
        self.is_field_focus(self.focus)
    }

    /// `focus` 位置是否落在可编辑字段上(与 `focus_is_field` 同规则,接受外部值)。
    pub(crate) fn is_field_focus(&self, focus: usize) -> bool {
        match self.step {
            WizardStep::WanConfig => self.wan_mode == WanMode::Static && (1..=2).contains(&focus),
            WizardStep::LanDhcp => focus <= 2,
            _ => false,
        }
    }

    /// 在页面内移动焦点;落到字段上时立即进入编辑。
    pub(crate) fn move_focus(&mut self, up: bool) {
        self.editing = false;
        let max = self.focus_max();
        self.focus = if up {
            self.focus.saturating_sub(1)
        } else {
            (self.focus + 1).min(max)
        };
        self.editing = self.focus_is_field();
    }

    /// 从 LAN 选择进入单页 DHCP 配置:按管理地址的默认池预填 DHCP 范围。
    pub(crate) fn enter_lan_dhcp(&mut self) {
        if self.dhcp_start.is_empty()
            && let Ok(management) = self.management.trim().parse::<Ipv4Cidr>()
            && let Ok((start, end)) = management.default_pool()
        {
            self.dhcp_start = start.to_string();
            self.dhcp_end = end.to_string();
        }
        self.step = WizardStep::LanDhcp;
        self.focus = 0;
        self.editing = true;
    }

    pub(crate) fn validate_wan_static(&self) -> Result<(), String> {
        self.address
            .trim()
            .parse::<Ipv4Cidr>()
            .map_err(|error| error.to_string())?;
        self.gateway
            .trim()
            .parse::<std::net::Ipv4Addr>()
            .map_err(|_| crate::tr!(crate::keys::CONSOLE_INVALID_WAN_GATEWAY))?;
        Ok(())
    }

    pub(crate) fn validate_lan_dhcp(&self) -> Result<(), String> {
        self.management
            .trim()
            .parse::<Ipv4Cidr>()
            .map_err(|error| error.to_string())?;
        self.dhcp_start
            .trim()
            .parse::<std::net::Ipv4Addr>()
            .map_err(|_| crate::tr!(crate::keys::CONSOLE_INVALID_LAN_DHCP_RANGE_START))?;
        self.dhcp_end
            .trim()
            .parse::<std::net::Ipv4Addr>()
            .map_err(|_| crate::tr!(crate::keys::CONSOLE_INVALID_LAN_DHCP_RANGE_END))?;
        Ok(())
    }

    pub(crate) fn value_mut(&mut self) -> Option<&mut String> {
        match self.step {
            WizardStep::WanConfig if self.wan_mode == WanMode::Static && self.focus == 1 => {
                Some(&mut self.address)
            }
            WizardStep::WanConfig if self.wan_mode == WanMode::Static && self.focus == 2 => {
                Some(&mut self.gateway)
            }
            WizardStep::LanDhcp if self.focus == 0 => Some(&mut self.management),
            WizardStep::LanDhcp if self.focus == 1 => Some(&mut self.dhcp_start),
            WizardStep::LanDhcp if self.focus == 2 => Some(&mut self.dhcp_end),
            _ => None,
        }
    }

    pub(crate) fn advance_after_edit(&mut self) -> Result<(), String> {
        match self.step {
            WizardStep::WanConfig if self.wan_mode == WanMode::Static && self.focus == 1 => {
                self.address
                    .trim()
                    .parse::<Ipv4Cidr>()
                    .map_err(|error| error.to_string())?;
                self.focus = 2;
                self.editing = true;
            }
            WizardStep::WanConfig if self.wan_mode == WanMode::Static && self.focus == 2 => {
                self.validate_wan_static()?;
                self.focus = 3;
                self.editing = false;
            }
            WizardStep::LanDhcp if self.focus == 0 => {
                self.management
                    .trim()
                    .parse::<Ipv4Cidr>()
                    .map_err(|error| error.to_string())?;
                self.focus = 1;
                self.editing = true;
            }
            WizardStep::LanDhcp if self.focus == 1 => {
                self.dhcp_start
                    .trim()
                    .parse::<std::net::Ipv4Addr>()
                    .map_err(|_| crate::tr!(crate::keys::CONSOLE_INVALID_LAN_DHCP_RANGE_START))?;
                self.focus = 2;
                self.editing = true;
            }
            WizardStep::LanDhcp if self.focus == 2 => {
                self.dhcp_end
                    .trim()
                    .parse::<std::net::Ipv4Addr>()
                    .map_err(|_| crate::tr!(crate::keys::CONSOLE_INVALID_LAN_DHCP_RANGE_END))?;
                self.focus = 3;
                self.editing = false;
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn plan(&self) -> Result<NetworkPlan, String> {
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

pub(crate) enum Snapshot {
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
    pub(crate) fn load(install_dir: &str) -> Self {
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

    pub(crate) fn badge(&self) -> (String, Color) {
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
pub(crate) fn render_network_wizard(
    frame: &mut Frame<'_>,
    wizard: &NetworkWizard,
    hits: &mut Clicks,
) {
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
    hits.add(body, Hit::WizardContinue);
    let mut lines = Vec::new();
    let mut clickables: Vec<(usize, Hit)> = Vec::new();
    macro_rules! push {
        ($line:expr) => {
            lines.push($line)
        };
    }
    match wizard.step {
        WizardStep::Wan => {
            push!(Line::styled(
                crate::tr!(crate::keys::CONSOLE_SELECT_WAN_INTERFACE),
                Style::default().add_modifier(Modifier::BOLD),
            ));
            push!(Line::raw(""));
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
                clickables.push((lines.len(), Hit::WizardWan(index)));
                push!(Line::styled(
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
        WizardStep::WanConfig => {
            push!(Line::styled(
                crate::tr!(crate::keys::CONSOLE_WAN_IPV4_MODE),
                Style::default().add_modifier(Modifier::BOLD),
            ));
            push!(Line::raw(""));
            let tab_focus = wizard.focus == 0;
            let content_width = body.width.saturating_sub(2);
            let tab_row = block_row_of(&lines, lines.len(), content_width);
            let mut tab_x = body.x.saturating_add(1);
            let mut tab_spans = Vec::new();
            for (mode, label) in [
                (WanMode::Static, crate::tr!(crate::keys::CONSOLE_TAB_STATIC)),
                (WanMode::Dhcp, crate::tr!(crate::keys::CONSOLE_TAB_DHCP)),
            ] {
                let tab_text = format!("[ {label} ]");
                let tab_width = UnicodeWidthStr::width(tab_text.as_str()) as u16;
                hits.add(
                    Rect::new(
                        tab_x,
                        body.y.saturating_add(1).saturating_add(tab_row),
                        tab_width,
                        1,
                    ),
                    Hit::WizardTab(mode),
                );
                tab_x = tab_x.saturating_add(tab_width).saturating_add(2);
                let active = wizard.wan_mode == mode;
                let style = if tab_focus && active {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else if tab_focus || active {
                    Style::default().add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                tab_spans.push(Span::styled(tab_text, style));
                tab_spans.push(Span::raw("  "));
            }
            push!(Line::from(tab_spans));
            push!(Line::raw(""));
            if wizard.wan_mode == WanMode::Static {
                clickables.push((lines.len(), Hit::WizardField(1)));
                push!(wizard_field_row(
                    wizard.focus == 1,
                    wizard.editing,
                    &crate::tr!(crate::keys::CONSOLE_IPV4_ADDRESS_CIDR),
                    &wizard.address,
                ));
                clickables.push((lines.len(), Hit::WizardField(2)));
                push!(wizard_field_row(
                    wizard.focus == 2,
                    wizard.editing,
                    &crate::tr!(crate::keys::CONSOLE_DEFAULT_GATEWAY),
                    &wizard.gateway,
                ));
            } else {
                push!(Line::styled(
                    crate::tr!(crate::keys::CONSOLE_WAN_DHCP_CLIENT_HINT),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            push!(Line::raw(""));
            push!(wizard_confirm_button_row(
                wizard.focus == wizard.focus_max(),
            ));
        }
        WizardStep::Lan => {
            push!(Line::styled(
                crate::tr!(crate::keys::CONSOLE_SELECT_LAN_INTERFACES),
                Style::default().add_modifier(Modifier::BOLD),
            ));
            push!(Line::raw(""));
            if wizard.lan_candidates.is_empty() {
                push!(Line::raw(crate::tr!(
                    crate::keys::CONSOLE_NO_OTHER_INTERFACES
                )));
            }
            for (index, iface) in wizard.lan_candidates.iter().enumerate() {
                let cursor = index == wizard.lan_cursor;
                clickables.push((lines.len(), Hit::WizardLan(index)));
                push!(Line::styled(
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
        WizardStep::LanDhcp => {
            push!(Line::styled(
                crate::tr!(crate::keys::CONSOLE_LAN_DHCP_CONFIGURATION),
                Style::default().add_modifier(Modifier::BOLD),
            ));
            push!(Line::raw(""));
            clickables.push((lines.len(), Hit::WizardField(0)));
            push!(wizard_field_row(
                wizard.focus == 0,
                wizard.editing,
                &crate::tr!(crate::keys::CONSOLE_LAN_MANAGEMENT_IPV4_ADDRESS),
                &wizard.management,
            ));
            clickables.push((lines.len(), Hit::WizardField(1)));
            push!(wizard_field_row(
                wizard.focus == 1,
                wizard.editing,
                &crate::tr!(crate::keys::CONSOLE_LAN_DHCP_RANGE_START),
                &wizard.dhcp_start,
            ));
            clickables.push((lines.len(), Hit::WizardField(2)));
            push!(wizard_field_row(
                wizard.focus == 2,
                wizard.editing,
                &crate::tr!(crate::keys::CONSOLE_LAN_DHCP_RANGE_END),
                &wizard.dhcp_end,
            ));
            push!(Line::raw(""));
            push!(wizard_confirm_button_row(wizard.focus == 3));
        }
        WizardStep::Confirm => {
            let wan = wizard.selected_wan();
            push!(Line::styled(
                crate::tr!(crate::keys::CONSOLE_CONFIRM_NETWORK_TAKEOVER_PLAN),
                Style::default().add_modifier(Modifier::BOLD),
            ));
            push!(Line::raw(""));
            push!(Line::raw(crate::tr!(
                crate::keys::CONSOLE_CONFIRM_WAN_INTERFACE,
                name = wan.name,
                mac = wan.mac
            )));
            push!(Line::raw(match wizard.wan_mode {
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
                push!(Line::raw(crate::tr!(
                    crate::keys::CONSOLE_CONFIRM_LAN_MODE_WAN_ONLY
                )));
            } else {
                let names = lan.join(", ");
                push!(Line::raw(crate::tr!(
                    crate::keys::CONSOLE_CONFIRM_LAN_INTERFACES,
                    names = names
                )));
                push!(Line::raw(crate::tr!(
                    crate::keys::CONSOLE_CONFIRM_MANAGEMENT,
                    management = wizard.management
                )));
                push!(Line::raw(crate::tr!(
                    crate::keys::CONSOLE_CONFIRM_DHCP_RANGE,
                    start = wizard.dhcp_start,
                    end = wizard.dhcp_end
                )));
            }
            push!(Line::raw(""));
            push!(Line::styled(
                crate::tr!(crate::keys::CONSOLE_CONFIRM_LAN_FLUSH_NOTE),
                Style::default().fg(Color::Yellow),
            ));
            push!(Line::styled(
                crate::tr!(crate::keys::CONSOLE_PRESS_ENTER_TO_START_INSTALLATION),
                Style::default().add_modifier(Modifier::BOLD),
            ));
        }
    }
    let content_width = body.width.saturating_sub(2);
    for (index, hit) in clickables {
        hits.block_row(body, block_row_of(&lines, index, content_width), hit);
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
        render_wizard_cancel_confirmation(frame, hits);
    }
}

fn wizard_field_row(focused: bool, editing: bool, label: &str, value: &str) -> Line<'static> {
    let style = if focused {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let marker = if focused && editing { "_" } else { "" };
    Line::from(vec![
        Span::styled(if focused { "> " } else { "  " }, style),
        Span::styled(display_pad(label, 20), style),
        Span::styled(format!("{value}{marker}"), style),
    ])
}

fn wizard_confirm_button_row(focused: bool) -> Line<'static> {
    let style = if focused {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD)
    };
    Line::from(vec![
        Span::styled(if focused { "> " } else { "  " }, style),
        Span::styled(
            format!(
                "[ {} ]",
                crate::tr!(crate::keys::CONSOLE_CONFIRM_AND_CONTINUE)
            ),
            style,
        ),
    ])
}

fn wizard_hints(wizard: &NetworkWizard) -> String {
    if wizard.cancel_confirming {
        return crate::tr!(crate::keys::CONSOLE_WIZARD_HINT_CANCEL);
    }
    match wizard.step {
        WizardStep::Wan => crate::tr!(crate::keys::CONSOLE_WIZARD_HINT_WAN),
        WizardStep::WanConfig => crate::tr!(crate::keys::CONSOLE_WIZARD_HINT_CONFIG),
        WizardStep::Lan => crate::tr!(crate::keys::CONSOLE_WIZARD_HINT_LAN),
        WizardStep::LanDhcp => crate::tr!(crate::keys::CONSOLE_WIZARD_HINT_EDIT),
        WizardStep::Confirm => crate::tr!(crate::keys::CONSOLE_WIZARD_HINT_CONFIRM),
    }
}

fn render_wizard_cancel_confirmation(frame: &mut Frame<'_>, hits: &mut Clicks) {
    let screen = frame.area();
    let width = 52.min(screen.width.saturating_sub(2));
    let height = 7.min(screen.height.saturating_sub(2));
    let area = Rect::new(
        screen.x + screen.width.saturating_sub(width) / 2,
        screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    );
    register_dialog_hits(hits, screen, area);
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
pub(crate) fn render_pending_takeover(frame: &mut Frame<'_>, app: &mut ConsoleApp) {
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
    let later_row = lines.len();
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
    let confirm_row = lines.len();
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
    let content_width = area.width.saturating_sub(2);
    app.hits.block_row(
        area,
        block_row_of(&lines, later_row, content_width),
        Hit::TakeoverChoice(0),
    );
    app.hits.block_row(
        area,
        block_row_of(&lines, confirm_row, content_width),
        Hit::TakeoverChoice(1),
    );
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines).alignment(Alignment::Center).block(
            Block::bordered().title(crate::tr!(crate::keys::CONSOLE_TAKEOVER_PENDING_WINDOW)),
        ),
        area,
    );
}
