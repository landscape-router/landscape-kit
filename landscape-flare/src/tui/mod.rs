//! Interactive TUI: a connection form followed by a
//! live session dashboard. This is the default entry point of `lflare`; the
//! `cli` subcommand offers the same functionality as flags for scripts.

use std::collections::{HashMap, VecDeque};
use std::io::IsTerminal;
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{self, Event};
use ratatui::DefaultTerminal;
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinHandle;

use crate::client::{
    ClientConfig, ClientEvent, Forward, ForwardCommand, ForwardRejection,
    ForwardStatus as ClientForwardStatus, LogLevel, LogSink, SessionStatus,
};

mod form;
mod render;
mod session;

#[cfg(test)]
mod snapshots;

#[cfg(test)]
use form::Field;
use form::{FormAction, FormState, OwnedConfig};
use render::{
    render_dash, render_exit_confirmation, render_form, render_too_small, terminal_too_small,
};
#[cfg(test)]
use render::{selection_offset, status_line};

const EVENT_POLL: Duration = Duration::from_millis(100);
const MAX_LOGS: usize = 400;

/// What happened when the TUI loop ended.
enum Outcome {
    /// Quit from the form before connecting.
    FormQuit,
    /// Session ended (client task returned Ok).
    Disconnected,
    /// The client task failed (e.g. link open error) and no form snapshot
    /// was kept to return to.
    Failed(String),
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
    /// 退出确认层:q/Esc 打开,Enter 确认断开,Ctrl-C 无条件立即退出。
    exit_confirming: bool,
    /// 连接时的表单快照:客户端任务以硬错误结束(设备消失、链路打开失败
    /// 等)时回到表单并显示原因,字段值保留,修正后可以直接重连。
    resume_form: Option<FormState>,
    session_ready: bool,
    connection: ConnectionState,
    advertised_ports: Vec<u16>,
    forward_states: HashMap<Forward, ClientForwardStatus>,
    device: Option<String>,
    notify: Arc<Notify>,
    forward_tx: mpsc::UnboundedSender<ForwardCommand>,
    client: Option<JoinHandle<Result<(), String>>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DashFocus {
    Forwards,
    Logs,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum ConnectionState {
    #[default]
    Searching,
    Authenticating,
    Ready {
        session_id: u32,
        server_mac: String,
    },
    AuthRejected(String),
    LinkLost,
    PeerClosed,
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
            self.apply_event(event);
        }
    }

    fn apply_event(&mut self, event: ClientEvent) {
        match event {
            ClientEvent::SessionStatus(status) => {
                self.session_ready = false;
                let preserve_advertised =
                    matches!(&status, SessionStatus::LinkLost | SessionStatus::PeerClosed);
                self.connection = match status {
                    SessionStatus::Searching => ConnectionState::Searching,
                    SessionStatus::Authenticating => ConnectionState::Authenticating,
                    SessionStatus::AuthRejected(reason) => ConnectionState::AuthRejected(reason),
                    SessionStatus::LinkLost => ConnectionState::LinkLost,
                    SessionStatus::PeerClosed => ConnectionState::PeerClosed,
                };
                if !preserve_advertised {
                    self.advertised_ports.clear();
                }
            }
            ClientEvent::SessionReady {
                session_id,
                server_mac,
                advertised_ports,
            } => {
                self.session_ready = true;
                self.connection = ConnectionState::Ready {
                    session_id,
                    server_mac,
                };
                self.advertised_ports = advertised_ports;
                self.forward_error = None;
            }
            ClientEvent::ForwardStatus { forward, status } => {
                self.forward_states.insert(forward, status);
            }
            ClientEvent::ForwardRejected { forward, reason } => {
                self.forwards.retain(|item| *item != forward);
                self.forward_states.remove(&forward);
                self.forward_index = self
                    .forward_index
                    .min(self.forwards.len().saturating_sub(1));
                self.forward_error = Some(match reason {
                    ForwardRejection::Duplicate => crate::tr!("tui.duplicate_mapping"),
                    ForwardRejection::DestinationNotAdvertised { port } => {
                        crate::tr!("tui.port_not_advertised", port = port)
                    }
                });
            }
        }
    }
}

/// 仅在 TUI 主循环内移动,低频构造(每次连接/断开一次),保持平坦布局。
#[allow(clippy::large_enum_variant)]
enum Phase {
    Form(FormState),
    Dash(DashState),
    Done(Outcome),
}

/// 渲染一个相位:终端小于最小尺寸时画整屏提示,与 run_tui 使用同一路径,
/// 快照测试因此同时钉住守卫屏本身。
fn draw_form<B: ratatui::backend::Backend>(
    terminal: &mut ratatui::Terminal<B>,
    form: &FormState,
) -> Result<(), B::Error> {
    terminal
        .draw(|f| {
            if terminal_too_small(f.area()) {
                render_too_small(f);
            } else {
                render_form(f, form);
            }
        })
        .map(|_| ())
}

fn draw_dash<B: ratatui::backend::Backend>(
    terminal: &mut ratatui::Terminal<B>,
    dash: &DashState,
) -> Result<(), B::Error> {
    terminal
        .draw(|f| {
            if terminal_too_small(f.area()) {
                render_too_small(f);
            } else {
                render_dash(f, dash);
            }
            // 确认层叠加在整屏之上,与 console 一致:too-small 提示屏期间
            // 仍然可见,用户不必盲目按键。
            if dash.exit_confirming {
                render_exit_confirmation(f);
            }
        })
        .map(|_| ())
}

pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut terminal =
        ratatui::try_init().map_err(|e| crate::tr!("tui.terminal_init_failed", error = e))?;
    let outcome = run_tui(&mut terminal).await;
    ratatui::restore();
    let result: Result<(), Box<dyn std::error::Error>> = match outcome {
        Ok(Outcome::FormQuit) => {
            println!("{}", crate::tr!("tui.form_quit"));
            Ok(())
        }
        Ok(Outcome::Disconnected) => {
            println!("{}", crate::tr!("tui.connected_summary"));
            Ok(())
        }
        Ok(Outcome::Failed(msg)) => {
            eprintln!("{}", crate::tr!("tui.failed", error = msg));
            Err(std::io::Error::other(msg).into())
        }
        Err(e) => {
            eprintln!("{}", crate::tr!("tui.tui_error", error = e));
            Err(e)
        }
    };
    if std::io::stdin().is_terminal() {
        println!("{}", crate::tr!("tui.press_key"));
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
                draw_form(terminal, form)?;
                if let Some(Event::Key(k)) = ev {
                    match form::handle_key(form, k) {
                        FormAction::Connect => connect = Some(form.build()),
                        FormAction::Quit => quit = true,
                        FormAction::None => {}
                    }
                }
            }
            Phase::Dash(dash) => {
                dash.drain();
                draw_dash(terminal, dash)?;
                if let Some(Event::Key(k)) = ev
                    && session::handle_key(dash, k)
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
                Ok(cfg) => {
                    // 连接时保留表单快照:硬错误不再把用户踢出 TUI,而是
                    // 回到表单显示原因,修正后重连。
                    let resume = if let Phase::Form(form) = &phase {
                        Some(form.clone())
                    } else {
                        None
                    };
                    let mut dash = spawn_client(cfg);
                    dash.resume_form = resume;
                    phase = Phase::Dash(dash);
                }
                Err(e) => {
                    if let Phase::Form(f) = &mut phase {
                        f.error = Some(e);
                    }
                }
            }
        }
        let mut finished: Option<Phase> = None;
        if let Phase::Dash(dash) = &mut phase
            && dash.client.as_ref().is_some_and(|h| h.is_finished())
        {
            let handle = dash.client.take().expect("client handle");
            let res = match handle.await {
                Ok(Ok(())) => Outcome::Disconnected,
                Ok(Err(e)) => Outcome::Failed(e),
                Err(e) => Outcome::Failed(format!("client task panicked: {e}")),
            };
            finished = Some(client_outcome_phase(dash, res));
        }
        if let Some(next) = finished {
            phase = next;
        }
    }
}

/// 客户端任务结束后的去向:硬错误且留有表单快照时回到表单并显示原因,
/// 其余结果(正常断开、用户退出)结束 TUI。
fn client_outcome_phase(dash: &mut DashState, outcome: Outcome) -> Phase {
    match outcome {
        Outcome::Failed(reason) => match dash.resume_form.take() {
            Some(mut form) => {
                form.error = Some(reason);
                Phase::Form(form)
            }
            None => Phase::Done(Outcome::Failed(reason)),
        },
        outcome => Phase::Done(outcome),
    }
}

fn spawn_client(cfg: OwnedConfig) -> DashState {
    let (log_tx, log_rx) = mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let (forward_tx, forward_rx) = mpsc::unbounded_channel();
    let notify = Arc::new(Notify::new());
    let task_notify = notify.clone();
    let device = (!cfg.devs.is_empty()).then(|| cfg.devs.join(","));
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
        exit_confirming: false,
        resume_form: None,
        session_ready: false,
        connection: ConnectionState::default(),
        advertised_ports: Vec::new(),
        forward_states: HashMap::new(),
        device,
        notify,
        forward_tx,
        client: Some(client),
    }
}

fn pause_any_key() -> std::io::Result<()> {
    crossterm::terminal::enable_raw_mode()?;
    let r = event::read();
    crossterm::terminal::disable_raw_mode()?;
    r.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::snapshots::LanguageGuard;
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn status_line_comes_from_structured_state() {
        {
            let _language = LanguageGuard::set(crate::i18n::Language::En);
            assert_eq!(
                status_line(&ConnectionState::Searching),
                "Searching for server..."
            );
            assert_eq!(
                status_line(&ConnectionState::Ready {
                    session_id: 42,
                    server_mac: "aa:bb:cc:dd:ee:ff".into(),
                }),
                "Connected · session 42 · aa:bb:cc:dd:ee:ff"
            );
        }
        let _language = LanguageGuard::set(crate::i18n::Language::Zh);
        assert_eq!(
            status_line(&ConnectionState::AuthRejected("lockout".into())),
            "认证被拒绝：lockout"
        );
    }

    #[test]
    fn form_build_validates() {
        // 断言依赖英文错误文案(小写 "ethertype"),锁语言防并行测试翻转全局。
        let _language = LanguageGuard::set(crate::i18n::Language::En);
        let mut form = FormState::from_devices(Vec::new(), None);
        form.psk = "test-psk-123456".into();
        let cfg = form.build().unwrap();
        assert!(cfg.devs.is_empty());
        assert_eq!(cfg.ethertype, 0x88b6);

        form.psk.clear();
        assert!(!form.build().unwrap_err().is_empty());

        form.psk = "test-psk-123456".into();
        form.ethertype = "0x1234".into();
        assert!(form.build().unwrap_err().contains("ethertype"));
    }

    #[test]
    fn device_picker_displays_description_but_submits_capture_name() {
        let mut form = FormState::from_interface_devices(
            vec![landscape_terrain_proto::transport::Interface {
                name: r#"\Device\NPF_{ABC}"#.into(),
                description: Some("Ethernet".into()),
            }],
            None,
        );
        let options = form.device_options();
        assert_eq!(options.len(), 2);
        assert_eq!(options[1], "Ethernet");

        form.focus = Field::Device;
        form.device_selecting = true;
        form::handle_key(&mut form, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(form.device, r#"\Device\NPF_{ABC}"#);
        assert_eq!(form.device_label(), "Ethernet");
    }

    #[test]
    fn device_picker_numbers_duplicate_descriptions() {
        // 断言含翻译文本(Auto 行),锁语言防并行快照测试翻转全局。
        let _language = LanguageGuard::set(crate::i18n::Language::En);
        let form = FormState::from_interface_devices(
            vec![
                landscape_terrain_proto::transport::Interface {
                    name: r#"\Device\NPF_{ONE}"#.into(),
                    description: Some("Ethernet".into()),
                },
                landscape_terrain_proto::transport::Interface {
                    name: r#"\Device\NPF_{TWO}"#.into(),
                    description: Some("Ethernet".into()),
                },
            ],
            None,
        );
        assert_eq!(
            form.device_options(),
            vec!["Auto (default route)", "Ethernet (1)", "Ethernet (2)"]
        );
    }

    #[test]
    fn form_enter_only_connects_from_button() {
        let mut form = FormState::from_devices(Vec::new(), None);
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);

        assert_eq!(form::handle_key(&mut form, enter), FormAction::None);
        assert_eq!(form.focus, Field::User);

        form.focus = Field::Connect;
        assert_eq!(form::handle_key(&mut form, enter), FormAction::Connect);
    }

    #[test]
    fn psk_accepts_v_and_f2_only_toggles_visibility() {
        let mut form = FormState::from_devices(Vec::new(), None);
        let v = KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE);
        assert_eq!(form::handle_key(&mut form, v), FormAction::None);
        assert_eq!(form.psk, "v");

        let f2 = KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE);
        assert!(!form.show_psk);
        form::handle_key(&mut form, f2);
        assert!(form.show_psk);
    }

    #[test]
    fn language_key_is_text_in_fields_and_toggle_on_connect() {
        let _language = LanguageGuard::set(crate::i18n::Language::En);
        let mut form = FormState::from_devices(Vec::new(), None);
        let language_key = KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE);

        form::handle_key(&mut form, language_key);
        assert_eq!(form.psk, "l");
        assert_eq!(crate::i18n::current(), crate::i18n::Language::En);

        form.focus = Field::Connect;
        form::handle_key(&mut form, language_key);
        assert_eq!(crate::i18n::current(), crate::i18n::Language::Zh);
    }

    /// 就绪会话的仪表盘夹具:通道 sender 全部关闭(drain 无事可做),
    /// 键盘路径测试只关心 handle_key 对状态的影响。
    fn test_dash() -> DashState {
        let (log_tx, log_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (forward_tx, _forward_rx) = mpsc::unbounded_channel();
        drop(log_tx);
        drop(event_tx);
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
            exit_confirming: false,
            resume_form: None,
            session_ready: true,
            connection: ConnectionState::Ready {
                session_id: 1,
                server_mac: "00:00:00:00:00:00".into(),
            },
            advertised_ports: vec![22],
            forward_states: HashMap::new(),
            device: None,
            notify: Arc::new(Notify::new()),
            forward_tx,
            client: None,
        }
    }

    #[test]
    fn add_key_moves_from_logs_to_mapping_editor() {
        let mut dash = test_dash();

        assert!(!session::handle_key(
            &mut dash,
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)
        ));
        assert_eq!(dash.focus, DashFocus::Forwards);
        assert!(dash.forward_edit);

        dash.forward_edit = false;
        dash.focus = DashFocus::Logs;
        session::handle_key(&mut dash, KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(dash.focus, DashFocus::Forwards);
        session::handle_key(&mut dash, KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(dash.focus, DashFocus::Logs);
    }

    #[test]
    fn ctrl_c_quits_from_mapping_editor() {
        // 编辑器此前把 Ctrl-C 当成不可编辑键吞掉,只能先 Esc 关闭再退出。
        let mut dash = test_dash();
        dash.forward_edit = true;
        dash.forward_input = "8022:22".into();
        assert!(session::handle_key(
            &mut dash,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
        ));
        assert_eq!(dash.forward_input, "8022:22");
    }

    #[test]
    fn exit_confirmation_flow() {
        let q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);

        let mut dash = test_dash();
        assert!(!session::handle_key(&mut dash, q));
        assert!(dash.exit_confirming);
        // 确认层是模态:其余键不穿透(此处 'a' 不得打开映射编辑器)。
        assert!(!session::handle_key(
            &mut dash,
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)
        ));
        assert!(dash.exit_confirming);
        assert!(!dash.forward_edit);
        // Esc 取消回到会话。
        assert!(!session::handle_key(&mut dash, esc));
        assert!(!dash.exit_confirming);
        // 再次打开后 Enter 确认,请求断开退出。
        assert!(!session::handle_key(&mut dash, q));
        assert!(session::handle_key(&mut dash, enter));

        // Ctrl-C 在确认层中仍然立即退出(硬出口)。
        let mut dash = test_dash();
        assert!(!session::handle_key(&mut dash, q));
        assert!(session::handle_key(
            &mut dash,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
        ));
    }

    #[test]
    fn delete_before_handshake_reports() {
        let mut dash = test_dash();
        dash.session_ready = false;
        dash.focus = DashFocus::Forwards;
        assert!(!session::handle_key(
            &mut dash,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE)
        ));
        assert!(dash.forward_error.is_some());
    }

    #[test]
    fn device_picker_does_not_trap_form_navigation() {
        let mut form = FormState::from_devices(vec!["eth0".into(), "eth1".into()], None);
        let up = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);

        form.focus = Field::Device;
        form::handle_key(&mut form, up);
        assert_eq!(form.focus, Field::ClientName);

        form.focus = Field::Device;
        form::handle_key(&mut form, enter);
        assert!(form.device_selecting);
        form::handle_key(&mut form, down);
        assert_eq!(form.device, "eth0");
        form::handle_key(&mut form, enter);
        assert!(!form.device_selecting);
        form::handle_key(&mut form, up);
        assert_eq!(form.focus, Field::ClientName);

        form.focus = Field::Device;
        form::handle_key(&mut form, enter);
        assert!(form.device_selecting);
        assert_eq!(
            form::handle_key(&mut form, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            FormAction::None
        );
        assert!(!form.device_selecting);
    }

    #[test]
    fn form_esc_does_not_quit() {
        // 表单页 Esc 只用于收起/取消设备选择器,退出统一走 Ctrl-Q/Ctrl-C
        // (与提示行文案一致)。
        let mut form = FormState::from_devices(Vec::new(), None);
        assert_eq!(
            form::handle_key(&mut form, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            FormAction::None
        );
        assert_eq!(form.focus, Field::Psk);
    }

    #[test]
    fn picker_esc_reverts_and_tab_commits() {
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        let tab = KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE);

        let mut form = FormState::from_devices(vec!["eth0".into(), "eth1".into()], None);
        form.focus = Field::Device;

        // 打开 → 移动两格 → Esc 取消:回滚到打开前的值。
        form::handle_key(&mut form, enter);
        assert!(form.device_selecting);
        form::handle_key(&mut form, down);
        form::handle_key(&mut form, down);
        assert_eq!(form.device, "eth1");
        form::handle_key(&mut form, esc);
        assert!(!form.device_selecting);
        assert_eq!(form.device, "");
        assert_eq!(form.device_index, 0);

        // Tab 离开选择器按提交处理:保留移动后的选择。
        form::handle_key(&mut form, enter);
        form::handle_key(&mut form, down);
        assert_eq!(form.device, "eth0");
        form::handle_key(&mut form, tab);
        assert!(!form.device_selecting);
        assert_eq!(form.device, "eth0");
    }

    #[test]
    fn editing_clears_stale_form_error() {
        let mut form = FormState::from_devices(Vec::new(), None);
        form.error = Some("stale".into());
        form::handle_key(
            &mut form,
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
        );
        assert_eq!(form.psk, "x");
        assert!(form.error.is_none());

        form.error = Some("stale".into());
        form::handle_key(
            &mut form,
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE),
        );
        assert!(form.error.is_none());
    }

    #[test]
    fn scroll_windows() {
        // Lists shorter than the window never scroll.
        assert_eq!(selection_offset(3, 6, 2), 0);
        assert_eq!(selection_offset(0, 0, 0), 0);

        // Selection lists keep the selected row visible: the window only
        // moves down once the selection passes the bottom edge.
        assert_eq!(selection_offset(8, 6, 3), 0);
        assert_eq!(selection_offset(8, 6, 5), 0);
        assert_eq!(selection_offset(8, 6, 6), 1);
        assert_eq!(selection_offset(8, 6, 7), 2);
        assert_eq!(selection_offset(8, 6, 99), 2);
    }

    #[test]
    fn client_failure_returns_to_form() {
        // 硬错误 + 表单快照:回表单、显示原因、字段值保留。
        let mut dash = test_dash();
        let mut form = FormState::from_devices(Vec::new(), None);
        form.psk = "kept-psk".into();
        dash.resume_form = Some(form);
        match client_outcome_phase(&mut dash, Outcome::Failed("no such device".into())) {
            Phase::Form(form) => {
                assert_eq!(form.psk, "kept-psk");
                assert_eq!(form.error.as_deref(), Some("no such device"));
            }
            _ => panic!("client failure should return to the form"),
        }

        // 没有快照(防御路径)或非错误结果:仍然结束 TUI。
        let mut dash = test_dash();
        assert!(matches!(
            client_outcome_phase(&mut dash, Outcome::Failed("x".into())),
            Phase::Done(Outcome::Failed(_))
        ));
        let mut dash = test_dash();
        assert!(matches!(
            client_outcome_phase(&mut dash, Outcome::Disconnected),
            Phase::Done(Outcome::Disconnected)
        ));
    }
}
