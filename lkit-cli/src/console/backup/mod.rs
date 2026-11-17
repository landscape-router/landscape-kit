mod keys;
mod render;

#[cfg(test)]
mod mock;

#[cfg(test)]
pub(crate) use mock::MockBackupOps;
#[cfg(test)]
pub(crate) use mock::{
    backup_rows, sample_backup_entry, sample_backup_metadata, sample_verified_backup,
};

use std::path::PathBuf;
use std::sync::Arc;
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

#[derive(Clone)]
pub(crate) struct VerifiedBackup {
    pub message: String,
    /// Integrity can succeed even when this installation cannot restore it.
    pub confirmation: Result<crate::commands::restore::RestoreConfirmation, String>,
}

pub(crate) enum BackupVerifyState {
    Idle,
    Running(Receiver<Result<VerifiedBackup, String>>),
    /// 完整性校验结果；当前安装的恢复资格保存在成功结果内，单独判断。
    Complete(Result<VerifiedBackup, String>),
}

pub(crate) enum BackupCreateMessage {
    Progress(BackupProgress),
    Done(Result<BackupMetadata, String>),
}

/// 备份面板的后台操作接缝:列表加载、创建备份与完整性校验。
/// UI 只消费返回的 `Receiver`;生产实现走真实备份流程,单测注入
/// 脚本化 `mock::MockBackupOps`。
pub(crate) trait BackupOps {
    fn list(&self) -> Receiver<Result<Vec<BackupEntry>, String>>;
    fn create(&self, remark: String) -> Receiver<BackupCreateMessage>;
    fn verify(&self, path: PathBuf) -> Receiver<Result<VerifiedBackup, String>>;
}

/// 生产实现:worker 线程执行真实列表/创建/校验流程。
pub(crate) struct RealBackupOps;

impl BackupOps for RealBackupOps {
    fn list(&self) -> Receiver<Result<Vec<BackupEntry>, String>> {
        let (sender, receiver) = mpsc::channel();
        let language = crate::i18n::current();
        landscape_terrain_proto::concurrency::spawn_thread(
            crate::task_names::CONSOLE_BACKUP_LIST,
            move || {
                let result = crate::i18n::with_language(language, load_backups);
                let _ = sender.send(result);
            },
        );
        receiver
    }

    fn create(&self, remark: String) -> Receiver<BackupCreateMessage> {
        let (sender, receiver) = mpsc::channel();
        landscape_terrain_proto::concurrency::spawn_thread(
            crate::task_names::CONSOLE_BACKUP_CREATE,
            move || {
                let result = (|| {
                    let root = crate::deployment::state::discover_landscape_root()
                        .map_err(|error| error.to_string())?
                        .ok_or_else(|| {
                            crate::tr!(crate::keys::BACKUP_REQUIRES_EXISTING_INSTALLATION)
                                .to_string()
                        })?;
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .thread_name_fn(
                            landscape_terrain_proto::concurrency::runtime_thread_name_fn(
                                crate::task_names::BACKUP_RUNTIME,
                            ),
                        )
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
            },
        );
        receiver
    }

    fn verify(&self, path: PathBuf) -> Receiver<Result<VerifiedBackup, String>> {
        let (sender, receiver) = mpsc::channel();
        let language = crate::i18n::current();
        landscape_terrain_proto::concurrency::spawn_thread(
            crate::task_names::CONSOLE_BACKUP_VERIFY,
            move || {
                let result = crate::i18n::with_language(language, || verify_backup(path));
                let _ = sender.send(result);
            },
        );
        receiver
    }
}

pub(super) fn verify_backup(path: PathBuf) -> Result<VerifiedBackup, String> {
    use sha2::{Digest, Sha256};
    crate::workflows::restore::validate_backup_file(&path).map_err(|e| e.to_string())?;
    let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
    let metadata = crate::backup::lkb::verify_lkb(&bytes).map_err(|e| e.to_string())?;
    let verify_dir =
        std::env::temp_dir().join(format!("lkit-backup-tui-verify-{}", uuid::Uuid::now_v7()));
    let integrity = crate::backup::lkb::create_secure_dir(&verify_dir, 0o700)
        .and_then(|()| crate::backup::lkb::extract_lkb(&bytes, &verify_dir));
    let _ = std::fs::remove_dir_all(&verify_dir);
    integrity.map_err(|e| e.to_string())?;
    let message = crate::tr!(
        crate::keys::CONSOLE_BACKUP_VERIFIED,
        backup_id = metadata.backup_id
    );
    let prepared = crate::workflows::restore::PreparedRestore {
        sha256: crate::release::artifacts::hex(&Sha256::digest(&bytes)),
        bytes,
        metadata,
    };
    let confirmation = (|| {
        let root = crate::deployment::state::discover_landscape_root()
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "no Landscape installation".to_string())?;
        let installed = crate::deployment::state::load_state(&root)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "no Landscape installation".to_string())?;
        crate::workflows::restore::check_architecture(&installed, &prepared.metadata)
            .map_err(|e| e.to_string())?;
        crate::workflows::install::parse_stable_version(&prepared.metadata.landscape_version)
            .map_err(|e| e.to_string())?;
        crate::commands::restore::confirmed_backup(&root, &installed, path, &prepared)
            .map_err(|e| e.to_string())
    })();
    Ok(VerifiedBackup {
        message,
        confirmation,
    })
}

fn default_backup_ops() -> Arc<dyn BackupOps> {
    Arc::new(RealBackupOps)
}

/// 在 TUI 内执行备份创建：worker 线程跑完整创建流程并通过 channel 回传进度。
pub(crate) struct BackupCreateRun {
    receiver: Receiver<BackupCreateMessage>,
    progress: BackupProgress,
}

/// 备份面板：列表 + 详情 + 创建备注/进度 + 删除/恢复确认。
pub(crate) struct BackupPanel {
    /// 后台操作接缝,默认真实现;测试注入 mock。
    pub(crate) ops: Arc<dyn BackupOps>,
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
    pub(crate) allow_no_backup: bool,
    pub(crate) delete_confirming: bool,
    pub(crate) delete_target: Option<String>,
}

impl Default for BackupPanel {
    fn default() -> Self {
        Self {
            ops: default_backup_ops(),
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
            allow_no_backup: false,
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
        self.state = BackupListState::Running(self.ops.list());
        self.selected = 0;
        self.editing = false;
        self.remark.clear();
        self.details = None;
        self.details_scroll = 0;
        self.verify = BackupVerifyState::Idle;
        self.corrupt_dialog = false;
        self.create = None;
        self.restore_confirming = false;
        self.allow_no_backup = false;
        self.delete_confirming = false;
        self.delete_target = None;
    }

    /// 在后台线程执行完整创建流程（与 CLI 共用 `create_manual_backup`），
    /// 进度经 channel 回传；结束后由 `poll` 刷新列表并显示结果。
    pub(crate) fn start_create(&mut self, remark: &str) {
        if self.create.is_some() {
            return;
        }
        let receiver = self.ops.create(remark.to_string());
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
                    *notice = match message.confirmation {
                        Ok(_) => Notice::Success(message.message),
                        Err(reason) => Notice::Info(crate::tr!(
                            crate::keys::CONSOLE_BACKUP_RESTORE_UNAVAILABLE,
                            reason = reason
                        )),
                    };
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
