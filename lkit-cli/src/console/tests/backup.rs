use super::super::backup::BackupVerifyState;
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
    // 恢复 Enter 前必须校验通过:注入校验结果。
    app.backup.verify =
        BackupVerifyState::Complete(Ok("backup 20260807-131500-ab12cd34 verified".into()));

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
fn opening_details_starts_automatic_verify() {
    let _language = LanguageGuard::set(Language::En);
    let mut app = backup_ready_app();

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.backup.details, Some(0));
    assert!(
        matches!(app.backup.verify, BackupVerifyState::Running(_)),
        "entering details must start the full verification automatically"
    );

    // 等后台校验结束:示例条目路径不存在,校验失败。
    // 注意:用 backup.poll 而非 app.update()——update() 首次进入菜单会重置
    // 列表并调用 start(),把 verify 一并重置为 Idle。
    for _ in 0..200 {
        app.backup.poll(&mut app.notice);
        if matches!(app.backup.verify, BackupVerifyState::Complete(_)) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(
        matches!(app.backup.verify, BackupVerifyState::Complete(Err(_))),
        "the missing sample file must fail verification"
    );
}

#[test]
fn restore_enter_verifies_before_submitting() {
    let _language = LanguageGuard::set(Language::En);
    let mut app = backup_ready_app();

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
    assert!(app.backup.restore_confirming);

    // 未校验(Idle)时 Enter 先启动校验,不提交。
    let action = app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(action.is_none());
    assert!(app.backup.restore_confirming);
    assert!(matches!(app.backup.verify, BackupVerifyState::Running(_)));

    // 校验失败后再次 Enter:弹损坏框,不提交。
    for _ in 0..200 {
        app.backup.poll(&mut app.notice);
        if matches!(app.backup.verify, BackupVerifyState::Complete(_)) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let action = app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(action.is_none());
    assert!(!app.backup.restore_confirming);
    assert!(
        app.backup.corrupt_dialog,
        "corrupt backups must show the dialog"
    );
}

#[test]
fn restore_enter_rejects_when_verify_failed_and_dialog_closes() {
    let _language = LanguageGuard::set(Language::En);
    let mut app = backup_ready_app();
    app.backup.verify = BackupVerifyState::Complete(Err("backup is corrupt".into()));

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
    assert!(
        !app.backup.restore_confirming,
        "corrupt backups must not open the restore layer"
    );
    assert!(app.backup.corrupt_dialog);

    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(
        content.contains("corrupt"),
        "the corrupt dialog must render"
    );

    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(
        !app.backup.corrupt_dialog,
        "Enter must close the corrupt dialog"
    );
    // 弹框关闭后 Esc 回到面板级语义:返回主菜单选择,不再进入退出等待态。
    app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert_eq!(app.focus, Focus::Navigation);
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

    // 面板内第一次 Esc 返回导航层,导航层连续两次 Esc 才进入退出确认。
    app.handle_key(escape);
    assert_eq!(app.focus, Focus::Navigation);
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
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.focus, Focus::Panel);
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.backup.details, Some(0));
}
