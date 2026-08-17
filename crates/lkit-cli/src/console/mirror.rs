use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};

use crossterm::event::{KeyCode, KeyEvent};
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use super::render::{panel_block, register_dialog_hits};
use super::widgets::{Focus, Hit, block_row_of};
use super::{ConsoleAction, ConsoleApp};
use crate::mirror::{Host, MirrorName, MirrorStatus};

/// 换源面板的确认层目标。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MirrorConfirm {
    Apply {
        mirror: MirrorName,
        /// 是否同时替换 Debian 独立 security 仓库（默认不替换）。
        replace_security: bool,
        /// 是否注释启用的 `deb cdrom:` 条目（默认注释）。
        disable_cdrom: bool,
        /// 确认层开关行的键盘焦点：[`apply_toggle_rows`] 返回列表中的下标。
        toggle: usize,
    },
    Restore,
}

/// 确认层里可切换的开关行。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MirrorToggleRow {
    /// 注释 `deb cdrom:` 条目（apt 家族可见）。
    Cdrom,
    /// 同时替换 Debian security 仓库（Debian 可见）。
    Security,
}

/// 当前主机确认层可见的开关行（顺序即焦点顺序）。
pub(crate) fn apply_toggle_rows(host: &Host) -> Vec<MirrorToggleRow> {
    match host.family {
        crate::mirror::Family::Debian => vec![MirrorToggleRow::Cdrom, MirrorToggleRow::Security],
        crate::mirror::Family::Ubuntu => vec![MirrorToggleRow::Cdrom],
        _ => Vec::new(),
    }
}

/// 把确认层的开关焦点移到 `target` 行（点击命中行时先移焦点再切换）。
pub(crate) fn focus_mirror_toggle(
    confirming: &mut Option<MirrorConfirm>,
    host: &Host,
    target: MirrorToggleRow,
) {
    if let Some(MirrorConfirm::Apply { toggle, .. }) = confirming
        && let Some(index) = apply_toggle_rows(host)
            .iter()
            .position(|row| *row == target)
    {
        *toggle = index;
    }
}

/// 换源面板的可选行:镜像列表后跟恢复备份动作。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MirrorRow {
    Mirror(MirrorName),
    Restore,
}

impl MirrorRow {
    /// 全部行的有序列表(与渲染次序一致)。明确不可用的镜像（探测 404）不参与
    /// 导航与确认；探测结果未就绪时全部可选中。
    fn rows(availability: &Option<HashMap<MirrorName, MirrorStatus>>) -> Vec<Self> {
        MirrorName::all()
            .into_iter()
            .filter(|mirror| status_of(availability, *mirror) != MirrorStatus::Unavailable)
            .map(Self::Mirror)
            .chain([Self::Restore])
            .collect()
    }
}

/// 面板的可用性状态：`None` 表示尚未探测（探测未完成或未启动，全部视为可用）。
pub(crate) fn status_of(
    availability: &Option<HashMap<MirrorName, MirrorStatus>>,
    mirror: MirrorName,
) -> MirrorStatus {
    availability
        .as_ref()
        .and_then(|statuses| statuses.get(&mirror).copied())
        .unwrap_or(MirrorStatus::Available)
}

/// 换源面板：显示发行版检测结果，选择镜像或恢复备份。
pub(crate) struct MirrorPanel {
    pub(crate) host: Option<Result<Host, String>>,
    pub(crate) detected: bool,
    /// 镜像可用性探测结果（worker 线程回填）。`None` = 未探测/探测中。
    pub(crate) availability: Option<HashMap<MirrorName, MirrorStatus>>,
    pub(crate) probing: bool,
    probing_rx: Option<Receiver<HashMap<MirrorName, MirrorStatus>>>,
    pub(crate) selected: MirrorRow,
    pub(crate) confirming: Option<MirrorConfirm>,
}

impl Default for MirrorPanel {
    fn default() -> Self {
        Self {
            host: None,
            detected: false,
            availability: None,
            probing: false,
            probing_rx: None,
            selected: MirrorRow::Mirror(MirrorName::all()[0]),
            confirming: None,
        }
    }
}

impl MirrorPanel {
    /// 进入面板时执行一次发行版检测（只读，快速），成功后后台并行探测镜像可用性。
    pub(crate) fn ensure_detected(&mut self) {
        if self.detected {
            return;
        }
        self.detected = true;
        self.host = Some(crate::mirror::detect_host().map_err(|error| error.to_string()));
        if matches!(&self.host, Some(Ok(_))) {
            self.start_probe();
        }
    }

    /// 后台 worker 探测全部镜像可用性（超时 2 秒/镜像，并行），不阻塞 TUI 主循环。
    fn start_probe(&mut self) {
        if self.probing || self.availability.is_some() {
            return;
        }
        let Some(Ok(host)) = &self.host else {
            return;
        };
        let (sender, receiver) = mpsc::channel();
        let host = host.clone();
        let language = crate::i18n::current();
        std::thread::spawn(move || {
            let statuses = crate::i18n::with_language(language, || crate::mirror::probe_all(&host));
            let _ = sender.send(statuses);
        });
        self.probing = true;
        self.probing_rx = Some(receiver);
    }

    /// 主循环轮询：探测完成后回填结果（由 `ConsoleApp::update` 调用）。
    pub(crate) fn poll(&mut self) {
        let Some(receiver) = &self.probing_rx else {
            return;
        };
        match receiver.try_recv() {
            Ok(statuses) => {
                self.availability = Some(statuses);
                self.probing = false;
                self.probing_rx = None;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                // worker 意外退出：保持 None，全部镜像按可用处理。
                self.probing = false;
                self.probing_rx = None;
            }
        }
    }

    /// 重新探测（语言切换后等）：清空结果重启 worker。
    pub(crate) fn restart_probe(&mut self) {
        self.availability = None;
        self.probing = false;
        self.probing_rx = None;
        self.start_probe();
    }
}

impl ConsoleApp {
    /// 执行换源或恢复。失败与成功都写入底栏 notice；确认层在调用前已关闭。
    pub(crate) fn execute_mirror(&mut self, confirm: MirrorConfirm) {
        let Some(Ok(host)) = &self.mirror.host else {
            self.notice = crate::tr!(crate::keys::CONSOLE_MIRROR_DETECT_FAILED);
            return;
        };
        if !crate::mirror::root_allowed() {
            self.notice = crate::tr!(crate::keys::SET_MIRROR_ROOT_REQUIRED);
            return;
        }
        if let MirrorConfirm::Apply { mirror, .. } = confirm
            && status_of(&self.mirror.availability, mirror) == MirrorStatus::Unavailable
        {
            self.notice = crate::tr!(
                crate::keys::SET_MIRROR_MIRROR_UNAVAILABLE,
                mirror = mirror.label()
            );
            return;
        }
        match confirm {
            MirrorConfirm::Apply {
                mirror,
                replace_security,
                disable_cdrom,
                ..
            } => match crate::mirror::apply(host, mirror, replace_security, disable_cdrom) {
                Ok(report) if report.changed_files == 0 => {
                    self.notice = crate::tr!(
                        crate::keys::SET_MIRROR_NO_CHANGE,
                        family = host.family.label(),
                        mirror = mirror.label()
                    );
                }
                Ok(report) => {
                    self.notice = crate::tr!(
                        crate::keys::SET_MIRROR_APPLIED,
                        family = host.family.label(),
                        mirror = mirror.label(),
                        files = report.changed_files
                    );
                    match report.fallback {
                        Some(crate::mirror::Fallback::CdromConverted) => {
                            self.notice = format!(
                                "{}\n{}",
                                self.notice,
                                crate::tr!(crate::keys::SET_MIRROR_CDROM_CONVERTED)
                            );
                        }
                        Some(crate::mirror::Fallback::CdromDisabled) => {
                            self.notice = format!(
                                "{}\n{}",
                                self.notice,
                                crate::tr!(crate::keys::SET_MIRROR_CDROM_DISABLED)
                            );
                        }
                        Some(crate::mirror::Fallback::SourceAdded) => {
                            self.notice = format!(
                                "{}\n{}",
                                self.notice,
                                crate::tr!(
                                    crate::keys::SET_MIRROR_SOURCE_ADDED,
                                    family = host.family.label()
                                )
                            );
                        }
                        None => {}
                    }
                    if report.cdrom_commented > 0 {
                        self.notice = format!(
                            "{}\n{}",
                            self.notice,
                            crate::tr!(
                                crate::keys::SET_MIRROR_CDROM_COMMENTED,
                                count = report.cdrom_commented
                            )
                        );
                    }
                    if report.skipped_repositories > 0 {
                        self.notice = format!(
                            "{}\n{}",
                            self.notice,
                            crate::tr!(
                                crate::keys::SET_MIRROR_SKIPPED,
                                count = report.skipped_repositories
                            )
                        );
                    }
                    if report.unrecognized_lines > 0 {
                        self.notice = format!(
                            "{}\n{}",
                            self.notice,
                            crate::tr!(
                                crate::keys::SET_MIRROR_UNRECOGNIZED_LINES,
                                count = report.unrecognized_lines
                            )
                        );
                    }
                }
                Err(error) => self.notice = error.to_string(),
            },
            MirrorConfirm::Restore => match crate::mirror::restore(host) {
                Ok(()) => {
                    self.notice = crate::tr!(crate::keys::SET_MIRROR_RESTORED);
                }
                Err(error) => self.notice = error.to_string(),
            },
        }
    }

    /// Mirror 面板按键：确认层与行导航。返回 `None` 表示未消费，回落到主流程。
    pub(crate) fn handle_mirror_key(&mut self, key: KeyEvent) -> Option<Option<ConsoleAction>> {
        if let Some(confirm) = self.mirror.confirming {
            match key.code {
                KeyCode::Enter => {
                    self.execute_mirror(confirm);
                    self.mirror.confirming = None;
                    return Some(None);
                }
                KeyCode::Esc => {
                    self.mirror.confirming = None;
                    return Some(None);
                }
                KeyCode::Up
                | KeyCode::Down
                | KeyCode::Char(' ')
                | KeyCode::Left
                | KeyCode::Right => {
                    if let MirrorConfirm::Apply {
                        mirror,
                        replace_security,
                        disable_cdrom,
                        toggle,
                    } = confirm
                    {
                        let rows = self
                            .mirror
                            .host
                            .as_ref()
                            .and_then(|result| result.as_ref().ok())
                            .map(apply_toggle_rows)
                            .unwrap_or_default();
                        match key.code {
                            // ↑/↓ 在可见开关行之间移动焦点。
                            KeyCode::Up => {
                                self.mirror.confirming = Some(MirrorConfirm::Apply {
                                    mirror,
                                    replace_security,
                                    disable_cdrom,
                                    toggle: toggle.saturating_sub(1),
                                });
                            }
                            KeyCode::Down => {
                                self.mirror.confirming = Some(MirrorConfirm::Apply {
                                    mirror,
                                    replace_security,
                                    disable_cdrom,
                                    toggle: (toggle + 1).min(rows.len().saturating_sub(1)),
                                });
                            }
                            // 空格/←/→ 切换焦点所在的开关行。
                            _ => {
                                let row = rows.get(toggle).copied();
                                self.mirror.confirming = Some(MirrorConfirm::Apply {
                                    mirror,
                                    replace_security: replace_security
                                        ^ (row == Some(MirrorToggleRow::Security)),
                                    disable_cdrom: disable_cdrom
                                        ^ (row == Some(MirrorToggleRow::Cdrom)),
                                    toggle,
                                });
                            }
                        }
                    }
                    return Some(None);
                }
                _ => return Some(None),
            }
        }
        match key.code {
            KeyCode::Up => {
                let rows = MirrorRow::rows(&self.mirror.availability);
                let index = rows
                    .iter()
                    .position(|row| *row == self.mirror.selected)
                    .unwrap_or(0);
                self.mirror.selected = rows[index.saturating_sub(1)];
            }
            KeyCode::Down => {
                let rows = MirrorRow::rows(&self.mirror.availability);
                let index = rows
                    .iter()
                    .position(|row| *row == self.mirror.selected)
                    .unwrap_or(0);
                self.mirror.selected = rows[(index + 1).min(rows.len() - 1)];
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                let unavailable = matches!(self.mirror.selected, MirrorRow::Mirror(mirror)
                    if status_of(&self.mirror.availability, mirror) == MirrorStatus::Unavailable);
                if unavailable {
                    self.notice = crate::tr!(
                        crate::keys::SET_MIRROR_MIRROR_UNAVAILABLE,
                        mirror = match self.mirror.selected {
                            MirrorRow::Mirror(mirror) => mirror.label(),
                            _ => unreachable!(),
                        }
                    );
                    return Some(None);
                }
                self.mirror.confirming = Some(match self.mirror.selected {
                    MirrorRow::Restore => MirrorConfirm::Restore,
                    MirrorRow::Mirror(mirror) => MirrorConfirm::Apply {
                        mirror,
                        replace_security: false,
                        disable_cdrom: true,
                        toggle: 0,
                    },
                });
            }
            _ => return None,
        }
        Some(None)
    }
}

/// 面板内容行数：主机行、空行、镜像列表、空行、恢复动作。
fn panel_lines(app: &ConsoleApp) -> Vec<Line<'_>> {
    let mut lines = Vec::new();
    match &app.mirror.host {
        None => {
            lines.push(Line::raw(crate::tr!(crate::keys::CONSOLE_MIRROR_DETECTING)));
        }
        Some(Err(error)) => {
            lines.push(Line::styled(
                crate::tr!(crate::keys::CONSOLE_MIRROR_DETECT_FAILED),
                Style::default().fg(Color::Red),
            ));
            lines.push(Line::styled(
                error.clone(),
                Style::default().fg(Color::DarkGray),
            ));
        }
        Some(Ok(host)) => {
            lines.push(Line::raw(crate::tr!(
                crate::keys::CONSOLE_MIRROR_HOST,
                summary = host.summary(),
                manager = host.family.package_manager()
            )));
            lines.push(Line::raw(""));
            for mirror in MirrorName::all() {
                let status = status_of(&app.mirror.availability, mirror);
                let marker = if app.mirror.selected == MirrorRow::Mirror(mirror) {
                    "> "
                } else {
                    "  "
                };
                let label = mirror.label();
                let (text, style) = match status {
                    MirrorStatus::Unavailable => (
                        format!(
                            "{marker}{label} ({})",
                            crate::tr!(crate::keys::MIRROR_STATUS_UNAVAILABLE)
                        ),
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::CROSSED_OUT),
                    ),
                    MirrorStatus::Unknown => (
                        format!(
                            "{marker}{label} ({})",
                            crate::tr!(crate::keys::MIRROR_STATUS_UNKNOWN)
                        ),
                        Style::default().fg(Color::Yellow).add_modifier(
                            if app.mirror.selected == MirrorRow::Mirror(mirror) {
                                Modifier::BOLD
                            } else {
                                Modifier::empty()
                            },
                        ),
                    ),
                    MirrorStatus::Available => (
                        format!("{marker}{label}"),
                        Style::default().add_modifier(
                            if app.mirror.selected == MirrorRow::Mirror(mirror) {
                                Modifier::BOLD
                            } else {
                                Modifier::empty()
                            },
                        ),
                    ),
                };
                lines.push(Line::from(Span::styled(text, style)));
            }
            lines.push(Line::raw(""));
            let marker = if app.mirror.selected == MirrorRow::Restore {
                "> "
            } else {
                "  "
            };
            lines.push(Line::from(Span::styled(
                format!(
                    "{marker}{}",
                    crate::tr!(crate::keys::CONSOLE_MIRROR_RESTORE_ROW)
                ),
                Style::default().add_modifier(if app.mirror.selected == MirrorRow::Restore {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
            )));
            if app.mirror.probing {
                lines.push(Line::raw(crate::tr!(crate::keys::CONSOLE_MIRROR_PROBING)));
            }
        }
    }
    lines
}

pub(crate) fn render_mirror(frame: &mut Frame<'_>, app: &mut ConsoleApp, area: Rect) {
    let lines = panel_lines(app);
    // 命中区：镜像行（内容行 2..）与恢复动作行（内容行 3 + 镜像数）。
    let row_hits: Vec<(u16, Hit)> = if matches!(&app.mirror.host, Some(Ok(_))) {
        let width = area.width.saturating_sub(2);
        let mut hits = Vec::with_capacity(MirrorName::all().len() + 1);
        for (index, mirror) in MirrorName::all().into_iter().enumerate() {
            hits.push((
                block_row_of(&lines, index + 2, width),
                Hit::MirrorField(mirror),
            ));
        }
        hits.push((
            block_row_of(&lines, MirrorName::all().len() + 3, width),
            Hit::MirrorRestore,
        ));
        hits
    } else {
        Vec::new()
    };
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel_block(
                &crate::tr!(crate::keys::CONSOLE_MIRROR_MENU),
                app.focus == Focus::Panel,
            ))
            .wrap(Wrap { trim: true }),
        area,
    );
    for (row, hit) in row_hits {
        app.hits.block_row(area, row, hit);
    }
}

pub(crate) fn render_mirror_confirmation(frame: &mut Frame<'_>, app: &mut ConsoleApp) {
    let screen = frame.area();
    let width = 60.min(screen.width.saturating_sub(2));
    let height = 10.min(screen.height.saturating_sub(2));
    let area = Rect::new(
        screen.x + screen.width.saturating_sub(width) / 2,
        screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    );
    register_dialog_hits(&mut app.hits, screen, area);
    frame.render_widget(Clear, area);
    // 可见开关行：`(行类型, 渲染行, 是否勾选)`。
    let mut toggle_rows: Vec<(MirrorToggleRow, Line<'static>, bool)> = Vec::new();
    let (title, question) = match &app.mirror.confirming {
        Some(MirrorConfirm::Apply {
            mirror,
            replace_security,
            disable_cdrom,
            toggle,
        }) => {
            let host = app
                .mirror
                .host
                .as_ref()
                .and_then(|result| result.as_ref().ok());
            if let Some(host) = host {
                for (index, row) in apply_toggle_rows(host).into_iter().enumerate() {
                    let (checked, label) = match row {
                        MirrorToggleRow::Cdrom => (
                            *disable_cdrom,
                            crate::tr!(crate::keys::CONSOLE_MIRROR_CDROM_ROW),
                        ),
                        MirrorToggleRow::Security => (
                            *replace_security,
                            crate::tr!(crate::keys::CONSOLE_MIRROR_SECURITY_ROW),
                        ),
                    };
                    let marker = if checked { "[x]" } else { "[ ]" };
                    let mut style = Style::default().fg(if checked {
                        Color::Cyan
                    } else {
                        Color::DarkGray
                    });
                    if *toggle == index {
                        style = style.add_modifier(Modifier::BOLD);
                    }
                    toggle_rows.push((
                        row,
                        Line::styled(format!("{marker} {label}"), style),
                        checked,
                    ));
                }
            }
            (
                crate::tr!(crate::keys::CONSOLE_MIRROR_CONFIRM_APPLY_TITLE),
                crate::tr!(
                    crate::keys::CONSOLE_MIRROR_CONFIRM_APPLY,
                    family = host.map(|host| host.family.label()).unwrap_or_default(),
                    mirror = mirror.label()
                ),
            )
        }
        Some(MirrorConfirm::Restore) => (
            crate::tr!(crate::keys::CONSOLE_MIRROR_CONFIRM_RESTORE_TITLE),
            crate::tr!(crate::keys::CONSOLE_MIRROR_CONFIRM_RESTORE),
        ),
        // 渲染入口（render.rs）已保证 confirming 非 None。
        None => unreachable!(),
    };
    let mut lines = vec![
        Line::styled(question, Style::default().add_modifier(Modifier::BOLD)),
        Line::raw(""),
    ];
    // 探测失败（未知）的镜像：确认层加一行警告，提示换源可能失败。
    if let Some(MirrorConfirm::Apply { mirror, .. }) = app.mirror.confirming
        && status_of(&app.mirror.availability, mirror) == MirrorStatus::Unknown
    {
        lines.push(Line::styled(
            crate::tr!(crate::keys::CONSOLE_MIRROR_CONFIRM_UNKNOWN_WARNING),
            Style::default().fg(Color::Yellow),
        ));
        lines.push(Line::raw(""));
    }
    // 开关行与命中区：点击某行先把焦点移过去再切换。
    let toggle_hits: Vec<(MirrorToggleRow, usize)> = if !toggle_rows.is_empty() {
        let mut hits = Vec::with_capacity(toggle_rows.len());
        for (row, line, _) in &toggle_rows {
            lines.push(line.clone());
            hits.push((*row, lines.len() - 1));
        }
        lines.push(Line::raw(""));
        hits
    } else {
        Vec::new()
    };
    let content_width = area.width.saturating_sub(2);
    for (row, line_index) in toggle_hits {
        let hit_row = block_row_of(&lines, line_index, content_width);
        let hit = match row {
            MirrorToggleRow::Cdrom => Hit::MirrorCdromToggle,
            MirrorToggleRow::Security => Hit::MirrorSecurityToggle,
        };
        app.hits.block_row(area, hit_row, hit);
    }
    lines.push(Line::styled(
        crate::tr!(crate::keys::CONSOLE_MIRROR_CONFIRM_ENTER),
        Style::default().fg(Color::Green),
    ));
    lines.push(Line::styled(
        crate::tr!(crate::keys::CONSOLE_MIRROR_CONFIRM_ESC),
        Style::default().fg(Color::DarkGray),
    ));
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .block(Block::bordered().title(title)),
        area,
    );
}
