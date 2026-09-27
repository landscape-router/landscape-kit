//! 面板门禁的负向交互测试:渲染层显示门禁提示(非 root / 未安装)时,面板
//! 按键不得穿透到被门禁的列表/表单动作——对话框不打开、worker 不启动。
//! 键处理与渲染必须共用同一门禁谓词(`Snapshot::Installed`),此处把 app 直接
//! 摆进门禁态(含残留的列表/表单状态,对应「列表成功后现场翻转」与会话中途
//! 状态刷新),逐键断言「什么都没发生」。

use super::super::backup::{BackupListState, BackupVerifyState};
use super::super::*;
use super::support::*;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// 非 root 下 Backup 面板显示 root 门禁提示,但列表残留着早前成功加载的行:
/// Enter 不得打开详情(也不得触发自动校验),R/D 不得打开恢复/删除确认层。
#[test]
fn backup_keys_inert_when_root_required() {
    let _language = LanguageGuard::set(crate::i18n::Language::En);
    let _territory = DaemonTerritory::new("gate-backup-root", false);
    let mut app = backup_ready_app();
    app.snapshot = Snapshot::RootRequired;
    app.backup.state = BackupListState::Complete(backup_rows());

    app.handle_key(key(KeyCode::Down)); // 光标落到第一行备份
    app.handle_key(key(KeyCode::Enter));
    assert!(
        app.backup.details.is_none(),
        "Enter must not open backup details behind the root-required notice"
    );
    assert!(matches!(app.backup.verify, BackupVerifyState::Idle));
    app.handle_key(key(KeyCode::Char('r')));
    assert!(!app.backup.restore_confirming);
    app.handle_key(key(KeyCode::Char('d')));
    assert!(!app.backup.delete_confirming);
}

/// 非 root 启动直接进 Backup 面板:面板只画「需要 root」提示,回车(光标在
/// 「创建备份」动作行)不得打开备注输入弹窗。
#[test]
fn backup_create_dialog_inert_when_root_required() {
    let _language = LanguageGuard::set(crate::i18n::Language::En);
    let _territory = DaemonTerritory::new("gate-backup-create", false);
    let mut app = backup_ready_app();
    app.snapshot = Snapshot::RootRequired;
    app.backup.state = BackupListState::NotRun;

    app.handle_key(key(KeyCode::Enter));
    assert!(
        !app.backup.editing,
        "Enter must not open the create-backup dialog behind the root-required notice"
    );

    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("Root privileges are required"));
}

/// 未安装世界(另一会话卸载后现场翻转)进 Update 面板:Enter 不得进入版本
/// 编辑,也不得启动目标解析 worker。
#[test]
fn update_keys_inert_when_not_installed() {
    let _language = LanguageGuard::set(crate::i18n::Language::En);
    let _territory = DaemonTerritory::new("gate-update", false);
    let mut app = update_ready_app();
    app.snapshot = Snapshot::NotInstalled;

    app.handle_key(key(KeyCode::Enter)); // 光标默认在版本字段
    assert!(
        !app.update.editing,
        "Enter must not open version editing behind the not-installed notice"
    );
    app.update.selected = crate::console::update::UpdateField::Start;
    app.handle_key(key(KeyCode::Enter));
    assert!(
        app.update.resolving.is_none(),
        "Enter must not start update resolution behind the not-installed notice"
    );
    assert!(app.update.confirming.is_none());
}

/// 备份创建进行中:进度弹层排他消费全部按键,底层列表的 Enter/R/D 不得
/// 穿透打开详情/恢复/删除层。
#[test]
fn backup_list_keys_inert_while_create_runs() {
    let _language = LanguageGuard::set(crate::i18n::Language::En);
    let _territory = DaemonTerritory::new("gate-backup-running", false);
    let mock = std::sync::Arc::new(super::super::backup::MockBackupOps::manual());
    let mut app = backup_ready_app();
    app.backup.state = BackupListState::Complete(backup_rows());
    app.backup.ops = mock.clone();
    app.backup.start_create("gate snapshot");
    let sender = mock.create_sender();
    sender
        .send(super::super::backup::BackupCreateMessage::Progress(
            crate::backup::lkb::BackupProgress::Exporting,
        ))
        .unwrap();
    app.backup.poll(&mut app.notice);

    app.handle_key(key(KeyCode::Enter));
    app.handle_key(key(KeyCode::Char('r')));
    app.handle_key(key(KeyCode::Char('d')));
    assert!(app.backup.details.is_none());
    assert!(!app.backup.restore_confirming);
    assert!(!app.backup.delete_confirming);
}
