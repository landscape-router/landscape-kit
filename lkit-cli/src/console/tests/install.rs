use super::super::install_form::*;
use super::super::render::*;
use super::super::*;
use super::support::*;

use super::super::widgets::*;
use crate::i18n::Language;
use ratatui::backend::TestBackend;
use ratatui::style::Color;

#[test]
fn install_form_requires_the_flare_psk() {
    let mut form = InstallForm {
        password: "Secret123".into(),
        password_confirmation: "Secret123".into(),
        ..InstallForm::default()
    };
    let error = form.command().err().expect("empty flare psk must fail");
    assert!(
        error.contains("flare recovery psk is required"),
        "the error must explain the requirement, got: {error}"
    );
}

#[test]
fn install_form_rejects_a_short_flare_psk() {
    let mut form = InstallForm {
        password: "Secret123".into(),
        password_confirmation: "Secret123".into(),
        flare_psk: "short".into(),
        ..InstallForm::default()
    };
    let error = form.command().err().expect("short flare psk must fail");
    assert!(
        error.contains("at least 12 characters"),
        "the error must mention the minimum length, got: {error}"
    );
}

#[test]
fn renders_sidebar_and_install_form() {
    let _language = LanguageGuard::set(Language::En);
    let backend = TestBackend::new(100, 28);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut app = ConsoleApp::new();
    app.menu_index = 1;
    app.focus = Focus::Panel;
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(content.contains("Landscape Kit"));
    assert!(content.contains("Navigation"));
    assert!(content.contains("Install root"));
    assert!(content.contains("Confirm password"));
    assert!(content.contains("Flare recovery psk"));
    assert!(content.contains("Start installation"));
    assert!(!content.contains("Repository URL"));
    assert!(content.contains("Environment checks"));
    assert!(content.contains("> Environment checks"));
    assert!(content.contains("NOT RUN"));
    assert!(content.contains("Enter Details"));
    assert!(content.contains("L  Language: English (en)"));
}

#[test]
fn installed_snapshot_renders_install_menu_disabled() {
    let _language = LanguageGuard::set(Language::En);
    let backend = TestBackend::new(100, 28);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut app = ConsoleApp::new();
    app.snapshot = installed_snapshot();

    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let buffer = terminal.backend().buffer();
    let width = buffer.area.width as usize;
    let mut found = false;
    for index in 0..buffer.content.len().saturating_sub(7) {
        if index % width >= 24 {
            continue;
        }
        let text: String = buffer.content[index..index + 7]
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        if text == "Install" && buffer.content[index + 7].symbol() != "e" {
            assert_eq!(buffer.content[index].fg, Color::DarkGray);
            found = true;
        }
    }
    assert!(found, "Install label rendered in sidebar");
}

#[test]
fn installed_snapshot_renders_install_panel_unavailable() {
    let _language = LanguageGuard::set(Language::En);
    let backend = TestBackend::new(100, 28);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut app = ConsoleApp::new();
    app.menu_index = 1;
    app.focus = Focus::Panel;
    app.snapshot = installed_snapshot();

    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("Landscape is installed"));
    assert!(content.contains("unavailable"));
}

#[test]
fn renders_portable_markers_for_install_focus() {
    let _language = LanguageGuard::set(Language::En);
    let backend = TestBackend::new(100, 28);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut app = ConsoleApp::new();
    app.menu_index = 1;
    app.focus = Focus::Panel;
    app.install.checks_selected = false;
    app.install.selected = InstallField::Version;

    terminal.draw(|frame| render(frame, &mut app)).unwrap();

    let buffer = terminal.backend().buffer();
    assert!(terminal_content(&terminal).contains("> Install"));
    assert!(terminal_content(&terminal).contains("> Version"));
    assert!(
        buffer
            .content
            .iter()
            .any(|cell| cell.symbol() == ">" && cell.bg == Color::Cyan)
    );
}

#[test]
fn repository_url_only_appears_for_custom_repository() {
    let backend = TestBackend::new(100, 28);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut app = ConsoleApp::new();
    app.menu_index = 1;
    app.focus = Focus::Panel;
    app.install.checks_selected = false;
    app.install.repository = RepositoryMode::Custom;

    terminal.draw(|frame| render(frame, &mut app)).unwrap();

    let content: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(content.contains("Repository URL"));
}

#[test]
fn renders_contextual_help_below_form_on_narrow_terminal() {
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut app = ConsoleApp::new();
    app.menu_index = 1;
    app.focus = Focus::Panel;
    app.install.checks_selected = false;

    terminal.draw(|frame| render(frame, &mut app)).unwrap();

    let content: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(content.contains("About: Version"));
    assert!(content.contains("Release to install"));
}

#[test]
fn renders_preflight_summary_and_expanded_results() {
    let _language = LanguageGuard::set(Language::En);
    let backend = TestBackend::new(100, 28);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut app = ConsoleApp::new();
    app.menu_index = 1;
    app.focus = Focus::Panel;
    app.preflight.state = PreflightState::Complete(sample_preflight_report());

    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let summary: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(summary.contains("1 pass / 1 warn / 0 error / 0 unknown"));

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.preflight.expanded);
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let details: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(details.contains("Host platform"));
    assert!(details.contains("Operating system"));
    assert!(details.contains("Release availability is unknown"));
    assert!(details.contains("Confirm that a compatible release asset exists"));
    assert!(details.contains("Ctrl+C Exit"));
    assert!(details.contains("Esc Close"));

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!app.preflight.expanded);
    assert_eq!(app.exit_state, ExitState::Idle);
}

#[test]
fn entering_install_starts_background_checks() {
    let mut app = ConsoleApp::new();
    app.menu_index = 1;

    app.update();

    assert!(!matches!(app.preflight.state, PreflightState::NotRun));
}

#[test]
fn every_install_field_has_contextual_help() {
    let mut form = InstallForm::default();
    for field in InstallField::ALL {
        form.selected = field;
        let (title, description) = form.selected_help();
        assert!(!title.is_empty(), "field {field:?} has no help title");
        assert!(
            description.len() > 20,
            "field {field:?} has no useful help description"
        );
    }
}

#[test]
fn field_navigation_moves_between_checks_and_settings() {
    let mut app = ConsoleApp::new();
    app.menu_index = 1;
    app.focus = Focus::Panel;
    app.preflight.state = PreflightState::Complete(pass_preflight_report());

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert!(!app.install.checks_selected);
    assert_eq!(app.install.selected, InstallField::Version);

    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert!(app.install.checks_selected);
}

#[test]
fn field_navigation_skips_hidden_repository_url() {
    let mut app = ConsoleApp::new();
    app.menu_index = 1;
    app.focus = Focus::Panel;
    app.install.selected = InstallField::Repository;
    app.install.checks_selected = false;

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.install.selected, InstallField::InstallRoot);

    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(app.install.selected, InstallField::Repository);

    app.install.repository = RepositoryMode::Custom;
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.install.selected, InstallField::RepositoryUrl);
}

#[test]
fn left_changes_install_choice_and_esc_returns_to_navigation() {
    let mut app = ConsoleApp::new();
    app.menu_index = 1;
    app.focus = Focus::Panel;
    app.install.selected = InstallField::Repository;
    app.install.checks_selected = false;

    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    assert_eq!(app.focus, Focus::Panel);
    assert_eq!(
        app.install.repository,
        RepositoryMode::Custom,
        "Left must change the repository choice backward instead of leaving the panel"
    );

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(app.focus, Focus::Navigation);
    assert_eq!(app.exit_state, ExitState::Idle);
    assert_eq!(
        app.install.repository,
        RepositoryMode::Custom,
        "Esc must not alter the repository choice"
    );
}

#[test]
fn right_still_changes_install_choices() {
    let mut app = ConsoleApp::new();
    app.menu_index = 1;
    app.focus = Focus::Panel;
    app.install.selected = InstallField::Repository;
    app.install.checks_selected = false;

    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));

    assert_eq!(app.focus, Focus::Panel);
    assert_eq!(app.install.repository, RepositoryMode::Github);
}

#[test]
fn install_form_builds_cli_and_domain_request() {
    let mut form = InstallForm {
        version: "1.2.3".into(),
        repository: RepositoryMode::Custom,
        repository_url: "https://example.com/releases/".into(),
        install_dir: "/opt/landscape".into(),
        admin_user: "operator".into(),
        password: "Secret123".into(),
        password_confirmation: "Secret123".into(),
        flare_psk: "recovery-secret-long-enough".into(),
        takeover_network: false,
        selected: InstallField::StartInstallation,
        checks_selected: false,
        editing: false,
    };
    let ConsoleAction::Command { command, args } = form.command().unwrap() else {
        panic!("expected install command");
    };
    let Commands::Install(install) = command else {
        panic!("expected install request");
    };
    assert_eq!(install.version.as_deref(), Some("1.2.3"));
    assert_eq!(
        install.repository,
        Some(Some("https://example.com/releases/".into()))
    );
    assert!(!format!("{install:?}").contains("Secret123"));
    assert!(format!("{install:?}").contains("interactive_flare_psk: Some(\"[REDACTED]\")"));
    assert_eq!(install.interactive_password.as_deref(), Some("Secret123"));
    assert_eq!(
        install.interactive_flare_psk.as_deref(),
        Some("recovery-secret-long-enough")
    );
    assert!(install.password_file.is_none());
    assert_eq!(args[0], "install");
    assert!(args.windows(2).any(|pair| pair == ["--version", "1.2.3"]));
    assert!(args.iter().all(|argument| !argument.contains("Secret123")));
    assert!(
        args.iter()
            .all(|argument| !argument.contains("recovery-secret"))
    );
}

#[test]
fn install_form_defaults_to_network_takeover() {
    let mut form = InstallForm {
        password: "Secret123".into(),
        password_confirmation: "Secret123".into(),
        flare_psk: "recovery-secret-long-enough".into(),
        ..InstallForm::default()
    };
    assert!(form.takeover_network);
    let ConsoleAction::Command { command, args } = form.command().unwrap() else {
        panic!("expected install command");
    };
    let Commands::Install(install) = command else {
        panic!("expected install request");
    };
    assert!(install.takeover_network);
    assert!(args.iter().any(|argument| argument == "--takeover-network"));
}

#[test]
fn install_form_maps_repository_modes_to_cli_flags() {
    let base = InstallForm {
        password: "Secret123".into(),
        password_confirmation: "Secret123".into(),
        flare_psk: "recovery-secret-long-enough".into(),
        ..InstallForm::default()
    };
    for (mode, repository, expected) in [
        (RepositoryMode::Default, None, Vec::<&str>::new()),
        (
            RepositoryMode::Github,
            Some(Some("github".into())),
            vec!["--repository", "github"],
        ),
        (RepositoryMode::Mirror, Some(None), vec!["--repository"]),
    ] {
        let mut form = base.clone();
        form.repository = mode;
        let ConsoleAction::Command { command, args } = form.command().unwrap() else {
            panic!("expected install command");
        };
        let Commands::Install(install) = command else {
            panic!("expected install request");
        };
        assert_eq!(install.repository, repository);
        for pair in expected.chunks(2) {
            if pair.len() == 1 {
                assert!(
                    args.iter().any(|argument| argument == pair[0]),
                    "{mode:?} must forward {:?}, got {args:?}",
                    pair[0]
                );
            } else {
                assert!(
                    args.windows(2).any(|window| window == pair),
                    "{mode:?} must forward {pair:?}, got {args:?}"
                );
            }
        }
        if expected.is_empty() {
            assert!(
                !args.iter().any(|argument| argument == "--repository"),
                "{mode:?} must not forward --repository, got {args:?}"
            );
        }
    }
}

#[test]
fn install_form_rejects_invalid_version_before_launch() {
    let mut form = InstallForm {
        version: "nightly".into(),
        ..InstallForm::default()
    };
    assert!(form.command().is_err());
}

#[test]
fn install_form_masks_and_confirms_password() {
    assert_eq!(mask("Secret123"), "*********");
    let mut form = InstallForm {
        password: "Secret123".into(),
        password_confirmation: "Different123".into(),
        ..InstallForm::default()
    };
    assert_eq!(
        form.command().err().unwrap(),
        "password confirmation does not match"
    );
}

#[test]
fn error_checks_block_form_entry_with_dialog() {
    let _language = LanguageGuard::set(Language::En);
    let mut app = ConsoleApp::new();
    app.menu_index = 1;
    app.focus = Focus::Panel;
    app.preflight.state = PreflightState::Complete(error_preflight_report());
    assert!(app.install.checks_selected);

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert!(app.install.checks_selected);
    assert!(app.preflight_dialog);

    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("Install blocked"));
    assert!(content.contains("Port 6443"));
    assert!(content.contains("stop the conflicting process"));

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(!app.preflight_dialog);
    assert!(app.preflight.expanded);
    assert!(app.install.checks_selected);

    app.preflight_dialog = true;
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!app.preflight_dialog);

    app.preflight_dialog = true;
    app.handle_key(KeyEvent::new(KeyCode::Char('R'), KeyModifiers::SHIFT));
    assert!(!app.preflight_dialog);
    assert!(matches!(app.preflight.state, PreflightState::Running(_)));
}

#[test]
fn running_checks_keep_focus_on_summary_without_dialog() {
    let mut app = ConsoleApp::new();
    app.menu_index = 1;
    app.focus = Focus::Panel;
    let (_, receiver) = std::sync::mpsc::channel();
    app.preflight.state = PreflightState::Running(receiver);

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert!(app.install.checks_selected);
    assert!(!app.preflight_dialog);
}

#[test]
fn warning_checks_allow_form_entry() {
    let mut app = ConsoleApp::new();
    app.menu_index = 1;
    app.focus = Focus::Panel;
    app.preflight.state = PreflightState::Complete(sample_preflight_report());

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert!(!app.install.checks_selected);
    assert_eq!(app.install.selected, InstallField::Version);
    assert!(!app.preflight_dialog);
}

#[test]
fn start_installation_is_blocked_when_checks_fail() {
    let mut app = ConsoleApp::new();
    app.menu_index = 1;
    app.focus = Focus::Panel;
    app.preflight.state = PreflightState::Complete(error_preflight_report());
    app.install.checks_selected = false;
    app.install.selected = InstallField::StartInstallation;

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.preflight_dialog);
    assert!(app.network_wizard.is_none());

    app.preflight.state = PreflightState::Complete(pass_preflight_report());
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(!app.preflight_dialog);
}

#[test]
fn enter_on_start_installation_dispatches_the_install_command() {
    let mut app = ConsoleApp::new();
    app.menu_index = 1;
    app.focus = Focus::Panel;
    app.preflight.state = PreflightState::Complete(pass_preflight_report());
    app.install.checks_selected = false;
    app.install.selected = InstallField::StartInstallation;
    app.install.takeover_network = false;
    app.install.password = "Secret123".into();
    app.install.password_confirmation = "Secret123".into();
    app.install.flare_psk = "recovery-secret-long-enough".into();

    let action = app
        .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .expect("Enter on the start installation row must dispatch");
    assert!(matches!(action, ConsoleAction::Command { .. }));
}

#[test]
fn mouse_click_install_field_enters_editing_and_checks_switches_back() {
    let _language = LanguageGuard::set(Language::En);
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    let mut app = ConsoleApp::new();
    app.menu_index = 1;
    app.focus = Focus::Panel;
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    app.handle_mouse(mouse_click(30, 6));
    assert!(
        app.install.editing,
        "clicking the version field must edit it"
    );
    assert_eq!(app.install.selected, InstallField::Version);
    app.handle_mouse(mouse_click(30, 3));
    assert!(app.install.checks_selected);
    assert!(!app.install.editing);
}

#[test]
fn mouse_scroll_moves_preflight_details() {
    let _language = LanguageGuard::set(Language::En);
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    let mut app = ConsoleApp::new();
    app.menu_index = 1;
    app.preflight.state = PreflightState::Complete(sample_preflight_report());
    app.preflight.expanded = true;
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    assert_eq!(app.preflight.scroll, 0);
    app.handle_mouse(mouse_scroll(true));
    assert_eq!(app.preflight.scroll, 1);
    app.handle_mouse(mouse_scroll(false));
    assert_eq!(app.preflight.scroll, 0);
}
