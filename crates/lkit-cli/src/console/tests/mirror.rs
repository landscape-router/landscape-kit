use super::super::mirror::*;
use super::super::*;
use super::support::*;
use crate::i18n::Language;
use crate::mirror::{Family, Host, MirrorName};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

/// 面板就绪：进入 Mirror 菜单、聚焦面板并注入确定的主机身份。
fn mirror_ready_app() -> ConsoleApp {
    let mut app = ConsoleApp::new();
    app.menu_index = 4;
    app.focus = Focus::Panel;
    app.mirror.host = Some(Ok(Host {
        family: Family::Ubuntu,
        codename: Some("noble".into()),
    }));
    app.mirror.detected = true;
    app
}

/// Debian 主机（确认层显示 security 开关）。
fn debian_ready_app() -> ConsoleApp {
    let mut app = mirror_ready_app();
    app.mirror.host = Some(Ok(Host {
        family: Family::Debian,
        codename: Some("bookworm".into()),
    }));
    app
}

#[test]
fn mirror_menu_is_navigable_when_not_installed() {
    let mut app = ConsoleApp::new();
    app.snapshot = Snapshot::NotInstalled;
    assert_eq!(app.menu(), Menu::Overview);
    for expected in [Menu::Install, Menu::Backup, Menu::Mirror] {
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(
            app.menu(),
            expected,
            "Update must be skipped when Landscape is not installed"
        );
    }
}

#[test]
fn mirror_menu_is_navigable_when_installed() {
    let mut app = ConsoleApp::new();
    app.snapshot = installed_snapshot();
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.menu(), Menu::Backup);
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.menu(), Menu::Update);
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.menu(), Menu::Mirror);
    assert!(app.menu_available(Menu::Mirror));
}

#[test]
fn mirror_panel_renders_host_and_mirror_options() {
    let _language = LanguageGuard::set(Language::En);
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    let mut app = mirror_ready_app();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("Host: Ubuntu (noble)"));
    assert!(content.contains("Tsinghua TUNA"));
    assert!(content.contains("Aliyun"));
    assert!(content.contains("USTC"));
    assert!(content.contains("Nanjing University"));
    assert!(content.contains("SJTU"));
    assert!(content.contains("Zhejiang University"));
    assert!(content.contains("Lanzhou University"));
    assert!(content.contains("BFSU"));
    assert!(content.contains("HUST"));
    assert!(content.contains("Official"));
    assert!(content.contains("Restore the backed-up original sources"));
}

#[test]
fn mirror_panel_detection_failure_is_shown() {
    let _language = LanguageGuard::set(Language::En);
    let mut app = mirror_ready_app();
    app.mirror.host = Some(Err("no os-release".into()));
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("could not be detected"));
    assert!(content.contains("no os-release"));
}

#[test]
fn mirror_panel_up_down_moves_selection() {
    let mut app = mirror_ready_app();
    let mirrors = MirrorName::all().len();
    assert_eq!(app.mirror.selected, MirrorRow::Mirror(MirrorName::Tuna));
    for _ in 0..mirrors {
        app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    }
    assert_eq!(app.mirror.selected, MirrorRow::Restore, "restore row");
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(
        app.mirror.selected,
        MirrorRow::Restore,
        "clamped at the restore row"
    );
    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(
        app.mirror.selected,
        MirrorRow::Mirror(MirrorName::all()[mirrors - 1]),
        "up from the restore row selects the last mirror"
    );
    for _ in 0..mirrors {
        app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    }
    assert_eq!(
        app.mirror.selected,
        MirrorRow::Mirror(MirrorName::Tuna),
        "clamped at the first mirror"
    );
}

#[test]
fn mirror_panel_enter_opens_apply_confirmation_and_esc_closes() {
    let mut app = mirror_ready_app();
    let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    let escape = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);

    app.handle_key(enter);
    assert_eq!(
        app.mirror.confirming,
        Some(MirrorConfirm::Apply {
            mirror: MirrorName::Tuna,
            replace_security: false,
        })
    );

    app.handle_key(escape);
    assert!(app.mirror.confirming.is_none());
}

#[test]
fn mirror_panel_restore_row_opens_restore_confirmation() {
    let mut app = mirror_ready_app();
    app.mirror.selected = MirrorRow::Restore;
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.mirror.confirming, Some(MirrorConfirm::Restore));
}

#[test]
fn mirror_confirmation_dialog_renders() {
    let _language = LanguageGuard::set(Language::En);
    let mut app = debian_ready_app();
    app.mirror.confirming = Some(MirrorConfirm::Apply {
        mirror: MirrorName::Aliyun,
        replace_security: false,
    });
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("Confirm mirror switch"));
    assert!(content.contains("Aliyun"));
    assert!(content.contains("Press Enter to confirm"));
    assert!(
        content.contains("[ ] Also replace the Debian security repository"),
        "the security toggle must be shown and unchecked by default"
    );
}

#[test]
fn mirror_confirmation_security_toggle_switches_with_space() {
    let _language = LanguageGuard::set(Language::En);
    let mut app = debian_ready_app();
    let space = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE);
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(matches!(
        app.mirror.confirming,
        Some(MirrorConfirm::Apply {
            replace_security: false,
            ..
        })
    ));

    app.handle_key(space);
    assert!(matches!(
        app.mirror.confirming,
        Some(MirrorConfirm::Apply {
            replace_security: true,
            ..
        })
    ));

    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("[x] Also replace the Debian security repository"));

    app.handle_key(space);
    assert!(matches!(
        app.mirror.confirming,
        Some(MirrorConfirm::Apply {
            replace_security: false,
            ..
        })
    ));
}

#[test]
fn mirror_confirmation_hides_security_toggle_for_non_debian() {
    let _language = LanguageGuard::set(Language::En);
    let mut app = mirror_ready_app();
    app.mirror.host = Some(Ok(Host {
        family: Family::Arch,
        codename: None,
    }));
    app.mirror.confirming = Some(MirrorConfirm::Apply {
        mirror: MirrorName::Tuna,
        replace_security: false,
    });
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(!content.contains("security repository"));
    // 非 Debian 家族时 Space 不切换开关。
    app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    assert!(matches!(
        app.mirror.confirming,
        Some(MirrorConfirm::Apply {
            replace_security: false,
            ..
        })
    ));
}

#[cfg(feature = "test-support")]
#[test]
fn mirror_confirmation_dialog_executes_apply() {
    let _language = LanguageGuard::set(Language::En);
    // 注入临时路径并允许非 root：验证真实的备份、重写与恢复流程，不触碰本机源。
    let temp = std::env::temp_dir().join(format!(
        "lkit-console-mirror-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let sources = temp.join("etc/apt/sources.list");
    std::fs::create_dir_all(sources.parent().unwrap()).unwrap();
    let original = concat!(
        "deb http://archive.ubuntu.com/ubuntu noble main universe\n",
        "deb http://security.ubuntu.com/ubuntu noble-security main\n",
    );
    std::fs::write(&sources, original).unwrap();
    let _paths = crate::mirror::test_support::TestPathsGuard::set(crate::mirror::MirrorPaths {
        os_release: temp.join("etc/os-release"),
        backup_root: temp.join("var/lib/lkit/mirror-backup"),
        apt_sources_list: sources.clone(),
        apt_sources_list_d: temp.join("etc/apt/sources.list.d"),
        dnf_repos_dir: temp.join("etc/yum.repos.d"),
        pacman_mirrorlist: temp.join("etc/pacman.d/mirrorlist"),
        restore_root: temp.clone(),
        allow_non_root: true,
    });

    let mut app = mirror_ready_app();
    app.mirror.confirming = Some(MirrorConfirm::Apply {
        mirror: MirrorName::Tuna,
        replace_security: false,
    });
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.mirror.confirming.is_none());
    assert!(
        app.notice
            .contains("switched the Ubuntu package sources to Tsinghua TUNA"),
        "unexpected notice: {}",
        app.notice
    );
    let rewritten = std::fs::read_to_string(&sources).unwrap();
    assert!(
        rewritten.contains("http://mirrors.tuna.tsinghua.edu.cn/ubuntu noble"),
        "source must be rewritten to the mirror: {rewritten}"
    );
    assert!(
        rewritten.contains("http://mirrors.tuna.tsinghua.edu.cn/ubuntu noble-security"),
        "ubuntu security merges into the mirror: {rewritten}"
    );
    let backup = temp.join("var/lib/lkit/mirror-backup/ubuntu/etc/apt/sources.list");
    assert!(
        backup.is_file(),
        "original sources must be backed up before rewriting"
    );
    assert_eq!(std::fs::read_to_string(&backup).unwrap(), original);

    // 恢复：备份内容写回，备份目录删除。
    app.mirror.selected = MirrorRow::Restore;
    app.mirror.confirming = Some(MirrorConfirm::Restore);
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.notice.contains("restored the original package sources"));
    assert_eq!(
        std::fs::read_to_string(&sources).unwrap(),
        original,
        "restore must write the backed-up original back"
    );
    assert!(
        !temp.join("var/lib/lkit/mirror-backup/ubuntu").exists(),
        "backup must be removed after a successful restore"
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

#[test]
fn mirror_rows_are_mouse_clickable() {
    let _language = LanguageGuard::set(Language::En);
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    let mut app = mirror_ready_app();
    app.focus = Focus::Panel;
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    // 在面板区域内找到第一行镜像的命中区。
    let mut first_row = None;
    for row in 4..12 {
        if let Some(Hit::MirrorField(MirrorName::Tuna)) = app.hits.hit_at(40, row) {
            first_row = Some(row);
            break;
        }
    }
    let Some(row) = first_row else {
        panic!("no clickable mirror row found");
    };
    app.handle_mouse(mouse_click(40, row));
    assert_eq!(app.mirror.selected, MirrorRow::Mirror(MirrorName::Tuna));
    assert!(matches!(
        app.mirror.confirming,
        Some(MirrorConfirm::Apply {
            mirror: MirrorName::Tuna,
            replace_security: false,
        })
    ));
    app.mirror.confirming = None;

    app.handle_mouse(mouse_click(40, row + 1));
    assert_eq!(app.mirror.selected, MirrorRow::Mirror(MirrorName::Aliyun));
    app.mirror.confirming = None;

    let restore_hit = (4..28).find_map(|row| {
        app.hits
            .hit_at(40, row)
            .is_some_and(|hit| hit == Hit::MirrorRestore)
            .then_some(row)
    });
    let Some(restore_row) = restore_hit else {
        panic!("no clickable restore row found");
    };
    app.handle_mouse(mouse_click(40, restore_row));
    assert_eq!(app.mirror.selected, MirrorRow::Restore);
    assert_eq!(app.mirror.confirming, Some(MirrorConfirm::Restore));
}
