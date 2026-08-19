use super::{DownloadState, OperationPhase, OperationResult, OperationScreen};
use super::{is_confirmation_line, step_phase_text};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Gauge, Paragraph, Wrap};

/// `lkit migrate` 委托切换阶段的全屏页:布局与 switch 一致,文案与结果标题
/// 使用迁移专用词(命令行结束提示也复用这些 key,前缀为 `migrate`)。
pub(crate) struct MigrateScreen;

impl OperationScreen for MigrateScreen {
    fn result_key(&self, result: OperationResult) -> &'static str {
        match result {
            OperationResult::Success => crate::keys::PRESENTATION_MIGRATE_COMPLETE,
            OperationResult::Failed => crate::keys::PRESENTATION_MIGRATE_FAILED,
            OperationResult::Cancelled => crate::keys::PRESENTATION_MIGRATE_CANCELLED,
        }
    }

    fn stop_ignored_key(&self) -> &'static str {
        crate::keys::PRESENTATION_OPERATION_IS_APPLYING
    }

    fn announce_prefix(&self) -> &'static str {
        "migrate"
    }

    fn cancellable_during_switch(&self) -> bool {
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
        _takeover_pending: bool,
    ) {
        let [header, body, footer] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(3),
        ])
        .areas(frame.area());
        let header_text = match result {
            Some(OperationResult::Success) => {
                crate::tr!(crate::keys::PRESENTATION_MIGRATE_COMPLETE)
            }
            Some(OperationResult::Failed) => crate::tr!(crate::keys::PRESENTATION_MIGRATE_FAILED),
            Some(OperationResult::Cancelled) => {
                crate::tr!(crate::keys::PRESENTATION_MIGRATE_CANCELLED)
            }
            None => crate::tr!(crate::keys::PRESENTATION_OPERATION_MIGRATE),
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
                    crate::tr!(crate::keys::PRESENTATION_OPERATION_FINISHED_SUCCESSFULLY)
                }
                Some(OperationResult::Failed) => {
                    crate::tr!(crate::keys::PRESENTATION_OPERATION_REPORTED_FAILURE)
                }
                Some(OperationResult::Cancelled) => {
                    crate::tr!(crate::keys::PRESENTATION_OPERATION_STOPPED_DURING_DOWNLOAD)
                }
                None => step_phase_text(phase),
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
            log_area,
        );
        let hint = if result.is_some() {
            crate::tr!(crate::keys::PRESENTATION_CTRL_C_CLOSE)
        } else if confirming_stop {
            crate::tr!(crate::keys::PRESENTATION_ENTER_STOP_ESC_CANCEL)
        } else if phase == OperationPhase::Downloading {
            crate::tr!(crate::keys::PRESENTATION_CTRL_C_STOP_ESC_OPTIONS)
        } else {
            crate::tr!(crate::keys::PRESENTATION_OPERATION_IN_PROGRESS_STOP_IGNORED)
        };
        let footer_text = if notice.is_empty() {
            hint
        } else {
            format!("{notice}  {hint}")
        };
        frame.render_widget(
            Paragraph::new(footer_text)
                .alignment(Alignment::Left)
                .style(Style::default().fg(Color::DarkGray))
                .block(Block::default().borders(Borders::TOP)),
            footer,
        );
        if confirming_stop {
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
                    .alignment(Alignment::Center)
                    .block(
                        Block::bordered().title(crate::tr!(crate::keys::PRESENTATION_CONFIRM_STOP)),
                    ),
                area,
            );
        }
    }
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
                MigrateScreen.render(
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
    fn renders_migrate_operation_screen() {
        let state = DownloadState {
            id: 1,
            label: "Landscape webserver".into(),
            total: 8,
            position: 4,
            elapsed_millis: 1_000,
            status: crate::interaction::presentation::DownloadStatus::Downloading,
        };
        let content = render(OperationPhase::Downloading, None, Some(&state), &[], None);
        assert!(content.contains("Migrating Landscape"));
        assert!(content.contains("Landscape webserver"));
        assert!(content.contains("Ctrl+C Stop"));
    }

    #[test]
    fn renders_migrate_completed_result() {
        let content = render(
            OperationPhase::Applying,
            None,
            None,
            &[],
            Some(OperationResult::Success),
        );
        assert!(content.contains("Migration complete"));
        assert!(content.contains("The operation finished successfully."));
        assert!(content.contains("Ctrl+C Close"));
    }
}
