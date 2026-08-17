mod keys;
mod render;

use ratatui::style::Color;

use crate::deployment::state;
use crate::network::config::{
    DEFAULT_MANAGEMENT_CIDR, Ipv4Cidr, NetworkMode, NetworkPlan, SelectedInterface, WanIpv4Config,
};
use crate::network::discovery::{self, DefaultRoute, Interface};

pub(crate) use self::render::{render_network_wizard, render_pending_takeover};

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
    pub(crate) fn load() -> Self {
        if unsafe { libc::geteuid() } != 0 {
            return Self::RootRequired;
        }
        let result = (|| -> Result<Self, crate::deployment::plan::InstallError> {
            // 待确认的接管安装尚未提交状态:与 `lkit network`/daemon 恢复相同,
            // 先从已提交状态发现根,失败再从未完成事务发现(见 takeover.rs)。
            let root = match crate::deployment::state::discover_landscape_root() {
                Ok(Some(root)) => root,
                _ => {
                    match crate::deployment::state::discover_landscape_root_from_unfinished_transaction()
                    {
                        Ok(Some(root)) => root,
                        _ => return Ok(Self::NotInstalled),
                    }
                }
            };
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
                    manager: "systemd",
                    initialized: installed.initialization.status == state::InitStatus::Complete,
                },
            })
        })();
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
