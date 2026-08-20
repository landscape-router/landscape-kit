use super::super::daemon_panel::PskDialogField;
use super::super::*;
use super::support::*;
use crate::check::model::{CheckGroup, CheckReport, CheckResult, Status, StatusCounts};
use crate::deployment::layout;
use crate::i18n::Language;
use ratatui::backend::TestBackend;
use std::time::Duration;

/// 隔离 lkit 地盘并写入指定 pidfile 内容,返回守卫。
fn territory_with_pidfile(
    name: &str,
    content: &str,
) -> (layout::TerritoryOverride, std::path::PathBuf) {
    let territory = std::env::temp_dir().join(format!(
        "lkit-console-daemon-test-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&territory);
    std::fs::create_dir_all(territory.join("run")).unwrap();
    let guard = layout::test_territory(&territory);
    let pidfile = layout::territory_pidfile();
    std::fs::write(&pidfile, content).unwrap();
    (guard, territory)
}

/// 预检报告:daemon 未运行(唯一阻断项)。
fn daemon_blocked_report() -> CheckReport {
    CheckReport {
        groups: vec![CheckGroup {
            title: "lkit resident service".to_string(),
            results: vec![CheckResult::new("service.lkit_daemon", "lkit daemon")
                .set(
                    Status::Error,
                    "not running",
                    "the lkit daemon is not running; install and lifecycle commands cannot be delegated to it",
                )
                .suggestion("run `lkit self install` to deploy and start the daemon")],
        }],
        summary: Status::Error,
        counts: StatusCounts {
            pass: 0,
            warning: 0,
            error: 1,
            unknown: 0,
        },
    }
}

#[test]
fn overview_shows_daemon_status_and_deploy_row_when_not_running() {
    let _language = LanguageGuard::set(Language::En);
    let (_guard, territory) = territory_with_pidfile("not-running", "99999999\n");
    let backend = TestBackend::new(100, 28);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut app = ConsoleApp::new();
    app.focus = Focus::Panel;
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("lkit daemon is not running"));
    assert!(content.contains("[ Deploy the lkit daemon ]"));
    let row = content
        .lines()
        .position(|line| line.contains("[ Deploy the lkit daemon ]"))
        .expect("deploy row must render") as u16;
    // 双栏布局:部署动作行位于右栏(约 62 列起)。
    assert_eq!(
        app.hits.hit_at(65, row),
        Some(Hit::OverviewDeploy),
        "the deploy row must be clickable"
    );
    drop(_guard);
    let _ = std::fs::remove_dir_all(&territory);
}

#[test]
fn overview_shows_running_without_deploy_row_when_daemon_is_alive() {
    let _language = LanguageGuard::set(Language::En);
    let (_guard, territory) =
        territory_with_pidfile("running", &format!("{}\n", std::process::id()));
    let backend = TestBackend::new(100, 28);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut app = ConsoleApp::new();
    app.focus = Focus::Panel;
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("lkit daemon: running"));
    assert!(!content.contains("[ Deploy the lkit daemon ]"));
    drop(_guard);
    let _ = std::fs::remove_dir_all(&territory);
}

#[test]
fn overview_enter_opens_confirm_and_esc_cancels() {
    let _language = LanguageGuard::set(Language::En);
    let (_guard, territory) = territory_with_pidfile("confirm", "99999999\n");
    let mut app = ConsoleApp::new();
    app.focus = Focus::Panel;
    assert!(!app.deploy_daemon_confirming);

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(
        app.deploy_daemon_confirming,
        "Enter must open the confirm layer"
    );

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(
        !app.deploy_daemon_confirming,
        "Esc must close the confirm layer"
    );
    assert_eq!(app.exit_state, ExitState::Idle);

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.deploy_daemon_confirming);
    let backend = TestBackend::new(100, 28);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("Deploy the lkit daemon?"));
    drop(_guard);
    let _ = std::fs::remove_dir_all(&territory);
}

#[test]
fn deploy_confirm_runs_in_background_and_writes_the_result_to_the_notice() {
    let _language = LanguageGuard::set(Language::En);
    let (_guard, territory) = territory_with_pidfile("deploy", "99999999\n");
    let mut app = ConsoleApp::new();
    app.focus = Focus::Panel;
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.deploy_daemon_confirming);
    // 空恢复码(留空自动生成):直接下移到「开始部署」动作行并确认。
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(
        app.deploy_daemon.is_some(),
        "confirming the deploy must start the background worker"
    );
    assert!(!app.deploy_daemon_confirming);

    for _ in 0..100 {
        app.update();
        if app.deploy_daemon.is_none() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(app.deploy_daemon.is_none(), "the deploy worker must finish");
    // 非 root 测试环境与 CLI 相同:部署要求 root。
    assert!(
        app.notice.contains("root") || app.notice.contains("daemon deploy worker"),
        "unexpected notice: {}",
        app.notice
    );
    drop(_guard);
    let _ = std::fs::remove_dir_all(&territory);
}

#[test]
fn running_daemon_enter_opens_the_show_psk_dialog() {
    let _language = LanguageGuard::set(Language::En);
    let (_guard, territory) =
        territory_with_pidfile("enter-ignored", &format!("{}\n", std::process::id()));
    let mut app = ConsoleApp::new();
    app.focus = Focus::Panel;
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(
        !app.deploy_daemon_confirming,
        "Enter must not open the deploy confirm when the daemon is running"
    );
    assert!(
        app.show_psk,
        "Enter must open the show psk dialog when the daemon is running"
    );
    drop(_guard);
    let _ = std::fs::remove_dir_all(&territory);
}

#[test]
fn mouse_click_on_deploy_row_opens_the_confirm_layer() {
    let _language = LanguageGuard::set(Language::En);
    let (_guard, territory) = territory_with_pidfile("mouse", "99999999\n");
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    let mut app = ConsoleApp::new();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    let row = content
        .lines()
        .position(|line| line.contains("[ Deploy the lkit daemon ]"))
        .expect("deploy row must render") as u16;
    assert_eq!(
        app.hits.hit_at(65, row),
        Some(Hit::OverviewDeploy),
        "the deploy row must be clickable"
    );
    app.handle_mouse(mouse_click(65, row));
    assert!(app.deploy_daemon_confirming);
    drop(_guard);
    let _ = std::fs::remove_dir_all(&territory);
}

#[test]
fn preflight_dialog_shows_deploy_button_when_the_daemon_check_blocks() {
    let _language = LanguageGuard::set(Language::En);
    let (_guard, territory) = territory_with_pidfile("dialog", "99999999\n");
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    let mut app = ConsoleApp::new();
    app.menu_index = 1;
    app.focus = Focus::Panel;
    app.preflight.state = PreflightState::Complete(daemon_blocked_report());
    app.preflight_dialog = true;
    assert!(
        app.preflight_daemon_blocked(),
        "the daemon check must be recognized as the blocker"
    );

    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("Install blocked"));
    assert!(content.contains("[ Deploy the lkit daemon ]"));
    assert!(content.contains("Enter deploy daemon"));
    let row = content
        .lines()
        .position(|line| line.contains("[ Deploy the lkit daemon ]"))
        .expect("the deploy button must render in the dialog") as u16;
    assert_eq!(
        app.hits.hit_at(50, row),
        Some(Hit::DeployDaemon),
        "the dialog deploy button must be clickable"
    );
    drop(_guard);
    let _ = std::fs::remove_dir_all(&territory);
}

#[test]
fn preflight_dialog_enter_opens_the_deploy_confirm_and_confirms_starts_deploy() {
    let _language = LanguageGuard::set(Language::En);
    let (_guard, territory) = territory_with_pidfile("dialog-enter", "99999999\n");
    let mut app = ConsoleApp::new();
    app.menu_index = 1;
    app.focus = Focus::Panel;
    app.preflight.state = PreflightState::Complete(daemon_blocked_report());
    app.preflight_dialog = true;

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(
        !app.preflight_dialog,
        "Enter must close the blocking dialog"
    );
    assert!(
        !app.preflight.expanded,
        "Enter must open the deploy confirm instead of the check list"
    );
    assert!(
        app.deploy_daemon_confirming,
        "Enter must open the deploy confirm dialog"
    );
    assert!(
        app.deploy_daemon.is_none(),
        "the deploy must not start before the confirm dialog is confirmed"
    );
    // 空恢复码(留空自动生成):下移到「开始部署」并确认。
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(
        app.deploy_daemon.is_some(),
        "confirming must start the daemon deploy in the background"
    );

    for _ in 0..100 {
        app.update();
        if app.deploy_daemon.is_none() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(app.deploy_daemon.is_none(), "the deploy worker must finish");
    assert!(
        app.notice.contains("root") || app.notice.contains("daemon deploy worker"),
        "unexpected notice: {}",
        app.notice
    );
    drop(_guard);
    let _ = std::fs::remove_dir_all(&territory);
}

#[test]
fn preflight_dialog_d_key_opens_the_deploy_confirm_and_confirms_starts_deploy() {
    let _language = LanguageGuard::set(Language::En);
    let (_guard, territory) = territory_with_pidfile("dialog-deploy", "99999999\n");
    let mut app = ConsoleApp::new();
    app.menu_index = 1;
    app.focus = Focus::Panel;
    app.preflight.state = PreflightState::Complete(daemon_blocked_report());
    app.preflight_dialog = true;

    app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    assert!(!app.preflight_dialog, "D must close the blocking dialog");
    assert!(
        app.deploy_daemon_confirming,
        "D must open the deploy confirm dialog"
    );
    // 输入恢复码与二次确认后部署。
    for character in "an-operator-chosen-code".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    for character in "an-operator-chosen-code".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(
        app.deploy_daemon.is_some(),
        "confirming must start the daemon deploy in the background"
    );
    assert_eq!(
        app.deploy_psk, "an-operator-chosen-code",
        "the psk entered in the dialog must be passed to the deploy"
    );

    for _ in 0..100 {
        app.update();
        if app.deploy_daemon.is_none() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(app.deploy_daemon.is_none(), "the deploy worker must finish");
    assert!(
        app.notice.contains("root") || app.notice.contains("daemon deploy worker"),
        "unexpected notice: {}",
        app.notice
    );
    drop(_guard);
    let _ = std::fs::remove_dir_all(&territory);
}

#[test]
fn preflight_dialog_mouse_click_on_deploy_button_opens_the_confirm_dialog() {
    let _language = LanguageGuard::set(Language::En);
    let (_guard, territory) = territory_with_pidfile("dialog-mouse", "99999999\n");
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    let mut app = ConsoleApp::new();
    app.menu_index = 1;
    app.focus = Focus::Panel;
    app.preflight.state = PreflightState::Complete(daemon_blocked_report());
    app.preflight_dialog = true;
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    let row = content
        .lines()
        .position(|line| line.contains("[ Deploy the lkit daemon ]"))
        .expect("the deploy button must render in the dialog") as u16;
    app.handle_mouse(mouse_click(50, row));
    assert!(
        !app.preflight_dialog,
        "clicking the button must close the dialog"
    );
    assert!(
        app.deploy_daemon_confirming,
        "clicking the button must open the deploy confirm dialog"
    );
    assert!(
        app.deploy_daemon.is_none(),
        "the deploy must not start until the confirm dialog is confirmed"
    );
    drop(_guard);
    let _ = std::fs::remove_dir_all(&territory);
}

#[test]
fn deploy_confirm_dialog_renders_on_the_install_menu_from_the_preflight_path() {
    let _language = LanguageGuard::set(Language::En);
    let (_guard, territory) = territory_with_pidfile("dialog-render", "99999999\n");
    let mut app = ConsoleApp::new();
    app.menu_index = 1;
    app.focus = Focus::Panel;
    app.preflight.state = PreflightState::Complete(daemon_blocked_report());
    app.preflight_dialog = true;
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.deploy_daemon_confirming);
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("Deploy the lkit daemon?"));
    assert!(content.contains("Confirm psk"));
    assert!(content.contains("[ Start deployment ]"));
    drop(_guard);
    let _ = std::fs::remove_dir_all(&territory);
}

#[test]
fn f_opens_the_flare_dialog_on_the_overview_panel() {
    let _language = LanguageGuard::set(Language::En);
    let (_guard, territory) = territory_with_pidfile("flare-open", "99999999\n");
    let mut app = ConsoleApp::new();
    app.focus = Focus::Panel;
    assert!(!app.flare.open);

    app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
    assert!(app.flare.open, "f must open the flare dialog");

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!app.flare.open, "Esc must close the flare dialog");
    drop(_guard);
    let _ = std::fs::remove_dir_all(&territory);
}

#[test]
fn flare_dialog_renders_the_current_configuration() {
    let _language = LanguageGuard::set(Language::En);
    let (_guard, territory) = territory_with_pidfile("flare-render", "99999999\n");
    let mut app = ConsoleApp::new();
    app.focus = Focus::Panel;
    app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("Flare recovery channel"));
    assert!(content.contains("Flare recovery psk"));
    assert!(content.contains("<not configured>"));
    drop(_guard);
    let _ = std::fs::remove_dir_all(&territory);
}

#[test]
fn flare_dialog_edits_and_saves_the_psk_into_the_config() {
    let _language = LanguageGuard::set(Language::En);
    let (_guard, territory) = territory_with_pidfile("flare-save", "99999999\n");
    let mut app = ConsoleApp::new();
    app.focus = Focus::Panel;
    app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE));
    for character in "an-operator-chosen-secret".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
    assert!(!app.flare.open, "saving must close the dialog");
    let section = crate::deployment::config::load_flare().unwrap();
    assert_eq!(
        section.psk.as_deref(),
        Some("an-operator-chosen-secret"),
        "the edited psk must be persisted to config.toml"
    );
    drop(_guard);
    let _ = std::fs::remove_dir_all(&territory);
}

#[test]
fn flare_dialog_rejects_a_short_psk_and_stays_open() {
    let _language = LanguageGuard::set(Language::En);
    let (_guard, territory) = territory_with_pidfile("flare-short", "99999999\n");
    let mut app = ConsoleApp::new();
    app.focus = Focus::Panel;
    app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE));
    for character in "short".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
    assert!(app.flare.open, "a rejected save must keep the dialog open");
    assert!(
        app.flare.notice.contains("at least 12 characters"),
        "unexpected notice: {}",
        app.flare.notice
    );
    assert!(crate::deployment::config::load_flare().is_none());
    drop(_guard);
    let _ = std::fs::remove_dir_all(&territory);
}

#[test]
fn deploy_confirm_shows_the_recovery_code_field_and_explains_its_purpose() {
    let _language = LanguageGuard::set(Language::En);
    let (_guard, territory) = territory_with_pidfile("deploy-flare-render", "99999999\n");
    let mut app = ConsoleApp::new();
    app.focus = Focus::Panel;
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.deploy_daemon_confirming);
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("Flare recovery psk"));
    assert!(content.contains("Confirm psk"));
    assert!(content.contains("recovery channel"));
    drop(_guard);
    let _ = std::fs::remove_dir_all(&territory);
}

#[test]
fn deploy_confirm_accepts_an_edited_recovery_code_and_starts_the_deploy() {
    let _language = LanguageGuard::set(Language::En);
    let (_guard, territory) = territory_with_pidfile("deploy-flare-edit", "99999999\n");
    let mut app = ConsoleApp::new();
    app.focus = Focus::Panel;
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    // psk 字段:直接输入进入编辑,Enter 提交编辑。
    for character in "an-operator-chosen-code".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    // Down 到二次确认字段:输入后提交。
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    for character in "an-operator-chosen-code".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    // Down 到「开始部署」动作行,Enter 启动部署。
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(
        app.deploy_daemon.is_some(),
        "confirming with a matching recovery code must start the deploy"
    );
    assert_eq!(app.deploy_psk, "an-operator-chosen-code");
    assert_eq!(app.deploy_psk_confirmation, "an-operator-chosen-code");
    for _ in 0..100 {
        app.update();
        if app.deploy_daemon.is_none() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(app.deploy_daemon.is_none(), "the deploy worker must finish");
    drop(_guard);
    let _ = std::fs::remove_dir_all(&territory);
}

#[test]
fn deploy_confirm_enter_edits_instead_of_deploying_and_arrows_navigate() {
    let _language = LanguageGuard::set(Language::En);
    let (_guard, territory) = territory_with_pidfile("deploy-flare-arrows", "99999999\n");
    let mut app = ConsoleApp::new();
    app.focus = Focus::Panel;
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    // 弹窗打开即聚焦 psk 字段:Enter 进入编辑而不是直接部署。
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(
        app.deploy_daemon.is_none(),
        "Enter on a field must not deploy directly"
    );
    assert!(
        app.deploy_psk_editing,
        "Enter must start editing the psk field"
    );
    // 提交编辑后 Down 到二次确认字段,直接输入落到确认字段。
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.deploy_psk_field, PskDialogField::Confirmation);
    app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
    assert_eq!(app.deploy_psk, "", "typing must not touch the psk field");
    assert_eq!(app.deploy_psk_confirmation, "xy");
    // 提交后 Up 回到 psk 字段,BackTab 再从 Action 回到 Confirmation。
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(app.deploy_psk_field, PskDialogField::Psk);
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(app.deploy_psk_field, PskDialogField::Action);
    app.handle_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
    assert_eq!(app.deploy_psk_field, PskDialogField::Confirmation);
    drop(_guard);
    let _ = std::fs::remove_dir_all(&territory);
}

#[test]
fn deploy_confirm_rejects_a_mismatched_confirmation_without_starting() {
    let _language = LanguageGuard::set(Language::En);
    let (_guard, territory) = territory_with_pidfile("deploy-flare-mismatch", "99999999\n");
    let mut app = ConsoleApp::new();
    app.focus = Focus::Panel;
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    for character in "an-operator-chosen-code".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    for character in "a-different-code".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(
        app.deploy_daemon.is_none(),
        "a mismatched confirmation must not start the deploy"
    );
    assert!(
        app.deploy_daemon_confirming,
        "the dialog must stay open on a mismatch"
    );
    assert!(
        app.notice.contains("do not match"),
        "unexpected notice: {}",
        app.notice
    );
    drop(_guard);
    let _ = std::fs::remove_dir_all(&territory);
}

#[test]
fn deploy_confirm_rejects_a_short_recovery_code_without_starting() {
    let _language = LanguageGuard::set(Language::En);
    let (_guard, territory) = territory_with_pidfile("deploy-flare-short", "99999999\n");
    let mut app = ConsoleApp::new();
    app.focus = Focus::Panel;
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    for character in "eshort".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(
        app.deploy_daemon.is_none(),
        "a short recovery code must not start the deploy"
    );
    assert!(app.deploy_daemon_confirming, "the dialog must stay open");
    assert!(
        app.notice.contains("at least 12 characters"),
        "unexpected notice: {}",
        app.notice
    );
    drop(_guard);
    let _ = std::fs::remove_dir_all(&territory);
}

#[test]
fn deploy_confirm_prefills_an_existing_recovery_code() {
    let _language = LanguageGuard::set(Language::En);
    let (_guard, territory) = territory_with_pidfile("deploy-flare-prefill", "99999999\n");
    crate::deployment::config::save_flare(&crate::deployment::config::FlareSection {
        psk: Some("an-existing-recovery-code".into()),
        ..crate::deployment::config::default_flare_section()
    })
    .unwrap();
    let mut app = ConsoleApp::new();
    app.focus = Focus::Panel;
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        app.deploy_psk, "an-existing-recovery-code",
        "an existing recovery code must be pre-filled"
    );
    assert_eq!(
        app.deploy_psk_confirmation, "",
        "the confirmation field must start empty"
    );
    drop(_guard);
    let _ = std::fs::remove_dir_all(&territory);
}

#[test]
fn overview_shows_show_psk_row_when_daemon_is_alive() {
    let _language = LanguageGuard::set(Language::En);
    let (_guard, territory) =
        territory_with_pidfile("show-psk-row", &format!("{}\n", std::process::id()));
    let backend = TestBackend::new(100, 28);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut app = ConsoleApp::new();
    app.focus = Focus::Panel;
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("[ Show recovery psk ]"));
    assert!(!content.contains("[ Deploy the lkit daemon ]"));
    let row = content
        .lines()
        .position(|line| line.contains("[ Show recovery psk ]"))
        .expect("the show psk row must render") as u16;
    // 双栏布局:动作行位于右栏(约 62 列起)。
    assert_eq!(
        app.hits.hit_at(65, row),
        Some(Hit::OverviewShowPsk),
        "the show psk row must be clickable"
    );
    drop(_guard);
    let _ = std::fs::remove_dir_all(&territory);
}

#[test]
fn overview_does_not_show_show_psk_row_when_daemon_is_down() {
    let _language = LanguageGuard::set(Language::En);
    let (_guard, territory) = territory_with_pidfile("no-show-psk-row", "99999999\n");
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    let mut app = ConsoleApp::new();
    app.focus = Focus::Panel;
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(
        !content.contains("[ Show recovery psk ]"),
        "the show psk row must not render while the daemon is down"
    );
    drop(_guard);
    let _ = std::fs::remove_dir_all(&territory);
}

#[test]
fn show_psk_dialog_displays_the_configured_psk_in_plain_text() {
    let _language = LanguageGuard::set(Language::En);
    let (_guard, territory) =
        territory_with_pidfile("show-psk-render", &format!("{}\n", std::process::id()));
    crate::deployment::config::save_flare(&crate::deployment::config::FlareSection {
        psk: Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into()),
        ..crate::deployment::config::default_flare_section()
    })
    .unwrap();
    let mut app = ConsoleApp::new();
    app.focus = Focus::Panel;
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.show_psk, "Enter must open the show psk dialog");
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(
        content.contains("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"),
        "the dialog must show the psk in plain text"
    );
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!app.show_psk, "Esc must close the show psk dialog");
    drop(_guard);
    let _ = std::fs::remove_dir_all(&territory);
}

#[test]
fn show_psk_dialog_edits_both_fields_and_saves() {
    let _language = LanguageGuard::set(Language::En);
    let (_guard, territory) =
        territory_with_pidfile("show-psk-save", &format!("{}\n", std::process::id()));
    crate::deployment::config::save_flare(&crate::deployment::config::FlareSection {
        psk: Some("an-existing-recovery-code".into()),
        ..crate::deployment::config::default_flare_section()
    })
    .unwrap();
    let mut app = ConsoleApp::new();
    app.focus = Focus::Panel;
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        app.show_psk_value, "an-existing-recovery-code",
        "the dialog must prefill the configured psk"
    );
    // Down 到二次确认字段,输入与 psk 一致后提交,再 Down 到「保存」执行。
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    for character in "an-existing-recovery-code".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(!app.show_psk, "saving must close the dialog");
    let section = crate::deployment::config::load_flare().unwrap();
    assert_eq!(
        section.psk.as_deref(),
        Some("an-existing-recovery-code"),
        "the saved psk must be persisted to config.toml"
    );
    drop(_guard);
    let _ = std::fs::remove_dir_all(&territory);
}

#[test]
fn show_psk_dialog_edits_the_psk_field_and_saves_the_new_value() {
    let _language = LanguageGuard::set(Language::En);
    let (_guard, territory) =
        territory_with_pidfile("show-psk-edit", &format!("{}\n", std::process::id()));
    crate::deployment::config::save_flare(&crate::deployment::config::FlareSection {
        psk: Some("an-existing-recovery-code".into()),
        ..crate::deployment::config::default_flare_section()
    })
    .unwrap();
    let mut app = ConsoleApp::new();
    app.focus = Focus::Panel;
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    // psk 字段:Enter 进入编辑,清空旧值后输入新值。
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    for _ in 0.."an-existing-recovery-code".chars().count() {
        app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
    }
    for character in "the-new-recovery-secret".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    for character in "the-new-recovery-secret".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(!app.show_psk, "saving must close the dialog");
    let section = crate::deployment::config::load_flare().unwrap();
    assert_eq!(
        section.psk.as_deref(),
        Some("the-new-recovery-secret"),
        "the edited psk must be persisted to config.toml"
    );
    drop(_guard);
    let _ = std::fs::remove_dir_all(&territory);
}

#[test]
fn show_psk_dialog_rejects_a_mismatched_confirmation_without_saving() {
    let _language = LanguageGuard::set(Language::En);
    let (_guard, territory) =
        territory_with_pidfile("show-psk-mismatch", &format!("{}\n", std::process::id()));
    crate::deployment::config::save_flare(&crate::deployment::config::FlareSection {
        psk: Some("an-existing-recovery-code".into()),
        ..crate::deployment::config::default_flare_section()
    })
    .unwrap();
    let mut app = ConsoleApp::new();
    app.focus = Focus::Panel;
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    for character in "a-different-code".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.show_psk, "a rejected save must keep the dialog open");
    assert!(
        app.notice.contains("do not match"),
        "unexpected notice: {}",
        app.notice
    );
    let section = crate::deployment::config::load_flare().unwrap();
    assert_eq!(
        section.psk.as_deref(),
        Some("an-existing-recovery-code"),
        "a rejected save must not modify the config"
    );
    drop(_guard);
    let _ = std::fs::remove_dir_all(&territory);
}

#[test]
fn mouse_click_on_show_psk_row_opens_the_dialog() {
    let _language = LanguageGuard::set(Language::En);
    let (_guard, territory) =
        territory_with_pidfile("show-psk-mouse", &format!("{}\n", std::process::id()));
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    let mut app = ConsoleApp::new();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    let row = content
        .lines()
        .position(|line| line.contains("[ Show recovery psk ]"))
        .expect("the show psk row must render") as u16;
    app.handle_mouse(mouse_click(65, row));
    assert!(app.show_psk, "clicking the row must open the dialog");
    drop(_guard);
    let _ = std::fs::remove_dir_all(&territory);
}
