//! 整屏 insta 快照:把每个面板/弹窗在固定尺寸下经 `TestBackend` 渲染的完整
//! 布局固化为 `snapshots/*.snap` 文本文件,任何一行偏移、列错位或截断都会在
//! `assert_snapshot!` 的 diff 里显形。
//!
//! 维护方式:有意变更布局后运行
//! `INSTA_UPDATE=always cargo test -p lkit-cli --features test-support --bin lkit console::tests::snapshots`
//! (或 `cargo insta review`)更新快照并逐屏审阅。备份列表的时间列依赖本地时区,
//! 断言前统一规范化为 `<DATE>`,快照因此与时区无关。

use super::super::backup::BackupEntry;
use super::super::*;
use super::support::*;
use crate::i18n::Language;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

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

/// 备份列表:一条常规记录 + 一条超长备注(钉住截断省略号)+ 一条损坏记录
/// (钉住红色 INVALID 徽标行),列对齐跨行可见。
fn backup_rows() -> Vec<BackupEntry> {
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
