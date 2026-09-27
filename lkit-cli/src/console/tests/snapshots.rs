//! 整屏 insta 快照:把每个面板/弹窗在固定尺寸下经 `TestBackend` 渲染的完整
//! 布局固化为 `snapshots/*.snap` 文本文件,任何一行偏移、列错位或截断都会在
//! `assert_snapshot!` 的 diff 里显形。
//!
//! 维护方式:有意变更布局后运行
//! `INSTA_UPDATE=always cargo test -p lkit-cli --features test-support --bin lkit console::tests::snapshots`
//! (或 `cargo insta review`)更新快照并逐屏审阅。备份列表的时间列依赖本地时区,
//! 断言前统一规范化为 `<DATE>`,快照因此与时区无关。

use super::super::backup::{BackupCreateMessage, BackupListState, MockBackupOps};
use super::super::daemon_panel::MockDeployOps;
use super::super::mirror::MockMirrorOps;
use super::super::software::{MockSoftwareOps, SoftwareInstallMessage};
use super::super::update::MockUpdateOps;
use super::super::*;
use super::support::*;
use crate::backup::lkb::BackupProgress;
use crate::i18n::Language;
use crate::mirror::{Family, Host, MirrorName, MirrorStatus};
use crate::software::InstallPhase;
use crate::software::base::BasePackage;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use std::collections::HashMap;
use std::sync::Arc;

/// 渲染一屏并断言 insta 快照。快照测试必须先固定语言与 daemon 状态
/// (`LanguageGuard` + `DaemonTerritory`),并在 `prepare` 中覆盖安装快照,
/// 保证屏幕逐字符确定。
fn assert_screen_snapshot(
    name: &'static str,
    width: u16,
    height: u16,
    prepare: impl FnOnce(&mut ConsoleApp),
) {
    let mut settings = insta::Settings::clone_current();
    settings.add_filter(r"\d{4}-\d{2}-\d{2} \d{2}:\d{2}", "<DATE>");
    settings.bind(|| {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut app = ConsoleApp::new();
        prepare(&mut app);
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        insta::assert_snapshot!(name, terminal.backend());
    });
}

#[test]
fn snapshot_overview_navigation_not_installed_en() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("overview-nav-en", false);
    assert_screen_snapshot("overview-navigation-not-installed-en", 100, 28, |app| {
        app.snapshot = Snapshot::NotInstalled;
    });
}

#[test]
fn snapshot_overview_panel_daemon_running_en() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("overview-running-en", true);
    assert_screen_snapshot("overview-panel-daemon-running-en", 100, 28, |app| {
        app.snapshot = installed_snapshot();
        app.focus = Focus::Panel;
    });
}

#[test]
fn snapshot_overview_panel_daemon_not_running_en() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("overview-down-en", false);
    assert_screen_snapshot("overview-panel-daemon-not-running-en", 100, 28, |app| {
        app.snapshot = installed_snapshot();
        app.focus = Focus::Panel;
    });
}

#[test]
fn snapshot_install_form_en() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("install-en", false);
    assert_screen_snapshot("install-form-en", 100, 28, |app| {
        app.snapshot = Snapshot::NotInstalled;
        app.menu_index = 1;
        app.focus = Focus::Panel;
    });
}

#[test]
fn snapshot_backup_list_en() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("backup-en", false);
    assert_screen_snapshot("backup-list-en", 100, 28, |app| {
        app.snapshot = installed_snapshot();
        app.menu_index = 2;
        app.focus = Focus::Panel;
        app.backup.state = BackupListState::Complete(backup_rows());
    });
}

#[test]
fn snapshot_update_panel_en() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("update-en", false);
    assert_screen_snapshot("update-panel-en", 100, 28, |app| {
        *app = update_ready_app();
    });
}

#[test]
fn snapshot_mirror_panel_en() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("mirror-en", false);
    assert_screen_snapshot("mirror-panel-en", 100, 28, |app| {
        app.snapshot = installed_snapshot();
        app.menu_index = 4;
        app.focus = Focus::Panel;
    });
}

#[test]
fn snapshot_software_panel_en() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("software-en", false);
    assert_screen_snapshot("software-panel-en", 100, 28, |app| {
        app.snapshot = installed_snapshot();
        app.menu_index = 5;
        app.focus = Focus::Panel;
    });
}

#[test]
fn snapshot_reinit_panel_en() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("reinit-en", false);
    assert_screen_snapshot("reinit-panel-en", 100, 28, |app| {
        app.snapshot = installed_snapshot();
        app.menu_index = 6;
        app.focus = Focus::Panel;
    });
}

#[test]
fn snapshot_minimum_size_72x18_en() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("minimum-en", false);
    assert_screen_snapshot("minimum-72x18-en", 72, 18, |app| {
        app.snapshot = Snapshot::NotInstalled;
    });
}

#[test]
fn snapshot_overview_stacked_74x18_en() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("stacked-en", false);
    assert_screen_snapshot("overview-stacked-74x18-en", 74, 18, |app| {
        app.snapshot = installed_snapshot();
        app.focus = Focus::Panel;
    });
}

#[test]
fn snapshot_too_small_71x17_en() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("too-small-en", false);
    assert_screen_snapshot("too-small-71x17-en", 71, 17, |app| {
        app.snapshot = Snapshot::NotInstalled;
    });
}

#[test]
fn snapshot_too_small_71x17_zh() {
    let _language = LanguageGuard::set(Language::Zh);
    let _territory = DaemonTerritory::new("too-small-zh", false);
    assert_screen_snapshot("too-small-71x17-zh", 71, 17, |app| {
        app.snapshot = Snapshot::NotInstalled;
    });
}

#[test]
fn snapshot_overview_navigation_zh() {
    let _language = LanguageGuard::set(Language::Zh);
    let _territory = DaemonTerritory::new("overview-nav-zh", false);
    assert_screen_snapshot("overview-navigation-zh", 100, 28, |app| {
        app.snapshot = Snapshot::NotInstalled;
    });
}

#[test]
fn snapshot_backup_list_zh() {
    let _language = LanguageGuard::set(Language::Zh);
    let _territory = DaemonTerritory::new("backup-zh", false);
    assert_screen_snapshot("backup-list-zh", 100, 28, |app| {
        app.snapshot = installed_snapshot();
        app.menu_index = 2;
        app.focus = Focus::Panel;
        app.backup.state = BackupListState::Complete(backup_rows());
    });
}

#[test]
fn snapshot_footer_long_zh_notice_72x18() {
    let _language = LanguageGuard::set(Language::Zh);
    let _territory = DaemonTerritory::new("footer-zh", false);
    assert_screen_snapshot("footer-long-zh-notice-72x18", 72, 18, |app| {
        app.snapshot = Snapshot::NotInstalled;
        app.notice = Notice::Error(
            "网络回滚正在进行中，请勿关闭终端；所有网络配置将在回滚完成后恢复到上一个已知良好的状态。".into(),
        );
    });
}

#[test]
fn snapshot_exit_confirmation_en() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("exit-en", false);
    assert_screen_snapshot("exit-confirmation-en", 100, 28, |app| {
        app.snapshot = Snapshot::NotInstalled;
        app.exit_state = ExitState::Confirming;
    });
}

#[test]
fn snapshot_preflight_dialog_en() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("preflight-en", false);
    assert_screen_snapshot("preflight-dialog-en", 100, 28, |app| {
        app.snapshot = Snapshot::NotInstalled;
        app.menu_index = 1;
        app.focus = Focus::Panel;
        app.preflight.state = PreflightState::Complete(error_preflight_report());
        app.preflight_dialog = true;
    });
}

// ---- 流程驱动快照:经 ops 接缝注入 `Mock*Ops`,把「进行中/成功瞬间」的
// 瞬态屏也钉进快照。这些屏只有在 worker 真正推进时才会出现,直接摆字段摆
// 不出来;驱动方式与 `console::tests::ops` 的流程测试一致——注入 mock、
// 按键或 start* 进入瞬态、向通道注入消息、poll 推进,再整屏渲染。 ----

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// 软件面板的可用起点:已安装世界、已检测发行版、Docker 行未安装
/// (`installed[0]` 显式置 false,开发机可能装有 docker)。
fn software_ready_app(ops: Arc<MockSoftwareOps>) -> ConsoleApp {
    let mut app = ConsoleApp::new();
    app.software.ops = ops;
    app.software.host = Some(Ok(Host {
        family: Family::Debian,
        codename: Some("bookworm".into()),
    }));
    app.software.detected = true;
    app.software.installed[0] = false;
    app.snapshot = installed_snapshot();
    app.menu_index = 5;
    app.focus = Focus::Panel;
    app
}

#[test]
fn snapshot_software_install_confirm_en() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("flow-sw-confirm", false);
    let mock = Arc::new(MockSoftwareOps::manual(true));
    assert_screen_snapshot("software-install-confirm-en", 100, 28, |app| {
        *app = software_ready_app(mock);
        // Docker 行 Enter 打开来源确认层。
        app.handle_key(key(KeyCode::Enter));
    });
}

#[test]
fn snapshot_software_install_progress_en() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("flow-sw-progress", false);
    let mock = Arc::new(MockSoftwareOps::manual(true));
    assert_screen_snapshot("software-install-progress-en", 100, 28, |app| {
        *app = software_ready_app(mock.clone());
        app.handle_key(key(KeyCode::Enter)); // 打开确认层
        app.handle_key(key(KeyCode::Enter)); // 启动后台安装
        // sender 必须活过 poll:临时值会在语句尾 drop,poll 下一条消息就是
        // Disconnected,直接走 worker-退出清理分支。
        let sender = mock.install_sender();
        sender
            .send(SoftwareInstallMessage::Phase(
                InstallPhase::InstallingPackages,
            ))
            .unwrap();
        let ConsoleApp {
            software, notice, ..
        } = app;
        software.poll(notice);
    });
}

#[test]
fn snapshot_software_base_progress_en() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("flow-sw-base", false);
    let mock = Arc::new(MockSoftwareOps::manual(true));
    assert_screen_snapshot("software-base-progress-en", 100, 28, |app| {
        *app = software_ready_app(mock);
        app.software.base_packages =
            super::super::software::BasePackagesState::Chosen(vec![BasePackage::all()[0]]);
        app.software.start_base_install().unwrap();
    });
}

#[test]
fn snapshot_mirror_probe_results_en() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("flow-mirror-probe", false);
    let mock = Arc::new(MockMirrorOps::manual());
    assert_screen_snapshot("mirror-probe-results-en", 100, 28, |app| {
        app.snapshot = installed_snapshot();
        app.menu_index = 4;
        app.focus = Focus::Panel;
        app.mirror.ops = mock.clone();
        app.mirror.host = Some(Ok(Host {
            family: Family::Debian,
            codename: None,
        }));
        app.mirror.detected = true;
        app.mirror.start_probe();
        let sender = mock.probe_sender();
        sender
            .send(HashMap::from([
                (MirrorName::Official, MirrorStatus::Available),
                (MirrorName::Ustc, MirrorStatus::Available),
                (MirrorName::Tencent, MirrorStatus::Unavailable),
                (MirrorName::Huawei, MirrorStatus::Unknown),
            ]))
            .unwrap();
        app.mirror.poll(&mut app.notice);
    });
}

#[test]
fn snapshot_backup_create_dialog_en() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("flow-backup-dialog", false);
    assert_screen_snapshot("backup-create-dialog-en", 100, 28, |app| {
        *app = backup_ready_app();
        app.backup.state = BackupListState::Complete(backup_rows());
        // selected == 0 是「创建备份」动作行,Enter 打开备注弹窗。
        app.handle_key(key(KeyCode::Enter));
    });
}

#[test]
fn snapshot_backup_create_progress_en() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("flow-backup-progress", false);
    let mock = Arc::new(MockBackupOps::manual());
    assert_screen_snapshot("backup-create-progress-en", 100, 28, |app| {
        *app = backup_ready_app();
        app.backup.ops = mock.clone();
        app.backup.start_create("pre-upgrade snapshot");
        let sender = mock.create_sender();
        sender
            .send(BackupCreateMessage::Progress(BackupProgress::Exporting))
            .unwrap();
        let ConsoleApp { backup, notice, .. } = app;
        backup.poll(notice);
    });
}

#[test]
fn snapshot_backup_detail_verified_en() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("flow-backup-detail", false);
    let mock = Arc::new(MockBackupOps::manual());
    assert_screen_snapshot("backup-detail-verified-en", 100, 28, |app| {
        *app = backup_ready_app();
        app.backup.ops = mock.clone();
        app.backup.state = BackupListState::Complete(backup_rows());
        app.backup.selected = 1;
        // Enter 打开详情页并自动启动后台校验(UI-11),结果回传后底栏为已校验。
        app.handle_key(key(KeyCode::Enter));
        let sender = mock.verify_sender();
        sender.send(Ok("verified".into())).unwrap();
        let ConsoleApp { backup, notice, .. } = app;
        backup.poll(notice);
    });
}

#[test]
fn snapshot_update_confirm_en() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("flow-update-confirm", false);
    let mock = Arc::new(MockUpdateOps::manual());
    assert_screen_snapshot("update-confirm-en", 100, 28, |app| {
        *app = update_ready_app();
        app.update.ops = mock.clone();
        app.start_update_resolution().unwrap();
        mock.resolve_sender()
            .send(Ok(resolved("1.2.3", "1.3.0")))
            .unwrap();
        let ConsoleApp { update, notice, .. } = app;
        update.poll(notice);
    });
}

#[test]
fn snapshot_daemon_deploy_dialog_en() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("flow-deploy-dialog", false);
    assert_screen_snapshot("daemon-deploy-dialog-en", 100, 28, |app| {
        app.snapshot = installed_snapshot();
        app.focus = Focus::Panel;
        app.open_deploy_dialog();
    });
}

#[test]
fn snapshot_daemon_deploy_progress_en() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("flow-deploy-progress", false);
    let mock = Arc::new(MockDeployOps::manual());
    assert_screen_snapshot("daemon-deploy-progress-en", 100, 28, |app| {
        app.snapshot = installed_snapshot();
        app.focus = Focus::Panel;
        app.deploy_ops = mock;
        app.open_deploy_dialog();
        app.start_daemon_deploy().unwrap();
    });
}

#[test]
fn snapshot_preflight_details_en() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("flow-preflight-details", false);
    assert_screen_snapshot("preflight-details-en", 100, 28, |app| {
        app.snapshot = Snapshot::NotInstalled;
        app.menu_index = 1;
        app.focus = Focus::Panel;
        app.preflight.state = PreflightState::Complete(sample_preflight_report());
        app.preflight.expanded = true;
    });
}
