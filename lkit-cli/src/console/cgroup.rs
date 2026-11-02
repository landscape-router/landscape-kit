use crossterm::event::KeyCode;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph, Wrap};

use super::ConsoleApp;
use super::preflight::{CgroupPreparation, PreflightState};
use crate::check::model::Status;

impl ConsoleApp {
    pub(crate) fn poll_preflight(&mut self) {
        let preparing = matches!(&self.preflight.cgroup, CgroupPreparation::Running(_));
        self.preflight.poll();
        if preparing && matches!(&self.preflight.cgroup, CgroupPreparation::Idle) {
            self.notice = super::Notice::Success(crate::tr!(crate::keys::CONSOLE_CGROUP_READY));
            if matches!(self.preflight_gate(), super::preflight::GateState::None) {
                self.preflight_dialog = false;
            }
        }
    }

    pub(crate) fn preflight_cgroup_blocked(&self) -> bool {
        self.preflight.ops.can_enable_cgroup()
            && matches!(&self.preflight.state, PreflightState::Complete(report) if report.groups.iter()
                .flat_map(|group| &group.results).any(|result|
                    matches!(result.id, "kernel.cgroup" | "kernel.cgroup_cpu" | "openrc.cgroup_boot")
                    && matches!(result.status, Status::Error | Status::Unknown)))
    }

    pub(crate) fn open_cgroup_preparation(&mut self) {
        if self.preflight_cgroup_blocked() {
            self.preflight.cgroup = CgroupPreparation::Confirming;
        }
    }

    pub(crate) fn handle_cgroup_preparation_key(&mut self, code: KeyCode) -> bool {
        match &self.preflight.cgroup {
            CgroupPreparation::Confirming => {
                match code {
                    KeyCode::Enter => {
                        self.preflight.cgroup =
                            CgroupPreparation::Running(self.preflight.ops.enable_cgroup())
                    }
                    KeyCode::Esc => self.preflight.cgroup = CgroupPreparation::Idle,
                    _ => {}
                }
                true
            }
            CgroupPreparation::Running(_) => true,
            _ => false,
        }
    }
}

pub(crate) fn render_confirmation(frame: &mut Frame<'_>, app: &ConsoleApp) {
    let lines = if matches!(&app.preflight.cgroup, CgroupPreparation::Running(_)) {
        vec![Line::raw(crate::tr!(crate::keys::CONSOLE_CGROUP_RUNNING))]
    } else {
        vec![
            Line::raw(crate::tr!(crate::keys::CONSOLE_CGROUP_CONFIRM)),
            Line::raw(""),
            Line::raw("rc-service cgroups start"),
            Line::raw("rc-update add cgroups boot"),
            Line::raw(""),
            Line::raw(crate::tr!(crate::keys::CONSOLE_CGROUP_PROTECTION)),
        ]
    };
    let mut lines = lines;
    lines.push(Line::raw(""));
    lines.push(super::render::dialog_hint_line(app));
    let screen = frame.area();
    let width = 64.min(screen.width.saturating_sub(2));
    let height = (super::widgets::block_row_of(&lines, lines.len() - 1, width.saturating_sub(2))
        + 3)
    .min(screen.height.saturating_sub(2));
    let area = Rect::new(
        screen.x + screen.width.saturating_sub(width) / 2,
        screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    );
    super::render::begin_dialog(frame, area);
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(Block::bordered().title(crate::tr!(crate::keys::CONSOLE_CGROUP_TITLE))),
        area,
    );
}
