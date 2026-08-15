use super::super::software::*;
use super::super::*;
use super::support::*;
use crate::i18n::Language;
use crate::mirror::{Family, Host};
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
    let _language = LanguageGuard::set(Language::En);
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    let mut app = software_ready_app();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("Host: Ubuntu (noble)"));
    assert!(content.contains("Common software"));
    assert!(content.contains("Docker"));
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
fn software_panel_up_down_clamps_at_single_row() {
    let mut app = software_ready_app();
    assert_eq!(app.software.selected, 0);
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.software.selected, 0, "only one software row");
    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(app.software.selected, 0);
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
        app.notice
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
        app.notice
    );
}

#[test]
fn software_progress_dialog_renders_phase_and_gauge() {
    let _language = LanguageGuard::set(Language::En);
    let (_, receiver) = std::sync::mpsc::channel();
    let mut app = software_ready_app();
    app.software.install = Some(SoftwareInstallRun {
        receiver,
        phase: InstallPhase::InstallingPackages,
        software: Software::Docker,
    });
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("Installing Docker"));
    assert!(content.contains("Installing packages"));
}

#[test]
fn software_rows_are_mouse_clickable() {
    let _language = LanguageGuard::set(Language::En);
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    let mut app = software_ready_app();
    app.focus = Focus::Panel;
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let row = (4..12).find_map(|row| {
        app.hits
            .hit_at(40, row)
            .is_some_and(|hit| hit == Hit::SoftwareField(0))
            .then_some(row)
    });
    let Some(row) = row else {
        panic!("no clickable software row found");
    };
    app.handle_mouse(mouse_click(40, row));
    assert_eq!(app.software.selected, 0);
    assert!(app.software.confirming.is_some());
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
        app.notice
    );
    std::fs::remove_dir_all(&temp).unwrap();
}

#[cfg(feature = "test-support")]
fn unique_suffix() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
}
