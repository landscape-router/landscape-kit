use std::collections::VecDeque;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::form::{Field, FormState};
use super::{ClientForwardStatus, ConnectionState, DashFocus, DashState};
use crate::client::LogLevel;

/// TUI 可用的最小终端尺寸:表单骨架(标题/提示行 + 七个字段 + 连接按钮 +
/// 错误行)需要 24 行,连接状态行与映射行在 60 列内完整显示。
pub(super) const MIN_TERMINAL_WIDTH: u16 = 60;
pub(super) const MIN_TERMINAL_HEIGHT: u16 = 24;

pub(super) fn terminal_too_small(area: Rect) -> bool {
    area.width < MIN_TERMINAL_WIDTH || area.height < MIN_TERMINAL_HEIGHT
}

pub(super) fn focused_style() -> Style {
    Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD)
}

pub(super) fn selection_offset(total: usize, visible: usize, selection: usize) -> usize {
    if total <= visible || visible == 0 {
        return 0;
    }
    selection.saturating_sub(visible - 1).min(total - visible)
}

pub(super) fn render_too_small(f: &mut Frame) {
    f.render_widget(
        Paragraph::new(crate::tr!(
            "tui.terminal_too_small",
            width = MIN_TERMINAL_WIDTH,
            height = MIN_TERMINAL_HEIGHT
        ))
        .alignment(Alignment::Center)
        .block(Block::bordered().title("lflare")),
        f.area(),
    );
}

/// 退出确认层:先把整屏底层压暗(前景退为暗灰、清掉高亮背景,弹窗外残词
/// 不再与弹窗争夺注意力,与 console 的 begin_dialog 同款),再清空并绘制
/// 居中的确认弹窗。
pub(super) fn render_exit_confirmation(f: &mut Frame) {
    for cell in f.buffer_mut().content.iter_mut() {
        cell.fg = Color::DarkGray;
        cell.bg = Color::Reset;
    }
    let screen = f.area();
    let width = 52.min(screen.width.saturating_sub(2));
    let height = 5.min(screen.height.saturating_sub(2));
    let area = Rect::new(
        screen.x + screen.width.saturating_sub(width) / 2,
        screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    );
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(vec![
            Line::raw(""),
            Line::styled(
                crate::tr!("tui.exit_confirm_message"),
                Style::default().fg(Color::White),
            ),
            Line::styled(
                crate::tr!("tui.exit_confirm_hint"),
                Style::default().fg(Color::DarkGray),
            ),
        ])
        .alignment(Alignment::Center)
        .block(Block::bordered().title(crate::tr!("tui.exit_confirm_title"))),
        area,
    );
}

pub(super) fn text_field(frame: &mut Frame, area: Rect, label: &str, value: &str, focused: bool) {
    let border = if focused {
        Color::Cyan
    } else {
        Color::DarkGray
    };
    let block = Block::bordered()
        .title(Line::styled(label, Style::default().fg(border)))
        .border_style(Style::default().fg(border));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(value).style(if focused {
            focused_style()
        } else {
            Style::default()
        }),
        inner,
    );
}

pub(super) fn render_form(f: &mut Frame, form: &FormState) {
    let dev_opts = form.device_options();
    let device_picker_open = form.focus == Field::Device && form.device_selecting;
    let err_h = usize::from(form.error.is_some() || form.devices_err.is_some());
    // 除设备区外的固定骨架:标题 + 提示行 + 五个 3 行字段 + 连接按钮 + 错误行。
    // 展开的设备列表从剩余预算里取行数,保证溢出提示行不会被布局器挤掉。
    let fixed_h = 2 + 3 * 5 + 3 + err_h;
    let device_budget = (f.area().height as usize).saturating_sub(fixed_h);
    let (dev_visible, device_h) = if device_picker_open {
        let content_rows = device_budget.saturating_sub(2);
        let mut visible = dev_opts.len().min(6);
        let mut overflow = usize::from(dev_opts.len() > visible);
        if visible + overflow > content_rows {
            visible = content_rows.saturating_sub(1).min(visible);
            overflow = usize::from(dev_opts.len() > visible);
        }
        let visible = visible.max(1);
        (visible, 2 + visible + overflow)
    } else {
        (1, 3)
    };
    let dev_offset = if device_picker_open {
        selection_offset(dev_opts.len(), dev_visible, form.device_index)
    } else {
        0
    };
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(device_h as u16),
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(3),
        Constraint::Length(err_h as u16),
    ])
    .areas(f.area());
    let [title, hint, psk, user, cn, dev, eth, tok, connect, err] = chunks;

    f.render_widget(
        Paragraph::new(Line::styled(
            crate::tr!("tui.title"),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .alignment(ratatui::layout::Alignment::Center),
        title,
    );
    f.render_widget(
        Paragraph::new(Line::styled(
            crate::tr!("tui.form_hint"),
            Style::default().fg(Color::DarkGray),
        )),
        hint,
    );

    let psk_value = if form.show_psk {
        form.psk.clone()
    } else {
        "*".repeat(form.psk.chars().count())
    };
    text_field(
        f,
        psk,
        &crate::tr!("tui.psk"),
        &psk_value,
        form.focus == Field::Psk,
    );
    text_field(
        f,
        user,
        &crate::tr!("tui.user"),
        &form.user,
        form.focus == Field::User,
    );
    text_field(
        f,
        cn,
        &crate::tr!("tui.client_name"),
        &form.client_name,
        form.focus == Field::ClientName,
    );

    // Device field: the list opens explicitly so Up/Down can still move
    // between form fields after a device has been selected.
    if device_picker_open {
        let border = Color::Cyan;
        let block = Block::bordered()
            .title(Line::styled(
                crate::tr!("tui.device_picker"),
                Style::default().fg(border),
            ))
            .border_style(Style::default().fg(border));
        let inner = block.inner(dev);
        f.render_widget(block, dev);
        let mut lines = Vec::with_capacity(dev_visible);
        for (i, name) in dev_opts
            .iter()
            .enumerate()
            .skip(dev_offset)
            .take(dev_visible)
        {
            let marker = if i == form.device_index { "▸ " } else { "  " };
            let style = if i == form.device_index {
                focused_style()
            } else {
                Style::default()
            };
            lines.push(Line::styled(format!("{marker}{name}"), style));
        }
        if dev_opts.len() > dev_visible {
            lines.push(Line::styled(
                crate::tr!("tui.device_overflow", count = dev_opts.len()),
                Style::default().fg(Color::DarkGray),
            ));
        }
        f.render_widget(Paragraph::new(lines), inner);
    } else {
        text_field(
            f,
            dev,
            &crate::tr!("tui.device"),
            &form.device_label(),
            form.focus == Field::Device,
        );
    }

    text_field(
        f,
        eth,
        &crate::tr!("tui.ethertype"),
        &form.ethertype,
        form.focus == Field::Ethertype,
    );
    text_field(
        f,
        tok,
        &crate::tr!("tui.token"),
        &form.token,
        form.focus == Field::Token,
    );

    let button_style = if form.focus == Field::Connect {
        focused_style().fg(Color::Black).bg(Color::Cyan)
    } else {
        Style::default().fg(Color::Cyan)
    };
    f.render_widget(
        Paragraph::new(Line::styled(
            format!("  {}  ", crate::tr!("tui.connect")),
            button_style,
        ))
        .alignment(ratatui::layout::Alignment::Center)
        .block(
            Block::bordered()
                .title(crate::tr!("tui.connect"))
                .border_style(if form.focus == Field::Connect {
                    Style::default().fg(Color::Cyan)
                } else {
                    Style::default().fg(Color::DarkGray)
                }),
        ),
        connect,
    );

    if let Some(e) = &form.error {
        f.render_widget(
            Paragraph::new(Line::styled(e, Style::default().fg(Color::Red))),
            err,
        );
    } else if let Some(e) = &form.devices_err {
        f.render_widget(
            Paragraph::new(Line::styled(e, Style::default().fg(Color::Yellow))),
            err,
        );
    }
}

fn log_marker(level: LogLevel) -> (&'static str, Color) {
    match level {
        LogLevel::Info => ("", Color::White),
        LogLevel::Warn => ("⚠ ", Color::Yellow),
        LogLevel::Error => ("✗ ", Color::Red),
    }
}

/// 按词界把一条日志折成若干物理行:首行占满可用宽度,续行缩进两格以区分
/// 新条目(与 console 的包裹缩进一致)。宽度按显示宽度(unicode-width)计,
/// 与 ratatui 网格一致;超过行宽的单词在字符边界硬切。
fn wrap_log_line(text: &str, width: usize) -> Vec<String> {
    fn flush(lines: &mut Vec<String>, line: &mut String, used: &mut usize) {
        if !line.is_empty() {
            lines.push(std::mem::take(line));
        }
        *used = 0;
        if !lines.is_empty() {
            line.push_str("  ");
            *used = 2;
        }
    }

    if width == 0 {
        return vec![text.to_string()];
    }
    let cont_width = width.saturating_sub(2).max(1);
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    let mut used = 0usize;
    for word in text.split(' ') {
        let word_w = UnicodeWidthStr::width(word);
        let sep = usize::from(!line.is_empty());
        let limit = if lines.is_empty() { width } else { cont_width };
        if used + sep + word_w <= limit {
            if sep == 1 {
                line.push(' ');
                used += 1;
            }
            line.push_str(word);
            used += word_w;
            continue;
        }
        // 整词放不下:收行后在缩进续行上按字符继续,超长词跨多行硬切。
        if !line.is_empty() {
            flush(&mut lines, &mut line, &mut used);
        }
        let mut limit = if lines.is_empty() { width } else { cont_width };
        for c in word.chars() {
            let char_w = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
            if used + char_w > limit {
                flush(&mut lines, &mut line, &mut used);
                limit = cont_width;
            }
            line.push(c);
            used += char_w;
        }
    }
    lines.push(line);
    lines
}

fn wrapped_row_count(entry: &(LogLevel, String), width: usize) -> usize {
    let (marker, _) = log_marker(entry.0);
    wrap_log_line(&format!("{}{}", marker, entry.1), width).len()
}

/// 自底向上按物理行预算选日志窗口:跳过最新的 skip 条,向旧的方向装满
/// budget 个物理行。返回窗口(旧 → 新)与是否覆盖到了最旧一条。
fn log_window(
    logs: &VecDeque<(LogLevel, String)>,
    skip: usize,
    budget: usize,
    width: usize,
) -> (Vec<&(LogLevel, String)>, bool) {
    let mut window = Vec::new();
    let mut used = 0usize;
    let mut covered_all = true;
    for entry in logs.iter().rev().skip(skip) {
        let rows = wrapped_row_count(entry, width);
        if used + rows > budget {
            covered_all = false;
            break;
        }
        used += rows;
        window.push(entry);
    }
    window.reverse();
    (window, covered_all)
}

/// Home 的目标位置:自最旧端装满 budget 个物理行得到 n 条,skip = total - n,
/// 窗口恰好贴顶且不留空行。
fn top_skip(logs: &VecDeque<(LogLevel, String)>, budget: usize, width: usize) -> usize {
    let mut used = 0usize;
    let mut n = 0usize;
    for entry in logs.iter() {
        let rows = wrapped_row_count(entry, width);
        if used + rows > budget {
            break;
        }
        used += rows;
        n += 1;
    }
    logs.len() - n
}

pub(super) fn status_line(state: &ConnectionState) -> String {
    match state {
        ConnectionState::Searching => crate::tr!("tui.searching"),
        ConnectionState::Authenticating => crate::tr!("tui.authenticating"),
        ConnectionState::Ready {
            session_id,
            server_mac,
        } => crate::tr!("tui.connected", session = session_id, mac = server_mac),
        ConnectionState::AuthRejected(reason) => crate::tr!("tui.auth_rejected", reason = reason),
        ConnectionState::LinkLost => crate::tr!("tui.link_lost"),
        ConnectionState::PeerClosed => crate::tr!("tui.peer_closed"),
    }
}

pub(super) fn render_dash(f: &mut Frame, dash: &DashState) {
    let fwd_visible = dash.forwards.len().clamp(1, 5);
    let fwd_offset = if dash.focus == DashFocus::Forwards {
        selection_offset(dash.forwards.len(), fwd_visible, dash.forward_index)
    } else {
        0
    };
    let fwd_h = 2
        + fwd_visible
        + usize::from(dash.forwards.len() > fwd_visible)
        + usize::from(dash.forward_edit) * 2
        + usize::from(dash.forward_error.is_some());
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Length(fwd_h as u16),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .areas(f.area());
    let [title, status, fwd, log, hint] = chunks;

    f.render_widget(
        Paragraph::new(Line::styled(
            crate::tr!("tui.session_title"),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .alignment(ratatui::layout::Alignment::Center),
        title,
    );

    let status_lines = vec![
        Line::styled(
            crate::tr!("tui.status", status = status_line(&dash.connection)),
            Style::default().fg(Color::Cyan),
        ),
        Line::from(crate::tr!(
            "tui.device_count",
            device = dash
                .device
                .as_deref()
                .map(str::to_owned)
                .unwrap_or_else(|| crate::tr!("tui.auto_device")),
            count = dash.forwards.len()
        )),
    ];
    f.render_widget(
        Paragraph::new(status_lines).block(Block::bordered().title(crate::tr!("tui.connection"))),
        status,
    );

    let mut fwd_lines: Vec<Line> = Vec::new();
    for (index, (lp, dp)) in dash
        .forwards
        .iter()
        .enumerate()
        .skip(fwd_offset)
        .take(fwd_visible)
    {
        let state = dash
            .forward_states
            .get(&(*lp, *dp))
            .copied()
            .unwrap_or(ClientForwardStatus::Starting);
        let (marker, color) = match state {
            ClientForwardStatus::Listening => ("✓", Color::Green),
            ClientForwardStatus::Starting => ("…", Color::Yellow),
            ClientForwardStatus::Failed => ("✗", Color::Red),
        };
        let selected = dash.focus == DashFocus::Forwards && index == dash.forward_index;
        let prefix = if selected { "▸ " } else { "  " };
        let line_style = if selected {
            focused_style()
        } else {
            Style::default()
        };
        fwd_lines.push(Line::from(vec![
            Span::styled(prefix, line_style),
            Span::styled(marker, Style::default().fg(color)),
            Span::styled(
                crate::tr!("tui.mapping_line", local = lp, remote = dp),
                line_style,
            ),
        ]));
    }
    if fwd_lines.is_empty() {
        fwd_lines.push(Line::styled(
            if dash.session_ready {
                crate::tr!("tui.no_mappings")
            } else {
                crate::tr!("tui.waiting_handshake")
            },
            Style::default().fg(if dash.session_ready {
                Color::DarkGray
            } else {
                Color::Yellow
            }),
        ));
    } else if dash.forwards.len() > fwd_visible {
        let focus = if dash.focus == DashFocus::Forwards {
            crate::tr!("tui.mapping_focus")
        } else {
            crate::tr!("tui.mapping_focus_hint")
        };
        fwd_lines.push(Line::styled(
            crate::tr!(
                "tui.mapping_overflow",
                count = dash.forwards.len(),
                focus = focus
            ),
            Style::default().fg(Color::DarkGray),
        ));
    }
    if dash.forward_edit {
        fwd_lines.push(Line::from(vec![
            Span::styled("> ", focused_style()),
            Span::styled(
                crate::tr!("tui.add_input", value = dash.forward_input),
                focused_style(),
            ),
        ]));
        fwd_lines.push(Line::styled(
            crate::tr!("tui.add_hint"),
            Style::default().fg(Color::DarkGray),
        ));
    }
    if let Some(error) = &dash.forward_error {
        fwd_lines.push(Line::styled(error, Style::default().fg(Color::Red)));
    }
    let forward_title = if dash.session_ready {
        crate::tr!("tui.mappings")
    } else {
        crate::tr!("tui.mappings_waiting")
    };
    f.render_widget(
        Paragraph::new(fwd_lines).block(Block::bordered().title(forward_title).border_style(
            if dash.focus == DashFocus::Forwards {
                if dash.session_ready {
                    Style::default().fg(Color::Cyan)
                } else {
                    Style::default().fg(Color::Yellow)
                }
            } else {
                Style::default().fg(Color::DarkGray)
            },
        )),
        fwd,
    );

    // 日志窗口按物理行(而非逻辑条目)自底向上装填:先按显示宽度词界折行,
    // 窗口数学与实际渲染的网格行严格一致。此前把换行交给 Paragraph::wrap
    // 处理,任何一条日志换行都会让物理行超出预算,最新日志与提示行被
    // Paragraph 从底部裁掉。
    let inner_width = (log.width as usize).saturating_sub(2).max(1);
    let inner_rows = (log.height as usize).saturating_sub(2).max(1);
    let focus_logs = dash.focus == DashFocus::Logs;
    // scroll 是自底部隐藏的逻辑条数,夹到「窗口恰好贴顶」的位置。
    let mut budget = inner_rows;
    let mut skip = dash.scroll.min(top_skip(&dash.logs, budget, inner_width));
    let (_, covered_all) = log_window(&dash.logs, skip, budget, inner_width);
    let show_bottom = focus_logs && skip == 0 && !covered_all;
    let show_top = focus_logs && skip > 0 && covered_all;
    if show_bottom || show_top {
        // 提示行占用一行:从预留后的预算重算窗口,避免追加后被裁掉。
        budget -= 1;
        skip = dash.scroll.min(top_skip(&dash.logs, budget, inner_width));
    }
    let (window, _) = log_window(&dash.logs, skip, budget, inner_width);
    let mut shown: Vec<Line> = Vec::new();
    if show_top {
        shown.push(Line::styled(
            crate::tr!("tui.log_top"),
            Style::default().fg(Color::DarkGray),
        ));
    }
    for (level, message) in window {
        let (marker, color) = log_marker(*level);
        for piece in wrap_log_line(&format!("{marker}{message}"), inner_width) {
            shown.push(Line::styled(piece, Style::default().fg(color)));
        }
    }
    if show_bottom {
        shown.push(Line::styled(
            crate::tr!("tui.log_bottom"),
            Style::default().fg(Color::DarkGray),
        ));
    }
    f.render_widget(
        Paragraph::new(shown).block(Block::bordered().title(crate::tr!("tui.log")).border_style(
            if dash.focus == DashFocus::Logs {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::DarkGray)
            },
        )),
        log,
    );

    let footer = match dash.focus {
        DashFocus::Forwards if dash.session_ready => crate::tr!("tui.footer_mapping"),
        DashFocus::Forwards => crate::tr!("tui.footer_mapping_waiting"),
        DashFocus::Logs => crate::tr!("tui.footer_logs"),
    };
    f.render_widget(
        Paragraph::new(Line::styled(
            format!("{footer} · {}", crate::tr!("tui.status_language")),
            Style::default().fg(Color::DarkGray),
        )),
        hint,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_short_line_stays_single() {
        assert_eq!(wrap_log_line("hello world", 58), vec!["hello world"]);
        assert_eq!(wrap_log_line("", 58), vec![""]);
    }

    #[test]
    fn wrap_indents_continuation() {
        assert_eq!(wrap_log_line("aaa bbb ccc", 7), vec!["aaa bbb", "  ccc"]);
        // 续行可用宽 = width - 2(缩进占 2):后续词继续按词界折。
        assert_eq!(
            wrap_log_line("aaaa bb cc dd", 8),
            vec!["aaaa bb", "  cc", "  dd"]
        );
    }

    #[test]
    fn wrap_hard_splits_overlong_words() {
        // 单词长于任何一行:首行占满 width,后续在缩进续行上按字符硬切。
        assert_eq!(
            wrap_log_line("abcdefghijklmnop", 8),
            vec!["abcdefgh", "  ijkl", "  mnop"]
        );
    }

    #[test]
    fn wrap_counts_wide_chars_by_display_width() {
        // 中日韩宽字符按显示宽度 2 计,与 ratatui 网格一致(缩进后的续行
        // 可用宽 width-2 = 6,恰好放下三个宽字符里的两个)。
        assert_eq!(
            wrap_log_line("中文 日志 条目", 8),
            vec!["中文", "  日志", "  条目"]
        );
    }

    fn make_logs(items: &[(LogLevel, &str)]) -> VecDeque<(LogLevel, String)> {
        items
            .iter()
            .map(|(level, message)| (*level, (*message).into()))
            .collect()
    }

    #[test]
    fn log_window_budgets_physical_rows() {
        let logs = make_logs(&[
            (LogLevel::Info, "line 0"),
            (LogLevel::Info, "line 1"),
            (LogLevel::Info, "line 2"),
            (LogLevel::Info, "line 3"),
            (LogLevel::Info, "line 4"),
        ]);
        // 全部单行,预算 3 → 最新 3 条,未覆盖到最旧。
        let (window, covered) = log_window(&logs, 0, 3, 20);
        assert_eq!(window.len(), 3);
        assert!(!covered);
        assert_eq!(window[0].1, "line 2");
        assert_eq!(window[2].1, "line 4");

        // 一条占两行的日志消耗两份预算;预算装不下整条时按条丢弃(最旧的
        // "short" 整条留在窗外)。
        let logs = make_logs(&[
            (LogLevel::Info, "short"),
            (LogLevel::Info, "aaaa bbbb cccc"),
            (LogLevel::Info, "tail"),
        ]);
        let (window, covered) = log_window(&logs, 0, 3, 10);
        assert_eq!(window.len(), 2);
        assert!(!covered);
        assert_eq!(window[0].1, "aaaa bbbb cccc");
        assert_eq!(window[1].1, "tail");
    }

    #[test]
    fn top_skip_fills_from_oldest() {
        let logs = make_logs(&[
            (LogLevel::Info, "line 0"),
            (LogLevel::Info, "line 1"),
            (LogLevel::Info, "line 2"),
            (LogLevel::Info, "line 3"),
            (LogLevel::Info, "line 4"),
        ]);
        assert_eq!(top_skip(&logs, 3, 20), 2);
        assert_eq!(top_skip(&logs, 9, 20), 0);

        // 换行条目按物理行计入:预算 3 只容 1 条单行 + 1 条两行(共 2 条),
        // 最新一条整条留在窗外。
        let logs = make_logs(&[
            (LogLevel::Info, "short"),
            (LogLevel::Info, "aaaa bbbb cccc"),
            (LogLevel::Info, "tail"),
        ]);
        assert_eq!(top_skip(&logs, 3, 10), 1);
    }
}
