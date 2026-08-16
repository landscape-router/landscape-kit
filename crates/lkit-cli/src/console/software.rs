use std::sync::mpsc::{self, Receiver, TryRecvError};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Gauge, Paragraph, Wrap};

use crossterm::event::{KeyCode, KeyEvent};

use super::render::{panel_block, register_dialog_hits, register_modal_hits};
use super::widgets::{Focus, Hit, block_row_of};
use super::{ConsoleAction, ConsoleApp};
use crate::mirror::Host;
use crate::software::{DockerSource, InstallPhase, Software};

/// 软件面板的确认层目标：软件与其安装来源。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SoftwareConfirm {
    pub(crate) software: Software,
    pub(crate) source: DockerSource,
}

/// 后台安装 worker 的消息：阶段进度与最终结果。
pub(crate) enum SoftwareInstallMessage {
    Phase(InstallPhase),
    Done(Result<(), String>),
}

/// 在 TUI 内执行软件安装：worker 线程跑完整安装流程并通过 channel 回传进度。
pub(crate) struct SoftwareInstallRun {
    pub(crate) receiver: Receiver<SoftwareInstallMessage>,
    pub(crate) phase: InstallPhase,
    pub(crate) software: Software,
}

/// 软件面板：显示发行版检测结果与软件列表（含安装状态），
/// 选择未安装的软件后通过确认层选择来源并后台安装。
pub(crate) struct SoftwarePanel {
    pub(crate) host: Option<Result<Host, String>>,
    pub(crate) detected: bool,
    /// 与 `Software::all()` 对齐的安装状态。
    pub(crate) installed: Vec<bool>,
    pub(crate) selected: Option<Software>,
    pub(crate) confirming: Option<SoftwareConfirm>,
    pub(crate) install: Option<SoftwareInstallRun>,
}

impl Default for SoftwarePanel {
    fn default() -> Self {
        Self {
            host: None,
            detected: false,
            installed: Software::all().into_iter().map(|_| false).collect(),
            selected: Software::all().first().copied(),
            confirming: None,
            install: None,
        }
    }
}

impl SoftwarePanel {
    /// 进入面板时执行一次发行版检测与软件状态检测（只读，快速）。
    pub(crate) fn ensure_detected(&mut self) {
        if self.detected {
            return;
        }
        self.detected = true;
        self.host = Some(crate::mirror::detect_host().map_err(|error| error.to_string()));
        self.refresh_status();
    }

    fn refresh_status(&mut self) {
        self.installed = Software::all()
            .into_iter()
            .map(|software| software.installed())
            .collect();
    }

    /// 在后台线程执行安装，进度经 channel 回传；结束后由 `poll` 刷新状态并显示结果。
    /// 前置条件不满足（非 root、检测失败）时返回底栏提示。
    pub(crate) fn start_install(&mut self, confirm: SoftwareConfirm) -> Result<(), String> {
        if self.install.is_some() {
            return Ok(());
        }
        let Some(Ok(host)) = &self.host else {
            return Err(crate::tr!(crate::keys::CONSOLE_SOFTWARE_DETECT_FAILED));
        };
        if !crate::software::root_allowed() {
            return Err(crate::tr!(crate::keys::SOFTWARE_ROOT_REQUIRED));
        }
        let host = host.clone();
        let (sender, receiver) = mpsc::channel();
        let language = crate::i18n::current();
        std::thread::spawn(move || {
            let result = crate::i18n::with_language(language, || {
                let phase_sender = sender.clone();
                crate::software::install(
                    &host,
                    confirm.software,
                    confirm.source,
                    false,
                    &mut |phase| {
                        let _ = phase_sender.send(SoftwareInstallMessage::Phase(phase));
                    },
                )
                .map_err(|error| error.to_string())
            });
            let _ = sender.send(SoftwareInstallMessage::Done(result));
        });
        self.install = Some(SoftwareInstallRun {
            receiver,
            phase: InstallPhase::Preparing,
            software: confirm.software,
        });
        Ok(())
    }

    pub(crate) fn poll(&mut self, notice: &mut String) {
        while let Some(run) = &self.install {
            let message = run.receiver.try_recv();
            match message {
                Ok(SoftwareInstallMessage::Phase(phase)) => {
                    if let Some(run) = &mut self.install {
                        run.phase = phase;
                    }
                }
                Ok(SoftwareInstallMessage::Done(result)) => {
                    self.install = None;
                    match result {
                        Ok(()) => {
                            self.refresh_status();
                            *notice = crate::tr!(crate::keys::CONSOLE_SOFTWARE_INSTALLED);
                        }
                        Err(error) => *notice = error,
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.install = None;
                    *notice = crate::tr!(crate::keys::CONSOLE_SOFTWARE_WORKER_STOPPED);
                }
            }
        }
    }
}

impl ConsoleApp {
    /// Software 面板按键：确认层（来源切换）与行导航。返回 `None` 表示未消费。
    pub(crate) fn handle_software_key(&mut self, key: KeyEvent) -> Option<Option<ConsoleAction>> {
        if let Some(confirm) = self.software.confirming {
            match key.code {
                KeyCode::Enter => {
                    match self.software.start_install(confirm) {
                        Ok(()) => {
                            self.notice = crate::tr!(
                                crate::keys::CONSOLE_SOFTWARE_INSTALLING,
                                software = confirm.software.label()
                            );
                        }
                        Err(message) => self.notice = message,
                    }
                    self.software.confirming = None;
                    return Some(None);
                }
                KeyCode::Esc => {
                    self.software.confirming = None;
                    return Some(None);
                }
                KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right => {
                    self.software.confirming = Some(SoftwareConfirm {
                        software: confirm.software,
                        source: cycle_source(confirm.source, key.code),
                    });
                    return Some(None);
                }
                _ => return Some(None),
            }
        }
        match key.code {
            KeyCode::Up => {
                let all = Software::all();
                let index = self
                    .software
                    .selected
                    .and_then(|software| all.iter().position(|entry| *entry == software))
                    .unwrap_or(0);
                self.software.selected = Some(all[index.saturating_sub(1)]);
            }
            KeyCode::Down => {
                let all = Software::all();
                let index = self
                    .software
                    .selected
                    .and_then(|software| all.iter().position(|entry| *entry == software))
                    .unwrap_or(0);
                self.software.selected = Some(all[(index + 1).min(all.len() - 1)]);
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                let software = self.software.selected?;
                let index = Software::all()
                    .iter()
                    .position(|entry| *entry == software)
                    .unwrap_or(0);
                if self.software.installed[index] {
                    self.notice = crate::tr!(
                        crate::keys::SOFTWARE_ALREADY_INSTALLED,
                        software = software.label()
                    );
                    return Some(None);
                }
                self.software.confirming = Some(SoftwareConfirm {
                    software,
                    source: DockerSource::Official,
                });
                return Some(None);
            }
            _ => return None,
        }
        Some(None)
    }
}

/// 确认层内循环切换来源：Space/Right 前进，Left 后退。
fn cycle_source(current: DockerSource, code: KeyCode) -> DockerSource {
    let all = DockerSource::all();
    let index = all
        .iter()
        .position(|source| *source == current)
        .unwrap_or(0);
    if code == KeyCode::Left {
        all[(index + all.len() - 1) % all.len()]
    } else {
        all[(index + 1) % all.len()]
    }
}

/// 面板内容行数：主机行、空行、软件行。
fn panel_lines(app: &ConsoleApp) -> Vec<Line<'_>> {
    let mut lines = Vec::new();
    match &app.software.host {
        None => {
            lines.push(Line::raw(crate::tr!(
                crate::keys::CONSOLE_SOFTWARE_DETECTING
            )));
        }
        Some(Err(error)) => {
            lines.push(Line::styled(
                crate::tr!(crate::keys::CONSOLE_SOFTWARE_DETECT_FAILED),
                Style::default().fg(Color::Red),
            ));
            lines.push(Line::styled(
                error.clone(),
                Style::default().fg(Color::DarkGray),
            ));
        }
        Some(Ok(host)) => {
            lines.push(Line::raw(crate::tr!(
                crate::keys::CONSOLE_SOFTWARE_HOST,
                summary = host.summary(),
                manager = host.family.package_manager()
            )));
            lines.push(Line::raw(""));
            for (index, software) in Software::all().into_iter().enumerate() {
                let selected = app.software.selected == Some(software);
                let marker = if selected { "> " } else { "  " };
                let installed = app.software.installed.get(index).copied().unwrap_or(false);
                let status = if installed {
                    crate::tr!(crate::keys::SOFTWARE_INSTALLED)
                } else {
                    crate::tr!(crate::keys::SOFTWARE_NOT_INSTALLED)
                };
                let line = Line::from(vec![
                    Span::styled(marker, Style::default().add_modifier(Modifier::BOLD)),
                    Span::styled(
                        software.label(),
                        Style::default().add_modifier(if selected {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        }),
                    ),
                    Span::raw("  ["),
                    Span::styled(
                        status,
                        Style::default().fg(if installed {
                            Color::Green
                        } else {
                            Color::DarkGray
                        }),
                    ),
                    Span::raw("]"),
                ]);
                lines.push(line);
            }
        }
    }
    lines
}

pub(crate) fn render_software(frame: &mut Frame<'_>, app: &mut ConsoleApp, area: Rect) {
    let lines = panel_lines(app);
    let row_hits: Vec<(u16, Hit)> = if matches!(&app.software.host, Some(Ok(_))) {
        let width = area.width.saturating_sub(2);
        let mut hits = Vec::with_capacity(Software::all().len());
        for (index, software) in Software::all().into_iter().enumerate() {
            hits.push((
                block_row_of(&lines, index + 2, width),
                Hit::SoftwareField(software),
            ));
        }
        hits
    } else {
        Vec::new()
    };
    frame.render_widget(
        Paragraph::new(lines)
            .block(panel_block(
                &crate::tr!(crate::keys::CONSOLE_SOFTWARE_MENU),
                app.focus == Focus::Panel,
            ))
            .wrap(Wrap { trim: true }),
        area,
    );
    for (row, hit) in row_hits {
        app.hits.block_row(area, row, hit);
    }
}

pub(crate) fn render_software_confirmation(frame: &mut Frame<'_>, app: &mut ConsoleApp) {
    let screen = frame.area();
    let width = 64.min(screen.width.saturating_sub(2));
    let height = 10.min(screen.height.saturating_sub(2));
    let area = Rect::new(
        screen.x + screen.width.saturating_sub(width) / 2,
        screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    );
    register_dialog_hits(&mut app.hits, screen, area);
    frame.render_widget(Clear, area);
    let Some(confirm) = app.software.confirming else {
        return;
    };
    let mut lines = vec![
        Line::styled(
            crate::tr!(
                crate::keys::CONSOLE_SOFTWARE_CONFIRM_QUESTION,
                software = confirm.software.label()
            ),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                crate::tr!(crate::keys::CONSOLE_SOFTWARE_SOURCE_ROW),
                Style::default().fg(Color::DarkGray),
            ),
            Span::raw("  "),
            Span::styled(
                format!("◀ {} ▶", confirm.source.label()),
                Style::default()
                    .bg(Color::Blue)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
        ]),
        Line::styled(
            crate::tr!(crate::keys::CONSOLE_SOFTWARE_CONFIRM_SWITCH),
            Style::default().fg(Color::Yellow),
        ),
        Line::raw(""),
    ];
    // 来源行命中区：点击切换来源。
    let content_width = area.width.saturating_sub(2);
    let hit_row = block_row_of(&lines, 2, content_width);
    app.hits.block_row(area, hit_row, Hit::SoftwareSourceToggle);
    lines.push(Line::styled(
        crate::tr!(crate::keys::CONSOLE_SOFTWARE_CONFIRM_ENTER),
        Style::default().fg(Color::Green),
    ));
    lines.push(Line::styled(
        crate::tr!(crate::keys::CONSOLE_SOFTWARE_CONFIRM_ESC),
        Style::default().fg(Color::DarkGray),
    ));
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .block(Block::bordered().title(crate::tr!(
                crate::keys::CONSOLE_SOFTWARE_CONFIRM_TITLE,
                software = confirm.software.label()
            ))),
        area,
    );
}

pub(crate) fn render_software_progress(frame: &mut Frame<'_>, app: &mut ConsoleApp) {
    let Some(run) = &app.software.install else {
        return;
    };
    let screen = frame.area();
    let width = 64.min(screen.width.saturating_sub(2));
    let height = 8.min(screen.height.saturating_sub(2));
    let area = Rect::new(
        screen.x + screen.width.saturating_sub(width) / 2,
        screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    );
    register_modal_hits(&mut app.hits, screen, area);
    frame.render_widget(Clear, area);
    let phase_text = match run.phase {
        InstallPhase::Preparing => crate::tr!(crate::keys::CONSOLE_SOFTWARE_PHASE_PREPARING),
        InstallPhase::InstallingPackages => {
            crate::tr!(crate::keys::CONSOLE_SOFTWARE_PHASE_PACKAGES)
        }
        InstallPhase::StartingService => crate::tr!(crate::keys::CONSOLE_SOFTWARE_PHASE_SERVICE),
    };
    let software_label = run.software.label();
    frame.render_widget(
        Paragraph::new(vec![
            Line::raw(crate::tr!(
                crate::keys::CONSOLE_SOFTWARE_INSTALLING,
                software = software_label
            )),
            Line::raw(""),
            Line::raw(&phase_text),
        ])
        .wrap(Wrap { trim: true })
        .block(Block::bordered().title(crate::tr!(
            crate::keys::CONSOLE_SOFTWARE_CONFIRM_TITLE,
            software = software_label
        ))),
        Rect::new(area.x, area.y, area.width, area.height.saturating_sub(2)),
    );
    let gauge_area = Rect::new(
        area.x.saturating_add(2),
        area.y.saturating_add(area.height.saturating_sub(2)),
        area.width.saturating_sub(4),
        1,
    );
    frame.render_widget(
        Gauge::default()
            .ratio(f64::from(run.phase.step()) / f64::from(InstallPhase::STEPS))
            .label(phase_text),
        gauge_area,
    );
}
