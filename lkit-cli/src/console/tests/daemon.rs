use super::super::*;
use super::support::*;
use crate::check::model::{CheckGroup, CheckReport, CheckResult, Status, StatusCounts};
use crate::i18n::Language;
use ratatui::backend::TestBackend;

/// 隔离 lkit 地盘并写入指定 pidfile 内容,返回守卫。
fn territory_with_pidfile(name: &str, content: &str) -> (DaemonTerritory, std::path::PathBuf) {
    let guard = DaemonTerritory::with_pidfile(name, content);
    let territory = guard.path().to_path_buf();
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
    let mut app = overview_app();
    app.focus = Focus::Panel;
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("lkit daemon is not running"));
    assert!(content.contains("[ Deploy the lkit daemon ]"));
    assert!(
        content.contains("Runs privileged tasks"),
        "the resident service section must carry a description"
    );
    assert!(
        !content.contains("flare channel"),
        "the recovery-code description belongs to the show-psk row, which is hidden while the daemon is down"
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
    let mut app = overview_app();
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
    let mut app = overview_app();
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
    assert!(content.contains("Deploy the lkit daemon"));
    drop(_guard);
    let _ = std::fs::remove_dir_all(&territory);
}

#[test]
fn deploy_confirm_runs_in_background_and_writes_the_result_to_the_notice() {
    let _language = LanguageGuard::set(Language::En);
    let (_guard, territory) = territory_with_pidfile("deploy", "99999999\n");
    let (mut app, deploy) = overview_app_with_deploy(Err("fixture deployment failed".into()));
    app.focus = Focus::Panel;
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.deploy_daemon_confirming);
    // 空恢复码(留空自动生成):直接下移到「开始部署」动作行并确认。
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(
        app.deploy_daemon.is_some(),
        "confirming the deploy must start the background worker"
    );
    assert!(!app.deploy_daemon_confirming);

    assert_eq!(deploy.requests(), [None]);
    app.update();
    assert!(app.deploy_daemon.is_none(), "the deploy worker must finish");
    assert!(
        matches!(&app.notice, Notice::Error(message) if message == "fixture deployment failed"),
        "unexpected notice: {}",
        app.notice.text()
    );
    assert!(matches!(app.preflight.state, PreflightState::NotRun));
    drop(_guard);
    let _ = std::fs::remove_dir_all(&territory);
}

#[test]
fn running_daemon_enter_opens_the_show_psk_dialog() {
    let _language = LanguageGuard::set(Language::En);
    let (_guard, territory) =
        territory_with_pidfile("enter-ignored", &format!("{}\n", std::process::id()));
    let mut app = overview_app();
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
fn preflight_dialog_shows_deploy_button_when_the_daemon_check_blocks() {
    let _language = LanguageGuard::set(Language::En);
    let (_guard, territory) = territory_with_pidfile("dialog", "99999999\n");
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    let mut app = overview_app();
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
    assert!(content.contains("Enter Deploy daemon"));
    drop(_guard);
    let _ = std::fs::remove_dir_all(&territory);
}

#[test]
fn preflight_dialog_enter_opens_the_deploy_confirm_and_confirms_starts_deploy() {
    let _language = LanguageGuard::set(Language::En);
    let (_guard, territory) = territory_with_pidfile("dialog-enter", "99999999\n");
    let (mut app, deploy) = overview_app_with_deploy(Ok("fixture daemon deployed".into()));
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
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(
        app.deploy_daemon.is_some(),
        "confirming must start the daemon deploy in the background"
    );

    assert_eq!(deploy.requests(), [None]);
    app.update();
    assert!(app.deploy_daemon.is_none(), "the deploy worker must finish");
    assert!(
        matches!(&app.notice, Notice::Success(message) if message == "fixture daemon deployed"),
        "unexpected notice: {}",
        app.notice.text()
    );
    assert!(matches!(app.preflight.state, PreflightState::Running(_)));
    drop(_guard);
    let _ = std::fs::remove_dir_all(&territory);
}

#[test]
fn preflight_dialog_d_key_opens_the_deploy_confirm_and_confirms_starts_deploy() {
    let _language = LanguageGuard::set(Language::En);
    let (_guard, territory) = territory_with_pidfile("dialog-deploy", "99999999\n");
    let (mut app, deploy) = overview_app_with_deploy(Ok("fixture daemon deployed".into()));
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
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.deploy_daemon.is_some());
    assert_eq!(deploy.requests(), [None]);
    app.update();
    assert!(app.deploy_daemon.is_none(), "the deploy worker must finish");
    assert!(
        matches!(&app.notice, Notice::Success(message) if message == "fixture daemon deployed"),
        "unexpected notice: {}",
        app.notice.text()
    );
    assert!(matches!(app.preflight.state, PreflightState::Running(_)));
    drop(_guard);
    let _ = std::fs::remove_dir_all(&territory);
}

#[test]
fn deploy_confirm_dialog_renders_on_the_install_menu_from_the_preflight_path() {
    let _language = LanguageGuard::set(Language::En);
    let (_guard, territory) = territory_with_pidfile("dialog-render", "99999999\n");
    let mut app = overview_app();
    app.menu_index = 1;
    app.focus = Focus::Panel;
    app.preflight.state = PreflightState::Complete(daemon_blocked_report());
    app.preflight_dialog = true;
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.deploy_daemon_confirming);
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("Deploy the lkit daemon"));
    assert!(!content.contains("Confirm psk"));
    assert!(content.contains("[ Start deployment ]"));
    drop(_guard);
    let _ = std::fs::remove_dir_all(&territory);
}

#[test]
fn f_opens_the_flare_dialog_on_the_overview_panel() {
    let _language = LanguageGuard::set(Language::En);
    let (_guard, territory) = territory_with_pidfile("flare-open", "99999999\n");
    let mut app = overview_app();
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
    let mut app = overview_app();
    app.focus = Focus::Panel;
    app.handle_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE));
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("Flare recovery channel"));
    assert!(content.contains("Discovery PSK"));
    assert!(content.contains("<not configured>"));
    drop(_guard);
    let _ = std::fs::remove_dir_all(&territory);
}

#[test]
fn flare_dialog_edits_and_saves_the_psk_into_the_config() {
    let _language = LanguageGuard::set(Language::En);
    let (_guard, territory) = territory_with_pidfile("flare-save", "99999999\n");
    let mut app = overview_app();
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
    let mut app = overview_app();
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
fn clearing_the_optional_key_hides_confirmation_and_skips_it_in_both_directions() {
    let (_guard, _territory) = territory_with_pidfile("deploy-clear-key", "99999999\n");
    let (mut app, deploy) = overview_app_with_deploy(Ok("deployed".into()));
    app.open_deploy_dialog();
    assert!(!app.psk_confirmation_visible());
    app.handle_paste("a");
    assert!(app.psk_confirmation_visible());
    let key = |code| KeyEvent::new(code, KeyModifiers::NONE);
    app.handle_key(key(KeyCode::Tab));
    app.handle_paste("a");
    app.handle_key(key(KeyCode::BackTab));
    app.handle_key(key(KeyCode::Enter));
    app.handle_key(key(KeyCode::Backspace));
    assert!(!app.psk_confirmation_visible());
    assert!(app.show_psk_confirmation.is_empty());
    app.handle_key(key(KeyCode::Tab));
    app.handle_key(key(KeyCode::Up));
    assert_eq!(
        app.show_psk_field,
        crate::console::daemon_panel::PskDialogField::Psk
    );
    app.handle_key(key(KeyCode::Down));
    app.handle_key(key(KeyCode::Enter));
    assert_eq!(deploy.requests(), [None]);
}

#[test]
fn deployment_accepts_a_private_key_in_the_dialog() {
    let (_guard, _territory) = territory_with_pidfile("deploy-public", "99999999\n");
    let (mut app, deploy) = overview_app_with_deploy(Ok("deployed".into()));
    app.open_deploy_dialog();
    app.handle_paste("private-test-key");
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.deploy_daemon_confirming);
    assert!(deploy.requests().is_empty());
    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    app.handle_paste("private-test-key");
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(deploy.requests(), [Some("private-test-key".into())]);
}

#[test]
fn overview_shows_show_psk_row_when_daemon_is_alive() {
    let _language = LanguageGuard::set(Language::En);
    let (_guard, territory) =
        territory_with_pidfile("show-psk-row", &format!("{}\n", std::process::id()));
    let backend = TestBackend::new(100, 28);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut app = overview_app();
    app.focus = Focus::Panel;
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("[ Show discovery key ]"));
    assert!(!content.contains("[ Deploy the lkit daemon ]"));
    assert!(
        content.contains("Runs privileged tasks"),
        "the section description must also render while the daemon is running"
    );
    let psk_row = content
        .lines()
        .position(|line| line.contains("[ Show discovery key ]"))
        .expect("the show psk row must render");
    let help_row = content
        .lines()
        .position(|line| line.contains("private discovery"))
        .expect("the recovery-code description must render");
    assert!(
        help_row < psk_row,
        "the recovery-code description must sit above the action row"
    );
    drop(_guard);
    let _ = std::fs::remove_dir_all(&territory);
}

#[test]
fn overview_does_not_show_show_psk_row_when_daemon_is_down() {
    let _language = LanguageGuard::set(Language::En);
    let (_guard, territory) = territory_with_pidfile("no-show-psk-row", "99999999\n");
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    let mut app = overview_app();
    app.focus = Focus::Panel;
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(
        !content.contains("[ Show discovery key ]"),
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
    let mut app = overview_app();
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
    let mut app = overview_app();
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
    let mut app = overview_app();
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
    let mut app = overview_app();
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
        app.notice.text()
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
