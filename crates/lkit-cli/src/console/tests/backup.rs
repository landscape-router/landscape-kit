use super::super::widgets::*;
use super::super::*;
use super::support::*;
use crate::i18n::Language;
use ratatui::backend::TestBackend;

#[test]
fn backup_menu_lists_backups_and_opens_details() {
    let _language = LanguageGuard::set(Language::En);
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    let mut app = backup_ready_app();

    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("Backup"));
    assert!(content.contains("Create backup"));
    assert!(content.contains("20260807-131500-ab12cd34"));
    assert!(content.contains("before upgrade"));

    let mut app = backup_ready_app();
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.backup.details, Some(0));

    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let details = terminal_content(&terminal);
    assert!(details.contains("Backup details"));
    assert!(details.contains("x86_64"));
    assert!(details.contains("edge"));
    assert!(details.contains("Press R to restore"));

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(app.backup.details, None);
}

#[test]
fn backup_menu_without_installation_shows_requirements() {
    let _language = LanguageGuard::set(Language::En);
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    let mut app = ConsoleApp::new();
    app.menu_index = 2;
    app.focus = Focus::Panel;
    app.snapshot = Snapshot::NotInstalled;

    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("Landscape is not installed"));
    assert!(content.contains("Backup and restore require an existing installation"));
}

#[test]
fn backup_create_runs_in_console_with_progress_dialog() {
    let _language = LanguageGuard::set(Language::En);
    let mut app = backup_ready_app();
    app.install.install_dir = std::env::temp_dir()
        .join(format!("lkit-console-create-{}", std::process::id()))
        .display()
        .to_string();

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.backup.editing);

    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(
        content.contains("Create backup"),
        "the backup create dialog must be visible while editing"
    );
    assert!(content.contains("Remark: _"));

    for character in "my-backup".chars() {
        app.handle_key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
    }
    assert_eq!(app.backup.remark, "my-backup");

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(!app.backup.editing);
    assert_eq!(app.backup.remark, "");
    assert!(
        app.backup.create.is_some(),
        "Enter must start the in-console backup create worker"
    );

    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(
        content.contains("Creating backup"),
        "the progress dialog must be visible while the backup is created"
    );
    assert!(
        content.contains("Exporting configuration"),
        "the progress dialog must show the export stage"
    );

    let _ = std::fs::remove_dir_all(&app.install.install_dir);
}

#[test]
fn backup_restore_flow_builds_restore_command() {
    let _language = LanguageGuard::set(Language::En);
    let mut app = backup_ready_app();

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.backup.selected, 1);
    app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
    assert!(app.backup.restore_confirming);

    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("Restore this backup?"));
    assert!(content.contains("version 1.2.3"));
    assert!(content.contains("Press Enter to restore."));
    assert!(
        content.contains("SQLite data file"),
        "the restore confirmation must warn about the minimal scope"
    );

    let action = app
        .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
    let ConsoleAction::Command { command, args } = action else {
        panic!("expected restore command");
    };
    let Commands::Restore(restore) = command else {
        panic!("expected restore request");
    };
    assert_eq!(restore.backup.as_deref(), Some("20260807-131500-ab12cd34"));
    assert!(restore.yes);
    assert!(
        restore.console_confirmed,
        "the console must mark the restore as confirmed so no TTY prompt appears"
    );
    assert!(
        args.windows(2)
            .any(|pair| pair == ["--backup", "20260807-131500-ab12cd34"])
    );
    assert!(args.iter().any(|argument| argument == "--yes"));
    assert!(
        args.iter()
            .any(|argument| argument == "--console-confirmed")
    );
}

#[test]
fn restore_confirmation_wraps_the_minimal_scope_note_on_narrow_terminals() {
    let _language = LanguageGuard::set(Language::En);
    let mut app = backup_ready_app();
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
    assert!(app.backup.restore_confirming);

    let mut terminal = Terminal::new(TestBackend::new(80, 18)).unwrap();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(
        content.contains("will be lost"),
        "the minimal scope note must wrap inside the restore dialog instead of truncating"
    );
}

#[test]
fn backup_delete_confirms_and_removes_the_backup() {
    use std::os::unix::fs::PermissionsExt;
    let _language = LanguageGuard::set(Language::En);
    let dir = std::env::temp_dir().join(format!("lkit-console-delete-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let _guard = crate::deployment::layout::test_territory(&dir);
    let install = dir.join("install");
    std::fs::create_dir_all(&install).unwrap();
    let state = crate::deployment::state::InstallState {
        schema_version: 1,
        layout_version: 2,
        install_root: install.display().to_string(),
        canonical_install_root: install.display().to_string(),
        active_version: "1.2.3".into(),
        assets: crate::deployment::state::Assets {
            webserver: crate::deployment::state::WebserverAsset {
                architecture: crate::deployment::state::StateArchitecture::X86_64,
                sha256: "a".repeat(64),
                size: 1,
            },
            static_archive: crate::deployment::state::ArchiveAsset {
                sha256: "b".repeat(64),
                size: 1,
            },
        },
        initialization: crate::deployment::state::InitializationState {
            status: crate::deployment::state::InitStatus::Complete,
            lock_present: true,
            initialized_at: Some(chrono::Utc::now()),
        },
        service: crate::deployment::state::ServiceState {
            manager: crate::deployment::state::StateServiceManager::Systemd,
            registered: true,
            enabled: true,
            verified: true,
            definition_path: Some("service/landscape-router.service".into()),
            definition_sha256: Some("c".repeat(64)),
        },
        last_transaction_id: None,
        committed_at: Some(chrono::Utc::now()),
    };
    crate::deployment::state::write_state(
        &crate::deployment::root::InstallRoot {
            install_root: install.clone(),
            canonical: install.clone(),
        },
        &state,
    )
    .unwrap();
    let backups = dir.join("backups");
    std::fs::create_dir_all(&backups).unwrap();
    let path = backups.join("20260807-131500-ab12cd34.lkb");
    std::fs::write(&path, b"lkb bytes").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

    let mut app = backup_ready_app();
    app.install.install_dir = dir.display().to_string();
    app.backup.state =
        BackupListState::Complete(vec![sample_backup_entry(), sample_backup_entry()]);

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    assert!(app.backup.delete_confirming);
    assert_eq!(
        app.backup.delete_target.as_deref(),
        Some("20260807-131500-ab12cd34")
    );

    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("Confirm delete"));
    assert!(content.contains("Delete this backup?"));
    assert!(content.contains("version 1.2.3"));
    assert!(content.contains("Press Enter to delete."));

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(!app.backup.delete_confirming);
    assert!(app.backup.delete_target.is_none());
    assert!(
        !path.exists(),
        "confirming the delete must remove the backup file"
    );
    assert!(app.notice.contains("deleted"));
    assert!(matches!(app.backup.state, BackupListState::NotRun));

    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn backup_delete_esc_cancels_confirmation() {
    let _language = LanguageGuard::set(Language::En);
    let mut app = backup_ready_app();
    app.backup.state = BackupListState::Complete(vec![sample_backup_entry()]);

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
    assert!(app.backup.delete_confirming);
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!app.backup.delete_confirming);
    assert!(app.backup.delete_target.is_none());
    assert_eq!(app.exit_state, ExitState::Idle);
}

#[test]
fn backup_esc_cancels_restore_confirmation_and_details() {
    let _language = LanguageGuard::set(Language::En);
    let mut app = backup_ready_app();

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
    assert!(app.backup.restore_confirming);
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!app.backup.restore_confirming);
    assert_eq!(app.exit_state, ExitState::Idle);

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.backup.details, Some(0));
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(app.backup.details, None);
    assert_eq!(app.exit_state, ExitState::Idle);
}

#[test]
fn exit_confirmation_takes_precedence_over_backup_panel_keys() {
    let _language = LanguageGuard::set(Language::En);
    let mut app = backup_ready_app();
    let escape = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);

    app.handle_key(escape);
    app.handle_key(escape);
    assert_eq!(app.exit_state, ExitState::Confirming);

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(
        app.backup.selected, 0,
        "panel keys must not leak into the backup list"
    );
    assert_eq!(app.exit_state, ExitState::Confirming);

    assert!(matches!(
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Some(ConsoleAction::Quit)
    ));
    assert_eq!(app.backup.details, None);

    app.handle_key(escape);
    assert_eq!(app.exit_state, ExitState::Idle);
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.backup.details, Some(0));
}

#[test]
fn mouse_click_backup_rows_open_details_and_create_dialog() {
    let _language = LanguageGuard::set(Language::En);
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    let mut app = backup_ready_app();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    app.handle_mouse(mouse_click(30, 5));
    assert_eq!(app.backup.details, Some(0));

    let mut app = backup_ready_app();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    app.handle_mouse(mouse_click(30, 4));
    assert!(
        app.backup.editing,
        "clicking the create row must open the remark dialog"
    );
}
