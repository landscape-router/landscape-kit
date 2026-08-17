//! Overview 面板的 flare 恢复通道弹窗:按 `f` 弹出,查看并修改 daemon 托管的
//! `[flare]` 配置(psk 等),写回 `config.toml` 后由 daemon 周期拾取。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, Paragraph, Wrap};

use super::ConsoleApp;
use super::render::register_modal_hits;
use crate::deployment::config::FLARE_PSK_MIN_LENGTH;

#[derive(Default)]
pub(super) struct FlareDialog {
    pub(super) open: bool,
    /// 当前 `[flare]` 段的 psk(编辑中为明文,渲染时掩码)。
    pub(super) psk: String,
    pub(super) editing: bool,
    /// 保存失败或校验失败的提示,显示在弹窗底部。
    pub(super) notice: String,
}

impl ConsoleApp {
    /// 打开 flare 弹窗:载入当前 `[flare]` 段的 psk。
    pub(super) fn open_flare_dialog(&mut self) {
        let section = crate::deployment::config::load_flare();
        self.flare.psk = section.and_then(|section| section.psk).unwrap_or_default();
        self.flare.editing = false;
        self.flare.notice.clear();
        self.flare.open = true;
    }

    /// 保存弹窗内的修改:校验长度后写回 `config.toml` 的 `[flare]` 段,
    /// 保留其它字段(设备、token 等);daemon 在下一周期拾取新配置。
    pub(super) fn save_flare_dialog(&mut self) {
        let psk = self.flare.psk.trim().to_string();
        if psk.is_empty() {
            self.flare.notice = crate::tr!(crate::keys::CONSOLE_FLARE_PSK_REQUIRED);
            return;
        }
        if psk.len() < FLARE_PSK_MIN_LENGTH {
            self.flare.notice = crate::tr!(crate::keys::CONSOLE_FLARE_PSK_TOO_SHORT);
            return;
        }
        let mut section = crate::deployment::config::load_flare()
            .unwrap_or_else(crate::deployment::config::default_flare_section);
        section.psk = Some(psk);
        match crate::deployment::config::save_flare(&section) {
            Ok(()) => {
                self.flare.open = false;
                self.notice = crate::tr!(crate::keys::CONSOLE_FLARE_SAVED);
            }
            Err(error) => {
                self.flare.notice =
                    crate::tr!(crate::keys::CONSOLE_FLARE_SAVE_FAILED, error = error);
            }
        }
    }
}

pub(crate) fn render_flare_dialog(frame: &mut Frame<'_>, app: &mut ConsoleApp) {
    if !app.flare.open {
        return;
    }
    let screen = frame.area();
    let width = 64.min(screen.width.saturating_sub(2));
    let height = 13.min(screen.height.saturating_sub(2));
    let area = Rect::new(
        screen.x + screen.width.saturating_sub(width) / 2,
        screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    );
    register_modal_hits(&mut app.hits, screen, area);
    frame.render_widget(Clear, area);
    let section = crate::deployment::config::load_flare();
    let devices = section
        .as_ref()
        .and_then(|section| section.devices.clone())
        .unwrap_or_else(|| "any".into());
    let ethertype = section.as_ref().map(|s| s.ethertype).unwrap_or(0x88B6);
    let forward_ports = section
        .as_ref()
        .map(|s| s.forward_ports.clone())
        .unwrap_or_else(|| "22,6443".into());
    let token = section
        .as_ref()
        .and_then(|s| s.token.clone())
        .unwrap_or_else(|| crate::tr!(crate::keys::CONSOLE_FLARE_TOKEN_UNSET));
    let psk_display = if app.flare.editing {
        app.flare.psk.clone()
    } else if app.flare.psk.is_empty() {
        "<not configured>".into()
    } else {
        super::render::mask(&app.flare.psk)
    };
    let cursor = if app.flare.editing { "_" } else { "" };
    let lines = vec![
        Line::styled(
            crate::tr!(crate::keys::CONSOLE_FLARE_DIALOG_PURPOSE),
            Style::default().fg(Color::DarkGray),
        ),
        Line::raw(""),
        Line::from(vec![
            super::render::display_pad(&crate::tr!(crate::keys::CONSOLE_FLARE_DEVICES_LABEL), 17)
                .into(),
            devices.clone().into(),
        ]),
        Line::from(vec![
            super::render::display_pad(&crate::tr!(crate::keys::CONSOLE_FLARE_ETHERTYPE_LABEL), 17)
                .into(),
            format!("0x{ethertype:04x}").into(),
        ]),
        Line::from(vec![
            super::render::display_pad(
                &crate::tr!(crate::keys::CONSOLE_FLARE_FORWARD_PORTS_LABEL),
                17,
            )
            .into(),
            forward_ports.clone().into(),
        ]),
        Line::from(vec![
            super::render::display_pad(&crate::tr!(crate::keys::CONSOLE_FLARE_TOKEN_LABEL), 17)
                .into(),
            token.into(),
        ]),
        Line::from(vec![
            super::render::display_pad(&crate::tr!(crate::keys::CONSOLE_FLARE_PSK_LABEL), 17)
                .into(),
            psk_display.into(),
            cursor.into(),
        ]),
        Line::raw(""),
        if app.flare.notice.is_empty() {
            Line::styled(
                crate::tr!(crate::keys::CONSOLE_FLARE_DIALOG_HINT),
                Style::default().fg(Color::Green),
            )
        } else {
            Line::styled(app.flare.notice.clone(), Style::default().fg(Color::Yellow))
        },
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .block(Block::bordered().title(crate::tr!(crate::keys::CONSOLE_FLARE_DIALOG_TITLE))),
        area,
    );
}
