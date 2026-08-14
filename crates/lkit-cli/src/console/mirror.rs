use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};

use crossterm::event::{KeyCode, KeyEvent};

use super::render::{panel_block, register_dialog_hits};
use super::widgets::{Focus, Hit, block_row_of};
use super::{ConsoleAction, ConsoleApp};
use crate::mirror::{Host, MirrorName};

/// 换源面板的确认层目标。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MirrorConfirm {
    Apply {
        mirror: MirrorName,
        /// 是否同时替换 Debian 独立 security 仓库（默认不替换）。
        replace_security: bool,
    },
    Restore,
}

/// 换源面板：显示发行版检测结果，选择镜像或恢复备份。
/// 行布局：0..4 为四个镜像（TUNA/阿里云/USTC/官方），4 为恢复备份动作。
pub(crate) struct MirrorPanel {
    pub(crate) host: Option<Result<Host, String>>,
    pub(crate) detected: bool,
    pub(crate) selected: usize,
    pub(crate) confirming: Option<MirrorConfirm>,
}

impl Default for MirrorPanel {
    fn default() -> Self {
        Self {
            host: None,
            detected: false,
            selected: 0,
            confirming: None,
        }
    }
}

const RESTORE_ROW: usize = 4;

impl MirrorPanel {
    /// 进入面板时执行一次发行版检测（只读，快速）。
    pub(crate) fn ensure_detected(&mut self) {
        if self.detected {
            return;
        }
        self.detected = true;
        self.host = Some(crate::mirror::detect_host().map_err(|error| error.to_string()));
    }
}

impl ConsoleApp {
    /// 执行换源或恢复。失败与成功都写入底栏 notice；确认层在调用前已关闭。
    pub(crate) fn execute_mirror(&mut self, confirm: MirrorConfirm) {
        let Some(Ok(host)) = &self.mirror.host else {
            self.notice = crate::tr!(crate::keys::CONSOLE_MIRROR_DETECT_FAILED).into();
            return;
        };
        if !crate::mirror::root_allowed() {
            self.notice = crate::tr!(crate::keys::SET_MIRROR_ROOT_REQUIRED).into();
            return;
        }
        match confirm {
            MirrorConfirm::Apply {
                mirror,
                replace_security,
            } => match crate::mirror::apply(host, mirror, replace_security) {
                Ok(report) if report.changed_files == 0 => {
                    self.notice = crate::tr!(
                        crate::keys::SET_MIRROR_NO_CHANGE,
                        family = host.family.label(),
                        mirror = mirror.label()
                    )
                    .into();
                }
                Ok(report) => {
                    self.notice = crate::tr!(
                        crate::keys::SET_MIRROR_APPLIED,
                        family = host.family.label(),
                        mirror = mirror.label(),
                        files = report.changed_files
                    )
                    .into();
                    match report.fallback {
                        Some(crate::mirror::Fallback::CdromConverted) => {
                            self.notice = format!(
                                "{}\n{}",
                                self.notice,
                                crate::tr!(crate::keys::SET_MIRROR_CDROM_CONVERTED)
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
                    self.notice = crate::tr!(crate::keys::SET_MIRROR_RESTORED).into();
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
                KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right => {
                    // 开关行：仅换源确认层且主机为 Debian 家族时可切换。
                    let is_debian = matches!(
                        &self.mirror.host,
                        Some(Ok(host)) if host.family == crate::mirror::Family::Debian
                    );
                    if let MirrorConfirm::Apply {
                        mirror,
                        replace_security,
                    } = confirm
                        && is_debian
                    {
                        self.mirror.confirming = Some(MirrorConfirm::Apply {
                            mirror,
                            replace_security: !replace_security,
                        });
                    }
                    return Some(None);
                }
                _ => return Some(None),
            }
        }
        match key.code {
            KeyCode::Up => {
                self.mirror.selected = self.mirror.selected.saturating_sub(1);
            }
            KeyCode::Down => {
                self.mirror.selected = self.mirror.selected.min(RESTORE_ROW - 1) + 1;
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                self.mirror.confirming = Some(if self.mirror.selected == RESTORE_ROW {
                    MirrorConfirm::Restore
                } else {
                    MirrorConfirm::Apply {
                        mirror: MirrorName::all()[self.mirror.selected],
                        replace_security: false,
                    }
                });
            }
            _ => return None,
        }
        Some(None)
    }
}

/// 面板内容行数：主机行、空行、4 个镜像、空行、恢复动作。
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
            let selected = app.mirror.selected;
            for (index, mirror) in MirrorName::all().into_iter().enumerate() {
                let marker = if index == selected { "> " } else { "  " };
                lines.push(Line::from(Span::styled(
                    format!("{marker}{}", mirror.label()),
                    Style::default().add_modifier(if index == selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
                )));
            }
            lines.push(Line::raw(""));
            let marker = if selected == RESTORE_ROW { "> " } else { "  " };
            lines.push(Line::from(Span::styled(
                format!(
                    "{marker}{}",
                    crate::tr!(crate::keys::CONSOLE_MIRROR_RESTORE_ROW)
                ),
                Style::default().add_modifier(if selected == RESTORE_ROW {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
            )));
        }
    }
    lines
}

pub(crate) fn render_mirror(frame: &mut Frame<'_>, app: &mut ConsoleApp, area: Rect) {
    let lines = panel_lines(app);
    // 命中区：镜像行（内容行 2..6）与恢复动作行（内容行 7）。
    let row_hits: Vec<(u16, Hit)> = if matches!(&app.mirror.host, Some(Ok(_))) {
        let width = area.width.saturating_sub(2);
        let mut hits = Vec::with_capacity(5);
        for index in 0..4 {
            hits.push((
                block_row_of(&lines, index + 2, width),
                Hit::MirrorField(index),
            ));
        }
        hits.push((block_row_of(&lines, 7, width), Hit::MirrorRestore));
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
    let height = 9.min(screen.height.saturating_sub(2));
    let area = Rect::new(
        screen.x + screen.width.saturating_sub(width) / 2,
        screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    );
    register_dialog_hits(&mut app.hits, screen, area);
    frame.render_widget(Clear, area);
    let (title, question, security_row) = match &app.mirror.confirming {
        Some(MirrorConfirm::Apply {
            mirror,
            replace_security,
        }) => {
            let host = app
                .mirror
                .host
                .as_ref()
                .and_then(|result| result.as_ref().ok());
            let security_row =
                if host.is_some_and(|host| host.family == crate::mirror::Family::Debian) {
                    let marker = if *replace_security { "[x]" } else { "[ ]" };
                    Some(Line::styled(
                        format!(
                            "{} {}",
                            marker,
                            crate::tr!(crate::keys::CONSOLE_MIRROR_SECURITY_ROW)
                        ),
                        Style::default().fg(if *replace_security {
                            Color::Cyan
                        } else {
                            Color::DarkGray
                        }),
                    ))
                } else {
                    None
                };
            (
                crate::tr!(crate::keys::CONSOLE_MIRROR_CONFIRM_APPLY_TITLE),
                crate::tr!(
                    crate::keys::CONSOLE_MIRROR_CONFIRM_APPLY,
                    family = host.map(|host| host.family.label()).unwrap_or_default(),
                    mirror = mirror.label()
                ),
                security_row,
            )
        }
        Some(MirrorConfirm::Restore) => (
            crate::tr!(crate::keys::CONSOLE_MIRROR_CONFIRM_RESTORE_TITLE),
            crate::tr!(crate::keys::CONSOLE_MIRROR_CONFIRM_RESTORE),
            None,
        ),
        // 渲染入口（render.rs）已保证 confirming 非 None。
        None => unreachable!(),
    };
    let mut lines = vec![
        Line::styled(question, Style::default().add_modifier(Modifier::BOLD)),
        Line::raw(""),
    ];
    if let Some(security_row) = security_row {
        lines.push(security_row);
        lines.push(Line::raw(""));
        // 开关行命中区：点击切换 security 替换。
        let row = lines.len() - 2;
        let content_width = area.width.saturating_sub(2);
        let hit_row = block_row_of(&lines, row, content_width);
        app.hits.block_row(area, hit_row, Hit::MirrorSecurityToggle);
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
