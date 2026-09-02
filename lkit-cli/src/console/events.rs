use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::sync::atomic::Ordering;

use super::daemon_panel::PskDialogField;
use super::install_form::InstallField;
use super::mirror::MirrorRow;
use super::network_wizard::{NetworkWizard, WizardStep};
use super::preflight::{GateState, PreflightState};
use super::reinit::{ReinitField, ReinitStep};
use super::software::SoftwareRow;
use super::widgets::{Focus, Hit, Menu};
use super::{ConsoleAction, ConsoleApp, ExitState, Notice};

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
        if self.software.cancel_confirming {
            // 取消确认层:Enter 确认取消(置位标志终止 worker),Esc 关闭继续安装。
            match key.code {
                KeyCode::Enter => {
                    if let Some(run) = &self.software.install {
                        run.cancel.store(true, Ordering::Relaxed);
                    }
                    self.software.cancel_confirming = false;
                }
                KeyCode::Esc => self.software.cancel_confirming = false,
                _ => {}
            }
            return None;
        }
        if self.software.install.is_some() {
            // 安装进行中:Esc 打开取消确认层,其余按键忽略。
            if key.code == KeyCode::Esc {
                self.software.cancel_confirming = true;
            }
            return None;
        }
        if self.software.base_cancel_confirming {
            // 基础包安装取消确认层:Enter 确认取消(置位标志终止 worker),
            // Esc 关闭继续安装。
            match key.code {
                KeyCode::Enter => {
                    if let Some(run) = &self.software.base_install {
                        run.cancel.store(true, Ordering::Relaxed);
                    }
                    self.software.base_cancel_confirming = false;
                }
                KeyCode::Esc => self.software.base_cancel_confirming = false,
                _ => {}
            }
            return None;
        }
        if self.software.base_install.is_some() {
            // 基础包安装进行中:Esc 打开取消确认层,其余按键忽略。
            if key.code == KeyCode::Esc {
                self.software.base_cancel_confirming = true;
            }
            return None;
        }
        if self.menu() == Menu::Software
            && matches!(
                &self.software.base_packages,
                super::software::BasePackagesState::Choosing { .. }
            )
        {
            // 基础包弹框消费全部按键(与 focus 无关)。
            let _ = self.handle_software_key(key);
            return None;
        }
        if self.deploy_daemon.is_some() {
            return None;
        }
        if self.flare.open {
            return self.handle_flare_dialog_key(key);
        }
        if self.show_psk {
            self.handle_show_psk_key(key);
            return None;
        }
        // 部署确认弹窗可从 Overview 动作行或安装阻断弹框发起,消费全部按键。
        if self.deploy_daemon_confirming {
            self.handle_deploy_psk_key(key);
            return None;
        }
        if self.preflight_dialog {
            match key.code {
                // daemon 未运行被阻断时 Enter 打开「部署 daemon」确认弹窗
                // (内嵌急救恢复码输入与二次确认),确认后在后台执行
                // `lkit self install`,完成后预检自动重跑并放行。
                KeyCode::Enter if self.preflight_daemon_blocked() => {
                    self.preflight_dialog = false;
                    self.open_deploy_dialog();
                }
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
                // daemon 未运行被阻断时直接部署:D 与 Enter 同义。
                KeyCode::Char('d' | 'D') if self.preflight_daemon_blocked() => {
                    self.preflight_dialog = false;
                    self.open_deploy_dialog();
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
                    self.notice = Notice::Ready;
                }
                _ => {}
            }
            return None;
        }
        if self.menu() == Menu::Backup
            && self.focus == Focus::Panel
            && let Some(action) = self.handle_backup_key(key)
        {
            return action;
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
        if self.menu() == Menu::Overview
            && self.focus == Focus::Panel
            && let Some(action) = self.handle_overview_key(key)
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
            // 面板内 Esc 返回主菜单选择;退出热键只在导航层生效。各面板的
            // 确认层/编辑态已在各自的处理器中先行消费 Esc,不会走到这里。
            if self.focus == Focus::Panel {
                self.exit_state = ExitState::Idle;
                self.focus = Focus::Navigation;
                return None;
            }
            match self.exit_state {
                ExitState::Idle => {
                    self.exit_state = ExitState::Armed;
                    self.notice =
                        Notice::Info("Exit armed - press Esc again for confirmation".into());
                }
                ExitState::Armed => {
                    self.exit_state = ExitState::Confirming;
                    self.notice = Notice::Ready;
                }
                ExitState::Confirming => unreachable!(),
            }
            return None;
        }
        if self.exit_state == ExitState::Armed {
            self.exit_state = ExitState::Idle;
            self.notice = Notice::Ready;
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
            KeyCode::Left if self.focus == Focus::Panel => {
                // Install/Update 的枚举字段保留 Left/Right 切换值(Update 在自己的
                // 处理器中消费);其余面板没有左右语义,Left 与 Right(进入面板)
                // 对称,返回侧栏导航,等同 Esc。
                if self.menu() == Menu::Install && !self.install.checks_selected {
                    self.install.change_choice(false);
                } else {
                    self.exit_state = ExitState::Idle;
                    self.focus = Focus::Navigation;
                }
            }
            KeyCode::Right
                if self.focus == Focus::Panel
                    && self.menu() == Menu::Install
                    && self.install.checks_selected =>
            {
                self.preflight.expanded = true;
            }
            // 仅 Install 面板把 Right 用作切换枚举;其他面板 Right 无动作,
            // 不得触碰 Install 表单状态。
            KeyCode::Right if self.focus == Focus::Panel && self.menu() == Menu::Install => {
                self.install.change_choice(true)
            }
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
                                                self.notice = Notice::Info(crate::tr!(
                                                    crate::keys::CONSOLE_CONFIGURE_NETWORK_TAKEOVER
                                                ));
                                            }
                                            Err(error) => self.notice = Notice::Error(error),
                                        },
                                        Err(error) => self.notice = Notice::Error(error),
                                    }
                                } else {
                                    match self.install.activate() {
                                        Ok(Some(action)) => return Some(action),
                                        Ok(None) => self.notice = Notice::Ready,
                                        Err(error) => self.notice = Notice::Error(error),
                                    }
                                }
                            }
                            GateState::Waiting => {
                                self.notice = Notice::Info(crate::tr!(
                                    crate::keys::CONSOLE_ENVIRONMENT_CHECKS_NOT_COMPLETED
                                ));
                            }
                            GateState::Dialog => self.preflight_dialog = true,
                        }
                    } else {
                        match self.install.activate() {
                            Ok(Some(action)) => return Some(action),
                            Ok(None) => self.notice = Notice::Ready,
                            Err(error) => self.notice = Notice::Error(error),
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

    /// Overview 面板键处理:daemon 未运行时 Enter 打开「部署 daemon」确认弹窗
    /// (内嵌急救恢复码输入与二次确认,输入不一致拒绝部署),方向键/Tab 在
    /// psk、确认与「开始部署」动作行间移动,Enter 在字段上进入编辑、在动作行上
    /// 后台执行 `lkit self install`(留在 TUI 内,不退出)。daemon 运行时
    /// Enter/空格打开「查看/修改急救恢复码」弹窗。`f` 打开 flare 恢复通道弹窗。
    /// 其余按键返回 `None` 交给通用处理(保持 Esc 返回菜单选择等标准语义)。
    pub(super) fn handle_overview_key(&mut self, key: KeyEvent) -> Option<Option<ConsoleAction>> {
        match key.code {
            KeyCode::Enter | KeyCode::Char(' ') if self.daemon_deploy_available() => {
                self.open_deploy_dialog();
                Some(None)
            }
            // daemon 运行时 Enter/空格打开「查看/修改急救恢复码」弹窗。
            KeyCode::Enter | KeyCode::Char(' ') => {
                self.open_show_psk();
                Some(None)
            }
            KeyCode::Char('f' | 'F') => {
                self.open_flare_dialog();
                Some(None)
            }
            _ => None,
        }
    }

    /// 部署确认弹窗键处理:方向键/Tab 在 psk、二次确认与「开始部署」动作行间
    /// 移动;Enter 在输入字段上进入编辑(编辑中 Enter/Esc 提交),在动作行上
    /// 校验并启动部署;直接输入即进入编辑;Esc 关闭弹窗。
    fn handle_deploy_psk_key(&mut self, key: KeyEvent) {
        if self.deploy_psk_editing {
            match key.code {
                KeyCode::Enter | KeyCode::Esc => self.deploy_psk_editing = false,
                KeyCode::Backspace => {
                    self.deploy_psk_value_mut().map(String::pop);
                }
                KeyCode::Char(character)
                    if !key.modifiers.contains(KeyModifiers::CONTROL)
                        && self
                            .deploy_psk_value_mut()
                            .is_some_and(|value| value.chars().count() < 1024) =>
                {
                    if let Some(value) = self.deploy_psk_value_mut() {
                        value.push(character);
                    }
                }
                _ => {}
            }
            return;
        }
        match key.code {
            KeyCode::Enter => {
                if self.deploy_psk_field == PskDialogField::Action {
                    self.deploy_with_validated_psk();
                } else {
                    self.deploy_psk_editing = true;
                }
            }
            KeyCode::Up | KeyCode::BackTab => {
                self.deploy_psk_field = self.deploy_psk_field.previous();
            }
            KeyCode::Down | KeyCode::Tab => {
                self.deploy_psk_field = self.deploy_psk_field.next();
            }
            // 直接输入即进入编辑并追加字符,无需先按编辑键。
            KeyCode::Char(character)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && self
                        .deploy_psk_value_mut()
                        .is_some_and(|value| value.chars().count() < 1024) =>
            {
                self.deploy_psk_editing = true;
                if let Some(value) = self.deploy_psk_value_mut() {
                    value.push(character);
                }
            }
            KeyCode::Esc => self.deploy_daemon_confirming = false,
            _ => {}
        }
    }

    /// 查看/修改弹窗键处理:方向键/Tab 在 psk、二次确认与「保存」动作行间
    /// 移动;Enter 在输入字段上进入编辑(编辑中 Enter/Esc 提交),在动作行上
    /// 校验一致后写回配置;直接输入即进入编辑;Esc 关闭弹窗。
    fn handle_show_psk_key(&mut self, key: KeyEvent) {
        if self.show_psk_editing {
            match key.code {
                KeyCode::Enter | KeyCode::Esc => self.show_psk_editing = false,
                KeyCode::Backspace => {
                    self.show_psk_value_mut().map(String::pop);
                }
                KeyCode::Char(character)
                    if !key.modifiers.contains(KeyModifiers::CONTROL)
                        && self
                            .show_psk_value_mut()
                            .is_some_and(|value| value.chars().count() < 1024) =>
                {
                    if let Some(value) = self.show_psk_value_mut() {
                        value.push(character);
                    }
                }
                _ => {}
            }
            return;
        }
        match key.code {
            KeyCode::Enter => {
                if self.show_psk_field == PskDialogField::Action {
                    self.save_show_psk_dialog();
                } else {
                    self.show_psk_editing = true;
                }
            }
            KeyCode::Up | KeyCode::BackTab => {
                self.show_psk_field = self.show_psk_field.previous();
            }
            KeyCode::Down | KeyCode::Tab => {
                self.show_psk_field = self.show_psk_field.next();
            }
            // 直接输入即进入编辑并追加字符,无需先按编辑键。
            KeyCode::Char(character)
                if !key.modifiers.contains(KeyModifiers::CONTROL)
                    && self
                        .show_psk_value_mut()
                        .is_some_and(|value| value.chars().count() < 1024) =>
            {
                self.show_psk_editing = true;
                if let Some(value) = self.show_psk_value_mut() {
                    value.push(character);
                }
            }
            KeyCode::Esc => self.show_psk = false,
            _ => {}
        }
    }

    /// 校验并启动 daemon 部署:急救恢复码非空时要求长度至少 12 且与二次确认
    /// 一致,否则提示并留在弹窗;通过后后台执行 `lkit self install`(留空由
    /// daemon 自动生成)。
    fn deploy_with_validated_psk(&mut self) {
        let psk = self.deploy_psk.trim();
        if !psk.is_empty() && psk.len() < crate::deployment::config::FLARE_PSK_MIN_LENGTH {
            self.notice = Notice::Error(crate::tr!(crate::keys::CONSOLE_FLARE_PSK_TOO_SHORT));
            return;
        }
        if !psk.is_empty() && self.deploy_psk_confirmation.trim() != psk {
            self.notice = Notice::Error(crate::tr!(crate::keys::CONSOLE_DEPLOY_PSK_MISMATCH));
            return;
        }
        if let Err(error) = self.start_daemon_deploy() {
            self.notice = Notice::Error(error);
        }
        self.deploy_daemon_confirming = false;
        self.deploy_psk_editing = false;
        self.deploy_psk_field = PskDialogField::Psk;
    }

    /// flare 弹窗键处理:弹窗开启时消费全部按键。Enter 保存(校验失败留在
    /// 弹窗内提示),Esc 关闭;`e`/Enter 进入 psk 编辑,编辑中直接输入字符。
    pub(super) fn handle_flare_dialog_key(&mut self, key: KeyEvent) -> Option<ConsoleAction> {
        if self.flare.editing {
            match key.code {
                KeyCode::Enter | KeyCode::Esc => {
                    self.flare.editing = false;
                }
                KeyCode::Backspace => {
                    self.flare.psk.pop();
                }
                KeyCode::Char(character)
                    if !key.modifiers.contains(KeyModifiers::CONTROL)
                        && self.flare.psk.chars().count() < 1024 =>
                {
                    self.flare.psk.push(character);
                }
                _ => {}
            }
            return None;
        }
        match key.code {
            KeyCode::Enter | KeyCode::Char('e' | 'E') => {
                self.flare.notice.clear();
                self.flare.editing = true;
            }
            KeyCode::Char('s' | 'S') => self.save_flare_dialog(),
            KeyCode::Esc => self.flare.open = false,
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
            self.notice = Notice::Ready;
        }
        let hit = self.hits.hit_at(mouse.column, mouse.row)?;
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
                // 点击行即焦点：先把开关焦点移到该行再切换。
                if let Some(Ok(host)) = &self.mirror.host {
                    super::mirror::focus_mirror_toggle(
                        &mut self.mirror.confirming,
                        host,
                        super::mirror::MirrorToggleRow::Security,
                    );
                }
                self.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE))
            }
            Hit::MirrorCdromToggle => {
                if let Some(Ok(host)) = &self.mirror.host {
                    super::mirror::focus_mirror_toggle(
                        &mut self.mirror.confirming,
                        host,
                        super::mirror::MirrorToggleRow::Cdrom,
                    );
                }
                self.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE))
            }
            Hit::SoftwareField(_) => {
                self.focus = Focus::Panel;
                self.software.selected = SoftwareRow::Docker;
                self.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            }
            Hit::SoftwareBasePackages => {
                self.focus = Focus::Panel;
                self.software.selected = SoftwareRow::BasePackages;
                self.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            }
            Hit::BasePackageRow(index) => {
                if let Some(dialog) = self.software.base_dialog_mut() {
                    dialog.cursor = index;
                }
                self.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE))
            }
            Hit::BasePackageConfirm => {
                if let Some(dialog) = self.software.base_dialog_mut() {
                    dialog.cursor = dialog.row_count() - 1;
                }
                self.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            }
            Hit::SoftwareSourceToggle => {
                self.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE))
            }
            Hit::UninstallAction => {
                self.focus = Focus::Panel;
                self.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            }
            Hit::OverviewDeploy => {
                self.focus = Focus::Panel;
                self.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            }
            Hit::OverviewShowPsk => {
                self.focus = Focus::Panel;
                self.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            }
            Hit::DeployDaemon => {
                self.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE))
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
                let wizard = self.network_wizard.as_mut()?;
                if wizard.step != WizardStep::Wan || wizard.cancel_confirming {
                    return None;
                }
                wizard.set_wan(index);
                self.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            }
            Hit::WizardTab(mode) => {
                let wizard = self.network_wizard.as_mut()?;
                if wizard.step != WizardStep::WanConfig || wizard.cancel_confirming {
                    return None;
                }
                wizard.wan_mode = mode;
                wizard.focus = 0;
                wizard.editing = false;
                None
            }
            Hit::WizardField(focus) => {
                let wizard = self.network_wizard.as_mut()?;
                if wizard.cancel_confirming || !wizard.is_field_focus(focus) {
                    return None;
                }
                wizard.focus = focus;
                wizard.editing = true;
                None
            }
            Hit::WizardLan(index) => {
                let wizard = self.network_wizard.as_mut()?;
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
                let wizard = self.network_wizard.as_mut()?;
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
            Hit::LanguageSwitch => {
                // 语言指示可点击:等价于按 L;不可切换(编辑中)时忽略。
                if self.language_switch_available() {
                    self.toggle_language();
                }
                None
            }
        }
    }

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
