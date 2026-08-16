use super::ConsoleAction;
use super::backup::{BackupListState, BackupPanel};
use super::install_form::InstallForm;
use super::mirror::MirrorPanel;
use super::network_wizard::{NetworkWizard, Snapshot};
use super::preflight::{Preflight, PreflightState};
use super::reinit;
use super::reinit::ReinitPanel;
use super::software::SoftwarePanel;
use super::update::{UninstallPanel, UpdatePanel};
use super::widgets::{Clicks, Focus, Menu};
use crate::commands::Commands;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ExitState {
    Idle,
    Armed,
    Confirming,
}

pub(super) struct ConsoleApp {
    pub(super) menu_index: usize,
    pub(super) focus: Focus,
    pub(super) install: InstallForm,
    pub(super) snapshot: Snapshot,
    pub(super) notice: String,
    pub(super) exit_state: ExitState,
    pub(super) preflight: Preflight,
    pub(super) preflight_dialog: bool,
    pub(super) network_wizard: Option<NetworkWizard>,
    pub(super) backup: BackupPanel,
    pub(super) backup_menu_active: bool,
    pub(super) update: UpdatePanel,
    pub(super) update_menu_active: bool,
    pub(super) mirror: MirrorPanel,
    pub(super) software: SoftwarePanel,
    pub(super) reinit: ReinitPanel,
    pub(super) uninstall: UninstallPanel,
    pub(super) takeover_choice: usize,
    pub(super) hits: Clicks,
}

impl ConsoleApp {
    pub(super) fn new() -> Self {
        let install = InstallForm::default();
        let snapshot = Snapshot::load();
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
            software: SoftwarePanel::default(),
            reinit: ReinitPanel::default(),
            uninstall: UninstallPanel::default(),
            takeover_choice: 0,
            hits: Clicks::default(),
        }
    }

    pub(super) fn menu(&self) -> Menu {
        Menu::ALL[self.menu_index]
    }

    /// 已安装或存在等待确认的网络接管时，首次安装表单不可用，Install 菜单不可选中。
    pub(super) fn install_available(&self) -> bool {
        !matches!(
            self.snapshot,
            Snapshot::Installed { .. } | Snapshot::AwaitingNetworkConfirmation { .. }
        )
    }

    /// 存在等待确认的网络接管时进入阻塞屏，不渲染菜单。
    pub(super) fn takeover_pending(&self) -> bool {
        matches!(self.snapshot, Snapshot::AwaitingNetworkConfirmation { .. })
    }

    /// 回滚进行中（rolling_back）时确认不可用，只提供"稍后"。
    pub(super) fn takeover_confirm_allowed(&self) -> bool {
        matches!(
            self.snapshot,
            Snapshot::AwaitingNetworkConfirmation { phase, .. } if phase != "rolling_back"
        )
    }

    /// 确认执行：退出 TUI 后按现状 CLI 语义内联运行 `lkit network confirm`。
    pub(super) fn takeover_confirm_action(&self) -> ConsoleAction {
        ConsoleAction::Command {
            command: Commands::Network(crate::commands::network::Network {
                action: crate::commands::network::NetworkAction::Confirm,
                #[cfg(feature = "test-support")]
                test_runtime: None,
            }),
            args: vec!["network".into(), "confirm".into()],
        }
    }

    pub(super) fn menu_available(&self, menu: Menu) -> bool {
        match menu {
            Menu::Install => self.install_available(),
            Menu::Update | Menu::Uninstall => matches!(self.snapshot, Snapshot::Installed { .. }),
            Menu::Reinit => reinit::reinit_eligible(self),
            _ => true,
        }
    }

    pub(super) fn select_next_menu(&mut self) {
        for index in (self.menu_index + 1)..Menu::ALL.len() {
            if self.menu_available(Menu::ALL[index]) {
                self.menu_index = index;
                return;
            }
        }
    }

    pub(super) fn select_previous_menu(&mut self) {
        for index in (0..self.menu_index).rev() {
            if self.menu_available(Menu::ALL[index]) {
                self.menu_index = index;
                return;
            }
        }
    }

    pub(super) fn update(&mut self) {
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
                self.backup.start();
            }
        } else {
            self.backup_menu_active = false;
        }
        self.backup.poll(&mut self.notice);
        if self.menu() == Menu::Update {
            if !self.update_menu_active {
                self.update_menu_active = true;
                self.update.load_config();
            }
        } else {
            self.update_menu_active = false;
        }
        self.update.poll(&mut self.notice);
        if self.menu() == Menu::Mirror {
            self.mirror.ensure_detected();
        }
        if self.menu() == Menu::Software {
            self.software.ensure_detected();
        }
        self.software.poll(&mut self.notice);
    }

    pub(super) fn toggle_language(&mut self) {
        crate::i18n::configure(crate::i18n::current().toggled());
        self.exit_state = ExitState::Idle;
        self.notice = "Ready".into();
        self.snapshot = Snapshot::load();
        if !self.menu_available(self.menu()) {
            self.menu_index = 0;
            self.focus = Focus::Navigation;
        }
        if self.install_available() && !matches!(&self.preflight.state, PreflightState::NotRun) {
            self.preflight.restart();
        }
    }

    pub(super) fn handle_paste(&mut self, value: &str) {
        if self.exit_state == ExitState::Confirming {
            return;
        }
        if let Some(wizard) = self.network_wizard.as_mut() {
            if wizard.editing
                && let Some(target) = wizard.value_mut()
            {
                let remaining = 128_usize.saturating_sub(target.chars().count());
                target.extend(
                    value
                        .chars()
                        .filter(|character| !character.is_control())
                        .take(remaining),
                );
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

    pub(super) fn hints(&self) -> String {
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
        } else if self.menu() == Menu::Software && self.focus == Focus::Panel {
            if self.software.install.is_some() {
                crate::tr!(crate::keys::CONSOLE_SOFTWARE_HINT_RUNNING)
            } else if self.software.confirming.is_some() {
                crate::tr!(crate::keys::CONSOLE_SOFTWARE_HINT_CONFIRM)
            } else {
                crate::tr!(crate::keys::CONSOLE_SOFTWARE_HINT_PANEL)
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

    /// 语言切换在所有非文本编辑、非退出确认状态下可用，包括确认层、详情页、
    /// 部署前检查弹窗与网络向导;编辑字段时 `l` 保持为普通输入字符。
    pub(super) fn language_switch_available(&self) -> bool {
        self.exit_state != ExitState::Confirming
            && !self.install.editing
            && !self.backup.editing
            && !self.update.editing
            && !self.reinit.editing
            && !self
                .network_wizard
                .as_ref()
                .is_some_and(|wizard| wizard.editing)
    }
}
