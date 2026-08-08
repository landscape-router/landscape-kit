use super::super::network_wizard::*;
use super::super::widgets::*;
use super::super::*;
use super::support::*;
use crate::i18n::Language;
use crate::network::config::{DEFAULT_MANAGEMENT_CIDR, NetworkMode};
use crate::network::discovery::Interface;
use ratatui::backend::TestBackend;

#[test]
fn network_wizard_is_full_screen_and_supports_keyboard_selection() {
    let _language = LanguageGuard::set(Language::En);
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    let mut app = ConsoleApp::new();
    app.network_wizard = Some(sample_network_wizard());

    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("Select the WAN interface"));
    assert!(content.contains("not found"));
    assert!(!content.contains("Navigation"));

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(
        app.network_wizard.as_ref().unwrap().selected_wan().name,
        "ens33"
    );
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let wizard = app.network_wizard.as_ref().unwrap();
    assert_eq!(wizard.step, WizardStep::WanConfig);
    assert_eq!(wizard.focus, 0);
    assert_eq!(wizard.wan_mode, WanMode::Dhcp);
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    assert_eq!(
        app.network_wizard.as_ref().unwrap().wan_mode,
        WanMode::Static
    );
    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    assert_eq!(app.network_wizard.as_ref().unwrap().wan_mode, WanMode::Dhcp);
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let wizard = app.network_wizard.as_ref().unwrap();
    assert_eq!(wizard.focus, 1);
    assert!(!wizard.editing);
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.network_wizard.as_ref().unwrap().step, WizardStep::Lan);
    app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    assert!(app.network_wizard.as_ref().unwrap().lan_selected[0]);
}

#[test]
fn network_wizard_dhcp_panel_highlights_confirm_button_on_focus() {
    let _language = LanguageGuard::set(Language::En);
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    let mut app = ConsoleApp::new();
    app.network_wizard = Some(sample_network_wizard());

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let wizard = app.network_wizard.as_ref().unwrap();
    assert_eq!(wizard.step, WizardStep::WanConfig);
    assert_eq!(wizard.focus, 0);

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let wizard = app.network_wizard.as_ref().unwrap();
    assert_eq!(wizard.focus, 1);
    assert!(!wizard.editing);

    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("> [ Confirm and continue ]"));
}

#[test]
fn network_wizard_builds_wan_only_dhcp_plan_without_lan() {
    let mut wizard = NetworkWizard {
        interfaces: vec![Interface {
            name: "ens32".into(),
            mac: "00:0c:29:a4:09:08".into(),
            operstate: "up".into(),
            addresses: Vec::new(),
        }],
        routes: Vec::new(),
        wan: 0,
        step: WizardStep::Lan,
        wan_mode: WanMode::Dhcp,
        address: String::new(),
        gateway: String::new(),
        focus: 0,
        lan_candidates: Vec::new(),
        lan_cursor: 0,
        lan_selected: Vec::new(),
        management: DEFAULT_MANAGEMENT_CIDR.into(),
        dhcp_start: String::new(),
        dhcp_end: String::new(),
        editing: false,
        cancel_confirming: false,
    };
    let plan = wizard.plan().unwrap();
    assert!(matches!(plan.mode, NetworkMode::WanDhcp { .. }));
    wizard.set_wan(0);
    assert!(wizard.lan_candidates.is_empty());
}

#[test]
fn network_wizard_prefills_static_from_discovery() {
    let mut app = ConsoleApp::new();
    app.network_wizard = Some(routes_armed_wizard());

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let wizard = app.network_wizard.as_ref().unwrap();
    assert_eq!(wizard.step, WizardStep::WanConfig);
    assert_eq!(wizard.wan_mode, WanMode::Static);
    assert_eq!(wizard.address, "10.1.1.105/24");
    assert_eq!(wizard.gateway, "10.1.1.1");
}

#[test]
fn network_wizard_defaults_to_dhcp_without_complete_static_pair() {
    let mut app = ConsoleApp::new();
    app.network_wizard = Some(sample_network_wizard());

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let wizard = app.network_wizard.as_ref().unwrap();
    assert_eq!(wizard.step, WizardStep::WanConfig);
    assert_eq!(wizard.wan_mode, WanMode::Dhcp);
    assert!(wizard.address.is_empty());
    assert!(wizard.gateway.is_empty());
}

#[test]
fn network_wizard_wan_config_edits_static_fields_and_validates() {
    let mut app = ConsoleApp::new();
    app.network_wizard = Some(routes_armed_wizard());
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let wizard = app.network_wizard.as_ref().unwrap();
    assert_eq!(wizard.step, WizardStep::WanConfig);
    assert!(wizard.editing);
    assert_eq!(wizard.focus, 1);

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.network_wizard.as_ref().unwrap().focus, 2);
    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(app.network_wizard.as_ref().unwrap().focus, 1);

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let wizard = app.network_wizard.as_ref().unwrap();
    assert_eq!(wizard.focus, 2);
    assert!(wizard.editing);

    app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let wizard = app.network_wizard.as_ref().unwrap();
    assert_eq!(wizard.step, WizardStep::WanConfig);
    assert!(wizard.editing);
    assert!(!app.notice.is_empty());

    app.network_wizard.as_mut().unwrap().gateway = "10.1.1.1".into();
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let wizard = app.network_wizard.as_ref().unwrap();
    assert_eq!(wizard.focus, 3);
    assert!(!wizard.editing);

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let wizard = app.network_wizard.as_ref().unwrap();
    assert_eq!(wizard.step, WizardStep::Lan);
    assert!(!wizard.editing);
}

#[test]
fn network_wizard_confirm_requires_enter_to_start() {
    let mut app = ConsoleApp::new();
    app.install.password = "Secret123".into();
    app.install.password_confirmation = "Secret123".into();
    app.network_wizard = Some(sample_network_wizard());

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        app.network_wizard.as_ref().unwrap().step,
        WizardStep::Confirm
    );

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(app.network_wizard.as_ref().unwrap().step, WizardStep::Lan);

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        app.network_wizard.as_ref().unwrap().step,
        WizardStep::Confirm
    );
    assert!(matches!(
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Some(ConsoleAction::Command { .. })
    ));
    assert!(app.network_wizard.is_none());
}

#[test]
fn network_wizard_lan_dhcp_edits_all_fields_on_one_page() {
    let mut app = ConsoleApp::new();
    app.network_wizard = Some(sample_network_wizard());

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let wizard = app.network_wizard.as_ref().unwrap();
    assert_eq!(wizard.step, WizardStep::LanDhcp);
    assert!(wizard.editing);
    assert_eq!(wizard.focus, 0);
    assert_eq!(wizard.dhcp_start, "192.168.10.100");
    assert_eq!(wizard.dhcp_end, "192.168.10.254");

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.network_wizard.as_mut().unwrap().dhcp_start = "192.168.10.150".into();
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let wizard = app.network_wizard.as_ref().unwrap();
    assert_eq!(wizard.focus, 2);
    assert!(wizard.editing);

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let wizard = app.network_wizard.as_ref().unwrap();
    assert_eq!(wizard.focus, 3);
    assert!(!wizard.editing);

    let plan = wizard.plan().unwrap();
    assert!(matches!(plan.mode, NetworkMode::RoutedLan { .. }));

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        app.network_wizard.as_ref().unwrap().step,
        WizardStep::Confirm
    );
}

#[test]
fn network_wizard_first_page_esc_opens_cancel_confirmation() {
    let mut app = ConsoleApp::new();
    app.network_wizard = Some(sample_network_wizard());

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.network_wizard.as_ref().unwrap().cancel_confirming);

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!app.network_wizard.as_ref().unwrap().cancel_confirming);

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.network_wizard.is_none());

    app.network_wizard = Some(sample_network_wizard());
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    let wizard = app.network_wizard.as_ref().unwrap();
    assert_eq!(wizard.step, WizardStep::Wan);
    assert!(!wizard.cancel_confirming);
}

#[test]
fn wizard_render_shows_gateway_and_confirm_summary() {
    let _language = LanguageGuard::set(Language::En);
    let mut terminal = Terminal::new(TestBackend::new(120, 32)).unwrap();
    let mut app = ConsoleApp::new();
    app.install.password = "Secret123".into();
    app.install.password_confirmation = "Secret123".into();
    app.network_wizard = Some(routes_armed_wizard());

    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("10.1.1.105/24"));
    assert!(content.contains("gw 10.1.1.1"));

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        app.network_wizard.as_ref().unwrap().step,
        WizardStep::WanConfig
    );
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("WAN IPv4 mode"));
    assert!(content.contains("[ Static ]"));
    assert!(content.contains("[ DHCP client ]"));
    assert!(content.contains("Confirm and continue"));

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        app.network_wizard.as_ref().unwrap().step,
        WizardStep::Confirm
    );

    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("Confirm network takeover plan"));
    assert!(content.contains("ens32"));
    assert!(content.contains("00:0c:29:a4:09:08"));
    assert!(content.contains("10.1.1.105/24"));
    assert!(content.contains("WAN-only"));
    assert!(content.contains("will have their IPv4/IPv6 addresses flushed"));
}

#[test]
fn mouse_click_wizard_tab_and_field() {
    let _language = LanguageGuard::set(Language::En);
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    let mut app = ConsoleApp::new();
    let mut wizard = sample_network_wizard();
    wizard.step = WizardStep::WanConfig;
    wizard.wan_mode = WanMode::Static;
    wizard.focus = 0;
    app.network_wizard = Some(wizard);
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    assert_eq!(app.hits.hit_at(3, 6), Some(Hit::WizardTab(WanMode::Static)));
    assert_eq!(app.hits.hit_at(15, 6), Some(Hit::WizardTab(WanMode::Dhcp)));
    app.handle_mouse(mouse_click(15, 6));
    assert_eq!(app.network_wizard.as_ref().unwrap().wan_mode, WanMode::Dhcp);
    app.handle_mouse(mouse_click(3, 6));
    assert_eq!(
        app.network_wizard.as_ref().unwrap().wan_mode,
        WanMode::Static
    );
    app.handle_mouse(mouse_click(30, 8));
    let wizard = app.network_wizard.as_ref().unwrap();
    assert_eq!(wizard.focus, 1);
    assert!(wizard.editing);
}
