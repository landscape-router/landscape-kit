//! 面板后台操作的进程内流程测试:注入手动 mock(`Mock*Ops`,worker 停着
//! 不动、测试直接向通道注入消息)后按键驱动完整流程。这类测试证明
//! ops 接缝把「按键 → 状态机 → 通道回传 → 状态更新」整条链路搬进了
//! 快速单测;真实 worker 的执行语义仍由 e2e/docker 层覆盖。

use super::super::backup::{BackupListState, MockBackupOps};
use super::super::preflight::{MockPreflightOps, PreflightState};
use super::super::software::{MockSoftwareOps, SoftwareConfirm, SoftwareInstallMessage};
use super::super::*;
use super::support::*;
use crate::i18n::Language;
use crate::mirror::{Family, Host};
use crate::software::InstallPhase;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// 走完 Docker 安装的完整生命周期:确认层 → 进度推进 → 取消确认层 →
/// 取消结果回传;失败后可重新进入确认层并走成功路径。
#[test]
fn software_install_lifecycle_through_mock_ops() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("ops-software", false);
    let mock = std::sync::Arc::new(MockSoftwareOps::manual(true));
    let mut app = ConsoleApp::new();
    app.software.ops = mock.clone();
    app.software.host = Some(Ok(Host {
        family: Family::Debian,
        codename: Some("bookworm".into()),
    }));
    app.software.detected = true;
    app.menu_index = 5;
    app.focus = Focus::Panel;

    // Docker 行 Enter 打开来源确认层,再 Enter 启动后台安装。
    app.handle_key(key(KeyCode::Enter));
    assert!(
        app.software.confirming.is_some(),
        "Enter on the Docker row must open the source confirmation layer"
    );
    app.handle_key(key(KeyCode::Enter));
    assert!(
        app.software.install.is_some(),
        "Enter in the confirmation layer must start the install run"
    );
    assert!(matches!(app.notice, Notice::Info(_)));

    // 阶段进度经通道回传,poll 更新当前阶段。
    let sender = mock.install_sender();
    sender
        .send(SoftwareInstallMessage::Phase(
            InstallPhase::InstallingPackages,
        ))
        .unwrap();
    poll_software(&mut app);
    assert!(matches!(
        app.software.install.as_ref().map(|run| run.phase),
        Some(InstallPhase::InstallingPackages)
    ));

    // 安装进行中 Esc 打开取消确认层,Enter 置位取消标志(安装仍在跑)。
    app.handle_key(key(KeyCode::Esc));
    assert!(app.software.cancel_confirming);
    app.handle_key(key(KeyCode::Enter));
    assert!(!app.software.cancel_confirming);
    let cancelled = app
        .software
        .install
        .as_ref()
        .expect("the run must outlive the cancel confirmation")
        .cancel
        .load(std::sync::atomic::Ordering::Relaxed);
    assert!(
        cancelled,
        "Enter in the cancel layer must set the cancel flag"
    );

    // worker 回传取消结果:run 清理、面板恢复可用、底栏为错误通知。
    // (强制未安装:开发机上可能装有 docker,refresh_status 会把状态翻成
    // 已安装,导致 Docker 行 Enter 被「已安装」短路。)
    sender
        .send(SoftwareInstallMessage::Done(Err("cancelled (demo)".into())))
        .unwrap();
    drop(sender);
    poll_software(&mut app);
    app.software.installed[0] = false;
    assert!(app.software.install.is_none());
    assert!(matches!(
        &app.notice,
        Notice::Error(message) if message == "cancelled (demo)"
    ));

    // 失败后可重新确认并成功:Done(Ok) 后底栏成功通知。
    app.handle_key(key(KeyCode::Enter));
    assert!(app.software.confirming.is_some());
    app.handle_key(key(KeyCode::Enter));
    let sender = mock.install_sender();
    sender.send(SoftwareInstallMessage::Done(Ok(()))).unwrap();
    drop(sender);
    poll_software(&mut app);
    assert!(app.software.install.is_none());
    assert!(matches!(app.notice, Notice::Success(_)));
}

/// 非 root 的手动 mock 让 start_install 在前置检查就被拒绝,不产生 run。
#[test]
fn software_install_rejected_without_root() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("ops-software-root", false);
    let mock = std::sync::Arc::new(MockSoftwareOps::manual(false));
    let mut app = ConsoleApp::new();
    app.software.ops = mock;
    app.software.host = Some(Ok(Host {
        family: Family::Debian,
        codename: None,
    }));
    let confirm = SoftwareConfirm {
        software: crate::software::Software::Docker,
        source: crate::software::DockerSource::Official,
    };
    assert!(app.software.start_install(confirm).is_err());
    assert!(app.software.install.is_none());
}

/// 备份列表的通道回传:Ok 回填 Complete,Err 落入 Failed,Disconnected
/// 报 worker 意外退出。
#[test]
fn backup_list_flow_through_mock_ops() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("ops-backup", false);
    let mock = std::sync::Arc::new(MockBackupOps::manual());
    let mut app = ConsoleApp::new();
    app.backup.ops = mock.clone();

    app.backup.start();
    assert!(matches!(app.backup.state, BackupListState::Running(_)));
    mock.list_sender()
        .send(Ok(vec![sample_backup_entry()]))
        .unwrap();
    poll_backup(&mut app);
    assert!(matches!(
        &app.backup.state,
        BackupListState::Complete(entries) if entries.len() == 1
    ));

    app.backup.start();
    mock.list_sender()
        .send(Err("no installation".into()))
        .unwrap();
    poll_backup(&mut app);
    assert!(
        matches!(&app.backup.state, BackupListState::Failed(message) if message == "no installation")
    );

    // 直接 drop 发送端 = worker 意外退出,列表落入 Failed 而非悬挂在 Running。
    app.backup.start();
    drop(mock.list_sender());
    poll_backup(&mut app);
    assert!(matches!(app.backup.state, BackupListState::Failed(_)));
}

/// 环境检查的通道回传:报告完成后状态落到 Complete,expanded 展开可见。
#[test]
fn preflight_flow_through_mock_ops() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("ops-preflight", false);
    let mock = std::sync::Arc::new(MockPreflightOps::manual());
    let mut app = ConsoleApp::new();
    app.preflight.ops = mock.clone();

    app.preflight.start();
    assert!(matches!(app.preflight.state, PreflightState::Running(_)));
    mock.run_sender().send(error_preflight_report()).unwrap();
    app.preflight.poll();
    assert!(matches!(app.preflight.state, PreflightState::Complete(_)));
}

fn poll_software(app: &mut ConsoleApp) {
    let ConsoleApp {
        software, notice, ..
    } = app;
    software.poll(notice);
}

fn poll_backup(app: &mut ConsoleApp) {
    let ConsoleApp { backup, notice, .. } = app;
    backup.poll(notice);
}

/// 基础包安装:通道回传 Ok 后 run 清理、底栏成功。
#[test]
fn software_base_install_flow_through_mock_ops() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("ops-base", false);
    let mock = std::sync::Arc::new(MockSoftwareOps::manual(true));
    let mut app = ConsoleApp::new();
    app.software.ops = mock.clone();
    app.software.base_packages = super::super::software::BasePackagesState::Chosen(vec![
        crate::software::base::BasePackage::all()[0],
    ]);

    app.software.start_base_install().unwrap();
    assert!(app.software.base_install.is_some());
    mock.base_sender().send(Ok(())).unwrap();
    let ConsoleApp {
        software, notice, ..
    } = &mut app;
    software.poll(notice);
    assert!(app.software.base_install.is_none());
    assert!(matches!(app.notice, Notice::Success(_)));
}

/// 镜像探测与换源后的索引刷新:通道回传可用性映射/刷新结果。
#[test]
fn mirror_probe_and_refresh_flow_through_mock_ops() {
    use crate::mirror::{MirrorName, MirrorStatus};
    use std::collections::HashMap;
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("ops-mirror", false);
    let mock = std::sync::Arc::new(super::super::mirror::MockMirrorOps::manual());
    let mut app = ConsoleApp::new();
    app.mirror.ops = mock.clone();
    app.mirror.host = Some(Ok(Host {
        family: Family::Debian,
        codename: None,
    }));
    app.mirror.detected = true;

    app.mirror.start_probe();
    assert!(app.mirror.probing);
    mock.probe_sender()
        .send(HashMap::from([(
            MirrorName::Official,
            MirrorStatus::Available,
        )]))
        .unwrap();
    app.mirror.poll(&mut app.notice);
    assert!(!app.mirror.probing);
    assert_eq!(
        app.mirror
            .availability
            .as_ref()
            .unwrap()
            .get(&MirrorName::Official),
        Some(&MirrorStatus::Available)
    );

    app.mirror.start_refresh(Family::Debian, &mut app.notice);
    assert!(app.mirror.refreshing.is_some());
    mock.refresh_sender().send(Ok(())).unwrap();
    app.mirror.poll_refresh(&mut app.notice);
    assert!(app.mirror.refreshing.is_none());
    assert!(matches!(app.notice, Notice::Success(_)));
}

/// Update 目标解析:回传升级结果后进入确认层;回传错误落到底栏。
#[test]
fn update_resolution_flow_through_mock_ops() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("ops-update", false);
    let mock = std::sync::Arc::new(super::super::update::MockUpdateOps::manual());
    let mut app = update_ready_app();
    app.update.ops = mock.clone();

    app.start_update_resolution().unwrap();
    assert!(app.update.resolving.is_some());
    mock.resolve_sender()
        .send(Ok(resolved("1.2.3", "1.3.0")))
        .unwrap();
    let ConsoleApp { update, notice, .. } = &mut app;
    update.poll(notice);
    assert!(app.update.resolving.is_none());
    assert!(
        app.update.confirming.is_some(),
        "an upgrade resolution must open the confirmation layer"
    );

    // 已是最新:不打开确认层,只提示。
    app.update.confirming = None;
    app.start_update_resolution().unwrap();
    mock.resolve_sender()
        .send(Ok(resolved("1.2.3", "1.2.3")))
        .unwrap();
    let ConsoleApp { update, notice, .. } = &mut app;
    update.poll(notice);
    assert!(app.update.confirming.is_none());
}

/// 备份创建与校验:进度消息推进、Done 清理 run,校验结果落 Complete。
#[test]
fn backup_create_and_verify_flow_through_mock_ops() {
    use super::super::backup::{BackupCreateMessage, BackupVerifyState};
    use crate::backup::lkb::BackupProgress;
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("ops-backup-create", false);
    let mock = std::sync::Arc::new(MockBackupOps::manual());
    let mut app = ConsoleApp::new();
    app.backup.ops = mock.clone();

    app.backup.start_create("demo remark");
    assert!(app.backup.create.is_some());
    let sender = mock.create_sender();
    sender
        .send(BackupCreateMessage::Progress(BackupProgress::Exporting))
        .unwrap();
    poll_backup(&mut app);
    sender
        .send(BackupCreateMessage::Done(Ok(sample_backup_metadata())))
        .unwrap();
    drop(sender);
    poll_backup(&mut app);
    assert!(app.backup.create.is_none());

    // 校验:列表完成后 R 校验选中条目,结果经通道回传。
    app.backup.state = BackupListState::Complete(vec![sample_backup_entry()]);
    // selected == 0 是「创建备份」动作行,列表条目从 1 开始。
    app.backup.selected = 1;
    app.start_backup_verify();
    assert!(matches!(app.backup.verify, BackupVerifyState::Running(_)));
    mock.verify_sender().send(Ok("verified".into())).unwrap();
    poll_backup(&mut app);
    assert!(matches!(
        &app.backup.verify,
        BackupVerifyState::Complete(Ok(message)) if message == "verified"
    ));
}

/// daemon 部署:确认弹窗 Enter 启动部署 run,结果回传后清理并成功提示。
#[test]
fn daemon_deploy_flow_through_mock_ops() {
    let _language = LanguageGuard::set(Language::En);
    let _territory = DaemonTerritory::new("ops-deploy", false);
    let mock = std::sync::Arc::new(super::super::daemon_panel::MockDeployOps::manual());
    let mut app = ConsoleApp::new();
    app.deploy_ops = mock.clone();

    app.start_daemon_deploy().unwrap();
    assert!(app.deploy_daemon.is_some());
    mock.deploy_sender()
        .send(Ok("daemon deployed".into()))
        .unwrap();
    app.poll_daemon_deploy();
    assert!(app.deploy_daemon.is_none());
    assert!(matches!(app.notice, Notice::Success(_)));
}
