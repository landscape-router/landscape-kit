use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use super::backup::{
    render_backup, render_backup_create_dialog, render_backup_create_progress,
    render_backup_delete_confirmation, render_backup_restore_confirmation,
};
use super::install_form::render_install;
use super::mirror::{render_mirror, render_mirror_confirmation};
use super::network_wizard::{Snapshot, render_network_wizard, render_pending_takeover};
use super::preflight::render_preflight_dialog;
use super::reinit::{render_reinit, render_reinit_confirmation};
use super::update::{
    render_uninstall, render_uninstall_confirmation, render_update, render_update_confirmation,
};
use super::widgets::{Clicks, Focus, Hit, Menu};
use super::{ConsoleApp, ExitState};
use crate::i18n::Language;

pub(crate) fn render(frame: &mut Frame<'_>, app: &mut ConsoleApp) {
    app.hits.clear();
    if frame.area().width < 72 || frame.area().height < 18 {
        frame.render_widget(
            Paragraph::new(crate::tr!(crate::keys::CONSOLE_TERMINAL_TOO_SMALL))
                .alignment(Alignment::Center)
                .block(Block::bordered().title("Landscape Kit")),
            frame.area(),
        );
        if app.exit_state == ExitState::Confirming {
            render_exit_confirmation(frame, &mut app.hits);
        }
        return;
    }
    if app.takeover_pending() {
        render_pending_takeover(frame, app);
        return;
    }
    if let Some(wizard) = &app.network_wizard {
        render_network_wizard(frame, wizard, &mut app.hits);
        return;
    }
    let [header, body, status] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(8),
        Constraint::Length(5),
    ])
    .areas(frame.area());
    render_header(frame, app, header);
    let [navigation, panel] =
        Layout::horizontal([Constraint::Length(24), Constraint::Min(24)]).areas(body);
    app.hits.add(navigation, Hit::Navigation);
    app.hits.add(panel, Hit::Panel);
    render_navigation(frame, app, navigation);
    render_panel(frame, app, panel);
    render_status(frame, app, status);
    if app.exit_state == ExitState::Confirming {
        render_exit_confirmation(frame, &mut app.hits);
    }
    if app.preflight_dialog {
        render_preflight_dialog(frame, app);
    }
    if app.menu() == Menu::Backup && app.backup.restore_confirming {
        render_backup_restore_confirmation(frame, app);
    }
    if app.menu() == Menu::Backup && app.backup.delete_confirming {
        render_backup_delete_confirmation(frame, app);
    }
    if app.menu() == Menu::Backup && app.backup.editing {
        render_backup_create_dialog(frame, app);
    }
    if app.menu() == Menu::Backup && app.backup.create.is_some() {
        render_backup_create_progress(frame, app);
    }
    if app.menu() == Menu::Update && app.update.confirming.is_some() {
        render_update_confirmation(frame, app);
    }
    if app.menu() == Menu::Mirror && app.mirror.confirming.is_some() {
        render_mirror_confirmation(frame, app);
    }
    if app.menu() == Menu::Uninstall && app.uninstall.confirming {
        render_uninstall_confirmation(frame, app);
    }
    if app.menu() == Menu::Reinit && app.reinit.confirming {
        render_reinit_confirmation(frame, app);
    }
}

/// 注册确认弹层的命中区:弹层整体视为 Enter,弹层外整屏视为 Esc。
pub(crate) fn register_dialog_hits(hits: &mut Clicks, screen: Rect, area: Rect) {
    hits.add(screen, Hit::Outside);
    hits.add(area, Hit::DialogConfirm);
}

/// 注册输入/进度弹层的命中区:弹层内部不响应,弹层外整屏视为 Esc。
pub(crate) fn register_modal_hits(hits: &mut Clicks, screen: Rect, area: Rect) {
    hits.add(screen, Hit::Outside);
    hits.add(area, Hit::Nothing);
}
fn render_status(frame: &mut Frame<'_>, app: &ConsoleApp, area: Rect) {
    frame.render_widget(Block::default().borders(Borders::TOP), area);
    let content = Rect::new(
        area.x,
        area.y.saturating_add(1),
        area.width,
        area.height.saturating_sub(1),
    );
    let [summary, hints] =
        Layout::vertical([Constraint::Length(2), Constraint::Length(2)]).areas(content);
    let language = language_status(crate::i18n::current(), app.language_switch_available());
    let language_width = (UnicodeWidthStr::width(language.as_str()) as u16)
        .saturating_add(2)
        .min(summary.width);
    let [notice, language_area] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(language_width)]).areas(summary);
    let notice_color = if app.notice == "Ready" {
        Color::DarkGray
    } else {
        Color::Red
    };
    frame.render_widget(
        Paragraph::new(if app.notice == "Ready" {
            crate::tr!(crate::keys::CONSOLE_READY)
        } else {
            app.notice.clone()
        })
        .wrap(Wrap { trim: true })
        .style(Style::default().fg(notice_color)),
        notice,
    );
    frame.render_widget(
        Paragraph::new(language)
            .alignment(Alignment::Right)
            .style(Style::default().fg(Color::Cyan)),
        language_area,
    );
    frame.render_widget(
        Paragraph::new(app.hints())
            .wrap(Wrap { trim: true })
            .style(Style::default().fg(Color::DarkGray)),
        hints,
    );
}

fn language_status(language: Language, switch_available: bool) -> String {
    match (language, switch_available) {
        (Language::En, true) => "L  Language: English (en)",
        (Language::En, false) => "Language: English (en)",
        (Language::Zh, true) => "L  语言：中文 (zh)",
        (Language::Zh, false) => "语言：中文 (zh)",
    }
    .into()
}
fn render_exit_confirmation(frame: &mut Frame<'_>, hits: &mut Clicks) {
    let screen = frame.area();
    let width = 48.min(screen.width.saturating_sub(2));
    let height = 7.min(screen.height.saturating_sub(2));
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
                crate::tr!(crate::keys::CONSOLE_EXIT_LANDSCAPE_KIT_QUESTION),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Line::raw(""),
            Line::raw(crate::tr!(crate::keys::CONSOLE_PRESS_ENTER_TO_EXIT)),
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_PRESS_ESC_TO_CANCEL),
                Style::default().fg(Color::DarkGray),
            ),
        ])
        .alignment(Alignment::Center)
        .block(Block::bordered().title(crate::tr!(crate::keys::CONSOLE_CONFIRM_EXIT))),
        area,
    );
}

fn render_header(frame: &mut Frame<'_>, app: &ConsoleApp, area: Rect) {
    let (badge, color) = app.snapshot.badge();
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "Landscape Kit",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(badge, Style::default().fg(color)),
        ]))
        .alignment(Alignment::Left)
        .block(Block::default().borders(Borders::BOTTOM)),
        area,
    );
}
fn render_navigation(frame: &mut Frame<'_>, app: &mut ConsoleApp, area: Rect) {
    let items: Vec<ListItem<'_>> = Menu::ALL
        .iter()
        .enumerate()
        .map(|(index, menu)| {
            app.hits.block_row(area, index as u16, Hit::Menu(index));
            let style = if app.menu_available(*menu) {
                Style::default()
            } else {
                Style::default().fg(Color::DarkGray)
            };
            ListItem::new(Span::styled(menu.label(), style))
        })
        .collect();
    let mut state = ListState::default().with_selected(Some(app.menu_index));
    let highlight = if app.focus == Focus::Navigation {
        Style::default().fg(Color::Black).bg(Color::Cyan)
    } else {
        Style::default().fg(Color::Cyan)
    };
    frame.render_stateful_widget(
        List::new(items)
            .block(Block::bordered().title(crate::tr!(crate::keys::CONSOLE_NAVIGATION)))
            .highlight_style(highlight)
            .highlight_symbol("> "),
        area,
        &mut state,
    );
}

fn render_panel(frame: &mut Frame<'_>, app: &mut ConsoleApp, area: Rect) {
    let focused = app.focus == Focus::Panel;
    match app.menu() {
        Menu::Overview => render_overview(frame, app, area),
        Menu::Install if app.install_available() => render_install(frame, app, area),
        Menu::Install => {
            frame.render_widget(
                Paragraph::new(vec![
                    Line::styled(
                        crate::tr!(crate::keys::CONSOLE_LANDSCAPE_IS_INSTALLED),
                        Style::default().fg(Color::Green),
                    ),
                    Line::raw(""),
                    Line::styled(
                        crate::tr!(crate::keys::CONSOLE_INSTALL_UNAVAILABLE),
                        Style::default().fg(Color::DarkGray),
                    ),
                ])
                .block(panel_block(
                    &crate::tr!(crate::keys::CONSOLE_INSTALL_MENU),
                    focused,
                ))
                .wrap(Wrap { trim: true }),
                area,
            );
        }
        Menu::Backup => render_backup(frame, app, area),
        Menu::Update => render_update(frame, app, area),
        Menu::Mirror => render_mirror(frame, app, area),
        Menu::Reinit => render_reinit(frame, app, area),
        Menu::Uninstall => render_uninstall(frame, app, area),
    }
}
fn render_overview(frame: &mut Frame<'_>, app: &ConsoleApp, area: Rect) {
    let lines = match &app.snapshot {
        Snapshot::RootRequired => vec![
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_ROOT_PRIVILEGES_REQUIRED),
                Style::default().fg(Color::Yellow),
            ),
            Line::raw(""),
            Line::raw(crate::tr!(
                crate::keys::CONSOLE_OVERVIEW_INSTALL_ROOT,
                root = app.install.install_dir
            )),
        ],
        Snapshot::AwaitingNetworkConfirmation { .. } => vec![Line::styled(
            crate::tr!(crate::keys::CONSOLE_TAKEOVER_PENDING_TITLE),
            Style::default().fg(Color::Yellow),
        )],
        Snapshot::NotInstalled => vec![
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_LANDSCAPE_NOT_INSTALLED),
                Style::default().fg(Color::Yellow),
            ),
            Line::raw(""),
            Line::raw(crate::tr!(
                crate::keys::CONSOLE_OVERVIEW_INSTALL_ROOT,
                root = app.install.install_dir
            )),
        ],
        Snapshot::Installed {
            version,
            manager,
            initialized,
        } => vec![
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_LANDSCAPE_IS_INSTALLED),
                Style::default().fg(Color::Green),
            ),
            Line::raw(""),
            Line::raw(crate::tr!(
                crate::keys::CONSOLE_OVERVIEW_VERSION,
                version = version
            )),
            Line::raw(crate::tr!(
                crate::keys::CONSOLE_OVERVIEW_SERVICE,
                manager = manager
            )),
            Line::raw(crate::tr!(if *initialized {
                crate::keys::CONSOLE_OVERVIEW_INITIALIZATION_COMPLETE
            } else {
                crate::keys::CONSOLE_OVERVIEW_INITIALIZATION_PENDING
            })),
            Line::raw(crate::tr!(
                crate::keys::CONSOLE_OVERVIEW_INSTALL_ROOT,
                root = app.install.install_dir
            )),
        ],
        Snapshot::Unavailable(error) => vec![
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_INSTALLATION_STATE_NEEDS_ATTENTION),
                Style::default().fg(Color::Red),
            ),
            Line::raw(""),
            Line::raw(error),
        ],
    };
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel_block(
                &crate::tr!(crate::keys::CONSOLE_OVERVIEW),
                app.focus == Focus::Panel,
            ))
            .wrap(Wrap { trim: false }),
        area,
    );
}
pub(crate) fn panel_block(title: &str, focused: bool) -> Block<'static> {
    let title = if focused {
        format!("> {title}")
    } else {
        title.to_string()
    };
    let border_style = if focused {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    Block::bordered().title(title).border_style(border_style)
}
pub(crate) fn mask(value: &str) -> String {
    "*".repeat(value.chars().count())
}

pub(crate) fn display_pad(value: &str, width: usize) -> String {
    format!(
        "{value}{}",
        " ".repeat(width.saturating_sub(UnicodeWidthStr::width(value)))
    )
}
