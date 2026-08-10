mod keys;
mod render;

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use crate::backup::lkb::{BackupMetadata, BackupProgress};
use crate::deployment::{lock, plan, root};

pub(crate) use self::render::{
    render_backup, render_backup_create_dialog, render_backup_create_progress,
    render_backup_delete_confirmation, render_backup_restore_confirmation,
};

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
pub(super) fn delete_backup_via_console(install_dir: &str, backup_id: &str) -> Result<(), String> {
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
