use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use super::super::ConsoleApp;
use super::super::render::{display_pad, register_dialog_hits};
use super::super::widgets::{Clicks, Hit, block_row_of};
use super::{NetworkWizard, Snapshot, WanMode, WizardStep};

pub(crate) fn render_network_wizard(
    frame: &mut Frame<'_>,
    wizard: &NetworkWizard,
    hits: &mut Clicks,
) {
    let area = frame.area();
    let [title, body, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(8),
        Constraint::Length(2),
    ])
    .areas(area);
    frame.render_widget(
        Paragraph::new(crate::tr!(crate::keys::CONSOLE_LANDSCAPE_NETWORK_TAKEOVER))
            .style(Style::default().add_modifier(Modifier::BOLD))
            .block(Block::default().borders(Borders::BOTTOM)),
        title,
    );
    hits.add(body, Hit::WizardContinue);
    let mut lines = Vec::new();
    let mut clickables: Vec<(usize, Hit)> = Vec::new();
    macro_rules! push {
        ($line:expr) => {
            lines.push($line)
        };
    }
    match wizard.step {
        WizardStep::Wan => {
            push!(Line::styled(
                crate::tr!(crate::keys::CONSOLE_SELECT_WAN_INTERFACE),
                Style::default().add_modifier(Modifier::BOLD),
            ));
            push!(Line::raw(""));
            for (index, iface) in wizard.interfaces.iter().enumerate() {
                let selected = index == wizard.wan;
                let address = iface
                    .addresses
                    .first()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| crate::tr!(crate::keys::CONSOLE_NO_IPV4).into());
                let gateway = wizard
                    .routes
                    .iter()
                    .find(|route| route.iface == iface.name)
                    .map(|route| route.gateway.to_string())
                    .unwrap_or_else(|| crate::tr!(crate::keys::CONSOLE_GATEWAY_NOT_FOUND).into());
                clickables.push((lines.len(), Hit::WizardWan(index)));
                push!(Line::styled(
                    format!(
                        "{}{}  {}  {}  {}  gw {}",
                        if selected { "> " } else { "  " },
                        index + 1,
                        iface.name,
                        iface.mac,
                        address,
                        gateway
                    ),
                    if selected {
                        Style::default().fg(Color::Black).bg(Color::Cyan)
                    } else {
                        Style::default()
                    },
                ));
            }
        }
        WizardStep::WanConfig => {
            push!(Line::styled(
                crate::tr!(crate::keys::CONSOLE_WAN_IPV4_MODE),
                Style::default().add_modifier(Modifier::BOLD),
            ));
            push!(Line::raw(""));
            let tab_focus = wizard.focus == 0;
            let content_width = body.width.saturating_sub(2);
            let tab_row = block_row_of(&lines, lines.len(), content_width);
            let mut tab_x = body.x.saturating_add(1);
            let mut tab_spans = Vec::new();
            for (mode, label) in [
                (WanMode::Static, crate::tr!(crate::keys::CONSOLE_TAB_STATIC)),
                (WanMode::Dhcp, crate::tr!(crate::keys::CONSOLE_TAB_DHCP)),
            ] {
                let tab_text = format!("[ {label} ]");
                let tab_width = UnicodeWidthStr::width(tab_text.as_str()) as u16;
                hits.add(
                    Rect::new(
                        tab_x,
                        body.y.saturating_add(1).saturating_add(tab_row),
                        tab_width,
                        1,
                    ),
                    Hit::WizardTab(mode),
                );
                tab_x = tab_x.saturating_add(tab_width).saturating_add(2);
                let active = wizard.wan_mode == mode;
                let style = if tab_focus && active {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else if tab_focus || active {
                    Style::default().add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                tab_spans.push(Span::styled(tab_text, style));
                tab_spans.push(Span::raw("  "));
            }
            push!(Line::from(tab_spans));
            push!(Line::raw(""));
            if wizard.wan_mode == WanMode::Static {
                clickables.push((lines.len(), Hit::WizardField(1)));
                push!(wizard_field_row(
                    wizard.focus == 1,
                    wizard.editing,
                    &crate::tr!(crate::keys::CONSOLE_IPV4_ADDRESS_CIDR),
                    &wizard.address,
                ));
                clickables.push((lines.len(), Hit::WizardField(2)));
                push!(wizard_field_row(
                    wizard.focus == 2,
                    wizard.editing,
                    &crate::tr!(crate::keys::CONSOLE_DEFAULT_GATEWAY),
                    &wizard.gateway,
                ));
            } else {
                push!(Line::styled(
                    crate::tr!(crate::keys::CONSOLE_WAN_DHCP_CLIENT_HINT),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            push!(Line::raw(""));
            push!(wizard_confirm_button_row(
                wizard.focus == wizard.focus_max(),
            ));
        }
        WizardStep::Lan => {
            push!(Line::styled(
                crate::tr!(crate::keys::CONSOLE_SELECT_LAN_INTERFACES),
                Style::default().add_modifier(Modifier::BOLD),
            ));
            push!(Line::raw(""));
            if wizard.lan_candidates.is_empty() {
                push!(Line::raw(crate::tr!(
                    crate::keys::CONSOLE_NO_OTHER_INTERFACES
                )));
            }
            for (index, iface) in wizard.lan_candidates.iter().enumerate() {
                let cursor = index == wizard.lan_cursor;
                clickables.push((lines.len(), Hit::WizardLan(index)));
                push!(Line::styled(
                    format!(
                        "{}[{}] {}  {}  {}",
                        if cursor { "> " } else { "  " },
                        if wizard.lan_selected[index] { "x" } else { " " },
                        iface.name,
                        iface.mac,
                        if iface.operstate == "up" {
                            crate::tr!(crate::keys::CONSOLE_LINK_UP)
                        } else {
                            crate::tr!(crate::keys::CONSOLE_LINK_DOWN)
                        }
                    ),
                    if cursor {
                        Style::default().fg(Color::Black).bg(Color::Cyan)
                    } else {
                        Style::default()
                    },
                ));
            }
        }
        WizardStep::LanDhcp => {
            push!(Line::styled(
                crate::tr!(crate::keys::CONSOLE_LAN_DHCP_CONFIGURATION),
                Style::default().add_modifier(Modifier::BOLD),
            ));
            push!(Line::raw(""));
            clickables.push((lines.len(), Hit::WizardField(0)));
            push!(wizard_field_row(
                wizard.focus == 0,
                wizard.editing,
                &crate::tr!(crate::keys::CONSOLE_LAN_MANAGEMENT_IPV4_ADDRESS),
                &wizard.management,
            ));
            clickables.push((lines.len(), Hit::WizardField(1)));
            push!(wizard_field_row(
                wizard.focus == 1,
                wizard.editing,
                &crate::tr!(crate::keys::CONSOLE_LAN_DHCP_RANGE_START),
                &wizard.dhcp_start,
            ));
            clickables.push((lines.len(), Hit::WizardField(2)));
            push!(wizard_field_row(
                wizard.focus == 2,
                wizard.editing,
                &crate::tr!(crate::keys::CONSOLE_LAN_DHCP_RANGE_END),
                &wizard.dhcp_end,
            ));
            push!(Line::raw(""));
            push!(wizard_confirm_button_row(wizard.focus == 3));
        }
        WizardStep::Confirm => {
            let wan = wizard.selected_wan();
            push!(Line::styled(
                crate::tr!(crate::keys::CONSOLE_CONFIRM_NETWORK_TAKEOVER_PLAN),
                Style::default().add_modifier(Modifier::BOLD),
            ));
            push!(Line::raw(""));
            push!(Line::raw(crate::tr!(
                crate::keys::CONSOLE_CONFIRM_WAN_INTERFACE,
                name = wan.name,
                mac = wan.mac
            )));
            push!(Line::raw(match wizard.wan_mode {
                WanMode::Static => crate::tr!(
                    crate::keys::CONSOLE_CONFIRM_WAN_MODE_STATIC,
                    address = wizard.address,
                    gateway = wizard.gateway
                ),
                WanMode::Dhcp => {
                    crate::tr!(crate::keys::CONSOLE_CONFIRM_WAN_MODE_DHCP)
                }
            }));
            let lan: Vec<&str> = wizard
                .lan_candidates
                .iter()
                .zip(&wizard.lan_selected)
                .filter(|(_, selected)| **selected)
                .map(|(iface, _)| iface.name.as_str())
                .collect();
            if lan.is_empty() {
                push!(Line::raw(crate::tr!(
                    crate::keys::CONSOLE_CONFIRM_LAN_MODE_WAN_ONLY
                )));
            } else {
                let names = lan.join(", ");
                push!(Line::raw(crate::tr!(
                    crate::keys::CONSOLE_CONFIRM_LAN_INTERFACES,
                    names = names
                )));
                push!(Line::raw(crate::tr!(
                    crate::keys::CONSOLE_CONFIRM_MANAGEMENT,
                    management = wizard.management
                )));
                push!(Line::raw(crate::tr!(
                    crate::keys::CONSOLE_CONFIRM_DHCP_RANGE,
                    start = wizard.dhcp_start,
                    end = wizard.dhcp_end
                )));
            }
            push!(Line::raw(""));
            push!(Line::styled(
                crate::tr!(crate::keys::CONSOLE_CONFIRM_LAN_FLUSH_NOTE),
                Style::default().fg(Color::Yellow),
            ));
            push!(Line::styled(
                crate::tr!(crate::keys::CONSOLE_PRESS_ENTER_TO_START_INSTALLATION),
                Style::default().add_modifier(Modifier::BOLD),
            ));
        }
    }
    let content_width = body.width.saturating_sub(2);
    for (index, hit) in clickables {
        hits.block_row(body, block_row_of(&lines, index, content_width), hit);
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title(crate::tr!(crate::keys::CONSOLE_NETWORK_PANEL_TITLE)))
            .wrap(Wrap { trim: true }),
        body,
    );
    frame.render_widget(
        Paragraph::new(wizard_hints(wizard)).style(Style::default().fg(Color::DarkGray)),
        footer,
    );
    if wizard.cancel_confirming {
        render_wizard_cancel_confirmation(frame, hits);
    }
}

fn wizard_field_row(focused: bool, editing: bool, label: &str, value: &str) -> Line<'static> {
    let style = if focused {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let marker = if focused && editing { "_" } else { "" };
    Line::from(vec![
        Span::styled(if focused { "> " } else { "  " }, style),
        Span::styled(display_pad(label, 20), style),
        Span::styled(format!("{value}{marker}"), style),
    ])
}

fn wizard_confirm_button_row(focused: bool) -> Line<'static> {
    let style = if focused {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD)
    };
    Line::from(vec![
        Span::styled(if focused { "> " } else { "  " }, style),
        Span::styled(
            format!(
                "[ {} ]",
                crate::tr!(crate::keys::CONSOLE_CONFIRM_AND_CONTINUE)
            ),
            style,
        ),
    ])
}

fn wizard_hints(wizard: &NetworkWizard) -> String {
    if wizard.cancel_confirming {
        return crate::tr!(crate::keys::CONSOLE_WIZARD_HINT_CANCEL);
    }
    match wizard.step {
        WizardStep::Wan => crate::tr!(crate::keys::CONSOLE_WIZARD_HINT_WAN),
        WizardStep::WanConfig => crate::tr!(crate::keys::CONSOLE_WIZARD_HINT_CONFIG),
        WizardStep::Lan => crate::tr!(crate::keys::CONSOLE_WIZARD_HINT_LAN),
        WizardStep::LanDhcp => crate::tr!(crate::keys::CONSOLE_WIZARD_HINT_EDIT),
        WizardStep::Confirm => crate::tr!(crate::keys::CONSOLE_WIZARD_HINT_CONFIRM),
    }
}

fn render_wizard_cancel_confirmation(frame: &mut Frame<'_>, hits: &mut Clicks) {
    let screen = frame.area();
    let width = 52.min(screen.width.saturating_sub(2));
    let height = 8.min(screen.height.saturating_sub(2));
    let area = Rect::new(
        screen.x + screen.width.saturating_sub(width) / 2,
        screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    );
    register_dialog_hits(hits, screen, area);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_CANCEL_NETWORK_WIZARD_QUESTION),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Line::raw(""),
            Line::raw(crate::tr!(
                crate::keys::CONSOLE_CANCEL_NETWORK_WIZARD_PRESS_ENTER
            )),
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_CANCEL_NETWORK_WIZARD_PRESS_ESC),
                Style::default().fg(Color::DarkGray),
            ),
        ])
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: true })
        .block(Block::bordered().title(crate::tr!(crate::keys::CONSOLE_CANCEL_WIZARD))),
        area,
    );
}
pub(crate) fn render_pending_takeover(frame: &mut Frame<'_>, app: &mut ConsoleApp) {
    let Snapshot::AwaitingNetworkConfirmation {
        transaction_id,
        phase,
        deadline,
        management_address,
    } = &app.snapshot
    else {
        return;
    };
    let confirm_allowed = app.takeover_confirm_allowed();
    let screen = frame.area();
    let width = 76.min(screen.width.saturating_sub(2));
    let height = 17.min(screen.height.saturating_sub(2));
    let area = Rect::new(
        screen.x + screen.width.saturating_sub(width) / 2,
        screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    );
    let mut lines = vec![
        Line::styled(
            crate::tr!(crate::keys::CONSOLE_TAKEOVER_PENDING_TITLE),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
        Line::raw(crate::tr!(
            crate::keys::CONSOLE_TAKEOVER_PENDING_TRANSACTION,
            id = transaction_id
        )),
        Line::raw(crate::tr!(
            crate::keys::CONSOLE_TAKEOVER_PENDING_PHASE,
            phase = phase
        )),
        Line::raw(crate::tr!(
            crate::keys::CONSOLE_TAKEOVER_PENDING_ADDRESS,
            address = management_address
                .as_deref()
                .map(str::to_string)
                .unwrap_or_else(|| crate::tr!(crate::keys::TAKEOVER_DHCP_LEASE))
        )),
        Line::raw(crate::tr!(
            crate::keys::CONSOLE_TAKEOVER_PENDING_DEADLINE,
            deadline = deadline
        )),
        Line::raw(""),
        Line::raw(crate::tr!(crate::keys::CONSOLE_TAKEOVER_PENDING_HINT)),
        Line::raw(""),
    ];
    let later = crate::tr!(crate::keys::CONSOLE_TAKEOVER_PENDING_LATER);
    let later_row = lines.len();
    lines.push(Line::from(Span::styled(
        if app.takeover_choice == 0 {
            format!("> {later}")
        } else {
            format!("  {later}")
        },
        if app.takeover_choice == 0 {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        },
    )));
    let confirm = crate::tr!(crate::keys::CONSOLE_TAKEOVER_PENDING_CONFIRM);
    let confirm_row = lines.len();
    let confirm_line = if confirm_allowed {
        if app.takeover_choice == 1 {
            format!("> {confirm}")
        } else {
            format!("  {confirm}")
        }
    } else {
        format!(
            "  {} ({})",
            confirm,
            crate::tr!(crate::keys::CONSOLE_TAKEOVER_PENDING_ROLLING_BACK)
        )
    };
    lines.push(Line::from(Span::styled(
        confirm_line,
        if confirm_allowed && app.takeover_choice == 1 {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        },
    )));
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        crate::tr!(crate::keys::CONSOLE_TAKEOVER_PENDING_KEY_HINT),
        Style::default().fg(Color::DarkGray),
    ));
    let content_width = area.width.saturating_sub(2);
    app.hits.block_row(
        area,
        block_row_of(&lines, later_row, content_width),
        Hit::TakeoverChoice(0),
    );
    app.hits.block_row(
        area,
        block_row_of(&lines, confirm_row, content_width),
        Hit::TakeoverChoice(1),
    );
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true })
            .block(
                Block::bordered().title(crate::tr!(crate::keys::CONSOLE_TAKEOVER_PENDING_WINDOW)),
            ),
        area,
    );
}
