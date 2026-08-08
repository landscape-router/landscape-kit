use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Gauge, Paragraph, Wrap};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::network_wizard::Snapshot;
use super::render::{panel_block, register_dialog_hits, register_modal_hits};
use super::widgets::{Focus, Hit, block_row_of};
use super::{ConsoleAction, ConsoleApp};
use crate::backup::lkb::{BackupMetadata, BackupProgress};
use crate::commands::Commands;
use crate::commands::backup::{architecture_key, scope_key};
use crate::deployment::{lock, plan, root};

/// 备份菜单数据：条目与 CLI `backup list` 同源，metadata 为 `None` 表示损坏。
pub(crate) struct BackupEntry {
    pub(crate) metadata: Option<BackupMetadata>,
    pub(crate) path: PathBuf,
}

pub(crate) enum BackupListState {
    NotRun,
    Running(Receiver<Result<Vec<BackupEntry>, String>>),
    Complete(Vec<BackupEntry>),
    Failed(String),
}

pub(crate) enum BackupVerifyState {
    Idle,
    Running(Receiver<Result<String, String>>),
}

enum BackupCreateMessage {
    Progress(BackupProgress),
    Done(Result<BackupMetadata, String>),
}

/// 在 TUI 内执行备份创建：worker 线程跑完整创建流程并通过 channel 回传进度。
pub(crate) struct BackupCreateRun {
    receiver: Receiver<BackupCreateMessage>,
    progress: BackupProgress,
}

/// 备份面板：列表 + 详情 + 创建备注/进度 + 删除/恢复确认。
pub(crate) struct BackupPanel {
    pub(crate) state: BackupListState,
    pub(crate) selected: usize,
    pub(crate) editing: bool,
    pub(crate) remark: String,
    pub(crate) details: Option<usize>,
    pub(crate) details_scroll: u16,
    pub(crate) verify: BackupVerifyState,
    pub(crate) create: Option<BackupCreateRun>,
    pub(crate) restore_confirming: bool,
    pub(crate) delete_confirming: bool,
    pub(crate) delete_target: Option<String>,
}

impl Default for BackupPanel {
    fn default() -> Self {
        Self {
            state: BackupListState::NotRun,
            selected: 0,
            editing: false,
            remark: String::new(),
            details: None,
            details_scroll: 0,
            verify: BackupVerifyState::Idle,
            create: None,
            restore_confirming: false,
            delete_confirming: false,
            delete_target: None,
        }
    }
}

impl BackupPanel {
    pub(crate) fn start(&mut self, install_dir: &str) {
        if matches!(self.state, BackupListState::Running(_)) {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let language = crate::i18n::current();
        let install_dir = install_dir.to_string();
        std::thread::spawn(move || {
            let result = crate::i18n::with_language(language, || load_backups(&install_dir));
            let _ = sender.send(result);
        });
        self.state = BackupListState::Running(receiver);
        self.selected = 0;
        self.editing = false;
        self.remark.clear();
        self.details = None;
        self.details_scroll = 0;
        self.verify = BackupVerifyState::Idle;
        self.create = None;
        self.restore_confirming = false;
        self.delete_confirming = false;
        self.delete_target = None;
    }

    /// 在后台线程执行完整创建流程（与 CLI 共用 `create_manual_backup`），
    /// 进度经 channel 回传；结束后由 `poll` 刷新列表并显示结果。
    pub(crate) fn start_create(&mut self, install_dir: &str, remark: &str) {
        if self.create.is_some() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let install_dir = install_dir.to_string();
        let remark = remark.to_string();
        std::thread::spawn(move || {
            let result = (|| {
                let requested = PathBuf::from(&install_dir);
                let selected = plan::select_install_root(
                    Some(&requested),
                    std::env::var("LKIT_INSTALL_DIR").ok().as_deref(),
                )
                .map_err(|error| error.to_string())?;
                let root =
                    root::normalize_install_root(&selected).map_err(|error| error.to_string())?;
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|error| format!("build backup create runtime: {error}"))?;
                runtime
                    .block_on(crate::commands::backup::create_manual_backup(
                        &root,
                        &crate::deployment::runtime::InstallRuntime::production(),
                        &remark,
                        None,
                        |progress| {
                            let _ = sender.send(BackupCreateMessage::Progress(progress));
                        },
                    ))
                    .map_err(|error| error.to_string())
            })();
            let _ = sender.send(BackupCreateMessage::Done(result));
        });
        self.create = Some(BackupCreateRun {
            receiver,
            progress: BackupProgress::Exporting,
        });
    }

    pub(crate) fn poll(&mut self, notice: &mut String) {
        match &self.state {
            BackupListState::Running(receiver) => match receiver.try_recv() {
                Ok(Ok(entries)) => {
                    self.state = BackupListState::Complete(entries);
                    self.details = None;
                    self.details_scroll = 0;
                }
                Ok(Err(error)) => self.state = BackupListState::Failed(error),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.state = BackupListState::Failed(
                        crate::tr!(crate::keys::CONSOLE_CHECK_WORKER_STOPPED).into(),
                    );
                }
            },
            _ => {}
        }
        if let BackupVerifyState::Running(receiver) = &self.verify {
            match receiver.try_recv() {
                Ok(Ok(message)) => {
                    self.verify = BackupVerifyState::Idle;
                    *notice = message;
                }
                Ok(Err(error)) => {
                    self.verify = BackupVerifyState::Idle;
                    *notice = error;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.verify = BackupVerifyState::Idle;
                    *notice = crate::tr!(crate::keys::CONSOLE_BACKUP_VERIFY_WORKER_STOPPED).into();
                }
            }
        }
        loop {
            let message = match &self.create {
                Some(run) => run.receiver.try_recv(),
                None => break,
            };
            match message {
                Ok(BackupCreateMessage::Progress(progress)) => {
                    if let Some(run) = &mut self.create {
                        run.progress = progress;
                    }
                }
                Ok(BackupCreateMessage::Done(result)) => {
                    self.create = None;
                    match result {
                        Ok(metadata) => {
                            self.state = BackupListState::NotRun;
                            *notice = crate::tr!(
                                crate::keys::CONSOLE_BACKUP_CREATED,
                                backup_id = metadata.backup_id
                            )
                            .into();
                        }
                        Err(error) => *notice = error,
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.create = None;
                    *notice = crate::tr!(crate::keys::CONSOLE_BACKUP_CREATE_WORKER_STOPPED).into();
                }
            }
        }
    }

    pub(crate) fn rows(&self) -> &[BackupEntry] {
        match &self.state {
            BackupListState::Complete(rows) => rows,
            _ => &[],
        }
    }

    /// 当前选中的备份行；第 0 行是“创建备份”动作。
    pub(crate) fn selected_entry(&self) -> Option<&BackupEntry> {
        if self.selected == 0 {
            None
        } else {
            self.rows().get(self.selected - 1)
        }
    }

    pub(crate) fn details_entry(&self) -> Option<&BackupEntry> {
        self.details.and_then(|index| self.rows().get(index))
    }
}

/// 与 CLI `backup list` 相同的解析与完整校验。
fn load_backups(install_dir: &str) -> Result<Vec<BackupEntry>, String> {
    let requested = PathBuf::from(install_dir);
    let selected = plan::select_install_root(
        Some(&requested),
        std::env::var("LKIT_INSTALL_DIR").ok().as_deref(),
    )
    .map_err(|error| error.to_string())?;
    let normalized = root::normalize_install_root(&selected).map_err(|error| error.to_string())?;
    let rows = crate::commands::backup::list_backups_with(
        &normalized,
        crate::commands::backup::BackupListCheck::Metadata,
    )
    .map_err(|error| error.to_string())?;
    Ok(rows
        .into_iter()
        .map(|(metadata, path)| BackupEntry { metadata, path })
        .collect())
}

/// 与 CLI `backup delete` 相同的根目录解析、安装锁与文件删除。
fn delete_backup_via_console(install_dir: &str, backup_id: &str) -> Result<(), String> {
    let requested = PathBuf::from(install_dir);
    let selected = plan::select_install_root(
        Some(&requested),
        std::env::var("LKIT_INSTALL_DIR").ok().as_deref(),
    )
    .map_err(|error| error.to_string())?;
    let root = root::normalize_install_root(&selected).map_err(|error| error.to_string())?;
    let _lock = lock::acquire_install_lock(&root).map_err(|error| error.to_string())?;
    crate::commands::backup::delete_backup(&root, backup_id).map_err(|error| error.to_string())
}

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

pub(crate) fn render_backup(frame: &mut Frame<'_>, app: &mut ConsoleApp, area: Rect) {
    let focused = app.focus == Focus::Panel;
    if app.backup.details.is_some() {
        render_backup_details(frame, app, focused, area);
        return;
    }
    if matches!(app.snapshot, Snapshot::RootRequired) {
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled(
                    crate::tr!(crate::keys::CONSOLE_ROOT_PRIVILEGES_REQUIRED),
                    Style::default().fg(Color::Yellow),
                ),
                Line::raw(""),
                Line::styled(
                    crate::tr!(crate::keys::CONSOLE_BACKUP_REQUIRES_INSTALL),
                    Style::default().fg(Color::DarkGray),
                ),
            ])
            .block(panel_block(
                &crate::tr!(crate::keys::CONSOLE_BACKUP_MENU),
                focused,
            ))
            .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }
    if !matches!(app.snapshot, Snapshot::Installed { .. }) {
        let message = match &app.snapshot {
            Snapshot::NotInstalled => {
                crate::tr!(crate::keys::CONSOLE_LANDSCAPE_NOT_INSTALLED)
            }
            Snapshot::Unavailable(error) => error.clone(),
            _ => unreachable!(),
        };
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled(message, Style::default().fg(Color::Yellow)),
                Line::raw(""),
                Line::styled(
                    crate::tr!(crate::keys::CONSOLE_BACKUP_REQUIRES_INSTALL),
                    Style::default().fg(Color::DarkGray),
                ),
            ])
            .block(panel_block(
                &crate::tr!(crate::keys::CONSOLE_BACKUP_MENU),
                focused,
            ))
            .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }
    render_backup_list(frame, app, focused, area);
}

fn render_backup_list(frame: &mut Frame<'_>, app: &mut ConsoleApp, focused: bool, area: Rect) {
    let create_selected = app.focus == Focus::Panel && app.backup.selected == 0;
    let highlight = Style::default()
        .fg(Color::Black)
        .bg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let mut lines = vec![Line::styled(
        format!(
            "{}{}",
            if create_selected { "> " } else { "  " },
            crate::tr!(crate::keys::CONSOLE_BACKUP_CREATE)
        ),
        if create_selected {
            highlight
        } else {
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD)
        },
    )];
    let mut entry_lines: Vec<usize> = Vec::new();
    match &app.backup.state {
        BackupListState::NotRun | BackupListState::Running(_) => {
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                crate::tr!(crate::keys::CONSOLE_BACKUP_LOADING),
                Style::default().fg(Color::DarkGray),
            ));
        }
        BackupListState::Failed(error) => {
            lines.push(Line::raw(""));
            lines.push(Line::styled(error.clone(), Style::default().fg(Color::Red)));
        }
        BackupListState::Complete(rows) => {
            if rows.is_empty() {
                lines.push(Line::raw(""));
                lines.push(Line::styled(
                    crate::tr!(crate::keys::CONSOLE_BACKUP_NONE_FOUND),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            for (index, entry) in rows.iter().enumerate() {
                let cursor = app.focus == Focus::Panel && app.backup.selected == index + 1;
                entry_lines.push(lines.len());
                match &entry.metadata {
                    Some(metadata) => {
                        let text = format!(
                            "{}  {}  {}{}",
                            metadata.backup_id,
                            metadata.created_at,
                            metadata.landscape_version,
                            if metadata.remark.is_empty() {
                                String::new()
                            } else {
                                format!("  {}", metadata.remark)
                            }
                        );
                        lines.push(Line::styled(
                            format!("{}{}", if cursor { "> " } else { "  " }, text),
                            if cursor { highlight } else { Style::default() },
                        ));
                    }
                    None => {
                        let name = entry
                            .path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .trim_end_matches(".lkb")
                            .to_string();
                        lines.push(Line::styled(
                            format!(
                                "{}{}  {}",
                                if cursor { "> " } else { "  " },
                                name,
                                crate::tr!(crate::keys::CONSOLE_BACKUP_INVALID_BADGE)
                            ),
                            if cursor {
                                highlight
                            } else {
                                Style::default().fg(Color::Red)
                            },
                        ));
                    }
                }
            }
        }
    }
    let content_width = area.width.saturating_sub(2);
    app.hits.block_row(area, 0, Hit::BackupRow(0));
    for (index, line) in entry_lines.iter().enumerate() {
        app.hits.block_row(
            area,
            block_row_of(&lines, *line, content_width),
            Hit::BackupRow(index + 1),
        );
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel_block(
                &crate::tr!(crate::keys::CONSOLE_BACKUP_MENU),
                focused,
            ))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn render_backup_details(frame: &mut Frame<'_>, app: &ConsoleApp, focused: bool, area: Rect) {
    let Some(entry) = app.backup.details_entry() else {
        return;
    };
    let Some(metadata) = &entry.metadata else {
        return;
    };
    let contents = format!(
        "binary={} static={} static_archive={} init_config={} geo_cache={}",
        metadata.contents.binary,
        metadata.contents.static_,
        metadata.contents.static_archive,
        metadata.contents.init_config,
        metadata.contents.geo_cache,
    );
    let lines = vec![
        Line::styled(
            crate::tr!(crate::keys::CONSOLE_BACKUP_DETAILS_TITLE),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
        Line::raw(format!(
            "{}  {}",
            crate::tr!(crate::keys::CONSOLE_BACKUP_ID_LABEL),
            metadata.backup_id
        )),
        Line::raw(format!(
            "{}  {}",
            crate::tr!(crate::keys::CONSOLE_BACKUP_CREATED_LABEL),
            metadata.created_at
        )),
        Line::raw(format!(
            "{}  {}",
            crate::tr!(crate::keys::CONSOLE_BACKUP_VERSION_LABEL),
            metadata.landscape_version
        )),
        Line::raw(format!(
            "{}  {}",
            crate::tr!(crate::keys::CONSOLE_BACKUP_LKIT_LABEL),
            metadata.lkit_version
        )),
        Line::raw(format!(
            "{}  {}",
            crate::tr!(crate::keys::CONSOLE_BACKUP_ARCH_LABEL),
            architecture_key(metadata.architecture)
        )),
        Line::raw(format!(
            "{}  {}",
            crate::tr!(crate::keys::CONSOLE_BACKUP_HOSTNAME_LABEL),
            metadata.hostname
        )),
        Line::raw(format!(
            "{}  {}",
            crate::tr!(crate::keys::CONSOLE_BACKUP_REMARK_LABEL),
            metadata.remark
        )),
        Line::raw(format!(
            "{}  {}",
            crate::tr!(crate::keys::CONSOLE_BACKUP_AUTO_LABEL),
            metadata.auto
        )),
        Line::raw(format!(
            "{}  {}",
            crate::tr!(crate::keys::CONSOLE_BACKUP_SCOPE_LABEL),
            scope_key(metadata.scope)
        )),
        Line::raw(format!(
            "{}  {}",
            crate::tr!(crate::keys::CONSOLE_BACKUP_CONTENTS_LABEL),
            contents
        )),
        Line::raw(""),
        Line::styled(
            crate::tr!(
                crate::keys::CONSOLE_BACKUP_DETAILS_RESTORE_HINT,
                id = metadata.backup_id
            ),
            Style::default().fg(Color::DarkGray),
        ),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel_block(
                &crate::tr!(crate::keys::CONSOLE_BACKUP_DETAILS_TITLE),
                focused,
            ))
            .wrap(Wrap { trim: true })
            .scroll((app.backup.details_scroll, 0)),
        area,
    );
}

pub(crate) fn render_backup_create_dialog(frame: &mut Frame<'_>, app: &mut ConsoleApp) {
    let screen = frame.area();
    let width = 68.min(screen.width.saturating_sub(2));
    let height = 9.min(screen.height.saturating_sub(2));
    let area = Rect::new(
        screen.x + screen.width.saturating_sub(width) / 2,
        screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    );
    register_modal_hits(&mut app.hits, screen, area);
    let remark = app.backup.remark.clone();
    let remark_display = if remark.is_empty() {
        "_".to_string()
    } else {
        format!("{remark}_")
    };
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(vec![
            Line::raw(crate::tr!(crate::keys::CONSOLE_BACKUP_CREATE_SCOPE)),
            Line::raw(""),
            Line::from(vec![
                Span::styled(
                    format!("{}: ", crate::tr!(crate::keys::CONSOLE_BACKUP_REMARK_LABEL)),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    remark_display,
                    Style::default().add_modifier(Modifier::UNDERLINED),
                ),
            ]),
            Line::raw(""),
            Line::raw(crate::tr!(crate::keys::CONSOLE_BACKUP_CREATE_HINT)),
        ])
        .alignment(Alignment::Center)
        .block(Block::bordered().title(crate::tr!(crate::keys::CONSOLE_BACKUP_CREATE_TITLE))),
        area,
    );
}

/// 创建备份进行中的居中弹窗：阶段文案 + 文件数 Gauge。
pub(crate) fn render_backup_create_progress(frame: &mut Frame<'_>, app: &mut ConsoleApp) {
    let Some(run) = &app.backup.create else {
        return;
    };
    let screen = frame.area();
    let width = 76.min(screen.width.saturating_sub(2));
    let height = 7.min(screen.height.saturating_sub(2));
    let area = Rect::new(
        screen.x + screen.width.saturating_sub(width) / 2,
        screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    );
    register_modal_hits(&mut app.hits, screen, area);
    let (stage_text, ratio) = match &run.progress {
        BackupProgress::Exporting => (
            crate::tr!(crate::keys::CONSOLE_BACKUP_CREATE_PROGRESS_EXPORT),
            0.0,
        ),
        BackupProgress::Archiving {
            done,
            total,
            current,
        } => {
            let ratio = if *total == 0 {
                0.0
            } else {
                *done as f64 / *total as f64
            };
            (
                crate::tr!(
                    crate::keys::CONSOLE_BACKUP_CREATE_PROGRESS_ARCHIVE,
                    done = *done,
                    total = *total,
                    current = current
                ),
                ratio,
            )
        }
        BackupProgress::Finalizing => (
            crate::tr!(crate::keys::CONSOLE_BACKUP_CREATE_PROGRESS_FINALIZE),
            1.0,
        ),
    };
    let percent = (ratio * 100.0).round() as u64;
    let inner = area.inner(Margin {
        vertical: 1,
        horizontal: 1,
    });
    let [stage_area, gauge_area, hint_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::bordered().title(crate::tr!(crate::keys::CONSOLE_BACKUP_CREATE_RUNNING)),
        area,
    );
    frame.render_widget(
        Paragraph::new(stage_text).wrap(Wrap { trim: true }),
        stage_area,
    );
    frame.render_widget(
        Gauge::default()
            .ratio(ratio.clamp(0.0, 1.0))
            .label(format!("{percent:>3}%"))
            .gauge_style(Style::default().fg(Color::Cyan))
            .use_unicode(false),
        gauge_area,
    );
    frame.render_widget(
        Paragraph::new(crate::tr!(crate::keys::CONSOLE_BACKUP_HINT_CREATE_RUNNING))
            .style(Style::default().fg(Color::DarkGray)),
        hint_area,
    );
}

pub(crate) fn render_backup_restore_confirmation(frame: &mut Frame<'_>, app: &mut ConsoleApp) {
    let Some(metadata) = app
        .backup
        .selected_entry()
        .and_then(|entry| entry.metadata.as_ref())
    else {
        return;
    };
    let screen = frame.area();
    let width = 76.min(screen.width.saturating_sub(2));
    let height = 11.min(screen.height.saturating_sub(2));
    let area = Rect::new(
        screen.x + screen.width.saturating_sub(width) / 2,
        screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    );
    register_dialog_hits(&mut app.hits, screen, area);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_BACKUP_RESTORE_QUESTION),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Line::raw(""),
            Line::raw(crate::tr!(
                crate::keys::CONSOLE_BACKUP_RESTORE_PLAN,
                id = metadata.backup_id,
                version = metadata.landscape_version
            )),
            Line::raw(crate::tr!(
                crate::keys::CONSOLE_BACKUP_RESTORE_MINIMAL_SCOPE
            )),
            Line::raw(""),
            Line::raw(crate::tr!(crate::keys::CONSOLE_BACKUP_RESTORE_PRESS_ENTER)),
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_PRESS_ESC_TO_CANCEL),
                Style::default().fg(Color::DarkGray),
            ),
        ])
        .alignment(Alignment::Center)
        .block(Block::bordered().title(crate::tr!(crate::keys::CONSOLE_BACKUP_RESTORE_TITLE))),
        area,
    );
}

pub(crate) fn render_backup_delete_confirmation(frame: &mut Frame<'_>, app: &mut ConsoleApp) {
    let Some(metadata) = app.backup.delete_target.as_deref().and_then(|id| {
        app.backup
            .rows()
            .iter()
            .find_map(|entry| entry.metadata.as_ref().filter(|m| m.backup_id == id))
    }) else {
        return;
    };
    let screen = frame.area();
    let width = 76.min(screen.width.saturating_sub(2));
    let height = 11.min(screen.height.saturating_sub(2));
    let area = Rect::new(
        screen.x + screen.width.saturating_sub(width) / 2,
        screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    );
    register_dialog_hits(&mut app.hits, screen, area);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_BACKUP_DELETE_QUESTION),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Line::raw(""),
            Line::raw(crate::tr!(
                crate::keys::CONSOLE_BACKUP_DELETE_PLAN,
                id = metadata.backup_id,
                version = metadata.landscape_version
            )),
            Line::raw(""),
            Line::raw(crate::tr!(crate::keys::CONSOLE_BACKUP_DELETE_PRESS_ENTER)),
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_PRESS_ESC_TO_CANCEL),
                Style::default().fg(Color::DarkGray),
            ),
        ])
        .alignment(Alignment::Center)
        .block(Block::bordered().title(crate::tr!(crate::keys::CONSOLE_BACKUP_DELETE_TITLE))),
        area,
    );
}
