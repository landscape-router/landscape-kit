mod keys;
mod render;

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use super::Notice;
use crate::backup::lkb::{BackupMetadata, BackupProgress};
use crate::deployment::lock;

pub(crate) use self::render::{
    render_backup, render_backup_corrupt_dialog, render_backup_create_dialog,
    render_backup_create_progress, render_backup_delete_confirmation,
    render_backup_restore_confirmation,
};

/// 备份菜单数据：条目与 CLI `backup list` 同源，metadata 为 `None` 表示损坏。
pub(crate) struct BackupEntry {
    pub(crate) metadata: Option<BackupMetadata>,
    pub(crate) path: PathBuf,
    /// 备份文件大小（字节）；文件在列表加载后消失等情况下为 `None`。
    pub(crate) size: Option<u64>,
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
    /// 最近一次完整校验的结果:Ok 表示校验通过,Err 表示备份损坏。
    Complete(Result<String, String>),
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
    /// 校验结果显示的损坏提示弹框（备份损坏时 R/恢复 Enter 触发）。
    pub(crate) corrupt_dialog: bool,
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
            corrupt_dialog: false,
            create: None,
            restore_confirming: false,
            delete_confirming: false,
            delete_target: None,
        }
    }
}

impl BackupPanel {
    pub(crate) fn start(&mut self) {
        if matches!(self.state, BackupListState::Running(_)) {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let language = crate::i18n::current();
        std::thread::spawn(move || {
            let result = crate::i18n::with_language(language, load_backups);
            let _ = sender.send(result);
        });
        self.state = BackupListState::Running(receiver);
        self.selected = 0;
        self.editing = false;
        self.remark.clear();
        self.details = None;
        self.details_scroll = 0;
        self.verify = BackupVerifyState::Idle;
        self.corrupt_dialog = false;
        self.create = None;
        self.restore_confirming = false;
        self.delete_confirming = false;
        self.delete_target = None;
    }

    /// 在后台线程执行完整创建流程（与 CLI 共用 `create_manual_backup`），
    /// 进度经 channel 回传；结束后由 `poll` 刷新列表并显示结果。
    pub(crate) fn start_create(&mut self, remark: &str) {
        if self.create.is_some() {
            return;
        }
        let (sender, receiver) = mpsc::channel();
        let remark = remark.to_string();
        std::thread::spawn(move || {
            let result = (|| {
                let root = crate::deployment::state::discover_landscape_root()
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| {
                        crate::tr!(crate::keys::BACKUP_REQUIRES_EXISTING_INSTALLATION).to_string()
                    })?;
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

    pub(crate) fn poll(&mut self, notice: &mut Notice) {
        if let BackupListState::Running(receiver) = &self.state {
            match receiver.try_recv() {
                Ok(Ok(entries)) => {
                    self.state = BackupListState::Complete(entries);
                    self.details = None;
                    self.details_scroll = 0;
                }
                Ok(Err(error)) => self.state = BackupListState::Failed(error),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.state = BackupListState::Failed(crate::tr!(
                        crate::keys::CONSOLE_CHECK_WORKER_STOPPED
                    ));
                }
            }
        }
        if let BackupVerifyState::Running(receiver) = &self.verify {
            match receiver.try_recv() {
                Ok(Ok(message)) => {
                    self.verify = BackupVerifyState::Complete(Ok(message.clone()));
                    *notice = Notice::Success(message);
                }
                Ok(Err(error)) => {
                    self.verify = BackupVerifyState::Complete(Err(error.clone()));
                    *notice = Notice::Error(error);
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    let error = crate::tr!(crate::keys::CONSOLE_BACKUP_VERIFY_WORKER_STOPPED);
                    self.verify = BackupVerifyState::Complete(Err(error.clone()));
                    *notice = Notice::Error(error);
                }
            }
        }
        while let Some(run) = &self.create {
            let message = run.receiver.try_recv();
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
                            *notice = Notice::Success(crate::tr!(
                                crate::keys::CONSOLE_BACKUP_CREATED,
                                backup_id = metadata.backup_id
                            ));
                        }
                        Err(error) => *notice = Notice::Error(error),
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.create = None;
                    *notice = Notice::Error(crate::tr!(
                        crate::keys::CONSOLE_BACKUP_CREATE_WORKER_STOPPED
                    ));
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
fn load_backups() -> Result<Vec<BackupEntry>, String> {
    let root = crate::deployment::state::discover_landscape_root()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| {
            crate::tr!(crate::keys::BACKUP_REQUIRES_EXISTING_INSTALLATION).to_string()
        })?;
    let rows = crate::commands::backup::list_backups_with(
        &root,
        crate::commands::backup::BackupListCheck::Metadata,
    )
    .map_err(|error| error.to_string())?;
    Ok(rows
        .into_iter()
        .map(|(metadata, path)| BackupEntry {
            size: std::fs::metadata(&path).ok().map(|file| file.len()),
            metadata,
            path,
        })
        .collect())
}

/// 与 CLI `backup delete` 相同的根目录解析、安装锁与文件删除。
pub(super) fn delete_backup_via_console(backup_id: &str) -> Result<(), String> {
    let root = crate::deployment::state::discover_landscape_root()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| {
            crate::tr!(crate::keys::BACKUP_REQUIRES_EXISTING_INSTALLATION).to_string()
        })?;
    let _lock = lock::acquire_install_lock().map_err(|error| error.to_string())?;
    crate::commands::backup::delete_backup(&root, backup_id).map_err(|error| error.to_string())
}
