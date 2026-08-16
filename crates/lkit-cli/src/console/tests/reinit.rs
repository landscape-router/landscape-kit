use super::super::network_wizard::*;
use super::super::reinit::*;
use super::super::widgets::*;
use super::super::*;
use super::support::*;
use crate::i18n::Language;
use crate::network::config::{NetworkMode, NetworkPlan};
use ratatui::backend::TestBackend;
use std::sync::Mutex;

/// 串行化所有读写 `LKIT_TEST_REINIT_ELIGIBLE` 的测试,避免并行竞争。
static ELIGIBLE_LOCK: Mutex<()> = Mutex::new(());

fn reinit_ready_app() -> ConsoleApp {
    let mut app = ConsoleApp::new();
    app.menu_index = 6;
    app.focus = Focus::Panel;
    app.snapshot = installed_snapshot();
    app
}

#[test]
fn reinit_panel_renders_summary_and_begin_action_when_eligible() {
    let _guard = ELIGIBLE_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let _language = LanguageGuard::set(Language::En);
    unsafe {
        std::env::set_var("LKIT_TEST_REINIT_ELIGIBLE", "1");
    }
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    let mut app = reinit_ready_app();
    assert!(app.menu_available(Menu::Reinit));

    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("Reinit"));
    assert!(content.contains("1.2.3"));
    assert!(
        content.contains("> Begin reinit"),
        "the focused panel must offer the begin action with the cursor marker"
    );
    unsafe {
        std::env::remove_var("LKIT_TEST_REINIT_ELIGIBLE");
    }
}

#[test]
fn reinit_panel_shows_unavailable_without_takeover() {
    let _language = LanguageGuard::set(Language::En);
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    let mut app = ConsoleApp::new();
    app.menu_index = 6;
    app.focus = Focus::Panel;
    app.snapshot = Snapshot::Installed {
        version: "1.2.3".into(),
        manager: "none",
        initialized: true,
    };
    assert!(!app.menu_available(Menu::Reinit));

    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(
        content.contains("Reinit is unavailable"),
        "a non-takeover installation must show the availability notice"
    );
}

#[test]
fn reinit_wizard_completion_moves_to_credentials_step() {
    let mut app = reinit_ready_app();
    app.reinit.wizard = true;
    app.network_wizard = Some(sample_network_wizard());
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        app.network_wizard.as_ref().unwrap().step,
        WizardStep::Confirm
    );

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.network_wizard.is_none());
    assert!(!app.reinit.wizard);
    assert!(app.reinit.plan.is_some());
    assert_eq!(app.reinit.step, ReinitStep::Credentials);
    assert_eq!(
        app.reinit.selected,
        ReinitField::AdminUser,
        "the credentials step must start on the admin user field"
    );
}

#[test]
fn reinit_cancelled_wizard_returns_to_overview() {
    let mut app = reinit_ready_app();
    app.reinit.wizard = true;
    app.network_wizard = Some(sample_network_wizard());

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.network_wizard.is_none());
    assert!(!app.reinit.wizard);
    assert_eq!(app.reinit.step, ReinitStep::Overview);
}

#[test]
fn reinit_credentials_edit_and_confirmation_builds_command() {
    let _language = LanguageGuard::set(Language::En);
    let mut app = reinit_ready_app();
    app.reinit.step = ReinitStep::Credentials;
    app.reinit.plan = Some(NetworkPlan {
        mode: NetworkMode::WanDhcp {
            wan: "ens32".into(),
        },
        selected_macs: Vec::new(),
    });

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.reinit.editing);
    app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(!app.reinit.editing);
    assert_eq!(app.reinit.admin_user, "adminro");

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.reinit.selected, ReinitField::Password);
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    for character in ['S', 'e', 'c', 'r', 'e', 't', '1', '2', '3'] {
        app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
    }
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(!app.reinit.editing);

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(
        app.reinit.selected,
        ReinitField::PasswordConfirmation,
        "the password confirmation field must follow the password"
    );
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.reinit.selected, ReinitField::Start);
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(
        !app.reinit.confirming,
        "a mismatched confirmation must not open the confirmation layer"
    );
    assert!(!app.notice.is_empty());

    app.reinit.password_confirmation = "Secret123".into();
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(
        app.reinit.confirming,
        "a matching confirmation must open the confirmation layer"
    );

    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("Confirm reinit"));
    assert!(content.contains("lkit network confirm"));

    let Some(ConsoleAction::Command { command, args }) =
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
    else {
        panic!("confirming Enter must dispatch the reinit command");
    };
    let Commands::Reinit(reinit) = command else {
        panic!("expected reinit request");
    };
    assert!(reinit.console_confirmed);
    assert!(reinit.yes);
    assert!(reinit.network_plan.is_some());
    assert_eq!(
        reinit.interactive_password.as_deref(),
        Some("Secret123"),
        "the console password must be forwarded through the credential channel"
    );
    assert_eq!(reinit.admin_user.as_deref(), Some("adminro"));
    assert!(args.contains(&"reinit".into()));
    assert!(args.contains(&"--yes".into()));
    assert!(args.contains(&"--console-confirmed".into()));
    assert!(args.contains(&"--admin-user".into()));
}

#[test]
fn left_returns_from_reinit_panel_to_navigation() {
    let _guard = ELIGIBLE_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let _language = LanguageGuard::set(Language::En);
    unsafe {
        std::env::set_var("LKIT_TEST_REINIT_ELIGIBLE", "1");
    }
    let mut app = reinit_ready_app();
    assert_eq!(app.focus, Focus::Panel);
    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    assert_eq!(
        app.focus,
        Focus::Navigation,
        "Left must return from the reinit panel to the navigation bar"
    );
    unsafe {
        std::env::remove_var("LKIT_TEST_REINIT_ELIGIBLE");
    }
}

#[test]
fn reinit_esc_cancels_confirmation_layer() {
    let mut app = reinit_ready_app();
    app.reinit.step = ReinitStep::Credentials;
    app.reinit.confirming = true;
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!app.reinit.confirming);
}

#[test]
fn reinit_plan_summary_lists_wan_and_lan_interfaces() {
    let _guard = ELIGIBLE_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let _language = LanguageGuard::set(Language::En);
    unsafe {
        std::env::set_var("LKIT_TEST_REINIT_ELIGIBLE", "1");
    }
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    let mut app = reinit_ready_app();
    app.reinit.step = ReinitStep::Credentials;
    app.reinit.plan = Some(NetworkPlan {
        mode: NetworkMode::RoutedLan {
            wan: "ens32".into(),
            wan_ipv4: None,
            lan: vec!["ens33".into(), "ens34".into()],
            management: "192.168.10.1/24".parse().unwrap(),
            dhcp_start: "192.168.10.100".parse().unwrap(),
            dhcp_end: "192.168.10.254".parse().unwrap(),
        },
        selected_macs: Vec::new(),
    });
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(
        content.contains("Plan: WAN ens32, LAN ens33, ens34"),
        "the plan summary must list the WAN and LAN interfaces"
    );

    app.reinit.plan = Some(NetworkPlan {
        mode: NetworkMode::WanDhcp {
            wan: "ens32".into(),
        },
        selected_macs: Vec::new(),
    });
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(
        content.contains("Plan: WAN ens32, LAN none"),
        "a WAN-only plan must show none for the LAN part"
    );
    unsafe {
        std::env::remove_var("LKIT_TEST_REINIT_ELIGIBLE");
    }
}
