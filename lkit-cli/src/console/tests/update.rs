use super::super::*;
use super::support::*;

use super::super::update::*;
use super::super::widgets::*;
use crate::deployment::config::RepositorySourceKind;
use crate::i18n::Language;
use ratatui::backend::TestBackend;

#[test]
fn update_panel_renders_current_version_and_form() {
    let _language = LanguageGuard::set(Language::En);
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    let mut app = update_ready_app();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("Current version"));
    assert!(content.contains("1.2.3"));
    assert!(content.contains("latest"));
    assert!(content.contains("Current source (http: https://example.com/releases/)"));
    assert!(content.contains("[ Start update ]"));
}

#[test]
fn update_menu_without_installation_shows_requirements() {
    let _language = LanguageGuard::set(Language::En);
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    let mut app = ConsoleApp::new();
    app.menu_index = 3;
    app.focus = Focus::Panel;
    app.snapshot = Snapshot::NotInstalled;
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("Landscape is not installed"));
    assert!(content.contains("Update requires an existing installation"));
}

#[test]
fn update_panel_navigation_edits_version_and_reaches_url_when_custom() {
    let mut app = update_ready_app();
    assert_eq!(app.update.selected, UpdateField::Version);

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.update.selected, UpdateField::Repository);
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(
        app.update.selected,
        UpdateField::Start,
        "the hidden URL row must be skipped for non-custom repositories"
    );
    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(app.update.selected, UpdateField::Repository);
    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(app.update.selected, UpdateField::Version);

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.update.editing);
    app.handle_key(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('.'), KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE));
    assert_eq!(app.update.version, "latest1.2");
    app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
    assert_eq!(app.update.version, "latest1.");
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(!app.update.editing);

    app.update.repository = UpdateRepositoryMode::Custom;
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.update.selected, UpdateField::Repository);
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.update.selected, UpdateField::RepositoryUrl);
    assert_eq!(app.exit_state, ExitState::Idle);
}

#[test]
fn update_repository_cycles_within_available_options() {
    let mut app = update_ready_app();
    app.update.selected = UpdateField::Repository;
    app.update.repository = UpdateRepositoryMode::Current;

    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    assert_eq!(app.update.repository, UpdateRepositoryMode::Github);
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    assert_eq!(app.update.repository, UpdateRepositoryMode::Mirror);
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    assert_eq!(app.update.repository, UpdateRepositoryMode::Custom);
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    assert_eq!(app.update.repository, UpdateRepositoryMode::Current);

    app.update.current_source = None;
    app.update.repository = UpdateRepositoryMode::Current;
    app.handle_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    assert_eq!(
        app.update.repository,
        UpdateRepositoryMode::Github,
        "Current must not be reachable without a config source"
    );
}

#[test]
fn update_resolution_branches_like_the_update_command() {
    let _language = LanguageGuard::set(Language::En);
    let mut app = update_ready_app();
    let mut notice = Notice::Ready;

    app.update
        .apply_resolution(&mut notice, resolved("1.2.3", "1.2.3"));
    assert!(notice.contains("already up to date"));
    assert!(app.update.confirming.is_none());

    notice = Notice::Ready;
    app.update
        .apply_resolution(&mut notice, resolved("1.2.4", "1.2.3"));
    assert!(notice.contains("downgrading"));
    assert!(app.update.confirming.is_none());

    notice = Notice::Ready;
    app.update
        .apply_resolution(&mut notice, resolved("1.2.3", "1.2.4"));
    assert!(notice.is_empty(), "an upgrade must not set the notice");
    let confirming = app.update.confirming.as_ref().unwrap();
    assert_eq!(confirming.current.to_string(), "1.2.3");
    assert_eq!(confirming.target.to_string(), "1.2.4");
}

#[test]
fn update_confirmation_builds_console_confirmed_command() {
    let _language = LanguageGuard::set(Language::En);
    let mut app = update_ready_app();
    app.update.confirming = Some(resolved("1.2.3", "1.2.4"));

    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("Confirm update"));
    assert!(content.contains("Confirm update"));
    assert!(content.contains("1.2.3 -> target 1.2.4"));
    assert!(content.contains("Ctrl+C Exit  Enter Update  Esc Cancel"));

    let action = app
        .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
    let ConsoleAction::Command { command, args } = action else {
        panic!("expected update command");
    };
    let Commands::Update(update) = command else {
        panic!("expected update request");
    };
    assert_eq!(update.version.as_deref(), Some("1.2.4"));
    assert!(
        update.repository.is_none(),
        "Current source must not forward --repository"
    );
    assert!(
        update.console_confirmed,
        "the console must mark the update as confirmed so no TTY prompt appears"
    );
    assert_eq!(args, vec!["update"]);
    assert_eq!(update.confirmed.unwrap(), resolved("1.2.3", "1.2.4"));
}

#[test]
fn update_dispatch_uses_resolved_context_not_mutable_form_fields() {
    let mut app = update_ready_app();
    for mode in [
        UpdateRepositoryMode::Current,
        UpdateRepositoryMode::Github,
        UpdateRepositoryMode::Mirror,
        UpdateRepositoryMode::Custom,
    ] {
        app.update.confirming = Some(resolved("1.2.3", "1.2.4"));
        app.update.repository = mode;
        app.update.version = "latest".into();
        app.update.repository_url = "https://changed.example/".into();
        let ConsoleAction::Command {
            command: Commands::Update(update),
            args,
        } = app.update_action()
        else {
            panic!("expected update");
        };
        assert_eq!(update.confirmed, Some(resolved("1.2.3", "1.2.4")));
        assert_eq!(update.version.as_deref(), Some("1.2.4"));
        assert_eq!(args, vec!["update"]);
    }
}
#[test]
fn editing_or_new_resolution_invalidates_confirmation() {
    let mut app = update_ready_app();
    app.update.confirming = Some(resolved("1.2.3", "1.2.4"));
    app.update.change(true);
    assert!(app.update.confirming.is_none());
    app.update.confirming = Some(resolved("1.2.3", "1.2.4"));
    app.update.editable_value_mut();
    assert!(app.update.confirming.is_none());
    app.update.confirming = Some(resolved("1.2.3", "1.2.4"));
    let mut notice = Notice::Ready;
    app.update
        .apply_resolution(&mut notice, resolved("1.2.3", "1.2.3"));
    assert!(app.update.confirming.is_none());
}

#[test]
fn update_confirmation_esc_cancels_and_stays_in_panel() {
    let mut app = update_ready_app();
    app.update.confirming = Some(resolved("1.2.3", "1.2.4"));
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.update.confirming.is_none());
    assert_eq!(app.exit_state, ExitState::Idle);
}

#[test]
fn update_confirmation_wraps_the_pipeline_note_on_narrow_terminals() {
    let _language = LanguageGuard::set(Language::En);
    let mut app = update_ready_app();
    app.update.confirming = Some(resolved("1.2.3", "1.3.0"));

    let mut terminal = Terminal::new(TestBackend::new(80, 18)).unwrap();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(
        content.contains("automatic rollback"),
        "the update pipeline note must wrap inside the confirm dialog instead of truncating"
    );
}

#[test]
fn update_load_config_offers_current_source_and_reports_corruption() {
    let dir = std::env::temp_dir().join(format!("lkit-console-config-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let _guard = crate::deployment::layout::test_territory(&dir);
    let install_dir = dir.display().to_string();

    let mut app = update_ready_app();
    app.install.install_dir = install_dir.clone();
    app.update.current_source = None;

    app.update.load_config();
    assert!(app.update.current_source.is_none());
    assert!(app.update.config_error.is_none());
    assert_eq!(app.update.repository, UpdateRepositoryMode::Github);

    let preset = "schema_version = 1\n\n[repository]\nkind = \"http\"\nlocation = \"https://example.com/releases/\"\n";
    std::fs::write(dir.join("config.toml"), preset).unwrap();
    app.update.load_config();
    assert_eq!(
        app.update.repository,
        UpdateRepositoryMode::Current,
        "a valid config source must become the default option"
    );
    let source = app.update.current_source.as_ref().unwrap();
    assert_eq!(source.kind, RepositorySourceKind::Http);
    assert_eq!(source.location, "https://example.com/releases/");
    assert!(app.update.config_error.is_none());

    std::fs::write(dir.join("config.toml"), "not a config").unwrap();
    app.update.load_config();
    assert!(app.update.current_source.is_none());
    assert!(app.update.config_error.is_some());
    assert_eq!(app.update.repository, UpdateRepositoryMode::Github);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn uninstall_panel_renders_summary_and_opens_confirmation() {
    let _language = LanguageGuard::set(Language::En);
    let mut app = ConsoleApp::new();
    app.focus = Focus::Panel;
    app.snapshot = installed_snapshot();

    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    terminal
        .draw(|frame| render_uninstall(frame, &mut app, frame.area()))
        .unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("Uninstall"));
    assert!(content.contains("1.2.3"));
    assert!(content.contains("Start uninstall"));
    assert!(
        content.contains("permanently deleted"),
        "the panel must warn about the data loss scope"
    );
    assert!(
        content.contains("config.toml, backups/, transactions/"),
        "the panel must list the retained items"
    );

    app.handle_uninstall_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.uninstall.confirming);

    terminal
        .draw(|frame| render_uninstall_confirmation(frame, &mut app))
        .unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("Confirm uninstall"));
    assert!(content.contains("Version 1.2.3"));
    // Uninstall is absent from Menu::ALL. Direct renderer coverage does not
    // assert an active-menu keyboard hint for this hidden panel.

    app.handle_uninstall_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(
        !app.uninstall.confirming,
        "Esc must cancel the confirmation"
    );
}

#[test]
fn uninstall_confirmation_builds_delegated_command() {
    let _language = LanguageGuard::set(Language::En);
    let mut app = ConsoleApp::new();
    app.focus = Focus::Panel;
    app.snapshot = installed_snapshot();

    app.handle_uninstall_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    let Some(Some(ConsoleAction::Command { command, args })) =
        app.handle_uninstall_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
    else {
        panic!("confirming Enter must dispatch the uninstall command");
    };
    assert!(matches!(command, Commands::Uninstall(_)));
    assert!(args.contains(&"uninstall".into()));
    assert!(args.contains(&"--yes".into()));
    assert!(args.contains(&"--console-confirmed".into()));
}

#[test]
fn uninstall_menu_requires_an_installation() {
    let _language = LanguageGuard::set(Language::En);
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    let mut app = ConsoleApp::new();
    app.focus = Focus::Panel;
    app.snapshot = Snapshot::NotInstalled;

    terminal
        .draw(|frame| render_uninstall(frame, &mut app, frame.area()))
        .unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("Uninstall requires an installed Landscape"));

    assert!(
        !app.menu_available(Menu::Uninstall),
        "the uninstall menu must be skipped when nothing is installed"
    );
}
