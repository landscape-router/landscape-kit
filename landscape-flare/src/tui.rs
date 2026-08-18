//! Interactive TUI: a connection form followed by a
//! live session dashboard. This is the default entry point of `lflare`; the
//! `cli` subcommand offers the same functionality as flags for scripts.

use std::collections::VecDeque;
use std::io::IsTerminal;
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use landscape_terrain_proto::cli::{parse_ethertype, parse_forward};
use landscape_terrain_proto::transport;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinHandle;

use crate::client::{ClientConfig, ClientEvent, Forward, ForwardCommand, LogLevel, LogSink};

const EVENT_POLL: Duration = Duration::from_millis(100);
const MAX_LOGS: usize = 400;
const AUTO_DEVICE: &str = "自动 (默认路由)";

/// What happened when the TUI loop ended.
enum Outcome {
    /// Quit from the form before connecting.
    FormQuit,
    /// Session ended (client task returned Ok).
    Disconnected,
    /// The client task failed (e.g. link open error).
    Failed(String),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Field {
    Psk,
    User,
    ClientName,
    Device,
    Ethertype,
    Token,
    Connect,
}

impl Field {
    fn next(self) -> Self {
        match self {
            Field::Psk => Field::User,
            Field::User => Field::ClientName,
            Field::ClientName => Field::Device,
            Field::Device => Field::Ethertype,
            Field::Ethertype => Field::Token,
            Field::Token => Field::Connect,
            Field::Connect => Field::Psk,
        }
    }

    fn prev(self) -> Self {
        match self {
            Field::Psk => Field::Connect,
            Field::User => Field::Psk,
            Field::ClientName => Field::User,
            Field::Device => Field::ClientName,
            Field::Ethertype => Field::Device,
            Field::Token => Field::Ethertype,
            Field::Connect => Field::Token,
        }
    }
}

/// Form phase state.
struct FormState {
    focus: Field,
    psk: String,
    show_psk: bool,
    user: String,
    client_name: String,
    /// Empty = auto-detect (default route interface).
    device: String,
    device_index: usize,
    device_selecting: bool,
    devices: Vec<String>,
    devices_err: Option<String>,
    ethertype: String,
    token: String,
    error: Option<String>,
}

impl FormState {
    fn new() -> Self {
        let (devices, devices_err) = match transport::list_interfaces() {
            Ok(mut d) => {
                d.sort();
                (d, None)
            }
            Err(e) => (Vec::new(), Some(format!("无法枚举网卡: {e}"))),
        };
        Self::from_devices(devices, devices_err)
    }

    fn from_devices(devices: Vec<String>, devices_err: Option<String>) -> Self {
        Self {
            focus: Field::Psk,
            psk: String::new(),
            show_psk: false,
            user: "admin".into(),
            client_name: "pc".into(),
            device: String::new(),
            device_index: 0,
            device_selecting: false,
            devices,
            devices_err,
            ethertype: "0x88b6".into(),
            token: String::new(),
            error: None,
        }
    }

    fn device_options(&self) -> Vec<String> {
        let mut v = vec![AUTO_DEVICE.to_string()];
        v.extend(self.devices.iter().cloned());
        v
    }

    fn device_label(&self) -> String {
        if self.device.is_empty() {
            AUTO_DEVICE.to_string()
        } else {
            self.device.clone()
        }
    }

    /// Validate the form and build an owned config for the client task.
    fn build(&self) -> Result<OwnedConfig, String> {
        let psk = self.psk.trim().to_string();
        if psk.is_empty() {
            return Err("请输入 psk (共享密钥)".into());
        }
        let ethertype = parse_ethertype(self.ethertype.trim())?;
        let devs = if self.device.is_empty() {
            Vec::new()
        } else {
            vec![self.device.clone()]
        };
        Ok(OwnedConfig {
            devs,
            user: self.user.trim().to_string(),
            client_name: self.client_name.trim().to_string(),
            psk,
            ethertype,
            token: self.token.trim().to_string(),
        })
    }
}

/// Owned values feeding the client task (lives as long as the task).
#[derive(Debug)]
struct OwnedConfig {
    devs: Vec<String>,
    user: String,
    client_name: String,
    psk: String,
    ethertype: u16,
    token: String,
}

/// Live session phase state.
struct DashState {
    log_rx: mpsc::UnboundedReceiver<(LogLevel, String)>,
    event_rx: mpsc::UnboundedReceiver<ClientEvent>,
    logs: VecDeque<(LogLevel, String)>,
    /// Scroll offset from the bottom (0 = newest visible).
    scroll: usize,
    forwards: Vec<Forward>,
    /// Which list the arrow keys navigate.
    focus: DashFocus,
    /// Selected row in the forwards list.
    forward_index: usize,
    forward_input: String,
    forward_edit: bool,
    forward_error: Option<String>,
    session_ready: bool,
    advertised_ports: Vec<u16>,
    device_label: String,
    notify: Arc<Notify>,
    forward_tx: mpsc::UnboundedSender<ForwardCommand>,
    client: Option<JoinHandle<Result<(), String>>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DashFocus {
    Forwards,
    Logs,
}

impl DashState {
    fn drain(&mut self) {
        while let Ok(item) = self.log_rx.try_recv() {
            self.logs.push_back(item);
            if self.logs.len() > MAX_LOGS {
                self.logs.pop_front();
            }
        }
        while let Ok(event) = self.event_rx.try_recv() {
            match event {
                ClientEvent::SessionReady { advertised_ports } => {
                    self.session_ready = true;
                    self.advertised_ports = advertised_ports;
                    self.forward_error = None;
                }
                ClientEvent::SessionLost => {
                    self.session_ready = false;
                    self.advertised_ports.clear();
                }
                ClientEvent::ForwardRejected { forward, reason } => {
                    self.forwards.retain(|item| *item != forward);
                    self.forward_index = self
                        .forward_index
                        .min(self.forwards.len().saturating_sub(1));
                    self.forward_error = Some(reason);
                }
            }
        }
    }
}

enum Phase {
    Form(FormState),
    Dash(DashState),
    Done(Outcome),
}

pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut terminal = ratatui::try_init().map_err(|e| format!("无法初始化终端: {e}"))?;
    let outcome = run_tui(&mut terminal).await;
    ratatui::restore();
    let result: Result<(), Box<dyn std::error::Error>> = match outcome {
        Ok(Outcome::FormQuit) => {
            println!("已退出 (未连接)");
            Ok(())
        }
        Ok(Outcome::Disconnected) => {
            println!("连接已断开");
            Ok(())
        }
        Ok(Outcome::Failed(msg)) => {
            eprintln!("连接失败: {msg}");
            Err(std::io::Error::other(msg).into())
        }
        Err(e) => {
            eprintln!("lflare: {e}");
            Err(e)
        }
    };
    if std::io::stdin().is_terminal() {
        println!("\n按任意键退出...");
        let _ = pause_any_key();
    }
    result
}

async fn run_tui(terminal: &mut DefaultTerminal) -> Result<Outcome, Box<dyn std::error::Error>> {
    let mut phase = Phase::Form(FormState::new());
    loop {
        let ev = if event::poll(EVENT_POLL)? {
            Some(event::read()?)
        } else {
            None
        };
        let mut connect: Option<Result<OwnedConfig, String>> = None;
        let mut quit = false;
        match &mut phase {
            Phase::Form(form) => {
                terminal.draw(|f| render_form(f, form))?;
                if let Some(Event::Key(k)) = ev {
                    match handle_form_key(form, k) {
                        FormAction::Connect => connect = Some(form.build()),
                        FormAction::Quit => quit = true,
                        FormAction::None => {}
                    }
                }
            }
            Phase::Dash(dash) => {
                dash.drain();
                terminal.draw(|f| render_dash(f, dash))?;
                if let Some(Event::Key(k)) = ev
                    && handle_dash_key(dash, k)
                {
                    dash.notify.notify_one();
                }
            }
            Phase::Done(outcome) => {
                let outcome = std::mem::replace(outcome, Outcome::Disconnected);
                return Ok(outcome);
            }
        }
        if quit {
            phase = Phase::Done(Outcome::FormQuit);
            continue;
        }
        if let Some(res) = connect {
            match res {
                Ok(cfg) => phase = Phase::Dash(spawn_client(cfg)),
                Err(e) => {
                    if let Phase::Form(f) = &mut phase {
                        f.error = Some(e);
                    }
                }
            }
        }
        let mut finished: Option<Outcome> = None;
        if let Phase::Dash(dash) = &mut phase
            && dash.client.as_ref().is_some_and(|h| h.is_finished())
        {
            let handle = dash.client.take().expect("client handle");
            let res = match handle.await {
                Ok(Ok(())) => Outcome::Disconnected,
                Ok(Err(e)) => Outcome::Failed(e),
                Err(e) => Outcome::Failed(format!("client task panicked: {e}")),
            };
            finished = Some(res);
        }
        if let Some(res) = finished {
            phase = Phase::Done(res);
        }
    }
}

#[derive(PartialEq, Debug)]
enum FormAction {
    None,
    Connect,
    Quit,
}

fn handle_form_key(form: &mut FormState, key: KeyEvent) -> FormAction {
    if key.kind != KeyEventKind::Press {
        return FormAction::None;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    // Editable text: anything without CONTROL/ALT. SHIFT must not block
    // uppercase letters (crossterm reports them with the SHIFT modifier).
    let editable = !ctrl && !key.modifiers.contains(KeyModifiers::ALT);

    match key.code {
        KeyCode::Char('c') if ctrl => FormAction::Quit,
        KeyCode::Char('q') if ctrl => FormAction::Quit,
        KeyCode::Esc if !form.device_selecting => FormAction::Quit,
        KeyCode::F(2) if form.focus == Field::Psk => {
            form.show_psk = !form.show_psk;
            FormAction::None
        }
        KeyCode::Tab => {
            form.device_selecting = false;
            form.focus = form.focus.next();
            FormAction::None
        }
        KeyCode::BackTab => {
            form.device_selecting = false;
            form.focus = form.focus.prev();
            FormAction::None
        }
        KeyCode::Down => {
            if form.focus == Field::Device && form.device_selecting {
                let opts = form.device_options();
                form.device_index = (form.device_index + 1).min(opts.len() - 1);
                form.device = if form.device_index == 0 {
                    String::new()
                } else {
                    opts[form.device_index].clone()
                };
            } else {
                form.device_selecting = false;
                form.focus = form.focus.next();
            }
            FormAction::None
        }
        KeyCode::Up => {
            if form.focus == Field::Device && form.device_selecting {
                form.device_index = form.device_index.saturating_sub(1);
                let opts = form.device_options();
                form.device = if form.device_index == 0 {
                    String::new()
                } else {
                    opts[form.device_index].clone()
                };
            } else {
                form.device_selecting = false;
                form.focus = form.focus.prev();
            }
            FormAction::None
        }
        KeyCode::Enter => {
            if form.focus == Field::Connect {
                return FormAction::Connect;
            }
            if form.focus == Field::Device {
                form.device_selecting = !form.device_selecting;
            } else {
                form.focus = form.focus.next();
            }
            FormAction::None
        }
        KeyCode::Esc => {
            form.device_selecting = false;
            FormAction::None
        }
        KeyCode::Backspace => {
            match form.focus {
                Field::Psk => {
                    form.psk.pop();
                }
                Field::User => {
                    form.user.pop();
                }
                Field::ClientName => {
                    form.client_name.pop();
                }
                Field::Ethertype => {
                    form.ethertype.pop();
                }
                Field::Token => {
                    form.token.pop();
                }
                Field::Device | Field::Connect => {}
            }
            FormAction::None
        }
        KeyCode::Char('u') if ctrl => {
            match form.focus {
                Field::Psk => form.psk.clear(),
                Field::User => form.user.clear(),
                Field::ClientName => form.client_name.clear(),
                Field::Ethertype => form.ethertype.clear(),
                Field::Token => form.token.clear(),
                Field::Device | Field::Connect => {}
            }
            FormAction::None
        }
        KeyCode::Char(c) if editable => {
            match form.focus {
                Field::Psk => form.psk.push(c),
                Field::User => form.user.push(c),
                Field::ClientName => form.client_name.push(c),
                Field::Ethertype => form.ethertype.push(c),
                Field::Token => form.token.push(c),
                Field::Device | Field::Connect => {}
            }
            FormAction::None
        }
        _ => FormAction::None,
    }
}

fn handle_dash_key(dash: &mut DashState, key: KeyEvent) -> bool {
    if key.kind != KeyEventKind::Press {
        return false;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let editable = !ctrl && !key.modifiers.contains(KeyModifiers::ALT);

    if dash.forward_edit {
        match key.code {
            KeyCode::Char(c) if editable => dash.forward_input.push(c),
            KeyCode::Backspace => {
                dash.forward_input.pop();
            }
            KeyCode::Enter => match parse_forward(dash.forward_input.trim()) {
                Ok(forward) if dash.forwards.contains(&forward) => {
                    dash.forward_error = Some("该映射已存在".into());
                }
                Ok(forward) if !dash.advertised_ports.contains(&forward.1) => {
                    dash.forward_error = Some(format!(
                        "服务器未允许目标端口 {}，当前会话不能添加该映射",
                        forward.1
                    ));
                }
                Ok(forward) => {
                    let _ = dash.forward_tx.send(ForwardCommand::Add(forward));
                    dash.forwards.push(forward);
                    dash.forward_index = dash.forwards.len() - 1;
                    dash.forward_input.clear();
                    dash.forward_error = None;
                    dash.forward_edit = false;
                }
                Err(e) => dash.forward_error = Some(e),
            },
            KeyCode::Esc => {
                dash.forward_input.clear();
                dash.forward_error = None;
                dash.forward_edit = false;
            }
            _ => {}
        }
        return false;
    }

    match key.code {
        KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => true,
        KeyCode::Char('c') if ctrl => true,
        KeyCode::Tab | KeyCode::BackTab => {
            dash.focus = match dash.focus {
                DashFocus::Forwards => DashFocus::Logs,
                DashFocus::Logs => DashFocus::Forwards,
            };
            false
        }
        KeyCode::Up | KeyCode::PageUp => {
            let step = if matches!(key.code, KeyCode::PageUp) {
                10
            } else {
                1
            };
            dash_scroll(dash, step);
            false
        }
        KeyCode::Down | KeyCode::PageDown => {
            let step = if matches!(key.code, KeyCode::PageDown) {
                10
            } else {
                1
            };
            dash_scroll(dash, -(step as i64));
            false
        }
        KeyCode::Home => {
            match dash.focus {
                DashFocus::Forwards => dash.forward_index = 0,
                DashFocus::Logs => dash.scroll = usize::MAX,
            }
            false
        }
        KeyCode::End => {
            match dash.focus {
                DashFocus::Forwards => dash.forward_index = dash.forwards.len().saturating_sub(1),
                DashFocus::Logs => dash.scroll = 0,
            }
            false
        }
        KeyCode::Char('a') if dash.focus == DashFocus::Forwards => {
            if dash.session_ready {
                dash.forward_input.clear();
                dash.forward_error = None;
                dash.forward_edit = true;
            } else {
                dash.forward_error = Some("握手成功后才能添加映射".into());
            }
            false
        }
        KeyCode::Char('d') if dash.focus == DashFocus::Forwards => {
            if dash.session_ready && !dash.forwards.is_empty() {
                let index = dash.forward_index.min(dash.forwards.len() - 1);
                let forward = dash.forwards.remove(index);
                let _ = dash.forward_tx.send(ForwardCommand::Remove(forward));
                dash.forward_index = dash
                    .forward_index
                    .min(dash.forwards.len().saturating_sub(1));
                dash.forward_error = None;
            }
            false
        }
        _ => false,
    }
}

fn dash_scroll(dash: &mut DashState, delta: i64) {
    match dash.focus {
        DashFocus::Logs => {
            dash.scroll = (dash.scroll as i64 + delta).max(0) as usize;
        }
        DashFocus::Forwards => {
            let max = dash.forwards.len().saturating_sub(1) as i64;
            dash.forward_index = (dash.forward_index as i64 + delta).clamp(0, max) as usize;
        }
    }
}

fn spawn_client(cfg: OwnedConfig) -> DashState {
    let (log_tx, log_rx) = mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let (forward_tx, forward_rx) = mpsc::unbounded_channel();
    let notify = Arc::new(Notify::new());
    let task_notify = notify.clone();
    let device_label = if cfg.devs.is_empty() {
        "自动".to_string()
    } else {
        cfg.devs.join(",")
    };
    let client = tokio::spawn(async move {
        let client_cfg = ClientConfig {
            devs: &cfg.devs,
            ethertype: cfg.ethertype,
            mac: None,
            user: &cfg.user,
            psk: &cfg.psk,
            client_name: &cfg.client_name,
            forwards: &[],
            token: &cfg.token,
            log: LogSink::Chan(log_tx),
            shutdown: Some(task_notify),
            forward_control: Some(forward_rx),
            events: Some(event_tx),
        };
        crate::client::run(client_cfg)
            .await
            .map_err(|e| e.to_string())
    });
    DashState {
        log_rx,
        event_rx,
        logs: VecDeque::new(),
        scroll: 0,
        forwards: Vec::new(),
        focus: DashFocus::Logs,
        forward_index: 0,
        forward_input: String::new(),
        forward_edit: false,
        forward_error: None,
        session_ready: false,
        advertised_ports: Vec::new(),
        device_label,
        notify,
        forward_tx,
        client: Some(client),
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn focused_style() -> Style {
    Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD)
}

/// Offset of a scroll window (top row shown) for a list of `total` items
/// showing `visible` rows, given a scroll/selection position.
#[cfg(test)]
fn visible_offset(total: usize, visible: usize, position: usize) -> usize {
    if total <= visible || visible == 0 {
        return 0;
    }
    position.min(total - visible)
}

/// Like `visible_offset`, but for a selection list: keeps the selected item
/// on screen, pushing the window down only once the selection passes the
/// bottom edge.
fn selection_offset(total: usize, visible: usize, selection: usize) -> usize {
    if total <= visible || visible == 0 {
        return 0;
    }
    selection.saturating_sub(visible - 1).min(total - visible)
}

fn text_field(f: &mut Frame, area: Rect, label: &str, value: &str, focused: bool) {
    let border = if focused {
        Color::Cyan
    } else {
        Color::DarkGray
    };
    let block = Block::bordered()
        .title(Line::styled(label, Style::default().fg(border)))
        .border_style(Style::default().fg(border));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let p = Paragraph::new(value).style(if focused {
        focused_style()
    } else {
        Style::default()
    });
    f.render_widget(p, inner);
}

fn render_form(f: &mut Frame, form: &FormState) {
    let dev_opts = form.device_options();
    let device_picker_open = form.focus == Field::Device && form.device_selecting;
    let dev_visible = if device_picker_open {
        dev_opts.len().min(6)
    } else {
        1
    };
    let dev_offset = if device_picker_open {
        selection_offset(dev_opts.len(), dev_visible, form.device_index)
    } else {
        0
    };
    // Bordered blocks need 2 rows for the borders plus one content row per
    // list item (plus the "… 共 N 个" overflow hint row).
    let device_h = if device_picker_open {
        2 + dev_visible + usize::from(dev_opts.len() > dev_visible)
    } else {
        3
    };
    let err_h = usize::from(form.error.is_some() || form.devices_err.is_some());
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
            "lflare · Landscape L2 客户端",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .alignment(ratatui::layout::Alignment::Center),
        title,
    );
    f.render_widget(
        Paragraph::new(Line::styled(
            "Tab/↑↓ 切换字段 · 最后聚焦连接后按 Enter · F2 显示/隐藏 psk · Ctrl-Q 退出",
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
        "psk (共享密钥, F2 显示/隐藏)",
        &psk_value,
        form.focus == Field::Psk,
    );
    text_field(f, user, "用户名", &form.user, form.focus == Field::User);
    text_field(
        f,
        cn,
        "客户端名称",
        &form.client_name,
        form.focus == Field::ClientName,
    );

    // Device field: the list opens explicitly so Up/Down can still move
    // between form fields after a device has been selected.
    if device_picker_open {
        let border = Color::Cyan;
        let block = Block::bordered()
            .title(Line::styled(
                "设备 (↑↓ 选择, Enter 确认)",
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
                format!("… 共 {} 个设备, ↑↓ 继续选择", dev_opts.len()),
                Style::default().fg(Color::DarkGray),
            ));
        }
        f.render_widget(Paragraph::new(lines), inner);
    } else {
        text_field(
            f,
            dev,
            "设备 (Enter 选择)",
            &form.device_label(),
            form.focus == Field::Device,
        );
    }

    text_field(
        f,
        eth,
        "ethertype",
        &form.ethertype,
        form.focus == Field::Ethertype,
    );
    text_field(
        f,
        tok,
        "token (可选, 需与服务器一致)",
        &form.token,
        form.focus == Field::Token,
    );

    let button_style = if form.focus == Field::Connect {
        focused_style().fg(Color::Black).bg(Color::Cyan)
    } else {
        Style::default().fg(Color::Cyan)
    };
    f.render_widget(
        Paragraph::new(Line::styled("  连接  ", button_style))
            .alignment(ratatui::layout::Alignment::Center)
            .block(
                Block::bordered()
                    .title("连接")
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

/// Parse "session {sid} established with {mac} (encrypted)" into (sid, mac).
fn session_info(logs: &[(LogLevel, String)]) -> Option<(u32, String)> {
    for (_, l) in logs.iter().rev() {
        let Some(rest) = l.strip_prefix("session ") else {
            continue;
        };
        let t: Vec<&str> = rest.split_whitespace().collect();
        if t.len() >= 4 && t[1] == "established" {
            let Ok(sid) = t[0].parse() else {
                continue;
            };
            return Some((sid, t[3].to_string()));
        }
    }
    None
}

/// Current connection state derived from the log tail.
fn status_line(logs: &[(LogLevel, String)]) -> String {
    for (_, l) in logs.iter().rev() {
        if l.contains("teardown from server") {
            return "服务器已关闭会话".into();
        }
        if let Some(reason) = l.strip_prefix("  auth rejected:") {
            return format!("认证被拒绝: {}", reason.trim());
        }
        if l.contains("link lost") {
            return "链路断开, 重新连接中".into();
        }
        if l.contains("handshake failed, retrying") {
            return "未发现服务器, 重试中".into();
        }
    }
    if let Some((sid, mac)) = session_info(logs) {
        return format!("已连接 · 会话 {sid} · {mac}");
    }
    "正在搜索服务器…".into()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ForwardState {
    Starting,
    Listening,
    Failed,
}

/// Listener state of one forward, derived from its log lines.
fn forward_state(logs: &[(LogLevel, String)], listen_port: u16) -> ForwardState {
    for (_, l) in logs.iter().rev() {
        if l.contains(&format!("cannot listen on 127.0.0.1:{listen_port}")) {
            return ForwardState::Failed;
        }
        if l.contains(&format!("forward: 127.0.0.1:{listen_port} ->")) {
            return ForwardState::Listening;
        }
    }
    ForwardState::Starting
}

fn render_dash(f: &mut Frame, dash: &DashState) {
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
            "lflare · Landscape L2 会话",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ))
        .alignment(ratatui::layout::Alignment::Center),
        title,
    );

    let logs: Vec<(LogLevel, String)> = dash.logs.iter().cloned().collect();
    let status_lines = vec![
        Line::styled(
            format!("状态: {}", status_line(&logs)),
            Style::default().fg(Color::Cyan),
        ),
        Line::from(format!(
            "设备: {}     映射: {} 条",
            dash.device_label,
            dash.forwards.len()
        )),
    ];
    f.render_widget(
        Paragraph::new(status_lines).block(Block::bordered().title("连接状态")),
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
        let state = forward_state(&logs, *lp);
        let (marker, color) = match state {
            ForwardState::Listening => ("✓", Color::Green),
            ForwardState::Starting => ("…", Color::Yellow),
            ForwardState::Failed => ("✗", Color::Red),
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
                format!(" 127.0.0.1:{lp} -> 路由器 127.0.0.1:{dp}"),
                line_style,
            ),
        ]));
    }
    if fwd_lines.is_empty() {
        fwd_lines.push(Line::styled(
            if dash.session_ready {
                "未配置端口映射"
            } else {
                "等待握手成功后管理映射"
            },
            Style::default().fg(if dash.session_ready {
                Color::DarkGray
            } else {
                Color::Yellow
            }),
        ));
    } else if dash.forwards.len() > fwd_visible {
        let focus = if dash.focus == DashFocus::Forwards {
            "已聚焦 · ↑↓ 滚动"
        } else {
            "Tab 切换到映射后 ↑↓ 滚动"
        };
        fwd_lines.push(Line::styled(
            format!("… 共 {} 条 ({focus})", dash.forwards.len()),
            Style::default().fg(Color::DarkGray),
        ));
    }
    if dash.forward_edit {
        fwd_lines.push(Line::from(vec![
            Span::styled("> ", focused_style()),
            Span::styled(format!("{}_", dash.forward_input), focused_style()),
        ]));
        fwd_lines.push(Line::styled(
            "输入 LOCAL_PORT:DST_PORT · Enter 确认 · Esc 取消",
            Style::default().fg(Color::DarkGray),
        ));
    }
    if let Some(error) = &dash.forward_error {
        fwd_lines.push(Line::styled(error, Style::default().fg(Color::Red)));
    }
    let forward_title = if dash.session_ready {
        "端口映射 (a 添加 · d 删除)"
    } else {
        "端口映射 (等待握手)"
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

    let mut log_lines: Vec<Line> = dash
        .logs
        .iter()
        .map(|(lvl, s)| {
            let (color, marker) = match lvl {
                LogLevel::Info => (Color::White, ""),
                LogLevel::Warn => (Color::Yellow, "⚠ "),
                LogLevel::Error => (Color::Red, "✗ "),
            };
            Line::styled(format!("{marker}{s}"), Style::default().fg(color))
        })
        .collect();
    let visible = (log.height as usize).saturating_sub(2).max(1);
    let total = log_lines.len();
    let scroll = dash.scroll.min(total.saturating_sub(1));
    let end = total.saturating_sub(scroll);
    let start = end.saturating_sub(visible);
    let mut shown: Vec<Line> = log_lines.drain(start..end).collect();
    if total > visible && scroll == 0 && dash.focus == DashFocus::Logs {
        shown.push(Line::styled(
            "(最底部 · ↑ 滚动查看历史日志)",
            Style::default().fg(Color::DarkGray),
        ));
    }
    f.render_widget(
        Paragraph::new(shown)
            .block(Block::bordered().title("日志"))
            .wrap(Wrap { trim: false }),
        log,
    );

    let focus_hint = match dash.focus {
        DashFocus::Forwards if dash.session_ready => "· a 添加 d 删除 · 滚动: 端口映射",
        DashFocus::Forwards => "· 映射将在握手成功后可用",
        DashFocus::Logs => "· 滚动: 日志",
    };
    f.render_widget(
        Paragraph::new(Line::styled(
            format!("Tab 切换列表 {focus_hint} · ↑↓/PgUp/PgDn 滚动 · q 断开并退出"),
            Style::default().fg(Color::DarkGray),
        )),
        hint,
    );
}

fn pause_any_key() -> std::io::Result<()> {
    crossterm::terminal::enable_raw_mode()?;
    let r = event::read();
    crossterm::terminal::disable_raw_mode()?;
    r.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn logs(items: &[(&str, &str)]) -> Vec<(LogLevel, String)> {
        items
            .iter()
            .map(|(lvl, msg)| {
                let level = match *lvl {
                    "w" => LogLevel::Warn,
                    "e" => LogLevel::Error,
                    _ => LogLevel::Info,
                };
                (level, msg.to_string())
            })
            .collect()
    }

    #[test]
    fn session_info_parses_established_line() {
        let l = logs(&[
            ("i", "discover: broadcast (try 1/5)"),
            (
                "i",
                "  discovered 'router' at aa:bb:cc:dd:ee:ff (forwards: 22,6443)",
            ),
            ("i", "  auth request sent for user 'admin'"),
            (
                "i",
                "session 42 established with aa:bb:cc:dd:ee:ff (encrypted)",
            ),
        ]);
        assert_eq!(
            session_info(&l),
            Some((42, "aa:bb:cc:dd:ee:ff".to_string()))
        );
        assert_eq!(status_line(&l), "已连接 · 会话 42 · aa:bb:cc:dd:ee:ff");
    }

    #[test]
    fn status_line_priority() {
        let l = logs(&[(
            "i",
            "session 1 established with aa:bb:cc:dd:ee:ff (encrypted)",
        )]);
        assert_eq!(status_line(&l), "已连接 · 会话 1 · aa:bb:cc:dd:ee:ff");

        let l = logs(&[("i", "handshake failed, retrying in 3s")]);
        assert_eq!(status_line(&l), "未发现服务器, 重试中");

        let l = logs(&[("w", "  auth rejected: lockout")]);
        assert_eq!(status_line(&l), "认证被拒绝: lockout");

        let l = logs(&[("i", "  link lost, restarting handshake")]);
        assert_eq!(status_line(&l), "链路断开, 重新连接中");

        let l = logs(&[("i", "  teardown from server, closing session")]);
        assert_eq!(status_line(&l), "服务器已关闭会话");

        assert_eq!(status_line(&[]), "正在搜索服务器…");
    }

    #[test]
    fn forward_state_derived_from_logs() {
        let l = logs(&[("i", "  forward: 127.0.0.1:2222 -> router 127.0.0.1:22")]);
        assert_eq!(forward_state(&l, 2222), ForwardState::Listening);
        assert_eq!(forward_state(&l, 3333), ForwardState::Starting);

        let l = logs(&[("w", "  cannot listen on 127.0.0.1:2222: addr in use")]);
        assert_eq!(forward_state(&l, 2222), ForwardState::Failed);

        // A later success overrides an earlier failure for the same port.
        let l = logs(&[
            ("w", "  cannot listen on 127.0.0.1:2222: addr in use"),
            ("i", "  forward: 127.0.0.1:2222 -> router 127.0.0.1:22"),
        ]);
        assert_eq!(forward_state(&l, 2222), ForwardState::Listening);
    }

    #[test]
    fn form_build_validates() {
        let mut form = FormState::from_devices(Vec::new(), None);
        form.psk = "test-psk-123456".into();
        let cfg = form.build().unwrap();
        assert!(cfg.devs.is_empty());
        assert_eq!(cfg.ethertype, 0x88b6);

        form.psk.clear();
        assert!(form.build().unwrap_err().contains("psk"));

        form.psk = "test-psk-123456".into();
        form.ethertype = "0x1234".into();
        assert!(form.build().unwrap_err().contains("ethertype"));
    }

    #[test]
    fn form_enter_only_connects_from_button() {
        let mut form = FormState::from_devices(Vec::new(), None);
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);

        assert_eq!(handle_form_key(&mut form, enter), FormAction::None);
        assert_eq!(form.focus, Field::User);

        form.focus = Field::Connect;
        assert_eq!(handle_form_key(&mut form, enter), FormAction::Connect);
    }

    #[test]
    fn psk_accepts_v_and_f2_only_toggles_visibility() {
        let mut form = FormState::from_devices(Vec::new(), None);
        let v = KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE);
        assert_eq!(handle_form_key(&mut form, v), FormAction::None);
        assert_eq!(form.psk, "v");

        let f2 = KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE);
        assert!(!form.show_psk);
        handle_form_key(&mut form, f2);
        assert!(form.show_psk);
    }

    #[test]
    fn device_picker_does_not_trap_form_navigation() {
        let mut form = FormState::from_devices(vec!["eth0".into(), "eth1".into()], None);
        let up = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);

        form.focus = Field::Device;
        handle_form_key(&mut form, up);
        assert_eq!(form.focus, Field::ClientName);

        form.focus = Field::Device;
        handle_form_key(&mut form, enter);
        assert!(form.device_selecting);
        handle_form_key(&mut form, down);
        assert_eq!(form.device, "eth0");
        handle_form_key(&mut form, enter);
        assert!(!form.device_selecting);
        handle_form_key(&mut form, up);
        assert_eq!(form.focus, Field::ClientName);

        form.focus = Field::Device;
        handle_form_key(&mut form, enter);
        assert!(form.device_selecting);
        assert_eq!(
            handle_form_key(&mut form, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            FormAction::None
        );
        assert!(!form.device_selecting);
    }

    #[test]
    fn scroll_windows() {
        // Lists shorter than the window never scroll.
        assert_eq!(visible_offset(3, 6, 0), 0);
        assert_eq!(selection_offset(3, 6, 2), 0);
        assert_eq!(visible_offset(0, 0, 0), 0);

        // Dashboard scroll: position is the top row, clamped to the end.
        assert_eq!(visible_offset(8, 5, 0), 0);
        assert_eq!(visible_offset(8, 5, 3), 3);
        assert_eq!(visible_offset(8, 5, 99), 3);

        // Selection lists keep the selected row visible: the window only
        // moves down once the selection passes the bottom edge.
        assert_eq!(selection_offset(8, 6, 3), 0);
        assert_eq!(selection_offset(8, 6, 5), 0);
        assert_eq!(selection_offset(8, 6, 6), 1);
        assert_eq!(selection_offset(8, 6, 7), 2);
        assert_eq!(selection_offset(8, 6, 99), 2);
    }
}
