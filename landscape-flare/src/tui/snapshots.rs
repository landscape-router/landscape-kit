//! 整屏 insta 快照:把连接表单与会话仪表盘的每个状态在固定尺寸下经
//! `TestBackend` 渲染的完整布局固化为 `snapshots/*.snap` 文本文件,任何一行
//! 偏移、列错位或截断都会在 `assert_snapshot!` 的 diff 里显形。
//!
//! 维护方式:有意变更布局后运行
//! `INSTA_UPDATE=always cargo test -p landscape-flare tui::snapshots`
//! (或 `cargo insta review`)更新快照并逐屏审阅。屏幕内容全部来自直接构造
//! 的 `FormState` / `DashState`(经通道 + `drain()` 推进事件),不含时钟、
//! 接口探测或网络输入,快照逐字符确定。

use std::collections::VecDeque;
use std::sync::Arc;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use tokio::sync::{Notify, mpsc};

use super::form::FormState;
use super::{ConnectionState, DashFocus, DashState};
use crate::client::{
    ClientEvent, Forward, ForwardCommand, ForwardStatus as ClientForwardStatus, LogLevel,
    SessionStatus,
};
use crate::i18n::Language;

/// 语言是进程级全局(thread-local + 全局回退),守卫在测试结束/换段时还原,
/// 避免并行测试互相污染渲染文案。
pub(super) struct LanguageGuard(Language);

impl LanguageGuard {
    pub(super) fn set(language: Language) -> Self {
        let previous = crate::i18n::current();
        crate::i18n::configure(language);
        Self(previous)
    }
}

impl Drop for LanguageGuard {
    fn drop(&mut self) {
        crate::i18n::configure(self.0);
    }
}

fn assert_form_snapshot(
    name: &'static str,
    width: u16,
    height: u16,
    language: Language,
    prepare: impl FnOnce(&mut FormState),
) {
    let _language = LanguageGuard::set(language);
    let settings = insta::Settings::clone_current();
    settings.bind(move || {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut form = FormState::from_interface_devices(Vec::new(), None);
        prepare(&mut form);
        super::draw_form(&mut terminal, &form).unwrap();
        insta::assert_snapshot!(name, terminal.backend());
    });
}

/// 通道 + `drain()` 驱动的仪表盘夹具:保持 sender 存活,经真实事件路径
/// (`apply_event`)推进状态,与运行期的客户端任务行为一致。
struct DashFixture {
    dash: DashState,
    log_tx: mpsc::UnboundedSender<(LogLevel, String)>,
    event_tx: mpsc::UnboundedSender<ClientEvent>,
    _forward_rx: mpsc::UnboundedReceiver<ForwardCommand>,
}

fn dash_fixture() -> DashFixture {
    let (log_tx, log_rx) = mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let (forward_tx, forward_rx) = mpsc::unbounded_channel();
    let dash = DashState {
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
        session_ready: false,
        connection: ConnectionState::Searching,
        advertised_ports: Vec::new(),
        forward_states: Default::default(),
        device: None,
        notify: Arc::new(Notify::new()),
        forward_tx,
        client: None,
    };
    DashFixture {
        dash,
        log_tx,
        event_tx,
        _forward_rx: forward_rx,
    }
}

impl DashFixture {
    fn log(&mut self, level: LogLevel, message: &str) {
        self.log_tx.send((level, message.into())).unwrap();
        self.dash.drain();
    }

    fn event(&mut self, event: ClientEvent) {
        self.event_tx.send(event).unwrap();
        self.dash.drain();
    }

    /// 走完「发现 → 认证 → 就绪」事件序列,并落几条常规日志。
    fn ready_session(&mut self, forwards: &[Forward], device: Option<&str>) {
        self.dash.device = device.map(str::to_owned);
        self.log(
            LogLevel::Info,
            "listening for server advertisements on the wire",
        );
        self.event(ClientEvent::SessionStatus(SessionStatus::Authenticating));
        self.log(
            LogLevel::Info,
            "server discovered, exchanging handshake keys",
        );
        self.event(ClientEvent::SessionReady {
            session_id: 42,
            server_mac: "aa:bb:cc:dd:ee:ff".into(),
            advertised_ports: vec![22, 443],
        });
        for forward in forwards {
            self.dash.forwards.push(*forward);
        }
    }

    fn forward_state(&mut self, forward: Forward, status: ClientForwardStatus) {
        self.event(ClientEvent::ForwardStatus { forward, status });
    }
}

fn assert_dash_snapshot(
    name: &'static str,
    width: u16,
    height: u16,
    language: Language,
    prepare: impl FnOnce(&mut DashFixture),
) {
    let _language = LanguageGuard::set(language);
    let settings = insta::Settings::clone_current();
    settings.bind(move || {
        let mut fixture = dash_fixture();
        prepare(&mut fixture);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        super::draw_dash(&mut terminal, &fixture.dash).unwrap();
        insta::assert_snapshot!(name, terminal.backend());
    });
}

// ---- 连接表单 ----

#[test]
fn snapshot_form_default_en() {
    assert_form_snapshot("form-default-en", 100, 28, Language::En, |_| {});
}

#[test]
fn snapshot_form_filled_en() {
    assert_form_snapshot("form-filled-en", 100, 28, Language::En, |form| {
        form.psk = "correct-horse-battery".into();
        form.user = "ops".into();
        form.client_name = "office-pc".into();
        form.ethertype = "0x88b6".into();
        form.token = "6f9c2ab1".into();
        form.focus = super::form::Field::Connect;
    });
}

#[test]
fn snapshot_form_psk_revealed_en() {
    assert_form_snapshot("form-psk-revealed-en", 100, 28, Language::En, |form| {
        form.psk = "correct-horse-battery".into();
        form.show_psk = true;
    });
}

#[test]
fn snapshot_form_validation_error_en() {
    assert_form_snapshot("form-validation-error-en", 100, 28, Language::En, |form| {
        form.user = "ops".into();
        // 走真实路径:Connect 上 Enter 触发 build(),空 PSK 的错误回写表单。
        let enter = crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Enter,
            crossterm::event::KeyModifiers::NONE,
        );
        form.focus = super::form::Field::Connect;
        let _ = super::form::handle_key(form, enter);
        form.error = Some(form.build().unwrap_err());
    });
}

#[test]
fn snapshot_form_devices_error_en() {
    assert_form_snapshot("form-devices-error-en", 100, 28, Language::En, |form| {
        form.devices_err = Some("pcap error: no permissions on /dev/bpf".into());
    });
}

#[test]
fn snapshot_form_device_picker_en() {
    assert_form_snapshot("form-device-picker-en", 100, 28, Language::En, |form| {
        *form = FormState::from_interface_devices(
            vec![
                landscape_terrain_proto::transport::Interface {
                    name: "enp5s0".into(),
                    description: None,
                },
                landscape_terrain_proto::transport::Interface {
                    name: r"\Device\NPF_{GUID-1}".into(),
                    description: Some("Intel Wi-Fi 6 AX200 160MHz".into()),
                },
                landscape_terrain_proto::transport::Interface {
                    name: r"\Device\NPF_{GUID-2}".into(),
                    description: Some("USB 3.0 GbE (Realtek)".into()),
                },
            ],
            None,
        );
        form.focus = super::form::Field::Device;
        form.device_selecting = true;
        form.device_index = 1;
    });
}

#[test]
fn snapshot_form_device_picker_overflow_en() {
    assert_form_snapshot(
        "form-device-picker-overflow-en",
        100,
        28,
        Language::En,
        |form| {
            let devices = (0..8).map(|i| landscape_terrain_proto::transport::Interface {
                name: format!("eth{i}"),
                description: Some(format!("NIC port {i}")),
            });
            *form = FormState::from_interface_devices(devices.collect(), None);
            form.focus = super::form::Field::Device;
            form.device_selecting = true;
            form.device_index = 6;
        },
    );
}

#[test]
fn snapshot_form_device_picker_budget_100x24_en() {
    assert_form_snapshot(
        "form-device-picker-budget-100x24-en",
        100,
        24,
        Language::En,
        |form| {
            // 9 个选项在 24 行高度里只放得下 1 行选项 + 溢出提示,
            // 溢出提示行不允许再被布局器挤掉。
            let devices = (0..8).map(|i| landscape_terrain_proto::transport::Interface {
                name: format!("eth{i}"),
                description: Some(format!("NIC port {i}")),
            });
            *form = FormState::from_interface_devices(devices.collect(), None);
            form.focus = super::form::Field::Device;
            form.device_selecting = true;
            form.device_index = 6;
        },
    );
}

#[test]
fn snapshot_form_80x24_en() {
    assert_form_snapshot("form-80x24-en", 80, 24, Language::En, |form| {
        form.psk = "correct-horse-battery".into();
        form.user = "ops".into();
        form.client_name = "office-pc".into();
        form.token = "6f9c2ab1".into();
    });
}

#[test]
fn snapshot_form_too_small_71x17_en() {
    assert_form_snapshot("too-small-71x17-en", 71, 17, Language::En, |form| {
        form.psk = "correct-horse-battery".into();
    });
}

#[test]
fn snapshot_form_too_small_71x17_zh() {
    assert_form_snapshot("too-small-71x17-zh", 71, 17, Language::Zh, |_| {});
}

#[test]
fn snapshot_form_minimum_60x24_en() {
    assert_form_snapshot("form-minimum-60x24-en", 60, 24, Language::En, |form| {
        form.psk = "correct-horse-battery".into();
        form.user = "ops".into();
        // 错误行占满最后一行:最小尺寸下必须仍然完整。
        form.error = Some(crate::tr!("tui.psk_required"));
    });
}

#[test]
fn snapshot_form_filled_zh() {
    assert_form_snapshot("form-filled-zh", 100, 28, Language::Zh, |form| {
        form.psk = "correct-horse-battery".into();
        form.user = "ops".into();
        form.client_name = "office-pc".into();
        form.ethertype = "0x88b6".into();
        form.focus = super::form::Field::Connect;
    });
}

#[test]
fn snapshot_form_device_picker_zh() {
    assert_form_snapshot("form-device-picker-zh", 100, 28, Language::Zh, |form| {
        *form = FormState::from_interface_devices(
            vec![
                landscape_terrain_proto::transport::Interface {
                    name: "enp5s0".into(),
                    description: None,
                },
                landscape_terrain_proto::transport::Interface {
                    name: "wlp3s0".into(),
                    description: Some("Intel Wi-Fi 6 AX200 160MHz".into()),
                },
            ],
            None,
        );
        form.focus = super::form::Field::Device;
        form.device_selecting = true;
        form.device_index = 1;
    });
}

// ---- 会话仪表盘 ----

#[test]
fn snapshot_dash_searching_en() {
    assert_dash_snapshot("dash-searching-en", 100, 28, Language::En, |fixture| {
        fixture.dash.device = Some("enp5s0".into());
        fixture.log(
            LogLevel::Info,
            "listening for server advertisements on the wire",
        );
    });
}

#[test]
fn snapshot_dash_ready_empty_en() {
    assert_dash_snapshot("dash-ready-empty-en", 100, 28, Language::En, |fixture| {
        fixture.ready_session(&[], Some("enp5s0"));
        fixture.log(LogLevel::Info, "tunnel established, waiting for mappings");
        // 超过可见行数的日志触发底部提示行,一并钉住。
        for i in 0..24 {
            fixture.log(LogLevel::Info, &format!("keepalive exchange {i} ok"));
        }
    });
}

#[test]
fn snapshot_dash_ready_mappings_en() {
    assert_dash_snapshot("dash-ready-mappings-en", 100, 28, Language::En, |fixture| {
        let forwards = [(8022, 22), (8443, 443), (8080, 80)];
        fixture.ready_session(&forwards, None);
        fixture.forward_state((8022, 22), ClientForwardStatus::Listening);
        fixture.forward_state((8443, 443), ClientForwardStatus::Starting);
        fixture.forward_state((8080, 80), ClientForwardStatus::Failed);
        fixture.dash.focus = DashFocus::Forwards;
        fixture.dash.forward_index = 1;
    });
}

#[test]
fn snapshot_dash_mapping_overflow_en() {
    assert_dash_snapshot(
        "dash-mapping-overflow-en",
        100,
        28,
        Language::En,
        |fixture| {
            let forwards: Vec<Forward> = (0..7).map(|i| (20_000 + i * 10, 8000 + i)).collect();
            fixture.ready_session(&forwards, None);
            for forward in &forwards {
                fixture.forward_state(*forward, ClientForwardStatus::Listening);
            }
            fixture.dash.focus = DashFocus::Forwards;
            fixture.dash.forward_index = 4;
        },
    );
}

#[test]
fn snapshot_dash_add_mapping_en() {
    assert_dash_snapshot("dash-add-mapping-en", 100, 28, Language::En, |fixture| {
        let forwards = [(8022, 22)];
        fixture.ready_session(&forwards, None);
        fixture.forward_state((8022, 22), ClientForwardStatus::Listening);
        fixture.dash.focus = DashFocus::Forwards;
        fixture.dash.forward_edit = true;
        fixture.dash.forward_input = "8043:44".into();
    });
}

#[test]
fn snapshot_dash_mapping_error_en() {
    assert_dash_snapshot("dash-mapping-error-en", 100, 28, Language::En, |fixture| {
        fixture.ready_session(&[], None);
        fixture.dash.forward_error = Some(crate::tr!("tui.port_not_advertised", port = 8080u16));
    });
}

#[test]
fn snapshot_dash_auth_rejected_en() {
    assert_dash_snapshot("dash-auth-rejected-en", 100, 28, Language::En, |fixture| {
        fixture.log(
            LogLevel::Info,
            "listening for server advertisements on the wire",
        );
        fixture.event(ClientEvent::SessionStatus(SessionStatus::Authenticating));
        fixture.log(
            LogLevel::Warn,
            "server discovered, exchanging handshake keys",
        );
        fixture.log(
            LogLevel::Error,
            "psk rejected: 3 attempts remaining before lockout",
        );
        fixture.event(ClientEvent::SessionStatus(SessionStatus::AuthRejected(
            "psk rejected by server".into(),
        )));
    });
}

#[test]
fn snapshot_dash_link_lost_en() {
    assert_dash_snapshot("dash-link-lost-en", 100, 28, Language::En, |fixture| {
        let forwards = [(8022, 22)];
        fixture.ready_session(&forwards, Some("enp5s0"));
        fixture.forward_state((8022, 22), ClientForwardStatus::Listening);
        fixture.log(
            LogLevel::Warn,
            "no advertisement seen for 10s, searching again",
        );
        fixture.event(ClientEvent::SessionStatus(SessionStatus::LinkLost));
    });
}

#[test]
fn snapshot_dash_peer_closed_en() {
    assert_dash_snapshot("dash-peer-closed-en", 100, 28, Language::En, |fixture| {
        fixture.ready_session(&[], Some("enp5s0"));
        fixture.log(LogLevel::Error, "server closed the session (shutdown)");
        fixture.event(ClientEvent::SessionStatus(SessionStatus::PeerClosed));
    });
}

#[test]
fn snapshot_dash_logs_scrolled_en() {
    assert_dash_snapshot("dash-logs-scrolled-en", 100, 28, Language::En, |fixture| {
        fixture.ready_session(&[], None);
        for i in 0..25 {
            fixture.log(LogLevel::Info, &format!("tunnel keepalive exchange {i} ok"));
        }
        fixture.dash.scroll = 6;
    });
}

#[test]
fn snapshot_dash_logs_top_en() {
    assert_dash_snapshot("dash-logs-top-en", 100, 28, Language::En, |fixture| {
        fixture.ready_session(&[], None);
        for i in 0..25 {
            fixture.log(LogLevel::Info, &format!("tunnel keepalive exchange {i} ok"));
        }
        // Home:scroll 夹到「窗口恰好贴顶」,顶部提示行可见而非越界空屏。
        fixture.dash.scroll = usize::MAX;
    });
}

#[test]
fn snapshot_dash_log_wrap_60x17_en() {
    assert_dash_snapshot("dash-log-wrap-60x17-en", 60, 17, Language::En, |fixture| {
        fixture.ready_session(&[], None);
        fixture.log(
            LogLevel::Info,
            "tunnel established · ethertype 0x88b6 · retransmit window 64 · keepalive 5s · device auto",
        );
        fixture.log(
            LogLevel::Warn,
            "advertisement interval jitter above threshold",
        );
        fixture.log(
            LogLevel::Error,
            "duplicate frame detected, dropping retransmit",
        );
    });
}

#[test]
fn snapshot_dash_80x24_en() {
    assert_dash_snapshot("dash-80x24-en", 80, 24, Language::En, |fixture| {
        let forwards = [(8022, 22), (8443, 443)];
        fixture.ready_session(&forwards, Some("enp5s0"));
        fixture.forward_state((8022, 22), ClientForwardStatus::Listening);
        fixture.forward_state((8443, 443), ClientForwardStatus::Starting);
        fixture.log(LogLevel::Info, "tunnel established, waiting for mappings");
    });
}

#[test]
fn snapshot_dash_too_small_71x17_en() {
    assert_dash_snapshot("dash-too-small-71x17-en", 71, 17, Language::En, |fixture| {
        fixture.ready_session(&[], None);
    });
}

#[test]
fn snapshot_dash_exit_confirm_en() {
    assert_dash_snapshot("dash-exit-confirm-en", 100, 28, Language::En, |fixture| {
        let forwards = [(8022, 22)];
        fixture.ready_session(&forwards, Some("enp5s0"));
        fixture.forward_state((8022, 22), ClientForwardStatus::Listening);
        fixture.dash.focus = DashFocus::Forwards;
        fixture.dash.exit_confirming = true;
    });
}

#[test]
fn snapshot_dash_exit_confirm_zh() {
    assert_dash_snapshot("dash-exit-confirm-zh", 100, 28, Language::Zh, |fixture| {
        let forwards = [(8022, 22)];
        fixture.ready_session(&forwards, Some("enp5s0"));
        fixture.forward_state((8022, 22), ClientForwardStatus::Listening);
        fixture.dash.focus = DashFocus::Forwards;
        fixture.dash.exit_confirming = true;
    });
}

#[test]
fn snapshot_dash_ready_mappings_zh() {
    assert_dash_snapshot("dash-ready-mappings-zh", 100, 28, Language::Zh, |fixture| {
        let forwards = [(8022, 22), (8443, 443)];
        fixture.ready_session(&forwards, Some("enp5s0"));
        fixture.forward_state((8022, 22), ClientForwardStatus::Listening);
        fixture.forward_state((8443, 443), ClientForwardStatus::Failed);
        fixture.dash.focus = DashFocus::Forwards;
        fixture.dash.forward_index = 0;
    });
}
