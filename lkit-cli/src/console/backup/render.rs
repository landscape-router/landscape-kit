use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Gauge, Paragraph, Wrap};
use unicode_width::UnicodeWidthChar;
use unicode_width::UnicodeWidthStr;

use super::super::ConsoleApp;
use super::super::network_wizard::Snapshot;
use super::super::render::panel_block;
use super::super::widgets::Focus;
use super::BackupListState;
use crate::backup::lkb::BackupProgress;
use crate::commands::backup::{architecture_key, scope_key};

pub(crate) fn render_backup(frame: &mut Frame<'_>, app: &mut ConsoleApp, area: Rect) {
    let focused = app.focus == Focus::Panel;
    if app.backup.details.is_some() {
        render_backup_details(frame, app, focused, area);
        return;
    }
    if matches!(app.snapshot, Snapshot::RootRequired) {
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled(
                    crate::tr!(crate::keys::CONSOLE_ROOT_PRIVILEGES_REQUIRED),
                    Style::default().fg(Color::Yellow),
                ),
                Line::raw(""),
                Line::styled(
                    crate::tr!(crate::keys::CONSOLE_BACKUP_REQUIRES_INSTALL),
                    Style::default().fg(Color::DarkGray),
                ),
            ])
            .block(panel_block(
                &crate::tr!(crate::keys::CONSOLE_BACKUP_MENU),
                focused,
            ))
            .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }
    if !matches!(app.snapshot, Snapshot::Installed { .. }) {
        let message = match &app.snapshot {
            Snapshot::NotInstalled => {
                crate::tr!(crate::keys::CONSOLE_LANDSCAPE_NOT_INSTALLED)
            }
            Snapshot::Unavailable(error) => error.clone(),
            _ => unreachable!(),
        };
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled(message, Style::default().fg(Color::Yellow)),
                Line::raw(""),
                Line::styled(
                    crate::tr!(crate::keys::CONSOLE_BACKUP_REQUIRES_INSTALL),
                    Style::default().fg(Color::DarkGray),
                ),
            ])
            .block(panel_block(
                &crate::tr!(crate::keys::CONSOLE_BACKUP_MENU),
                focused,
            ))
            .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }
    render_backup_list(frame, app, focused, area);
}

fn render_backup_list(frame: &mut Frame<'_>, app: &mut ConsoleApp, focused: bool, area: Rect) {
    let create_selected = app.focus == Focus::Panel && app.backup.selected == 0;
    let highlight = Style::default()
        .fg(Color::Black)
        .bg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let mut lines = vec![Line::styled(
        format!(
            "{}{}",
            if create_selected { "> " } else { "  " },
            crate::tr!(crate::keys::CONSOLE_BACKUP_CREATE)
        ),
        if create_selected {
            highlight
        } else {
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD)
        },
    )];
    match &app.backup.state {
        BackupListState::NotRun | BackupListState::Running(_) => {
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                crate::tr!(crate::keys::CONSOLE_BACKUP_LOADING),
                Style::default().fg(Color::DarkGray),
            ));
        }
        BackupListState::Failed(error) => {
            lines.push(Line::raw(""));
            lines.push(Line::styled(error.clone(), Style::default().fg(Color::Red)));
        }
        BackupListState::Complete(rows) => {
            if rows.is_empty() {
                lines.push(Line::raw(""));
                lines.push(Line::styled(
                    crate::tr!(crate::keys::CONSOLE_BACKUP_NONE_FOUND),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            for (index, entry) in rows.iter().enumerate() {
                let cursor = app.focus == Focus::Panel && app.backup.selected == index + 1;
                match &entry.metadata {
                    Some(metadata) => {
                        let available = usize::from(area.width.saturating_sub(2));
                        let marker = if cursor { "> " } else { "  " };
                        // 备注排第一,按剩余长度占位:一行内其他信息
                        // (ID/时间/版本)固定,备注最多占其余宽度并截断。
                        let fixed = format!(
                            "{}  {}  {}",
                            metadata.backup_id, metadata.created_at, metadata.landscape_version
                        );
                        let fixed_width =
                            UnicodeWidthStr::width(marker) + UnicodeWidthStr::width(fixed.as_str());
                        let remark_room = available.saturating_sub(fixed_width + 2);
                        let text = if remark_room == 0 {
                            fixed
                        } else {
                            let remark = truncate_width(&metadata.remark, remark_room);
                            format!("{remark}  {fixed}")
                        };
                        lines.push(Line::styled(
                            format!("{marker}{text}"),
                            if cursor { highlight } else { Style::default() },
                        ));
                    }
                    None => {
                        let name = entry
                            .path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .trim_end_matches(".lkb")
                            .to_string();
                        let truncated = truncate_width(
                            &format!(
                                "{}{}  {}",
                                if cursor { "> " } else { "  " },
                                name,
                                crate::tr!(crate::keys::CONSOLE_BACKUP_INVALID_BADGE)
                            ),
                            usize::from(area.width.saturating_sub(4)),
                        );
                        lines.push(Line::styled(
                            truncated,
                            if cursor {
                                highlight
                            } else {
                                Style::default().fg(Color::Red)
                            },
                        ));
                    }
                }
            }
        }
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel_block(
                &crate::tr!(crate::keys::CONSOLE_BACKUP_MENU),
                focused,
            ))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn render_backup_details(frame: &mut Frame<'_>, app: &ConsoleApp, focused: bool, area: Rect) {
    let Some(entry) = app.backup.details_entry() else {
        return;
    };
    let Some(metadata) = &entry.metadata else {
        return;
    };
    let contents = format!(
        "binary={} static={} static_archive={} init_config={} geo_cache={}",
        metadata.contents.binary,
        metadata.contents.static_,
        metadata.contents.static_archive,
        metadata.contents.init_config,
        metadata.contents.geo_cache,
    );
    let lines = vec![
        Line::styled(
            crate::tr!(crate::keys::CONSOLE_BACKUP_DETAILS_TITLE),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
        // 备注排第一:列表行超长被截断,完整内容在这里查看。
        Line::raw(format!(
            "{}  {}",
            crate::tr!(crate::keys::CONSOLE_BACKUP_REMARK_LABEL),
            metadata.remark
        )),
        Line::raw(format!(
            "{}  {}",
            crate::tr!(crate::keys::CONSOLE_BACKUP_ID_LABEL),
            metadata.backup_id
        )),
        Line::raw(format!(
            "{}  {}",
            crate::tr!(crate::keys::CONSOLE_BACKUP_CREATED_LABEL),
            metadata.created_at
        )),
        Line::raw(format!(
            "{}  {}",
            crate::tr!(crate::keys::CONSOLE_BACKUP_VERSION_LABEL),
            metadata.landscape_version
        )),
        Line::raw(format!(
            "{}  {}",
            crate::tr!(crate::keys::CONSOLE_BACKUP_LKIT_LABEL),
            metadata.lkit_version
        )),
        Line::raw(format!(
            "{}  {}",
            crate::tr!(crate::keys::CONSOLE_BACKUP_ARCH_LABEL),
            architecture_key(metadata.architecture)
        )),
        Line::raw(format!(
            "{}  {}",
            crate::tr!(crate::keys::CONSOLE_BACKUP_HOSTNAME_LABEL),
            metadata.hostname
        )),
        Line::raw(format!(
            "{}  {}",
            crate::tr!(crate::keys::CONSOLE_BACKUP_AUTO_LABEL),
            metadata.auto
        )),
        Line::raw(format!(
            "{}  {}",
            crate::tr!(crate::keys::CONSOLE_BACKUP_SCOPE_LABEL),
            scope_key(metadata.scope)
        )),
        Line::raw(format!(
            "{}  {}",
            crate::tr!(crate::keys::CONSOLE_BACKUP_CONTENTS_LABEL),
            contents
        )),
        Line::raw(""),
        Line::styled(
            crate::tr!(
                crate::keys::CONSOLE_BACKUP_DETAILS_RESTORE_HINT,
                id = metadata.backup_id
            ),
            Style::default().fg(Color::DarkGray),
        ),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel_block(
                &crate::tr!(crate::keys::CONSOLE_BACKUP_DETAILS_TITLE),
                focused,
            ))
            .wrap(Wrap { trim: true })
            .scroll((app.backup.details_scroll, 0)),
        area,
    );
}

pub(crate) fn render_backup_create_dialog(frame: &mut Frame<'_>, app: &mut ConsoleApp) {
    let screen = frame.area();
    let width = 68.min(screen.width.saturating_sub(2));
    let height = 11.min(screen.height.saturating_sub(2));
    let area = Rect::new(
        screen.x + screen.width.saturating_sub(width) / 2,
        screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    );
    let remark = app.backup.remark.clone();
    let remark_display = if remark.is_empty() {
        "_".to_string()
    } else {
        format!("{remark}_")
    };
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(vec![
            Line::raw(crate::tr!(crate::keys::CONSOLE_BACKUP_CREATE_SCOPE)),
            Line::raw(""),
            Line::from(vec![
                Span::styled(
                    format!("{}: ", crate::tr!(crate::keys::CONSOLE_BACKUP_REMARK_LABEL)),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    remark_display,
                    Style::default().add_modifier(Modifier::UNDERLINED),
                ),
            ]),
            Line::raw(""),
            Line::raw(crate::tr!(crate::keys::CONSOLE_BACKUP_CREATE_HINT)),
        ])
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true })
        .block(Block::bordered().title(crate::tr!(crate::keys::CONSOLE_BACKUP_CREATE_TITLE))),
        area,
    );
}

/// 创建备份进行中的居中弹窗：阶段文案 + 文件数 Gauge。
pub(crate) fn render_backup_create_progress(frame: &mut Frame<'_>, app: &mut ConsoleApp) {
    let Some(run) = &app.backup.create else {
        return;
    };
    let screen = frame.area();
    let width = 76.min(screen.width.saturating_sub(2));
    let height = 7.min(screen.height.saturating_sub(2));
    let area = Rect::new(
        screen.x + screen.width.saturating_sub(width) / 2,
        screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    );
    let (stage_text, ratio) = match &run.progress {
        BackupProgress::Exporting => (
            crate::tr!(crate::keys::CONSOLE_BACKUP_CREATE_PROGRESS_EXPORT),
            0.0,
        ),
        BackupProgress::Archiving {
            done,
            total,
            current,
        } => {
            let ratio = if *total == 0 {
                0.0
            } else {
                *done as f64 / *total as f64
            };
            (
                crate::tr!(
                    crate::keys::CONSOLE_BACKUP_CREATE_PROGRESS_ARCHIVE,
                    done = *done,
                    total = *total,
                    current = current
                ),
                ratio,
            )
        }
        BackupProgress::Finalizing => (
            crate::tr!(crate::keys::CONSOLE_BACKUP_CREATE_PROGRESS_FINALIZE),
            1.0,
        ),
    };
    let percent = (ratio * 100.0).round() as u64;
    let inner = area.inner(Margin {
        vertical: 1,
        horizontal: 1,
    });
    let [stage_area, gauge_area, hint_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::bordered().title(crate::tr!(crate::keys::CONSOLE_BACKUP_CREATE_RUNNING)),
        area,
    );
    frame.render_widget(
        Paragraph::new(stage_text).wrap(Wrap { trim: true }),
        stage_area,
    );
    frame.render_widget(
        Gauge::default()
            .ratio(ratio.clamp(0.0, 1.0))
            .label(format!("{percent:>3}%"))
            .gauge_style(Style::default().fg(Color::Cyan))
            .use_unicode(false),
        gauge_area,
    );
    frame.render_widget(
        Paragraph::new(crate::tr!(crate::keys::CONSOLE_BACKUP_HINT_CREATE_RUNNING))
            .style(Style::default().fg(Color::DarkGray)),
        hint_area,
    );
}

/// 备份损坏提示弹框:校验失败时 R 键/恢复 Enter 触发,Enter/Esc 关闭。
pub(crate) fn render_backup_corrupt_dialog(frame: &mut Frame<'_>) {
    let screen = frame.area();
    let width = 64.min(screen.width.saturating_sub(2));
    let height = 9.min(screen.height.saturating_sub(2));
    let area = Rect::new(
        screen.x + screen.width.saturating_sub(width) / 2,
        screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_BACKUP_CORRUPT_TITLE),
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Line::raw(""),
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_BACKUP_CORRUPT_QUESTION),
                Style::default().fg(Color::DarkGray),
            ),
            Line::raw(""),
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_PRESS_ESC_TO_CANCEL),
                Style::default().fg(Color::DarkGray),
            ),
        ])
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true })
        .block(Block::bordered().title(crate::tr!(crate::keys::CONSOLE_BACKUP_CORRUPT_DIALOG))),
        area,
    );
}

pub(crate) fn render_backup_restore_confirmation(frame: &mut Frame<'_>, app: &mut ConsoleApp) {
    let Some(metadata) = app
        .backup
        .selected_entry()
        .and_then(|entry| entry.metadata.as_ref())
    else {
        return;
    };
    let screen = frame.area();
    let width = 76.min(screen.width.saturating_sub(2));
    let height = 13.min(screen.height.saturating_sub(2));
    let area = Rect::new(
        screen.x + screen.width.saturating_sub(width) / 2,
        screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_BACKUP_RESTORE_QUESTION),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Line::raw(""),
            Line::raw(crate::tr!(
                crate::keys::CONSOLE_BACKUP_RESTORE_PLAN,
                id = metadata.backup_id,
                version = metadata.landscape_version
            )),
            Line::raw(crate::tr!(
                crate::keys::CONSOLE_BACKUP_RESTORE_MINIMAL_SCOPE
            )),
            Line::raw(""),
            Line::raw(crate::tr!(crate::keys::CONSOLE_BACKUP_RESTORE_PRESS_ENTER)),
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_PRESS_ESC_TO_CANCEL),
                Style::default().fg(Color::DarkGray),
            ),
        ])
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true })
        .block(Block::bordered().title(crate::tr!(crate::keys::CONSOLE_BACKUP_RESTORE_TITLE))),
        area,
    );
}

pub(crate) fn render_backup_delete_confirmation(frame: &mut Frame<'_>, app: &mut ConsoleApp) {
    let Some(metadata) = app.backup.delete_target.as_deref().and_then(|id| {
        app.backup
            .rows()
            .iter()
            .find_map(|entry| entry.metadata.as_ref().filter(|m| m.backup_id == id))
    }) else {
        return;
    };
    let screen = frame.area();
    let width = 76.min(screen.width.saturating_sub(2));
    let height = 11.min(screen.height.saturating_sub(2));
    let area = Rect::new(
        screen.x + screen.width.saturating_sub(width) / 2,
        screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_BACKUP_DELETE_QUESTION),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Line::raw(""),
            Line::raw(crate::tr!(
                crate::keys::CONSOLE_BACKUP_DELETE_PLAN,
                id = metadata.backup_id,
                version = metadata.landscape_version
            )),
            Line::raw(""),
            Line::raw(crate::tr!(crate::keys::CONSOLE_BACKUP_DELETE_PRESS_ENTER)),
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_PRESS_ESC_TO_CANCEL),
                Style::default().fg(Color::DarkGray),
            ),
        ])
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true })
        .block(Block::bordered().title(crate::tr!(crate::keys::CONSOLE_BACKUP_DELETE_TITLE))),
        area,
    );
}

/// 按显示宽度截断文本:列表行超长时截断为省略号,不换行。
fn truncate_width(text: &str, max_width: usize) -> String {
    use unicode_width::UnicodeWidthStr;
    let width = UnicodeWidthStr::width(text);
    if width <= max_width {
        return text.to_string();
    }
    if max_width <= 1 {
        return "…".to_string();
    }
    let mut result = String::new();
    let mut used = 0usize;
    for character in text.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if used + character_width > max_width.saturating_sub(1) {
            break;
        }
        result.push(character);
        used += character_width;
    }
    format!("{result}…")
}
