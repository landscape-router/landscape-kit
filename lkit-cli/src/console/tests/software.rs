use super::super::software::*;
use super::super::*;
use super::support::*;
use crate::i18n::Language;
use crate::mirror::{Family, Host};
use crate::software::base::{BasePackage, BasePackageDialog};
use crate::software::{DockerSource, InstallPhase, Software};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

/// 面板就绪：进入 Software 菜单、聚焦面板并注入确定的主机身份。
fn software_ready_app() -> ConsoleApp {
    let mut app = ConsoleApp::new();
    app.menu_index = 5;
    app.focus = Focus::Panel;
    app.software.host = Some(Ok(Host {
        family: Family::Ubuntu,
        codename: Some("noble".into()),
    }));
    app.software.detected = true;
    app
}

#[test]
fn software_menu_is_navigable_when_not_installed() {
    let mut app = ConsoleApp::new();
    app.snapshot = Snapshot::NotInstalled;
    for expected in [Menu::Install, Menu::Backup, Menu::Mirror, Menu::Software] {
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.menu(), expected, "menu must reach {expected:?}");
    }
}

#[test]
fn software_menu_is_navigable_when_installed() {
    let mut app = ConsoleApp::new();
    app.snapshot = installed_snapshot();
    for expected in [Menu::Backup, Menu::Update, Menu::Mirror, Menu::Software] {
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(app.menu(), expected, "menu must reach {expected:?}");
    }
    assert!(app.menu_available(Menu::Software));
}

#[test]
fn software_panel_renders_host_and_software() {
    use ratatui::style::Color;
    let _language = LanguageGuard::set(Language::En);
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    let mut app = software_ready_app();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("Host: Ubuntu (noble)"));
    assert!(content.contains("Common software"));
    assert!(content.contains("Docker"));
    // 进入面板时默认选中唯一软件(Docker),行使用 FOCUS_SELECTED 反色高亮。
    let buffer = terminal.backend().buffer();
    let selected = buffer
        .content
        .iter()
        .filter(|cell| cell.symbol() == "D" && cell.bg == Color::Cyan)
        .count();
    assert_eq!(selected, 1, "the selected software row must be highlighted");
}

#[test]
fn software_panel_detection_failure_is_shown() {
    let _language = LanguageGuard::set(Language::En);
    let mut app = software_ready_app();
    app.software.host = Some(Err("no os-release".into()));
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("could not be detected"));
    assert!(content.contains("no os-release"));
}

#[test]
fn software_panel_up_down_navigates_between_rows() {
    let mut app = software_ready_app();
    assert_eq!(app.software.selected, SoftwareRow::Docker);
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.software.selected, SoftwareRow::BasePackages);
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(
        app.software.selected,
        SoftwareRow::BasePackages,
        "cursor must clamp at the last row"
    );
    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(app.software.selected, SoftwareRow::Docker);
    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(
        app.software.selected,
        SoftwareRow::Docker,
        "cursor must clamp at the first row"
    );
}

#[test]
fn software_panel_enter_opens_confirmation_and_esc_closes() {
    let mut app = software_ready_app();
    let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    let escape = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);

    app.handle_key(enter);
    assert_eq!(
        app.software.confirming,
        Some(SoftwareConfirm {
            software: Software::Docker,
            source: DockerSource::Official,
        })
    );

    app.handle_key(escape);
    assert!(app.software.confirming.is_none());
}

#[test]
fn software_panel_enter_on_installed_shows_notice() {
    let _language = LanguageGuard::set(Language::En);
    let mut app = software_ready_app();
    app.software.installed[0] = true;
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.software.confirming.is_none());
    assert!(
        app.notice.contains("Docker is already installed"),
        "unexpected notice: {}",
        app.notice.text()
    );
}

#[test]
fn software_confirmation_source_cycles_with_space_and_left() {
    let mut app = software_ready_app();
    let space = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE);
    let left = KeyEvent::new(KeyCode::Left, KeyModifiers::NONE);
    let right = KeyEvent::new(KeyCode::Right, KeyModifiers::NONE);

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        app.software.confirming.map(|confirm| confirm.source),
        Some(DockerSource::Official)
    );

    for expected in [
        DockerSource::Aliyun,
        DockerSource::Tencent,
        DockerSource::Huawei,
        DockerSource::Tuna,
        DockerSource::Ustc,
        DockerSource::Official,
    ] {
        app.handle_key(space);
        assert_eq!(
            app.software.confirming.map(|confirm| confirm.source),
            Some(expected)
        );
    }

    app.handle_key(left);
    assert_eq!(
        app.software.confirming.map(|confirm| confirm.source),
        Some(DockerSource::Ustc)
    );
    app.handle_key(right);
    assert_eq!(
        app.software.confirming.map(|confirm| confirm.source),
        Some(DockerSource::Official)
    );
}

#[test]
fn software_confirmation_dialog_renders() {
    let _language = LanguageGuard::set(Language::En);
    let mut app = software_ready_app();
    app.software.confirming = Some(SoftwareConfirm {
        software: Software::Docker,
        source: DockerSource::Tuna,
    });
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("Install Docker"));
    assert!(content.contains("Source"));
    assert!(content.contains("Tsinghua TUNA mirror"));
    assert!(content.contains("Press Space or"));
    assert!(content.contains("Press Enter to install"));
}

#[test]
fn software_confirmation_enter_after_detection_failure_shows_notice() {
    let _language = LanguageGuard::set(Language::En);
    let mut app = software_ready_app();
    app.software.host = Some(Err("no os-release".into()));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.software.confirming.is_some());
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.software.confirming.is_none());
    assert!(app.software.install.is_none(), "no worker must be started");
    assert!(
        app.notice.contains("could not be detected"),
        "unexpected notice: {}",
        app.notice.text()
    );
}

#[test]
fn software_progress_dialog_renders_phase_gauge_and_esc_cancel_hint() {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    let _language = LanguageGuard::set(Language::En);
    let (_, receiver) = std::sync::mpsc::channel();
    let mut app = software_ready_app();
    app.software.install = Some(SoftwareInstallRun {
        receiver,
        phase: InstallPhase::InstallingPackages,
        software: Software::Docker,
        cancel: Arc::new(AtomicBool::new(false)),
    });
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("Installing Docker"));
    assert!(content.contains("Installing packages"));
    assert!(
        content.contains("Esc Cancel installation"),
        "the progress dialog must show the Esc cancel hint"
    );
}

#[test]
fn software_esc_opens_cancel_layer_and_enter_sets_the_cancel_flag() {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    let mut app = software_ready_app();
    let cancel = Arc::new(AtomicBool::new(false));
    let (_, receiver) = std::sync::mpsc::channel();
    app.software.install = Some(SoftwareInstallRun {
        receiver,
        phase: InstallPhase::InstallingPackages,
        software: Software::Docker,
        cancel: Arc::clone(&cancel),
    });

    // 安装中 Esc 打开取消确认层。
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.software.cancel_confirming);
    assert!(!cancel.load(std::sync::atomic::Ordering::Relaxed));

    // 确认层 Esc 关闭,标志不置位,安装继续。
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!app.software.cancel_confirming);
    assert!(!cancel.load(std::sync::atomic::Ordering::Relaxed));

    // 再次 Esc 打开后 Enter 确认取消。
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.software.cancel_confirming);
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(!app.software.cancel_confirming);
    assert!(
        cancel.load(std::sync::atomic::Ordering::Relaxed),
        "confirming the cancel must set the worker cancel flag"
    );
}

#[test]
fn software_cancel_layer_renders() {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    let _language = LanguageGuard::set(Language::En);
    let mut app = software_ready_app();
    let (_, receiver) = std::sync::mpsc::channel();
    app.software.install = Some(SoftwareInstallRun {
        receiver,
        phase: InstallPhase::Preparing,
        software: Software::Docker,
        cancel: Arc::new(AtomicBool::new(false)),
    });
    app.software.cancel_confirming = true;
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("Cancel the installation?"));
    assert!(content.contains("Press Enter to cancel."));
}

#[test]
fn software_cancel_after_confirm_allows_reselecting_source() {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    let mut app = software_ready_app();
    let cancel = Arc::new(AtomicBool::new(false));
    let (_, receiver) = std::sync::mpsc::channel();
    app.software.install = Some(SoftwareInstallRun {
        receiver,
        phase: InstallPhase::InstallingPackages,
        software: Software::Docker,
        cancel: Arc::clone(&cancel),
    });
    // 取消后 worker 结束:模拟 Done(Err(cancelled))。
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let (sender, new_receiver) = std::sync::mpsc::channel();
    let _ = sender.send(SoftwareInstallMessage::Done(Err(crate::tr!(
        crate::keys::SOFTWARE_CANCELLED
    )
    .to_string())));
    app.software.install = Some(SoftwareInstallRun {
        receiver: new_receiver,
        phase: InstallPhase::InstallingPackages,
        software: Software::Docker,
        cancel: Arc::clone(&cancel),
    });
    app.update();
    assert!(app.software.install.is_none());
    assert_eq!(
        app.notice.text(),
        crate::tr!(crate::keys::SOFTWARE_CANCELLED),
        "the cancellation notice must be shown"
    );
    // 面板恢复:未安装状态下可重新按 Enter 打开来源选择。
    // (refresh_status 会做真实检测,测试环境可能已装 docker,这里强制未装。)
    app.software.installed = vec![false];
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(
        app.software.confirming.is_some(),
        "after cancellation the panel must allow reselecting the source"
    );
}

#[cfg(feature = "test-support")]
#[test]
fn software_confirmation_enter_with_non_root_policy_shows_notice() {
    let _language = LanguageGuard::set(Language::En);
    // 注入临时路径但保持非 root 权限策略：确认 Enter 必须拒绝并提示 root 权限，
    // 且不启动任何安装 worker。
    let temp = std::env::temp_dir().join(format!(
        "lkit-console-software-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    std::fs::create_dir_all(&temp).unwrap();
    let _paths =
        crate::software::test_support::TestPathsGuard::set(crate::software::SoftwarePaths {
            os_release: temp.join("os-release"),
            apt_keyrings_dir: temp.join("etc/apt/keyrings"),
            apt_sources_list_d: temp.join("etc/apt/sources.list.d"),
            dnf_repos_dir: temp.join("etc/yum.repos.d"),
            docker_bin: vec![temp.join("usr/bin/docker")],
            allow_non_root: false,
        });

    let mut app = software_ready_app();
    app.software.confirming = Some(SoftwareConfirm {
        software: Software::Docker,
        source: DockerSource::Official,
    });
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.software.confirming.is_none());
    assert!(app.software.install.is_none(), "no worker must be started");
    assert!(
        app.notice.contains("root privileges are required"),
        "unexpected notice: {}",
        app.notice.text()
    );
    std::fs::remove_dir_all(&temp).unwrap();
}

#[cfg(feature = "test-support")]
fn unique_suffix() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::SeqCst)
}

fn controlled_base_dialog() -> BasePackageDialog {
    use crate::software::base::BasePackageEntry;
    BasePackageDialog {
        entries: vec![
            BasePackageEntry {
                package: BasePackage::Ppp,
                installed: false,
                selected: true,
            },
            BasePackageEntry {
                package: BasePackage::Iproute2,
                installed: true,
                selected: true,
            },
            BasePackageEntry {
                package: BasePackage::Iw,
                installed: false,
                selected: false,
            },
        ],
        cursor: 0,
    }
}

fn choosing_base_dialog(previous: BasePackagesState) -> BasePackagesState {
    BasePackagesState::Choosing {
        dialog: controlled_base_dialog(),
        previous: Box::new(previous),
    }
}

#[test]
fn software_panel_renders_base_packages_row_with_missing_count() {
    let _language = LanguageGuard::set(Language::En);
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    let mut app = software_ready_app();
    app.software.base_installed = vec![false, true, false, false, true];
    app.software.selected = SoftwareRow::BasePackages;
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("Base packages"));
    assert!(content.contains("3 missing"));
}

#[test]
fn base_packages_dialog_renders_installed_and_selected_rows() {
    let _language = LanguageGuard::set(Language::En);
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    let mut app = software_ready_app();
    app.software.base_packages = choosing_base_dialog(BasePackagesState::NotChosen);
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("Base packages"));
    assert!(content.contains("pppd (ppp)"));
    assert!(content.contains("[x]"));
    assert!(content.contains("ip (iproute2)"));
    assert!(content.contains("✓"));
    assert!(content.contains("installed"));
    assert!(content.contains("[ ]"));
    assert!(content.contains("Install selected packages"));
    assert!(content.contains("Space toggles a package"));
}

#[test]
fn base_packages_dialog_toggles_and_confirms_selection() {
    let mut app = software_ready_app();
    app.software.base_packages = choosing_base_dialog(BasePackagesState::NotChosen);

    // 第一行(ppp 缺失已勾选):Space 取消勾选。
    app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    let dialog = app.software.base_dialog_mut().unwrap();
    assert!(!dialog.entries[0].selected);

    // 已安装的行不可切换。
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    let dialog = app.software.base_dialog_mut().unwrap();
    assert!(
        dialog.entries[1].selected,
        "installed rows cannot be toggled"
    );

    // 移到 iw 行勾选,再移到确认行提交。
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let dialog = app.software.base_dialog_mut().unwrap();
    assert!(dialog.entries[2].selected);

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let BasePackagesState::Choosing { dialog, .. } = &app.software.base_packages else {
        panic!("dialog must stay open");
    };
    assert!(dialog.on_confirm_row());
}

#[test]
fn base_packages_dialog_confirm_starts_install_with_selection() {
    let _language = LanguageGuard::set(Language::En);
    let mut app = software_ready_app();
    app.software.base_packages = choosing_base_dialog(BasePackagesState::NotChosen);
    // 移到 iw 行勾选,再移到确认行后 Enter:弹框关闭,启动后台安装
    // (非 root 权限策略下会拒绝并提示)。
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let BasePackagesState::Chosen(packages) = &app.software.base_packages else {
        panic!("confirm must close the dialog with the selection");
    };
    assert_eq!(packages, &[BasePackage::Ppp, BasePackage::Iw]);
    if app.software.base_install.is_none() {
        assert!(
            app.notice.contains("root privileges are required"),
            "unexpected notice: {}",
            app.notice.text()
        );
    }
}

#[test]
fn base_packages_dialog_esc_restores_previous_choice() {
    let mut app = software_ready_app();
    app.software.base_packages =
        choosing_base_dialog(BasePackagesState::Chosen(vec![BasePackage::Ppp]));
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    let BasePackagesState::Chosen(packages) = &app.software.base_packages else {
        panic!("Esc must restore the previous choice");
    };
    assert_eq!(packages, &[BasePackage::Ppp]);

    app.software.base_packages = choosing_base_dialog(BasePackagesState::NotChosen);
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(matches!(
        app.software.base_packages,
        BasePackagesState::NotChosen
    ));
}

#[test]
fn base_packages_enter_on_row_opens_dialog_and_navigates_back_to_docker() {
    let mut app = software_ready_app();
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.software.selected, SoftwareRow::BasePackages);
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(matches!(
        app.software.base_packages,
        BasePackagesState::Choosing { .. }
    ));
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(matches!(
        app.software.base_packages,
        BasePackagesState::NotChosen
    ));
    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(app.software.selected, SoftwareRow::Docker);
}

#[test]
fn base_packages_progress_dialog_renders_and_esc_opens_cancel_layer() {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    let _language = LanguageGuard::set(Language::En);
    let (_, receiver) = std::sync::mpsc::channel();
    let mut app = software_ready_app();
    app.software.base_install = Some(BasePackagesInstallRun {
        receiver,
        cancel: Arc::new(AtomicBool::new(false)),
    });
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("Installing base packages"));
    assert!(content.contains("Esc Cancel installation"));

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.software.base_cancel_confirming);
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!app.software.base_cancel_confirming);
}

#[test]
fn base_packages_cancel_confirm_sets_the_cancel_flag() {
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    let mut app = software_ready_app();
    let cancel = Arc::new(AtomicBool::new(false));
    let (_, receiver) = std::sync::mpsc::channel();
    app.software.base_install = Some(BasePackagesInstallRun {
        receiver,
        cancel: Arc::clone(&cancel),
    });
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(!app.software.base_cancel_confirming);
    assert!(
        cancel.load(std::sync::atomic::Ordering::Relaxed),
        "confirming the cancel must set the worker cancel flag"
    );
}
