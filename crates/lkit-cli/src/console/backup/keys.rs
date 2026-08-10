use std::path::PathBuf;
use std::sync::mpsc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::super::{ConsoleAction, ConsoleApp};
use super::{BackupListState, BackupVerifyState, delete_backup_via_console};
use crate::commands::Commands;

impl ConsoleApp {
    pub(crate) fn handle_backup_key(&mut self, key: KeyEvent) -> Option<Option<ConsoleAction>> {
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
}

impl ConsoleApp {
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
