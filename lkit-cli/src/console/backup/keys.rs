use std::sync::mpsc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::super::{ConsoleAction, ConsoleApp, Notice};
use super::{BackupListState, BackupVerifyState, delete_backup_via_console};
use crate::commands::Commands;

impl ConsoleApp {
    pub(crate) fn handle_backup_key(&mut self, key: KeyEvent) -> Option<Option<ConsoleAction>> {
        // 损坏提示弹框:Enter/Esc 关闭,恢复确认前弹出,不触发任何动作。
        if self.backup.corrupt_dialog {
            match key.code {
                KeyCode::Enter | KeyCode::Esc => self.backup.corrupt_dialog = false,
                _ => {}
            }
            return Some(None);
        }
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
                    // 恢复执行前完整校验:未校验先启动(提示校验中,留在确认层),
                    // 校验失败弹损坏框,只有校验通过才提交 Restore 请求。
                    match &self.backup.verify {
                        BackupVerifyState::Complete(Ok(_)) => {
                            let backup_id = metadata.backup_id.clone();
                            self.backup.restore_confirming = false;
                            return Some(Some(self.backup_restore_action(&backup_id)));
                        }
                        BackupVerifyState::Complete(Err(_)) => {
                            self.backup.restore_confirming = false;
                            self.backup.corrupt_dialog = true;
                            return Some(None);
                        }
                        BackupVerifyState::Running(_) => {
                            self.notice = Notice::Info(crate::tr!(
                                crate::keys::CONSOLE_BACKUP_VERIFY_RUNNING
                            ));
                            return Some(None);
                        }
                        BackupVerifyState::Idle => {
                            self.start_backup_verify();
                            self.notice = Notice::Info(crate::tr!(
                                crate::keys::CONSOLE_BACKUP_VERIFY_RUNNING
                            ));
                            return Some(None);
                        }
                    }
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
                            self.backup.start_create(&remark);
                        }
                        Err(error) => self.notice = Notice::Error(error.to_string()),
                    }
                }
                KeyCode::Esc => {
                    self.backup.editing = false;
                    self.backup.remark.clear();
                }
                KeyCode::Backspace => {
                    self.backup.remark.pop();
                }
                KeyCode::Char(character)
                    if !key.modifiers.contains(KeyModifiers::CONTROL)
                        && self.backup.remark.chars().count() < 256 =>
                {
                    self.backup.remark.push(character);
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
                    self.backup.corrupt_dialog = false;
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
                        if self.backup_corrupt() {
                            self.backup.corrupt_dialog = true;
                        } else {
                            self.backup.restore_confirming = true;
                        }
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
                        // 进入详情即自动完整校验(读文件 + verify_lkb + 解包),
                        // 结果写底栏,不阻塞查看;V 键可随时手动重校验。
                        self.backup.verify = BackupVerifyState::Idle;
                        self.start_backup_verify();
                    } else {
                        self.notice = Notice::Error(crate::tr!(
                            crate::keys::CONSOLE_BACKUP_INVALID,
                            id = entry
                                .path
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .trim_end_matches(".lkb")
                        ));
                    }
                }
            }
            KeyCode::Char('r' | 'R') => {
                if let Some(entry) = self.backup.selected_entry()
                    && entry.metadata.is_some()
                {
                    if self.backup_corrupt() {
                        self.backup.corrupt_dialog = true;
                    } else {
                        self.backup.restore_confirming = true;
                    }
                } else {
                    self.notice =
                        Notice::Info(crate::tr!(crate::keys::CONSOLE_BACKUP_SELECT_TO_RESTORE));
                }
            }
            KeyCode::Char('d' | 'D') => {
                if let Some(entry) = self.backup.selected_entry()
                    && let Some(metadata) = &entry.metadata
                {
                    self.backup.delete_target = Some(metadata.backup_id.clone());
                    self.backup.delete_confirming = true;
                } else {
                    self.notice =
                        Notice::Info(crate::tr!(crate::keys::CONSOLE_BACKUP_SELECT_TO_DELETE));
                }
            }
            _ => return None,
        }
        Some(None)
    }
}

impl ConsoleApp {
    fn delete_backup(&mut self, backup_id: String) {
        let result = delete_backup_via_console(&backup_id);
        match result {
            Ok(()) => {
                self.backup.details = None;
                self.backup.details_scroll = 0;
                self.backup.state = BackupListState::NotRun;
                self.notice = Notice::Success(crate::tr!(
                    crate::keys::CONSOLE_BACKUP_DELETED,
                    backup_id = backup_id
                ));
            }
            Err(error) => self.notice = Notice::Error(format!("backup: {error}")),
        }
    }

    /// 最近一次校验已完成且失败:备份损坏,恢复前(R 键/恢复 Enter)弹框提示。
    fn backup_corrupt(&self) -> bool {
        matches!(&self.backup.verify, BackupVerifyState::Complete(Err(_)))
    }

    /// 校验选中的备份:详情页校验详情条目,列表页校验选中条目。
    fn start_backup_verify(&mut self) {
        let Some(entry) = self
            .backup
            .details_entry()
            .or_else(|| self.backup.selected_entry())
        else {
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
                    .inspect(|_message| {
                        let _ = std::fs::remove_dir_all(&verify_dir);
                    })
            });
            let _ = sender.send(result);
        });
        self.backup.verify = BackupVerifyState::Running(receiver);
        self.notice = Notice::Info(crate::tr!(crate::keys::CONSOLE_BACKUP_VERIFY_RUNNING));
    }

    fn backup_restore_action(&self, backup_id: &str) -> ConsoleAction {
        let command = Commands::Restore(crate::commands::restore::Restore {
            backup: Some(backup_id.to_string()),
            file: None,
            allow_no_backup: false,
            yes: true,
            console_confirmed: true,
            #[cfg(feature = "test-support")]
            test_runtime: None,
        });
        let args = vec![
            "restore".into(),
            "--backup".into(),
            backup_id.to_string(),
            "--yes".into(),
            "--console-confirmed".into(),
        ];
        ConsoleAction::Command { command, args }
    }
}
