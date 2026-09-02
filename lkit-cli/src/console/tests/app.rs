use super::super::install_form::InstallField;
use super::super::*;
use super::super::{network_wizard::WizardStep, update::*, widgets::*};
use super::support::*;
use crate::i18n::Language;
use crate::network::config::{NetworkMode, NetworkPlan, SelectedInterface};
use ratatui::backend::TestBackend;

#[test]
fn renders_panel_focus_marker_on_overview() {
    let _language = LanguageGuard::set(Language::En);
    let backend = TestBackend::new(100, 28);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut app = ConsoleApp::new();
    app.focus = Focus::Panel;

    terminal.draw(|frame| render(frame, &mut app)).unwrap();

    assert!(terminal_content(&terminal).contains("> Overview"));

    app.menu_index = 2;
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    assert!(terminal_content(&terminal).contains("> Backup"));
}

#[test]
fn install_menu_is_skipped_when_landscape_is_installed() {
    let mut app = ConsoleApp::new();
    app.snapshot = installed_snapshot();
    assert_eq!(app.menu(), Menu::Overview);

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.menu(), Menu::Backup);

    app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(app.menu(), Menu::Overview);
}

#[test]
fn install_menu_stays_selectable_when_not_installed() {
    let mut app = ConsoleApp::new();
    app.snapshot = Snapshot::NotInstalled;

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.menu(), Menu::Install);

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.menu(), Menu::Backup);
}

#[test]
fn language_key_switches_the_tui_and_updates_the_footer() {
    let _language = LanguageGuard::set(Language::En);
    let backend = TestBackend::new(100, 28);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut app = ConsoleApp::new();

    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let english = terminal_content(&terminal);
    assert!(english.contains("Navigation"));
    assert!(english.contains("Ctrl+C Exit"));
    // 可切换时底栏显示目标语言(所见即所得),不显示当前语言。
    assert!(english.contains("[L] Switch to 中文 (zh)"));
    assert!(!english.contains("Language: English"));

    app.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));
    assert_eq!(crate::i18n::current(), Language::Zh);
    let mut chinese_terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    chinese_terminal
        .draw(|frame| render(frame, &mut app))
        .unwrap();
    let chinese = terminal_content(&chinese_terminal);
    assert!(chinese.contains("导航"));
    assert!(chinese.contains("Ctrl+C 退出"));
    assert!(chinese.contains("[L] 切换到 English (en)"));
    assert!(!chinese.contains("Switch to"));

    app.handle_key(KeyEvent::new(KeyCode::Char('L'), KeyModifiers::SHIFT));
    assert_eq!(crate::i18n::current(), Language::En);
}

#[test]
fn language_indicator_click_switches_to_the_shown_target() {
    let _language = LanguageGuard::set(Language::En);
    let backend = TestBackend::new(100, 28);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut app = ConsoleApp::new();

    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    let row = content
        .lines()
        .position(|line| line.contains("[L] Switch to"))
        .expect("the language indicator must render") as u16;
    let column = content
        .lines()
        .nth(row as usize)
        .and_then(|line| line.find("[L]"))
        .expect("the keycap must render") as u16;
    assert_eq!(
        app.hits.hit_at(column + 1, row),
        Some(Hit::LanguageSwitch),
        "the language indicator must be clickable"
    );

    app.handle_mouse(mouse_click(column + 1, row));
    assert_eq!(
        crate::i18n::current(),
        Language::Zh,
        "clicking the indicator must switch to the shown target language"
    );
}

#[test]
fn long_panel_hint_wraps_on_dynamic_status_lines() {
    let _language = LanguageGuard::set(Language::En);
    let mut terminal = Terminal::new(TestBackend::new(72, 18)).unwrap();
    let mut app = backup_ready_app();

    terminal.draw(|frame| render(frame, &mut app)).unwrap();

    let content = terminal_content(&terminal);
    assert!(
        content.contains("Esc Menu"),
        "the long backup list hint must wrap onto the second hint line instead of truncating"
    );
}

#[test]
fn long_notice_wraps_instead_of_truncating() {
    let _language = LanguageGuard::set(Language::En);
    let mut terminal = Terminal::new(TestBackend::new(72, 18)).unwrap();
    let mut app = ConsoleApp::new();
    let notice = "this is a long status notice that must wrap onto a second line: backup failed";
    app.notice = notice.into();

    terminal.draw(|frame| render(frame, &mut app)).unwrap();

    let content = terminal_content(&terminal);
    assert!(
        content.contains("backup failed"),
        "a long status notice must wrap onto the second status line instead of truncating"
    );
}

#[test]
fn language_key_switches_on_install_panel_when_not_editing() {
    let _language = LanguageGuard::set(Language::En);
    let mut app = ConsoleApp::new();
    app.menu_index = 1;
    app.focus = Focus::Panel;
    app.install.editing = false;

    app.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));

    assert_eq!(crate::i18n::current(), Language::Zh);
    assert!(!app.install.editing);
}

#[test]
fn language_key_remains_text_while_editing() {
    let _language = LanguageGuard::set(Language::En);
    let mut app = ConsoleApp::new();
    app.menu_index = 1;
    app.focus = Focus::Panel;
    app.install.checks_selected = false;
    app.install.selected = InstallField::Version;
    app.install.editing = true;
    app.install.version.clear();

    app.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));

    assert_eq!(crate::i18n::current(), Language::En);
    assert_eq!(app.install.version, "l");
    assert!(!app.language_switch_available());
}

#[test]
fn language_key_switches_on_update_confirm_layer() {
    let _language = LanguageGuard::set(Language::En);
    let mut app = update_ready_app();
    app.update.confirming = Some(resolved("1.2.3", "1.3.0"));

    app.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));

    assert_eq!(crate::i18n::current(), Language::Zh);
    assert!(
        app.update.confirming.is_some(),
        "the confirm layer must stay open after the switch"
    );
}

#[test]
fn language_key_switches_on_backup_details_page() {
    let _language = LanguageGuard::set(Language::En);
    let mut app = backup_ready_app();
    app.backup.details = Some(0);

    app.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));

    assert_eq!(crate::i18n::current(), Language::Zh);
    assert_eq!(
        app.backup.details,
        Some(0),
        "the details page must stay open after the switch"
    );
}

#[test]
fn language_key_switches_inside_network_wizard() {
    let _language = LanguageGuard::set(Language::En);
    let mut app = ConsoleApp::new();
    app.network_wizard = Some(sample_network_wizard());

    app.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));

    assert_eq!(crate::i18n::current(), Language::Zh);
    assert!(
        app.network_wizard.is_some(),
        "the wizard must stay open after the switch"
    );
}

#[test]
fn language_key_remains_text_while_editing_in_network_wizard() {
    let _language = LanguageGuard::set(Language::En);
    let mut app = ConsoleApp::new();
    let mut wizard = routes_armed_wizard();
    wizard.step = WizardStep::WanConfig;
    wizard.focus = 1;
    wizard.editing = true;
    app.network_wizard = Some(wizard);

    app.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));

    assert_eq!(crate::i18n::current(), Language::En);
    assert_eq!(
        app.network_wizard.as_ref().unwrap().address,
        "l",
        "l must be typed into the wizard field instead of switching language"
    );
}

#[test]
fn language_key_switches_on_preflight_dialog() {
    let _language = LanguageGuard::set(Language::En);
    let mut app = ConsoleApp::new();
    app.preflight_dialog = true;

    app.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));

    assert_eq!(crate::i18n::current(), Language::Zh);
    assert!(app.preflight_dialog, "the dialog must stay open");
}

#[test]
fn language_key_stays_disabled_while_exit_confirmation_is_open() {
    let _language = LanguageGuard::set(Language::En);
    let mut app = ConsoleApp::new();
    app.exit_state = ExitState::Confirming;

    app.handle_key(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));

    assert_eq!(crate::i18n::current(), Language::En);
    assert_eq!(app.exit_state, ExitState::Confirming);
}

#[test]
fn double_escape_opens_confirmation_before_enter_exits() {
    let mut app = ConsoleApp::new();
    let escape = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);

    assert!(app.handle_key(escape).is_none());
    assert_eq!(app.exit_state, ExitState::Armed);

    let backend = TestBackend::new(100, 28);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let armed: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(armed.contains("Exit armed"));
    assert!(!armed.contains("Exit Landscape Kit?"));

    assert!(
        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE))
            .is_none()
    );
    assert_eq!(app.exit_state, ExitState::Idle);

    assert!(app.handle_key(escape).is_none());
    assert!(app.handle_key(escape).is_none());
    assert_eq!(app.exit_state, ExitState::Confirming);

    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let confirmation: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(confirmation.contains("Exit Landscape Kit?"));
    assert!(confirmation.contains("Press Enter to exit"));

    assert!(matches!(
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Some(ConsoleAction::Quit)
    ));
}

#[test]
fn renders_stable_small_terminal_state() {
    let backend = TestBackend::new(60, 14);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut app = ConsoleApp::new();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content: String = terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(content.contains("Terminal too small"));
}

#[test]
fn pending_takeover_snapshot_is_detected_from_transaction() {
    let temp = std::env::temp_dir().join(format!("lkit-console-pending-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp);
    std::fs::create_dir_all(&temp).unwrap();
    let territory = temp.join("territory");
    std::fs::create_dir_all(&territory).unwrap();
    let _guard = crate::deployment::layout::test_territory(&territory);
    let root = crate::deployment::root::normalize_install_root(&temp).unwrap();
    let state = crate::deployment::state::InstallState {
        schema_version: 1,
        layout_version: 2,
        install_root: root.canonical.display().to_string(),
        canonical_install_root: root.canonical.display().to_string(),
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
    crate::deployment::state::write_state(&root, &state).unwrap();
    let mut transaction = crate::deployment::transaction::TransactionFile::new_install(
        &root,
        &semver::Version::new(1, 0, 0),
    )
    .unwrap();
    transaction.phase = crate::deployment::transaction::Phase::AwaitingNetworkConfirmation;
    let id = transaction.transaction_id.clone();
    transaction.network_takeover =
        Some(crate::deployment::transaction::NetworkTakeoverTransaction {
            plan: NetworkPlan {
                mode: NetworkMode::RoutedLan {
                    wan: "ens3".into(),
                    wan_ipv4: None,
                    lan: vec!["ens4".into()],
                    management: "192.168.10.1/24".parse().unwrap(),
                    dhcp_start: "192.168.10.100".parse().unwrap(),
                    dhcp_end: "192.168.10.254".parse().unwrap(),
                },
                selected_macs: vec![
                    SelectedInterface {
                        name: "ens3".into(),
                        mac: "02:00:00:00:00:03".into(),
                    },
                    SelectedInterface {
                        name: "ens4".into(),
                        mac: "02:00:00:00:00:04".into(),
                    },
                ],
            },
            host_services: Vec::new(),
            confirmation_deadline: chrono::Utc::now() + chrono::Duration::minutes(10),
            rollback_service: format!("lkit-network-{id}-rollback.service"),
            rollback_timer: format!("lkit-network-{id}-rollback.timer"),
            boot_rollback_service: format!("lkit-network-{id}-boot-rollback.service"),
            recovery_binary: "service/lkit-network-recovery".into(),
            pending_state: format!("transactions/{id}/pending-install-state.json"),
        });
    crate::deployment::transaction::persist(&root, &transaction).unwrap();
    let snapshot = Snapshot::load();
    let _ = std::fs::remove_dir_all(&temp);
    match snapshot {
        // 以 root 运行测试时 Snapshot::load 返回 RootRequired，跳过检测断言。
        Snapshot::RootRequired => {}
        Snapshot::AwaitingNetworkConfirmation {
            transaction_id,
            phase,
            management_address,
            ..
        } => {
            assert_eq!(transaction_id, id);
            assert_eq!(phase, "awaiting_network_confirmation");
            assert_eq!(management_address.as_deref(), Some("192.168.10.1/24"));
        }
        _ => panic!("expected pending snapshot, got a different state"),
    }
}

#[test]
fn pending_takeover_without_committed_state_is_detected_from_transaction() {
    // 等待确认的接管安装尚未提交状态:根只能从未完成事务发现
    // (discover_landscape_root 返回 None 时必须回退到事务目录)。
    let temp = std::env::temp_dir().join(format!("lkit-console-pending-tx-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp);
    std::fs::create_dir_all(&temp).unwrap();
    let territory = temp.join("territory");
    std::fs::create_dir_all(&territory).unwrap();
    let _guard = crate::deployment::layout::test_territory(&territory);
    let root = crate::deployment::root::normalize_install_root(&temp).unwrap();
    let mut transaction = crate::deployment::transaction::TransactionFile::new_install(
        &root,
        &semver::Version::new(1, 0, 0),
    )
    .unwrap();
    transaction.phase = crate::deployment::transaction::Phase::AwaitingNetworkConfirmation;
    let id = transaction.transaction_id.clone();
    transaction.network_takeover =
        Some(crate::deployment::transaction::NetworkTakeoverTransaction {
            plan: NetworkPlan {
                mode: NetworkMode::RoutedLan {
                    wan: "ens3".into(),
                    wan_ipv4: None,
                    lan: vec!["ens4".into()],
                    management: "192.168.10.1/24".parse().unwrap(),
                    dhcp_start: "192.168.10.100".parse().unwrap(),
                    dhcp_end: "192.168.10.254".parse().unwrap(),
                },
                selected_macs: vec![
                    SelectedInterface {
                        name: "ens3".into(),
                        mac: "02:00:00:00:00:03".into(),
                    },
                    SelectedInterface {
                        name: "ens4".into(),
                        mac: "02:00:00:00:00:04".into(),
                    },
                ],
            },
            host_services: Vec::new(),
            confirmation_deadline: chrono::Utc::now() + chrono::Duration::minutes(10),
            rollback_service: format!("lkit-network-{id}-rollback.service"),
            rollback_timer: format!("lkit-network-{id}-rollback.timer"),
            boot_rollback_service: format!("lkit-network-{id}-boot-rollback.service"),
            recovery_binary: "service/lkit-network-recovery".into(),
            pending_state: format!("transactions/{id}/pending-install-state.json"),
        });
    crate::deployment::transaction::persist(&root, &transaction).unwrap();
    let snapshot = Snapshot::load();
    let _ = std::fs::remove_dir_all(&temp);
    match snapshot {
        // 以 root 运行测试时 Snapshot::load 返回 RootRequired，跳过检测断言。
        Snapshot::RootRequired => {}
        Snapshot::AwaitingNetworkConfirmation {
            transaction_id,
            phase,
            management_address,
            ..
        } => {
            assert_eq!(transaction_id, id);
            assert_eq!(phase, "awaiting_network_confirmation");
            assert_eq!(management_address.as_deref(), Some("192.168.10.1/24"));
        }
        _ => panic!("expected pending snapshot from the unfinished transaction, got another state"),
    }
}

#[test]
fn pending_takeover_blocking_screen_renders_instead_of_menu() {
    let _language = LanguageGuard::set(Language::En);
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    let mut app = ConsoleApp::new();
    app.snapshot = pending_takeover_snapshot();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("Network takeover awaiting confirmation"));
    assert!(content.contains("tx-1"));
    assert!(content.contains("awaiting_network_confirmation"));
    assert!(content.contains("192.168.10.1/24"));
    assert!(content.contains("current time:"));
    assert!(content.contains("auto rollback in"));
    assert!(content.contains("Later"));
    assert!(content.contains("Confirm now"));
    assert!(!content.contains("Navigation"));
}

#[test]
fn pending_takeover_enter_confirm_executes_network_confirm() {
    let _language = LanguageGuard::set(Language::En);
    let mut app = ConsoleApp::new();
    app.snapshot = pending_takeover_snapshot();
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.takeover_choice, 1);
    let action = app
        .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .expect("enter on confirm must return an action");
    let ConsoleAction::Command { command, args } = action else {
        panic!("expected a command action");
    };
    assert!(matches!(
        command,
        Commands::Network(crate::commands::network::Network {
            action: crate::commands::network::NetworkAction::Confirm,
            ..
        })
    ));
    assert_eq!(args[0], "network");
    assert_eq!(args[1], "confirm");
    assert!(
        !args.iter().any(|argument| argument == "--install-dir"),
        "network confirm must not forward --install-dir"
    );
}

#[test]
fn pending_takeover_later_and_esc_quit_the_console() {
    let _language = LanguageGuard::set(Language::En);
    let mut app = ConsoleApp::new();
    app.snapshot = pending_takeover_snapshot();
    assert!(matches!(
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Some(ConsoleAction::Quit)
    ));
    assert!(matches!(
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        Some(ConsoleAction::Quit)
    ));
    assert!(
        app.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE))
            .is_none()
    );
}

#[test]
fn rolling_back_pending_disables_confirm() {
    let _language = LanguageGuard::set(Language::En);
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    let mut app = ConsoleApp::new();
    app.snapshot = Snapshot::AwaitingNetworkConfirmation {
        transaction_id: "tx-1".into(),
        phase: "rolling_back",
        deadline: chrono::Utc::now() + chrono::Duration::minutes(10),
        management_address: None,
    };
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("rollback in progress"));
    assert!(content.contains("DHCP lease"));
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.takeover_choice, 0);
    assert!(matches!(
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Some(ConsoleAction::Quit)
    ));
}

#[test]
fn pending_takeover_hides_install_menu() {
    let mut app = ConsoleApp::new();
    app.snapshot = pending_takeover_snapshot();
    assert!(!app.install_available());
    assert!(!app.menu_available(Menu::Install));
}

#[test]
fn update_menu_is_only_available_when_installed() {
    let mut app = ConsoleApp::new();
    app.snapshot = Snapshot::NotInstalled;
    assert_eq!(app.menu(), Menu::Overview);

    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.menu(), Menu::Install);
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.menu(), Menu::Backup);
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(
        app.menu(),
        Menu::Mirror,
        "Update must be skipped when Landscape is not installed"
    );

    let mut app = ConsoleApp::new();
    app.snapshot = installed_snapshot();
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.menu(), Menu::Backup);
    app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    assert_eq!(app.menu(), Menu::Update);
}

#[test]
fn start_update_validates_before_background_resolution() {
    let mut app = update_ready_app();
    app.update.version = "nightly".into();
    app.update.selected = UpdateField::Start;
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(
        app.update.resolving.is_none(),
        "an invalid version must not start the resolver"
    );
    assert!(!app.notice.is_empty());

    let mut app = update_ready_app();
    app.update.repository = UpdateRepositoryMode::Custom;
    app.update.repository_url = "not a url".into();
    app.update.selected = UpdateField::Start;
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.update.resolving.is_none());
    assert!(!app.notice.is_empty());

    let mut app = update_ready_app();
    app.install.install_dir = std::env::temp_dir()
        .join(format!("lkit-console-update-{}", std::process::id()))
        .display()
        .to_string();
    app.update.selected = UpdateField::Start;
    app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(
        app.update.resolving.is_some(),
        "a valid form must start the background resolver"
    );
    let _ = std::fs::remove_dir_all(&app.install.install_dir);
}

#[test]
fn mouse_click_selects_navigation_menu_and_switches_focus() {
    let _language = LanguageGuard::set(Language::En);
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    let mut app = ConsoleApp::new();
    app.menu_index = 1;
    app.focus = Focus::Panel;
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    assert_eq!(app.hits.hit_at(10, 4), Some(Hit::Menu(1)));
    assert_eq!(app.hits.hit_at(10, 5), Some(Hit::Menu(2)));
    assert_eq!(app.hits.hit_at(30, 3), Some(Hit::InstallChecks));
    assert_eq!(app.hits.hit_at(50, 19), Some(Hit::Panel));
    app.handle_mouse(mouse_click(10, 5));
    assert_eq!(app.menu_index, 2);
    assert_eq!(app.focus, Focus::Panel);
    app.handle_mouse(mouse_click(5, 25));
    assert_eq!(
        app.focus,
        Focus::Panel,
        "clicks outside any region are ignored"
    );
    app.handle_mouse(mouse_click(10, 3));
    assert_eq!(app.menu_index, 0);
    assert_eq!(app.focus, Focus::Panel);
}

#[test]
fn mouse_click_dialog_inside_confirms_and_outside_cancels() {
    let _language = LanguageGuard::set(Language::En);
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    let mut app = ConsoleApp::new();
    app.exit_state = ExitState::Confirming;
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    assert!(matches!(
        app.handle_mouse(mouse_click(49, 13)),
        Some(ConsoleAction::Quit)
    ));

    let mut app = ConsoleApp::new();
    app.exit_state = ExitState::Confirming;
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    assert!(app.handle_mouse(mouse_click(2, 2)).is_none());
    assert_eq!(app.exit_state, ExitState::Idle);
}

#[test]
fn mouse_right_click_acts_as_escape_and_returns_to_navigation() {
    let _language = LanguageGuard::set(Language::En);
    let mut app = ConsoleApp::new();
    app.focus = Focus::Panel;
    app.handle_mouse(mouse_right_click(30, 10));
    assert_eq!(app.focus, Focus::Navigation);
    assert_eq!(app.exit_state, ExitState::Idle);
}
