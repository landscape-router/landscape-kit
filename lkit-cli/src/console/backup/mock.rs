//! 备份面板的脚本化操作与演示样例数据:cfg(test) 手动 mock 供单测注入,
//! cfg(demo) Demo*Ops 供演示构建;样例条目同时经 `console::tests::support`
//! 供快照/gallery 测试复用,数据只有一份。

use std::path::PathBuf;
use std::sync::mpsc;

use super::{BackupCreateMessage, BackupEntry, BackupOps};
#[cfg(feature = "demo")]
use crate::backup::lkb::BackupProgress;
use crate::backup::lkb::{BackupArchitecture, BackupContents, BackupMetadata, BackupScope};

#[cfg(test)]
type ListSender = mpsc::Sender<Result<Vec<BackupEntry>, String>>;

/// 单测注入用的手动 mock:worker 停着不动,测试经 `list_sender`/
/// `create_sender`/`verify_sender` 直接向通道注入消息。
#[cfg(test)]
pub(crate) struct MockBackupOps {
    list_tx: std::sync::Mutex<Option<ListSender>>,
    create_tx: std::sync::Mutex<Option<mpsc::Sender<BackupCreateMessage>>>,
    verify_tx: std::sync::Mutex<Option<mpsc::Sender<Result<String, String>>>>,
}

#[cfg(test)]
impl MockBackupOps {
    pub(crate) fn manual() -> Self {
        Self {
            list_tx: std::sync::Mutex::new(None),
            create_tx: std::sync::Mutex::new(None),
            verify_tx: std::sync::Mutex::new(None),
        }
    }

    pub(crate) fn list_sender(&self) -> ListSender {
        self.list_tx
            .lock()
            .unwrap()
            .take()
            .expect("list() must be called before grabbing the sender")
    }

    pub(crate) fn create_sender(&self) -> mpsc::Sender<BackupCreateMessage> {
        self.create_tx
            .lock()
            .unwrap()
            .take()
            .expect("create() must be called before grabbing the sender")
    }

    pub(crate) fn verify_sender(&self) -> mpsc::Sender<Result<String, String>> {
        self.verify_tx
            .lock()
            .unwrap()
            .take()
            .expect("verify() must be called before grabbing the sender")
    }
}

#[cfg(test)]
impl BackupOps for MockBackupOps {
    fn list(&self) -> mpsc::Receiver<Result<Vec<BackupEntry>, String>> {
        let (sender, receiver) = mpsc::channel();
        *self.list_tx.lock().unwrap() = Some(sender);
        receiver
    }

    fn create(&self, _remark: String) -> mpsc::Receiver<BackupCreateMessage> {
        let (sender, receiver) = mpsc::channel();
        *self.create_tx.lock().unwrap() = Some(sender);
        receiver
    }

    fn verify(&self, _path: PathBuf) -> mpsc::Receiver<Result<String, String>> {
        let (sender, receiver) = mpsc::channel();
        *self.verify_tx.lock().unwrap() = Some(sender);
        receiver
    }
}

/// demo 构建的脚本化操作:列表延迟回填样例条目、创建走进度阶段、校验直接通过。
#[cfg(feature = "demo")]
pub(crate) struct DemoBackupOps;

#[cfg(feature = "demo")]
impl BackupOps for DemoBackupOps {
    fn list(&self) -> mpsc::Receiver<Result<Vec<BackupEntry>, String>> {
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            std::thread::sleep(super::super::DEMO_STEP_DELAY);
            let _ = sender.send(Ok(backup_rows()));
        });
        receiver
    }

    fn create(&self, _remark: String) -> mpsc::Receiver<BackupCreateMessage> {
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            for progress in [BackupProgress::Exporting, BackupProgress::Finalizing] {
                let _ = sender.send(BackupCreateMessage::Progress(progress));
                std::thread::sleep(super::super::DEMO_STEP_DELAY);
            }
            let _ = sender.send(BackupCreateMessage::Done(Ok(sample_backup_metadata())));
        });
        receiver
    }

    fn verify(&self, path: PathBuf) -> mpsc::Receiver<Result<String, String>> {
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            std::thread::sleep(super::super::DEMO_STEP_DELAY);
            let id = path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default();
            let _ = sender.send(Ok(format!("backup {id} verified (demo)")));
        });
        receiver
    }
}

pub(crate) fn sample_backup_metadata() -> BackupMetadata {
    BackupMetadata {
        schema_version: 1,
        backup_id: "20260807-131500-ab12cd34".into(),
        created_at: chrono::DateTime::parse_from_rfc3339("2026-08-07T13:15:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
        landscape_version: "1.2.3".into(),
        lkit_version: "0.1.3".into(),
        architecture: BackupArchitecture::X86_64,
        hostname: "edge".into(),
        remark: "before upgrade".into(),
        auto: false,
        scope: BackupScope::Minimal,
        contents: BackupContents {
            binary: true,
            static_: true,
            static_archive: false,
            init_config: true,
            geo_cache: false,
        },
        checksum: "sha256:00".into(),
    }
}

pub(crate) fn sample_backup_entry() -> BackupEntry {
    BackupEntry {
        metadata: Some(sample_backup_metadata()),
        path: PathBuf::from("/opt/landscape/backups/20260807-131500-ab12cd34.lkb"),
        // 1.5 MiB:列表与详情页按人类可读单位渲染该值。
        size: Some(1_572_864),
    }
}

/// 备份列表:一条常规记录 + 一条超长备注(钉住截断省略号)+ 一条损坏记录
/// (钉住红色 INVALID 徽标行),列对齐跨行可见。快照与 gallery 共用。
pub(crate) fn backup_rows() -> Vec<BackupEntry> {
    let mut long_remark = sample_backup_metadata();
    long_remark.backup_id = "20260901-090000-feedface".into();
    long_remark.created_at = chrono::DateTime::parse_from_rfc3339("2026-09-01T09:00:00Z")
        .unwrap()
        .into();
    long_remark.remark = "urgent snapshot taken right before the firewall migration window".into();
    long_remark.landscape_version = "0.9.0".into();
    vec![
        sample_backup_entry(),
        BackupEntry {
            metadata: Some(long_remark),
            path: PathBuf::from("/opt/landscape/backups/20260901-090000-feedface.lkb"),
            size: Some(3_500_000),
        },
        BackupEntry {
            metadata: None,
            path: PathBuf::from("/opt/landscape/backups/20260902-1010-broken.lkb"),
            size: None,
        },
    ]
}
