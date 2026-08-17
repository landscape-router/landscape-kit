use super::{DownloadState, OperationPhase, OperationResult, OperationScreen};
use super::{is_confirmation_line, step_phase_text};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Gauge, Paragraph, Wrap};

pub(crate) struct InstallScreen;

impl OperationScreen for InstallScreen {
    fn result_key(&self, result: OperationResult) -> &'static str {
        match result {
            OperationResult::Success => crate::keys::PRESENTATION_INSTALLATION_COMPLETE,
            OperationResult::Failed => crate::keys::PRESENTATION_INSTALLATION_FAILED,
            OperationResult::Cancelled => crate::keys::PRESENTATION_INSTALLATION_CANCELLED,
        }
    }

    fn stop_ignored_key(&self) -> &'static str {
        crate::keys::PRESENTATION_INSTALLATION_IS_APPLYING
    }

    fn announce_prefix(&self) -> &'static str {
        "install"
    }

    fn takeover_confirmable(&self) -> bool {
        true
    }

    #[allow(clippy::too_many_arguments)]
    fn render(
        &self,
        frame: &mut Frame<'_>,
        phase: OperationPhase,
        step_progress: Option<(u8, u8)>,
        current: Option<&DownloadState>,
        logs: &[String],
        notice: &str,
        confirming_stop: bool,
        result: Option<OperationResult>,
        takeover_pending: bool,
    ) {
        let [header, body, footer] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(3),
        ])
        .areas(frame.area());
        let header_text = match result {
            Some(OperationResult::Success) => {
                crate::tr!(crate::keys::PRESENTATION_INSTALLATION_COMPLETE)
            }
            Some(OperationResult::Failed) => {
                crate::tr!(crate::keys::PRESENTATION_INSTALLATION_FAILED)
            }
            Some(OperationResult::Cancelled) => {
                crate::tr!(crate::keys::PRESENTATION_INSTALLATION_CANCELLED)
            }
            None => crate::tr!(crate::keys::PRESENTATION_OPERATION_INSTALL),
        };
        frame.render_widget(
            Paragraph::new(header_text)
                .style(Style::default().add_modifier(Modifier::BOLD))
                .block(Block::default().borders(Borders::BOTTOM)),
            header,
        );
        let [progress, log_area] =
            Layout::vertical([Constraint::Length(4), Constraint::Min(3)]).areas(body);
        if let Some(state) = current {
            let percent = if state.total == 0 {
                0.0
            } else {
                state.position as f64 / state.total as f64
            };
            let label = format!(
                "{}  {:>3}%  {} / {}",
                state.label,
                (percent * 100.0).round() as u64,
                crate::interaction::presentation::human_bytes(state.position),
                crate::interaction::presentation::human_bytes(state.total),
            );
            frame.render_widget(
                Gauge::default()
                    .ratio(percent.clamp(0.0, 1.0))
                    .label(label)
                    .gauge_style(Style::default().fg(Color::Cyan))
                    .use_unicode(false)
                    .block(Block::bordered().title(crate::tr!(crate::keys::PRESENTATION_DOWNLOAD))),
                progress,
            );
        } else if let Some((step, total)) = step_progress {
            let ratio = if total == 0 {
                0.0
            } else {
                f64::from(step) / f64::from(total)
            };
            let label = format!("{}  {step}/{total}", step_phase_text(phase));
            frame.render_widget(
                Gauge::default()
                    .ratio(ratio.clamp(0.0, 1.0))
                    .label(label)
                    .gauge_style(Style::default().fg(Color::Cyan))
                    .use_unicode(false)
                    .block(Block::bordered().title(crate::tr!(crate::keys::PRESENTATION_STATUS))),
                progress,
            );
        } else {
            let status = match result {
                Some(OperationResult::Success) => {
                    crate::tr!(crate::keys::PRESENTATION_INSTALLATION_FINISHED_SUCCESSFULLY)
                }
                Some(OperationResult::Failed) => {
                    crate::tr!(crate::keys::PRESENTATION_INSTALLATION_REPORTED_FAILURE)
                }
                Some(OperationResult::Cancelled) => {
                    crate::tr!(crate::keys::PRESENTATION_INSTALLATION_STOPPED_DURING_DOWNLOAD)
                }
                None => match phase {
                    OperationPhase::Preparing => {
                        crate::tr!(crate::keys::PRESENTATION_PREPARING_INSTALLATION)
                    }
                    OperationPhase::Downloading => {
                        crate::tr!(crate::keys::PRESENTATION_WAITING_FOR_DOWNLOAD_PROGRESS)
                    }
                    OperationPhase::Applying => {
                        crate::tr!(crate::keys::PRESENTATION_APPLYING_CONFIGURATION)
                    }
                    OperationPhase::Stopping => {
                        crate::tr!(crate::keys::PRESENTATION_STOPPING)
                    }
                    OperationPhase::Activating => {
                        crate::tr!(crate::keys::PRESENTATION_ACTIVATING)
                    }
                    OperationPhase::Verifying => {
                        crate::tr!(crate::keys::PRESENTATION_VERIFYING)
                    }
                },
            };
            frame.render_widget(
                Paragraph::new(status)
                    .style(if matches!(result, Some(OperationResult::Success)) {
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::Green)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    })
                    .block(Block::bordered().title(crate::tr!(crate::keys::PRESENTATION_STATUS))),
                progress,
            );
        }
        render_log_panel(frame, log_area, logs);
        let hint = if result.is_some() {
            if takeover_pending {
                crate::tr!(crate::keys::PRESENTATION_TAKEOVER_CONFIRM_HINT)
            } else {
                crate::tr!(crate::keys::PRESENTATION_CTRL_C_CLOSE)
            }
        } else if confirming_stop {
            crate::tr!(crate::keys::PRESENTATION_ENTER_STOP_ESC_CANCEL)
        } else if phase == OperationPhase::Downloading {
            crate::tr!(crate::keys::PRESENTATION_CTRL_C_STOP_ESC_OPTIONS)
        } else {
            crate::tr!(crate::keys::PRESENTATION_INSTALLATION_IN_PROGRESS_STOP_IGNORED)
        };
        render_footer(frame, footer, notice, &hint);
        if confirming_stop {
            render_stop_confirmation(frame);
        }
    }
}

/// 安装页日志面板：最近 8 行，网络接管确认与回滚提示行醒目标出。
fn render_log_panel(frame: &mut Frame<'_>, area: Rect, logs: &[String]) {
    let visible_logs = logs.iter().rev().take(8).rev().collect::<Vec<_>>();
    let log_lines: Vec<Line<'_>> = visible_logs
        .iter()
        .map(|line| {
            if is_confirmation_line(line) {
                Line::styled(
                    line.as_str(),
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Line::raw(line.as_str())
            }
        })
        .collect();
    frame.render_widget(
        Paragraph::new(log_lines)
            .block(Block::bordered().title(crate::tr!(crate::keys::PRESENTATION_OUTPUT)))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn render_footer(frame: &mut Frame<'_>, area: Rect, notice: &str, hint: &str) {
    let footer_text = if notice.is_empty() {
        hint.to_string()
    } else {
        format!("{notice}  {hint}")
    };
    frame.render_widget(
        Paragraph::new(footer_text)
            .alignment(ratatui::layout::Alignment::Left)
            .style(Style::default().fg(Color::DarkGray))
            .block(Block::default().borders(Borders::TOP)),
        area,
    );
}

fn render_stop_confirmation(frame: &mut Frame<'_>) {
    let width = 48.min(frame.area().width.saturating_sub(2));
    let height = 5.min(frame.area().height.saturating_sub(2));
    let area = Rect::new(
        frame.area().x + frame.area().width.saturating_sub(width) / 2,
        frame.area().y + frame.area().height.saturating_sub(height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(crate::tr!(crate::keys::PRESENTATION_STOP_DOWNLOAD_CONFIRM))
            .alignment(ratatui::layout::Alignment::Center)
            .block(Block::bordered().title(crate::tr!(crate::keys::PRESENTATION_CONFIRM_STOP))),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn render(
        phase: OperationPhase,
        step_progress: Option<(u8, u8)>,
        current: Option<&DownloadState>,
        logs: &[String],
        result: Option<OperationResult>,
    ) -> String {
        let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
        terminal
            .draw(|frame| {
                InstallScreen.render(
                    frame,
                    phase,
                    step_progress,
                    current,
                    logs,
                    "",
                    false,
                    result,
                    false,
                )
            })
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn renders_full_screen_operation_without_sidebar() {
        let state = DownloadState {
            id: 1,
            label: "Landscape webserver".into(),
            total: 8,
            position: 4,
            elapsed_millis: 1_000,
            status: crate::interaction::presentation::DownloadStatus::Downloading,
        };
        let content = render(OperationPhase::Downloading, None, Some(&state), &[], None);
        assert!(content.contains("Installing Landscape"));
        assert!(content.contains("Landscape webserver"));
        assert!(content.contains("Ctrl+C Stop"));
        assert!(!content.contains("Navigation"));
    }

    #[test]
    fn renders_completed_operation_result() {
        let content = render(
            OperationPhase::Applying,
            None,
            None,
            &[],
            Some(OperationResult::Success),
        );
        assert!(content.contains("Installation complete"));
        assert!(content.contains("The installation finished successfully."));
        assert!(content.contains("Ctrl+C Close"));
    }

    #[test]
    fn highlights_network_confirmation_lines_in_the_output_panel() {
        let rollback_line = "install: confirm the network takeover within 10 minutes or the installation will be rolled back automatically";
        let content = render(
            OperationPhase::Applying,
            None,
            None,
            &[
                "install: systemd unit landscape-router.service is registered".into(),
                "install: network takeover is awaiting confirmation".into(),
                "install: reconnect to 10.1.1.105 and run `lkit network confirm`".into(),
                rollback_line.into(),
            ],
            Some(OperationResult::Success),
        );
        assert!(content.contains("network takeover is awaiting confirmation"));
        assert!(content.contains("run `lkit network confirm`"));
        assert!(content.contains("rolled back automatically"));
    }

    #[test]
    fn result_footer_offers_takeover_confirmation_when_pending() {
        let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
        terminal
            .draw(|frame| {
                InstallScreen.render(
                    frame,
                    OperationPhase::Applying,
                    None,
                    None,
                    &[],
                    "",
                    false,
                    Some(OperationResult::Success),
                    true,
                )
            })
            .unwrap();
        let content: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(content.contains("Enter Confirm takeover"));
        assert!(content.contains("Ctrl+C Close"));
    }
}
