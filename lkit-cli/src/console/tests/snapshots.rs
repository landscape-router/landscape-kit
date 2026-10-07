//! 整屏 insta 快照:把每个面板/弹窗在固定尺寸下经 `TestBackend` 渲染的完整
//! 布局固化为 `snapshots/*.snap` 文本文件,任何一行偏移、列错位或截断都会在
//! `assert_snapshot!` 的 diff 里显形。
//!
//! 维护方式:有意变更布局后运行
//! `INSTA_UPDATE=always cargo test -p lkit-cli --features test-support --bin lkit console::tests::snapshots`
//! (或 `cargo insta review`)更新快照并逐屏审阅。备份列表的时间列(分)与
//! takeover 屏的截止/当前时刻(秒+时区偏移)依赖运行时刻和本地时区,断言前
//! 统一规范化为 `<DATE>`,快照因此与时刻、时区无关。Overview 等屏显示的
//! lkit 版本号在渲染前固定为 `<VERSION>`(版本长度会改变右栏折行,事后过滤
//! 无法覆盖),快照因此不随 release 版本提交漂移。

use super::super::backup::{
    BackupCreateMessage, BackupListState, BackupVerifyState, MockBackupOps,
};
use super::super::daemon_panel::MockDeployOps;
use super::super::mirror::{MirrorConfirm, MockMirrorOps};
use super::super::render::pin_test_version;
use super::super::software::{BasePackagesState, MockSoftwareOps, SoftwareInstallMessage};
use super::super::update::MockUpdateOps;
use super::super::*;
use super::support::*;
use crate::backup::lkb::BackupProgress;
use crate::i18n::Language;
use crate::mirror::{Family, Host, MirrorName, MirrorStatus};
use crate::software::InstallPhase;
use crate::software::base::{BasePackage, BasePackageDialog};
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
    // 日期时间统一屏蔽到 <DATE>:备份列表精确到分,takeover 屏精确到秒并带本地
    // 时区偏移;秒数与偏移随运行时刻/主机时区变化,一并纳入过滤。
    settings.add_filter(
        r"\d{4}-\d{2}-\d{2} \d{2}:\d{2}(:\d{2})?( [+-]\d{2}:\d{2})?",
        "<DATE>",
    );
    // 版本号在渲染前固定为 <VERSION>:insta 过滤替换不了它——版本长度会先
    // 改变 Overview 右栏的词界折行。
    let _version = pin_test_version();
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
    // 同 snapshot_reinit_confirm_en:与设置 `LKIT_TEST_REINIT_ELIGIBLE` 的测试串行。
    let _eligible = super::reinit::ELIGIBLE_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
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

// ---- 弹框/确认层与整屏状态全覆盖:此前未钉的对话框与向导/阻塞屏。纯对话框
// 状态直接置字段(与 exit-confirmation/preflight-dialog 同风格),安装取消层
// 这类流程瞬态仍经 mock 驱动。卸载面板已从侧栏隐藏(死路径),不钉。 ----

#[test]
fn snapshot_mirror_confirm_en() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("mirror-confirm", false);
    assert_screen_snapshot("mirror-confirm-en", 100, 28, |app| {
        app.snapshot = installed_snapshot();
        app.menu_index = 4;
        app.focus = Focus::Panel;
        app.mirror.host = Some(Ok(Host {
            family: Family::Debian,
            codename: None,
        }));
        app.mirror.detected = true;
        app.mirror.confirming = Some(MirrorConfirm::Apply {
            mirror: MirrorName::Ustc,
            replace_security: false,
            disable_cdrom: true,
            toggle: 0,
        });
    });
}

#[test]
fn snapshot_base_packages_dialog_en() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("base-dialog", false);
    assert_screen_snapshot("base-packages-dialog-en", 100, 28, |app| {
        *app = software_ready_app(Arc::new(MockSoftwareOps::manual(true)));
        // 手工构造全部 5 个条目,不探测宿主 PATH:ip 已装、其余缺失,缺失项
        // 勾选态取 `selected_by_default`(iw/hostapd 不勾),快照跨机器稳定。
        let dialog = BasePackageDialog {
            entries: crate::software::base::BasePackage::all()
                .into_iter()
                .map(|package| {
                    let installed = package == crate::software::base::BasePackage::Iproute2;
                    crate::software::base::BasePackageEntry {
                        selected: !installed && package.selected_by_default(),
                        package,
                        installed,
                    }
                })
                .collect(),
            cursor: 0,
        };
        app.software.base_packages = BasePackagesState::Choosing {
            dialog,
            previous: Box::new(BasePackagesState::NotChosen),
        };
    });
}

#[test]
fn snapshot_reinit_confirm_en() {
    // `reinit_eligible` 读取 `LKIT_TEST_REINIT_ELIGIBLE`(进程级环境变量),
    // 与设置该变量的 reinit 测试共用串行锁,渲染结果才由夹具唯一决定。
    let _eligible = super::reinit::ELIGIBLE_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("reinit-confirm", false);
    assert_screen_snapshot("reinit-confirm-en", 100, 28, |app| {
        app.snapshot = installed_snapshot();
        app.menu_index = 6;
        app.focus = Focus::Panel;
        app.reinit.confirming = true;
    });
}

#[test]
fn snapshot_show_psk_dialog_en() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("show-psk", false);
    assert_screen_snapshot("show-psk-dialog-en", 100, 28, |app| {
        app.snapshot = installed_snapshot();
        app.focus = Focus::Panel;
        app.show_psk = true;
        app.show_psk_value = "recovery-psk-example".into();
    });
}

#[test]
fn snapshot_flare_dialog_en() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("flare-dialog", false);
    assert_screen_snapshot("flare-dialog-en", 100, 28, |app| {
        app.snapshot = installed_snapshot();
        app.focus = Focus::Panel;
        app.flare.open = true;
        app.flare.psk = "recovery-psk-example".into();
    });
}

#[test]
fn snapshot_backup_restore_confirm_en() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("restore-confirm", false);
    assert_screen_snapshot("backup-restore-confirm-en", 100, 28, |app| {
        *app = backup_ready_app();
        app.backup.state = BackupListState::Complete(backup_rows());
        app.backup.selected = 1;
        app.backup.verify = BackupVerifyState::Complete(Ok("verified".into()));
        app.backup.restore_confirming = true;
    });
}

#[test]
fn snapshot_backup_delete_confirm_en() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("delete-confirm", false);
    assert_screen_snapshot("backup-delete-confirm-en", 100, 28, |app| {
        *app = backup_ready_app();
        app.backup.state = BackupListState::Complete(backup_rows());
        app.backup.selected = 1;
        // 删除确认层按 delete_target 在列表中查元数据,不设则不渲染弹窗。
        app.backup.delete_target = Some("20260807-131500-ab12cd34".into());
        app.backup.delete_confirming = true;
    });
}

#[test]
fn snapshot_backup_corrupt_dialog_en() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("corrupt-dialog", false);
    assert_screen_snapshot("backup-corrupt-dialog-en", 100, 28, |app| {
        *app = backup_ready_app();
        app.backup.state = BackupListState::Complete(backup_rows());
        // 损坏条目是列表第三行(selected 从 1 起)。
        app.backup.selected = 3;
        app.backup.corrupt_dialog = true;
    });
}

#[test]
fn snapshot_software_cancel_confirm_en() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("sw-cancel", false);
    let mock = Arc::new(MockSoftwareOps::manual(true));
    assert_screen_snapshot("software-cancel-confirm-en", 100, 28, |app| {
        *app = software_ready_app(mock);
        app.handle_key(key(KeyCode::Enter)); // 打开确认层
        app.handle_key(key(KeyCode::Enter)); // 启动安装
        app.handle_key(key(KeyCode::Esc)); // 安装中 Esc 打开取消确认层
    });
}

#[test]
fn snapshot_base_cancel_confirm_en() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("base-cancel", false);
    let mock = Arc::new(MockSoftwareOps::manual(true));
    assert_screen_snapshot("base-cancel-confirm-en", 100, 28, |app| {
        *app = software_ready_app(mock);
        app.software.base_packages = BasePackagesState::Chosen(vec![BasePackage::all()[0]]);
        app.software.start_base_install().unwrap();
        app.handle_key(key(KeyCode::Esc)); // 基础包安装中 Esc 打开取消确认层
    });
}

#[test]
fn snapshot_takeover_blocking_en() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("takeover-block", false);
    assert_screen_snapshot("takeover-blocking-en", 100, 28, |app| {
        app.snapshot = pending_takeover_snapshot();
    });
}

#[test]
fn snapshot_wizard_wan_en() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("wizard-wan", false);
    assert_screen_snapshot("wizard-wan-en", 100, 28, |app| {
        app.snapshot = Snapshot::NotInstalled;
        app.network_wizard = Some(sample_network_wizard());
    });
}

#[test]
fn snapshot_wizard_wan_config_en() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("wizard-wan-config", false);
    assert_screen_snapshot("wizard-wan-config-en", 100, 28, |app| {
        app.snapshot = Snapshot::NotInstalled;
        let mut wizard = routes_armed_wizard();
        wizard.step = super::super::network_wizard::WizardStep::WanConfig;
        wizard.address = "10.1.1.105/24".into();
        wizard.gateway = "10.1.1.1".into();
        app.network_wizard = Some(wizard);
    });
}

#[test]
fn snapshot_wizard_confirm_en() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("wizard-confirm", false);
    assert_screen_snapshot("wizard-confirm-en", 100, 28, |app| {
        app.snapshot = Snapshot::NotInstalled;
        let mut wizard = routes_armed_wizard();
        // 确认页的 WAN 行读 address/gateway 字段(与 routes 列表无关),不设则显示空值。
        wizard.address = "10.1.1.105/24".into();
        wizard.gateway = "10.1.1.1".into();
        wizard.step = super::super::network_wizard::WizardStep::Confirm;
        app.network_wizard = Some(wizard);
    });
}
