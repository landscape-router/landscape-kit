use super::super::mirror::*;
use super::super::*;
use super::support::*;
use crate::i18n::Language;
use crate::mirror::{Family, Host, MirrorName, MirrorStatus};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use std::collections::HashMap;

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

/// 注入探测结果：`nju` 不可用、`sjtu` 未知，其余可用。
fn availability_with(nju: MirrorStatus, sjtu: MirrorStatus) -> HashMap<MirrorName, MirrorStatus> {
    let mut statuses: HashMap<_, _> = MirrorName::all()
        .into_iter()
        .map(|mirror| (mirror, MirrorStatus::Available))
        .collect();
    statuses.insert(MirrorName::Nju, nju);
    statuses.insert(MirrorName::Sjtu, sjtu);
    statuses
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
    assert_eq!(
        app.mirror.selected,
        MirrorRow::Mirror(MirrorName::all()[0]),
        "the first mirror in the list order is selected by default"
    );
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
        MirrorRow::Mirror(MirrorName::all()[0]),
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
            mirror: MirrorName::all()[0],
            replace_security: false,
            disable_cdrom: true,
            toggle: 0,
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
        disable_cdrom: true,
        toggle: 0,
    });
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("Confirm mirror switch"));
    assert!(content.contains("Aliyun"));
    assert!(content.contains("Press Enter to confirm"));
    assert!(
        content.contains("[x] Comment out the CD-ROM source entry"),
        "the cdrom toggle must be shown and checked by default"
    );
    assert!(
        content.contains("[ ] Also replace the Debian security repository"),
        "the security toggle must be shown and unchecked by default"
    );
}

#[test]
fn mirror_confirmation_cdrom_toggle_is_checked_and_switches_with_space() {
    let _language = LanguageGuard::set(Language::En);
    let mut app = debian_ready_app();
    let space = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE);
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(matches!(
        app.mirror.confirming,
        Some(MirrorConfirm::Apply {
            replace_security: false,
            disable_cdrom: true,
            toggle: 0,
            ..
        })
    ));

    // 焦点默认在 CD 源行：空格取消勾选（保留 CD 源）。
    app.handle_key(space);
    assert!(matches!(
        app.mirror.confirming,
        Some(MirrorConfirm::Apply {
            disable_cdrom: false,
            toggle: 0,
            ..
        })
    ));

    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("[ ] Comment out the CD-ROM source entry"));

    app.handle_key(space);
    assert!(matches!(
        app.mirror.confirming,
        Some(MirrorConfirm::Apply {
            disable_cdrom: true,
            ..
        })
    ));
}

#[test]
fn mirror_confirmation_security_toggle_switches_with_space() {
    let _language = LanguageGuard::set(Language::En);
    let mut app = debian_ready_app();
    let space = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE);
    let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(matches!(
        app.mirror.confirming,
        Some(MirrorConfirm::Apply {
            replace_security: false,
            disable_cdrom: true,
            toggle: 0,
            ..
        })
    ));

    // 焦点下移到 security 行后再切换。
    app.handle_key(down);
    assert!(matches!(
        app.mirror.confirming,
        Some(MirrorConfirm::Apply { toggle: 1, .. })
    ));
    app.handle_key(space);
    assert!(matches!(
        app.mirror.confirming,
        Some(MirrorConfirm::Apply {
            replace_security: true,
            disable_cdrom: true,
            toggle: 1,
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
            toggle: 1,
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
        disable_cdrom: true,
        toggle: 0,
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
            disable_cdrom: true,
            toggle: 0,
            ..
        })
    ));
}

#[test]
fn mirror_confirmation_shows_cdrom_toggle_for_apt_families() {
    let _language = LanguageGuard::set(Language::En);
    let mut app = mirror_ready_app();
    app.mirror.host = Some(Ok(Host {
        family: Family::Ubuntu,
        codename: Some("noble".into()),
    }));
    app.mirror.confirming = Some(MirrorConfirm::Apply {
        mirror: MirrorName::Tuna,
        replace_security: false,
        disable_cdrom: true,
        toggle: 0,
    });
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(
        content.contains("[x] Comment out the CD-ROM source entry"),
        "Ubuntu hosts get the cdrom toggle too: {content}"
    );
    assert!(
        !content.contains("security repository"),
        "the security toggle stays Debian-only: {content}"
    );
    // Ubuntu 上 Space 只切换 CD 源行。
    app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    assert!(matches!(
        app.mirror.confirming,
        Some(MirrorConfirm::Apply {
            disable_cdrom: false,
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
        skip_refresh: true,
    });

    let mut app = mirror_ready_app();
    app.mirror.confirming = Some(MirrorConfirm::Apply {
        mirror: MirrorName::Tuna,
        replace_security: false,
        disable_cdrom: true,
        toggle: 0,
    });
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.mirror.confirming.is_none());
    assert!(
        app.notice
            .text()
            .contains("switched the Ubuntu package sources to Tsinghua TUNA"),
        "unexpected notice: {}",
        app.notice.text()
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
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::SeqCst)
}

#[cfg(feature = "test-support")]
#[test]
fn mirror_apply_skips_refresh_worker_under_test_injection() {
    let temp = std::env::temp_dir().join(format!(
        "lkit-console-mirror-refresh-{}-{}",
        std::process::id(),
        unique_suffix()
    ));
    let _paths = crate::mirror::test_support::TestPathsGuard::set(crate::mirror::MirrorPaths {
        os_release: temp.join("etc/os-release"),
        backup_root: temp.join("var/lib/lkit/mirror-backup"),
        apt_sources_list: temp.join("etc/apt/sources.list"),
        apt_sources_list_d: temp.join("etc/apt/sources.list.d"),
        dnf_repos_dir: temp.join("etc/yum.repos.d"),
        pacman_mirrorlist: temp.join("etc/pacman.d/mirrorlist"),
        restore_root: temp.clone(),
        allow_non_root: true,
        skip_refresh: true,
    });
    let mut notice = Notice::Success("applied".into());
    let mut panel = MirrorPanel::default();
    panel.start_refresh(Family::Ubuntu, &mut notice);
    assert!(panel.refreshing.is_none());
    assert!(
        notice.contains("package index refreshed"),
        "test injection must complete the refresh synchronously: {}",
        notice.text()
    );
    let _ = std::fs::remove_dir_all(&temp);
}

#[test]
fn mirror_refresh_worker_blocks_until_done() {
    use std::sync::mpsc;
    let (_, receiver) = mpsc::channel();
    let mut app = mirror_ready_app();
    app.mirror.refreshing = Some(MirrorRefreshRun { receiver });
    app.execute_mirror(MirrorConfirm::Restore);
    assert!(
        app.notice.contains("refreshing the package index"),
        "while refreshing, further mirror operations must be blocked with a hint: {}",
        app.notice.text()
    );
}

#[test]
fn mirror_refresh_completion_writes_notice_and_unblocks() {
    let (sender, receiver) = std::sync::mpsc::channel();
    let _ = sender.send(Ok::<(), String>(()));
    let mut app = mirror_ready_app();
    app.mirror.refreshing = Some(MirrorRefreshRun { receiver });
    let mut notice = Notice::Ready;
    app.mirror.poll_refresh(&mut notice);
    assert!(app.mirror.refreshing.is_none());
    assert!(notice.contains("package index refreshed"));
}

#[test]
fn mirror_navigation_skips_unavailable_mirrors() {
    let mut app = mirror_ready_app();
    app.mirror.availability = Some(availability_with(
        MirrorStatus::Unavailable,
        MirrorStatus::Available,
    ));
    app.mirror.selected = MirrorRow::Mirror(MirrorName::Aliyun);
    // 向下：NJU 不可用被跳过（阿里云 → SJTU）。
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.mirror.selected, MirrorRow::Mirror(MirrorName::Sjtu));
    // 不可用行按 Enter：不打开确认层，底栏提示拒绝。
    app.mirror.selected = MirrorRow::Mirror(MirrorName::Nju);
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(
        app.mirror.confirming.is_none(),
        "an unavailable mirror must not open the confirmation layer"
    );
    assert!(
        app.notice.contains("does not provide"),
        "unexpected notice: {}",
        app.notice.text()
    );
    // 恢复动作仍可达。
    app.mirror.selected = MirrorRow::Mirror(MirrorName::Bfsu);
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(
        app.mirror.selected,
        MirrorRow::Mirror(MirrorName::Tuna),
        "TUNA is the last mirror in the list order"
    );
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.mirror.selected, MirrorRow::Restore);
}

#[test]
fn mirror_panel_renders_status_markers() {
    let _language = LanguageGuard::set(Language::En);
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    let mut app = mirror_ready_app();
    app.mirror.availability = Some(availability_with(
        MirrorStatus::Unavailable,
        MirrorStatus::Unknown,
    ));
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(
        content.contains("Nanjing University (unavailable)"),
        "unavailable mirrors must be marked: {content}"
    );
    assert!(
        content.contains("SJTU (unknown)"),
        "unverifiable mirrors must be marked: {content}"
    );
    assert!(
        content.contains("Tsinghua TUNA"),
        "available mirrors stay unmarked: {content}"
    );
}

#[test]
fn mirror_confirm_dialog_warns_when_availability_is_unknown() {
    let _language = LanguageGuard::set(Language::En);
    let mut app = mirror_ready_app();
    app.mirror.availability = Some(availability_with(
        MirrorStatus::Available,
        MirrorStatus::Unknown,
    ));
    app.mirror.confirming = Some(MirrorConfirm::Apply {
        mirror: MirrorName::Sjtu,
        replace_security: false,
        disable_cdrom: true,
        toggle: 0,
    });
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(
        content.contains("availability could not be verified"),
        "an unverifiable mirror must warn in the confirmation dialog: {content}"
    );
    // 已知可用的镜像不显示警告。
    app.mirror.confirming = Some(MirrorConfirm::Apply {
        mirror: MirrorName::Tuna,
        replace_security: false,
        disable_cdrom: true,
        toggle: 0,
    });
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(!content.contains("availability could not be verified"));
}

#[test]
fn mirror_probing_hint_is_shown_until_results_arrive() {
    let _language = LanguageGuard::set(Language::En);
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    let mut app = mirror_ready_app();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let before = terminal_content(&terminal);
    assert!(!before.contains("Checking mirror availability"));
    // 模拟探测 worker 启动中：probing 为 true 但结果未回填。
    app.mirror.probing = true;
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let during = terminal_content(&terminal);
    assert!(
        during.contains("Checking mirror availability"),
        "the probing hint must be shown while probing: {during}"
    );
    app.mirror.probing = false;
    app.mirror.availability = Some(availability_with(
        MirrorStatus::Available,
        MirrorStatus::Available,
    ));
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let after = terminal_content(&terminal);
    assert!(!after.contains("Checking mirror availability"));
}
