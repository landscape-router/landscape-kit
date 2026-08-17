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
fn running_daemon_ignores_enter_on_overview() {
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
    assert!(content.contains("D deploy daemon"));
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
fn preflight_dialog_d_key_starts_deploy_and_closes_the_dialog() {
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
        app.deploy_daemon.is_some(),
        "D must start the daemon deploy in the background"
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
fn preflight_dialog_mouse_click_on_deploy_button_starts_the_deploy() {
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
        app.deploy_daemon.is_some(),
        "clicking the button must start the deploy"
    );
    drop(_guard);
    let _ = std::fs::remove_dir_all(&territory);
}
