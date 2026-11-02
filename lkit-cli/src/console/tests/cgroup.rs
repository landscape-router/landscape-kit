use std::sync::Arc;

use super::super::preflight::{CgroupPreparation, GateState, MockPreflightOps, PreflightState};
use super::super::*;
use crate::check::model::{CheckGroup, CheckReport, CheckResult, Status, StatusCounts};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

const OPENRC_BOOT: &str = "openrc.cgroup_boot";

fn report(id: &'static str, status: Status) -> CheckReport {
    CheckReport {
        groups: vec![CheckGroup {
            title: "OpenRC startup requirements".into(),
            results: vec![CheckResult::new(id, "Cgroup boot").set(status, "fixture", "fixture")],
        }],
        summary: status,
        counts: StatusCounts::default(),
    }
}

fn app_with(id: &'static str) -> (ConsoleApp, Arc<MockPreflightOps>) {
    let ops = Arc::new(MockPreflightOps::with_cgroup());
    let mut app = ConsoleApp::new();
    app.menu_index = 1;
    app.preflight.ops = ops.clone();
    app.preflight.state = PreflightState::Complete(report(id, Status::Error));
    app.preflight_dialog = true;
    (app, ops)
}

fn key(app: &mut ConsoleApp, code: KeyCode) {
    app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
}

#[test]
fn cgroup_action_matches_the_openrc_domain_boot_check_only() {
    for id in [OPENRC_BOOT, "kernel.cgroup", "kernel.cgroup_cpu"] {
        let (app, _) = app_with(id);
        assert!(app.preflight_cgroup_blocked());
    }
    for id in [
        "kernel.cgroup_boot",
        "kernel.openrc.cgroup_boot",
        "kernel.systemd.cgroup_boot",
        "kernel.cgroup_bpf",
    ] {
        let (app, _) = app_with(id);
        assert!(
            !app.preflight_cgroup_blocked(),
            "unexpected repair match for {id}"
        );
    }
}

#[test]
fn enabling_requires_confirmation_and_refreshes_the_report() {
    let (mut app, ops) = app_with(OPENRC_BOOT);
    key(&mut app, KeyCode::Enter);
    assert!(matches!(
        app.preflight.cgroup,
        CgroupPreparation::Confirming
    ));
    key(&mut app, KeyCode::Esc);
    assert!(matches!(app.preflight.cgroup, CgroupPreparation::Idle));
    assert!(matches!(app.preflight_gate(), GateState::Dialog));
    key(&mut app, KeyCode::Char('c'));
    assert!(matches!(
        app.preflight.cgroup,
        CgroupPreparation::Confirming
    ));
    key(&mut app, KeyCode::Enter);
    assert!(matches!(
        app.preflight.cgroup,
        CgroupPreparation::Running(_)
    ));
    key(&mut app, KeyCode::Char('r'));
    assert!(matches!(
        app.preflight.cgroup,
        CgroupPreparation::Running(_)
    ));
    ops.cgroup_sender()
        .send(Ok(report(OPENRC_BOOT, Status::Pass)))
        .unwrap();
    app.poll_preflight();
    assert!(matches!(app.preflight.cgroup, CgroupPreparation::Idle));
    assert!(matches!(app.preflight_gate(), GateState::None));
    assert!(
        !app.preflight_dialog,
        "successful preparation must close the stale blocking dialog"
    );
}

#[test]
fn confirmation_shows_both_mandatory_commands_without_an_optional_boot_toggle() {
    use super::support::{LanguageGuard, terminal_content};
    use ratatui::{Terminal, backend::TestBackend};
    let _language = LanguageGuard::set(crate::i18n::Language::En);
    let (mut app, _) = app_with(OPENRC_BOOT);
    key(&mut app, KeyCode::Enter);
    let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let content = terminal_content(&terminal);
    assert!(content.contains("rc-service cgroups start"));
    assert!(content.contains("rc-update add cgroups boot"));
    // The confirmation paragraph wraps inside the modal.
    assert!(content.contains("Both"), "{content}");
    assert!(content.contains("required"), "{content}");
}

#[test]
fn other_failures_still_block_after_cgroup_preparation() {
    let (mut app, ops) = app_with(OPENRC_BOOT);
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Enter);
    ops.cgroup_sender()
        .send(Ok(report("kernel.bpf", Status::Error)))
        .unwrap();
    app.poll_preflight();
    assert!(matches!(app.preflight_gate(), GateState::Dialog));
    assert!(app.preflight_dialog);
    assert!(!app.preflight_cgroup_blocked());
}

#[test]
fn unsupported_hosts_do_not_offer_cgroup_preparation() {
    let (mut app, _) = app_with(OPENRC_BOOT);
    app.preflight.ops = Arc::new(MockPreflightOps::manual());
    assert!(!app.preflight_cgroup_blocked());
    key(&mut app, KeyCode::Char('c'));
    assert!(matches!(app.preflight.cgroup, CgroupPreparation::Idle));
}

#[test]
fn disconnected_worker_keeps_the_old_report_blocking() {
    let (mut app, ops) = app_with(OPENRC_BOOT);
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Enter);
    app.preflight.ops = Arc::new(MockPreflightOps::manual());
    drop(ops);
    app.poll_preflight();
    assert!(matches!(app.preflight.cgroup, CgroupPreparation::Failed(_)));
    assert!(matches!(app.preflight_gate(), GateState::Dialog));
    assert!(app.preflight_dialog);
}

#[test]
fn preparation_failure_keeps_installation_blocked() {
    let (mut app, ops) = app_with(OPENRC_BOOT);
    key(&mut app, KeyCode::Enter);
    key(&mut app, KeyCode::Enter);
    ops.cgroup_sender()
        .send(Err("boot registration failed".into()))
        .unwrap();
    app.poll_preflight();
    assert!(app.preflight_dialog);
    assert!(
        matches!(&app.preflight.cgroup, CgroupPreparation::Failed(error) if error == "boot registration failed")
    );
    assert!(matches!(app.preflight_gate(), GateState::Dialog));
}
