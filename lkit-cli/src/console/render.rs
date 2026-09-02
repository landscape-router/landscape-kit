use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use unicode_width::UnicodeWidthStr;

use super::backup::{
    render_backup, render_backup_corrupt_dialog, render_backup_create_dialog,
    render_backup_create_progress, render_backup_delete_confirmation,
    render_backup_restore_confirmation,
};
use super::daemon_panel::{
    render_daemon_deploy_confirmation, render_daemon_deploy_progress, render_show_psk_dialog,
};
use super::flare_panel::render_flare_dialog;
use super::install_form::render_install;
use super::mirror::{render_mirror, render_mirror_confirmation};
use super::network_wizard::{Snapshot, render_network_wizard, render_pending_takeover};
use super::preflight::render_preflight_dialog;
use super::reinit::{render_reinit, render_reinit_confirmation};
use super::software::{
    render_base_packages_dialog, render_base_packages_progress, render_software,
    render_software_confirmation, render_software_progress,
};
use super::update::{
    render_uninstall, render_uninstall_confirmation, render_update, render_update_confirmation,
};
use super::widgets::{Clicks, Focus, Hit, Menu, block_row_of};
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
    // status 高度随内容行数动态:短内容 3 行(边框+状态+提示),
    // 长内容最多 5 行,换行不截断、不留空行。
    let status_height = status_height_for(app, frame.area().width);
    let [header, body, status] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(8),
        Constraint::Length(status_height),
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
    if app.menu() == Menu::Backup && app.backup.corrupt_dialog {
        render_backup_corrupt_dialog(frame, app);
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
    if app.menu() == Menu::Software && app.software.confirming.is_some() {
        render_software_confirmation(frame, app);
    }
    if app.menu() == Menu::Software && app.software.install.is_some() {
        render_software_progress(frame, app);
    }
    if app.menu() == Menu::Software
        && matches!(
            &app.software.base_packages,
            super::software::BasePackagesState::Choosing { .. }
        )
    {
        render_base_packages_dialog(frame, app);
    }
    if app.menu() == Menu::Software && app.software.base_install.is_some() {
        render_base_packages_progress(frame, app);
    }
    if app.menu() == Menu::Uninstall && app.uninstall.confirming {
        render_uninstall_confirmation(frame, app);
    }
    if app.menu() == Menu::Reinit && app.reinit.confirming {
        render_reinit_confirmation(frame, app);
    }
    // 部署确认弹窗可从 Overview 动作行或安装阻断弹框发起,不限定菜单。
    if app.deploy_daemon_confirming {
        render_daemon_deploy_confirmation(frame, app);
    }
    if app.menu() == Menu::Overview && app.show_psk {
        render_show_psk_dialog(frame, app);
    }
    if app.flare.open {
        render_flare_dialog(frame, app);
    }
    // 部署可从 Overview 动作行或安装阻断弹框发起,进度弹层不限定菜单。
    if app.deploy_daemon.is_some() {
        render_daemon_deploy_progress(frame, app);
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
fn render_status(frame: &mut Frame<'_>, app: &mut ConsoleApp, area: Rect) {
    frame.render_widget(Block::default().borders(Borders::TOP), area);
    let content = Rect::new(
        area.x,
        area.y.saturating_add(1),
        area.width,
        area.height.saturating_sub(1),
    );
    let language = language_status(
        crate::i18n::current(),
        app.language_switch_available(),
        app.editing_any_field(),
    );
    let language_width = (UnicodeWidthStr::width(language.as_str()) as u16)
        .saturating_add(2)
        .min(content.width);
    // 状态与提示都先按实际宽度预折行,行数与渲染共用同一折行结果:若交给
    // Paragraph 自行词级换行,其行数会与按字符模拟的预留高度不一致(词级换行
    // 预留行尾空白),多行 notice 可能被截掉最后一行。
    let notice_lines = super::widgets::wrap_to_width(
        content.width.saturating_sub(language_width).max(1),
        &app.notice.text(),
    );
    let hints_lines = super::widgets::wrap_to_width(content.width, &app.hints());
    let [summary, hints] = Layout::vertical([
        Constraint::Length(notice_lines.len().max(1) as u16),
        Constraint::Length(hints_lines.len().max(1) as u16),
    ])
    .areas(content);
    let [notice_area, language_area] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(language_width)]).areas(summary);
    let notice_color = app.notice.color();
    frame.render_widget(
        Paragraph::new(notice_lines.into_iter().map(Line::from).collect::<Vec<_>>())
            .style(Style::default().fg(notice_color)),
        notice_area,
    );
    // 语言指示可点击:点击等价于按 L(编辑中不可切换时点击无效)。
    if app.language_switch_available() {
        app.hits.add(language_area, Hit::LanguageSwitch);
    }
    frame.render_widget(
        Paragraph::new(language)
            .alignment(Alignment::Right)
            .style(Style::default().fg(Color::Cyan)),
        language_area,
    );
    frame.render_widget(
        Paragraph::new(hints_lines.into_iter().map(Line::from).collect::<Vec<_>>())
            .style(Style::default().fg(Color::DarkGray)),
        hints,
    );
}

/// 计算 status 区所需行数:1 行边框 + 状态行数 + 提示行数(均至少 1 行)。
/// 与 `render_status` 使用同一个 `wrap_to_width` 预折行,保证预留高度与实际
/// 渲染行数完全一致。
fn status_height_for(app: &ConsoleApp, width: u16) -> u16 {
    let language = language_status(
        crate::i18n::current(),
        app.language_switch_available(),
        app.editing_any_field(),
    );
    let language_width = (UnicodeWidthStr::width(language.as_str()) as u16)
        .saturating_add(2)
        .min(width);
    let notice_rows = super::widgets::wrap_to_width(
        width.saturating_sub(language_width).max(1),
        &app.notice.text(),
    )
    .len()
    .max(1) as u16;
    let hints_rows = super::widgets::wrap_to_width(width, &app.hints())
        .len()
        .max(1) as u16;
    1 + notice_rows + hints_rows
}

/// 状态栏右下角的语言指示。可切换时显示**目标语言**(按 `L` 或点击即切换到
/// 所示目标,所见即所得);文本编辑中退回当前语言并解释 `L` 暂停(此时 `l` 是
/// 普通输入字符),退出确认层等其余不可切换状态只显示当前语言。
fn language_status(language: Language, switch_available: bool, editing: bool) -> String {
    if switch_available {
        crate::tr!(
            crate::keys::CONSOLE_LANGUAGE_SWITCH_HINT,
            language = language.toggled().native_label()
        )
    } else if editing {
        crate::tr!(
            crate::keys::CONSOLE_LANGUAGE_PAUSED,
            language = language.native_label()
        )
    } else {
        crate::tr!(
            crate::keys::CONSOLE_LANGUAGE_CURRENT,
            language = language.native_label()
        )
    }
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
    let (daemon_badge, daemon_color) = if crate::daemon_worker::daemon_is_running() {
        (
            crate::tr!(crate::keys::CONSOLE_HEADER_DAEMON_RUNNING),
            Color::Green,
        )
    } else {
        (
            crate::tr!(crate::keys::CONSOLE_HEADER_DAEMON_NOT_RUNNING),
            Color::Red,
        )
    };
    // 窄终端放不下两个徽标时全部隐藏,只保留品牌标题。
    let title_width = UnicodeWidthStr::width("Landscape Kit");
    let badge_width = UnicodeWidthStr::width(badge.as_str());
    let daemon_width = UnicodeWidthStr::width(daemon_badge.as_str());
    let badges_width = badge_width + daemon_width + 4;
    let fits = title_width + badges_width + 4 <= usize::from(area.width);
    let title = Paragraph::new(Line::from(vec![Span::styled(
        "Landscape Kit",
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )]))
    .alignment(Alignment::Left);
    if !fits {
        frame.render_widget(title.block(Block::default().borders(Borders::BOTTOM)), area);
        return;
    }
    // 标题靠左、徽标组靠右(space-between 布局)。
    let [title_area, badges_area] = Layout::horizontal([
        Constraint::Length(usize::from(area.width) as u16),
        Constraint::Length(badges_width as u16),
    ])
    .areas(area);
    frame.render_widget(title, title_area);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(badge, Style::default().fg(color)),
            Span::raw("  "),
            Span::styled(daemon_badge, Style::default().fg(daemon_color)),
        ]))
        .alignment(Alignment::Right),
        badges_area,
    );
    frame.render_widget(Block::default().borders(Borders::BOTTOM), area);
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
        Menu::Software => render_software(frame, app, area),
        Menu::Reinit => render_reinit(frame, app, area),
        Menu::Uninstall => render_uninstall(frame, app, area),
    }
}
fn render_overview(frame: &mut Frame<'_>, app: &mut ConsoleApp, area: Rect) {
    let focused = app.focus == Focus::Panel;
    // 左栏:Landscape 安装信息。
    let landscape_lines = overview_landscape_lines(app);
    // 窄面板回退为上下堆叠,保证 72 列终端(面板约 46 列)可用。
    if area.width < 52 {
        let content_width = area.width.saturating_sub(2);
        let (lkit_lines, action_rows) = overview_lkit_lines(focused, content_width);
        let mut lines = landscape_lines;
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            crate::tr!(crate::keys::CONSOLE_OVERVIEW_LKIT_SECTION),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        ));
        let lkit_start = lines.len();
        lines.extend(lkit_lines);
        for (row, hit) in action_rows {
            app.hits.block_row(
                area,
                block_row_of(&lines, lkit_start + row, content_width),
                hit,
            );
        }
        frame.render_widget(
            Paragraph::new(lines)
                .block(panel_block(
                    &crate::tr!(crate::keys::CONSOLE_OVERVIEW),
                    focused,
                ))
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    // 宽面板左右双栏:左 Landscape,右 lkit 常驻服务,中间竖线分隔。
    let block = panel_block(&crate::tr!(crate::keys::CONSOLE_OVERVIEW), focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [landscape_area, lkit_area] =
        Layout::horizontal([Constraint::Min(24), Constraint::Min(24)]).areas(inner);
    let (lkit_lines, action_rows) = overview_lkit_lines(focused, lkit_area.width);
    frame.render_widget(
        Paragraph::new(landscape_lines)
            .block(Block::default().borders(Borders::RIGHT))
            .wrap(Wrap { trim: false }),
        landscape_area,
    );
    for (row, hit) in action_rows {
        // 右栏无边框:直接注册内容区坐标(不用 block_row 的边框偏移)。
        let content_row = block_row_of(&lkit_lines, row, lkit_area.width);
        app.hits.add(
            Rect::new(lkit_area.x, lkit_area.y + content_row, lkit_area.width, 1),
            hit,
        );
    }
    frame.render_widget(
        Paragraph::new(lkit_lines).wrap(Wrap { trim: false }),
        lkit_area,
    );
}

/// Overview 左栏:Landscape 安装状态与详情。
fn overview_landscape_lines(app: &ConsoleApp) -> Vec<Line<'static>> {
    match &app.snapshot {
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
            Line::raw(error.clone()),
        ],
    }
}

/// Overview 右栏:lkit 常驻服务。返回行与动作行(行号,命中区)列表。
/// 小节标题与动作行上方各带灰色简介,解释常驻服务与急救恢复码是什么,避免
/// 只有状态没有语义。所有可能超宽的动态行(简介、版本号、daemon 状态)均按
/// `wrap_width` 预折行(见 `wrap_to_width`),保证命中区行号模拟与实际渲染
/// 一致——任何一行漏折都会让 `block_row_of` 的按字符模拟与 ratatui 的词级
/// 换行漂移,动作行命中区错位一整行。
fn overview_lkit_lines(focused: bool, wrap_width: u16) -> (Vec<Line<'static>>, Vec<(usize, Hit)>) {
    let version = env!("CARGO_PKG_VERSION");
    let running = crate::daemon_worker::daemon_is_running();
    let muted = Style::default().fg(Color::DarkGray);
    let wrapped = |text: String, style: Style| -> Vec<Line<'static>> {
        super::widgets::wrap_to_width(wrap_width, &text)
            .into_iter()
            .map(|line| Line::styled(line, style))
            .collect()
    };
    let mut lines = vec![Line::styled(
        crate::tr!(crate::keys::CONSOLE_OVERVIEW_LKIT_SECTION),
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    )];
    lines.extend(wrapped(
        crate::tr!(crate::keys::CONSOLE_OVERVIEW_LKIT_SECTION_HELP),
        muted,
    ));
    lines.extend(wrapped(
        crate::tr!(
            crate::keys::CONSOLE_OVERVIEW_LKIT_VERSION,
            version = version
        ),
        Style::default(),
    ));
    lines.push(Line::raw(""));
    let (daemon_status, daemon_style) = if running {
        (
            crate::tr!(crate::keys::CONSOLE_OVERVIEW_LKIT_DAEMON_RUNNING),
            Style::default().fg(Color::Green),
        )
    } else {
        (
            crate::tr!(crate::keys::CONSOLE_OVERVIEW_LKIT_DAEMON_NOT_RUNNING),
            Style::default().fg(Color::Red),
        )
    };
    lines.extend(wrapped(daemon_status, daemon_style));
    let selected_style = if focused {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    let action_style = if focused {
        selected_style
    } else {
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD)
    };
    // daemon 未运行时提供「部署 daemon」动作行:确认后在 TUI 内后台执行
    // `lkit self install`,不退出控制台。
    let mut action_rows = Vec::new();
    if !running {
        let deploy_row = lines.len();
        lines.push(Line::from(vec![
            Span::styled(if focused { "> " } else { "  " }, selected_style),
            Span::styled(
                crate::tr!(crate::keys::CONSOLE_OVERVIEW_DEPLOY_DAEMON),
                action_style,
            ),
        ]));
        action_rows.push((deploy_row, Hit::OverviewDeploy));
    } else {
        // daemon 运行时提供「查看急救恢复码」动作行:弹出展示当前 `[flare]`
        // 段 psk 明文,供分发给恢复操作员;动作行上方一行简介说明其用途。
        lines.extend(wrapped(
            crate::tr!(crate::keys::CONSOLE_OVERVIEW_LKIT_PSK_HELP),
            muted,
        ));
        let show_row = lines.len();
        lines.push(Line::from(vec![
            Span::styled(if focused { "> " } else { "  " }, selected_style),
            Span::styled(
                crate::tr!(crate::keys::CONSOLE_OVERVIEW_SHOW_PSK),
                action_style,
            ),
        ]));
        action_rows.push((show_row, Hit::OverviewShowPsk));
    }
    (lines, action_rows)
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
