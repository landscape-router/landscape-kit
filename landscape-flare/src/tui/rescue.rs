//! Transient device picker and credential prompt. A new handshake always returns here.
use super::{DefaultTerminal, FormState, Outcome, OwnedConfig, Phase, spawn_client};
use crate::{client::rescue, tr};
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use landscape_terrain_proto::{cli::parse_ethertype, protocol::v6};
use ratatui::widgets::{Block, Paragraph, Wrap};
use std::time::Duration;
use zeroize::Zeroizing;

pub(super) type Login = (rescue::Connection, u8, Zeroizing<String>);
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn draw(term: &mut DefaultTerminal, lines: &[String]) -> Result<()> {
    term.draw(|frame| {
        frame.render_widget(
            Paragraph::new(lines.join("\n"))
                .wrap(Wrap { trim: false })
                .block(Block::bordered().title(tr!("tui.title"))),
            frame.area(),
        );
    })?;
    Ok(())
}

fn key() -> Result<event::KeyEvent> {
    loop {
        if event::poll(Duration::from_millis(100))?
            && let Event::Key(k) = event::read()?
            && k.kind != event::KeyEventKind::Release
        {
            if k.modifiers.contains(KeyModifiers::CONTROL)
                && matches!(k.code, KeyCode::Char('c' | 'q'))
            {
                return Err(std::io::Error::from(std::io::ErrorKind::Interrupted).into());
            }
            return Ok(k);
        }
    }
}

fn prompt(
    term: &mut DefaultTerminal,
    label: String,
    mut text: Zeroizing<String>,
    secret: bool,
) -> Result<Option<Zeroizing<String>>> {
    loop {
        let display = if secret {
            "*".repeat(text.chars().count())
        } else {
            text.to_string()
        };
        draw(term, &[label.clone(), display, tr!("tui.rescue_accept")])?;
        let k = key()?;
        match k.code {
            KeyCode::Esc => return Ok(None),
            KeyCode::Enter => return Ok(Some(text)),
            KeyCode::Backspace => {
                text.pop();
            }
            KeyCode::Char(ch)
                if !ch.is_control()
                    && text.len() + ch.len_utf8() <= 8192
                    && (k.modifiers.is_empty() || k.modifiers == KeyModifiers::SHIFT) =>
            {
                text.push(ch)
            }
            _ => (),
        }
    }
}

fn confirm_device(term: &mut DefaultTerminal, device: &rescue::Device) -> Result<bool> {
    const WIDTH: u16 = 60;
    const HEIGHT: u16 = 24;
    loop {
        let area = term.size()?;
        let fits = area.width >= WIDTH && area.height >= HEIGHT;
        if fits {
            draw(
                term,
                &[
                    device.name.clone(),
                    device.fingerprint.clone(),
                    tr!("tui.rescue_confirm"),
                ],
            )?;
        } else {
            draw(
                term,
                &[tr!(
                    "tui.terminal_too_small",
                    width = WIDTH,
                    height = HEIGHT
                )],
            )?;
        }
        match key()?.code {
            KeyCode::Enter if fits => return Ok(true),
            KeyCode::Esc => return Ok(false),
            _ => (),
        }
    }
}

/// Called at startup and after every completed or failed v6 connection.
pub(super) async fn choose(term: &mut DefaultTerminal, mut notice: String) -> Result<Phase> {
    let mut form = FormState::new();
    let mut index = 0usize;
    let mut needs_scan = true;
    let mut devices = Vec::new();
    loop {
        let devs: Vec<_> = (!form.device.is_empty())
            .then(|| form.device.clone())
            .into_iter()
            .collect();
        let eth = parse_ethertype(&form.ethertype)?;
        let secret = (!form.psk.is_empty()).then_some(form.psk.as_str());
        if needs_scan {
            draw(term, &[tr!("tui.rescue_scan")])?;
            let gate = v6::Discovery::new(secret);
            match rescue::discover(&devs, eth, None, &gate, &form.token).await {
                Ok(found) => devices = found,
                Err(e) => {
                    devices.clear();
                    notice = e.to_string();
                }
            }
            index = 0;
            needs_scan = false;
        }
        let mut lines = vec![
            tr!("tui.rescue_devices"),
            form.device_label(),
            String::new(),
        ];
        let rows = usize::from(term.size()?.height.saturating_sub(11)).max(1);
        for (i, d) in devices
            .iter()
            .enumerate()
            .skip(index.saturating_sub(rows - 1))
            .take(rows)
        {
            lines.push(format!(
                "{} {}  {}",
                if i == index { ">" } else { " " },
                d.name,
                &d.fingerprint[..16]
            ));
        }
        if devices.is_empty() {
            lines.push(tr!("tui.rescue_empty"));
        }
        lines.extend([String::new(), notice.clone(), tr!("tui.rescue_keys")]);
        draw(term, &lines)?;
        let k = key()?;
        match k.code {
            KeyCode::Esc => return Ok(Phase::Done(Outcome::FormQuit)),
            KeyCode::F(3) => crate::i18n::toggle(),
            KeyCode::F(5) => return Ok(Phase::Form(form)),
            KeyCode::Up => index = index.saturating_sub(1),
            KeyCode::Down => index = (index + 1).min(devices.len().saturating_sub(1)),
            KeyCode::Char('r') => needs_scan = true,
            KeyCode::Char('i') => {
                form.device_index = (form.device_index + 1) % (form.devices.len() + 1);
                form.device = form
                    .device_index
                    .checked_sub(1)
                    .and_then(|i| form.devices.get(i))
                    .map(|d| d.name.clone())
                    .unwrap_or_default();
                needs_scan = true;
            }
            KeyCode::Char('p') => {
                if let Some(value) = prompt(
                    term,
                    tr!("tui.rescue_psk"),
                    Zeroizing::new(String::new()),
                    true,
                )? {
                    form.psk = value.to_string();
                    needs_scan = true;
                }
            }
            KeyCode::Char('t') => {
                if let Some(value) = prompt(
                    term,
                    tr!("tui.token"),
                    Zeroizing::new(form.token.clone()),
                    true,
                )? {
                    form.token = value.to_string();
                    needs_scan = true;
                }
            }
            KeyCode::Char('e') => {
                if let Some(value) = prompt(
                    term,
                    tr!("tui.ethertype"),
                    Zeroizing::new(form.ethertype.clone()),
                    false,
                )? {
                    if parse_ethertype(&value).is_ok() {
                        form.ethertype = value.to_string();
                        needs_scan = true;
                    } else {
                        notice = tr!("tui.invalid_ethertype", error = value.as_str());
                    }
                }
            }
            KeyCode::Enter if !devices.is_empty() => {
                let device = devices[index].clone();
                let gate = v6::Discovery::new(secret);
                let connection =
                    match rescue::connect(&devs, eth, None, &gate, device.clone()).await {
                        Ok(c) => c,
                        Err(e) => {
                            notice = e.to_string();
                            continue;
                        }
                    };
                if !confirm_device(term, &device)? {
                    continue;
                }
                draw(term, &[tr!("tui.rescue_method")])?;
                let (method, user) = match key()?.code {
                    KeyCode::Char('1') => (
                        v6::AUTH_LANDSCAPE,
                        match prompt(term, tr!("tui.user"), Zeroizing::new("admin".into()), false)?
                        {
                            Some(s) => s.to_string(),
                            None => continue,
                        },
                    ),
                    KeyCode::Char('2') => (v6::AUTH_ROOT, "root".into()),
                    _ => continue,
                };
                let Some(password) = prompt(
                    term,
                    tr!("tui.rescue_password"),
                    Zeroizing::new(String::new()),
                    true,
                )?
                else {
                    continue;
                };
                let cfg = OwnedConfig {
                    devs,
                    user,
                    ethertype: eth,
                    token: form.token,
                    client_name: form.client_name,
                    psk: String::new(),
                };
                return Ok(Phase::Dash(spawn_client(
                    cfg,
                    Some((connection, method, password)),
                )));
            }
            _ => (),
        }
    }
}
