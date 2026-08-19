use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use landscape_terrain_proto::ipstack::{IpStack, SocketHandle};
use landscape_terrain_proto::protocol::crypto::{MasterKey, SessionCrypto};
use landscape_terrain_proto::protocol::frame;
use landscape_terrain_proto::protocol::session::{
    ClientPhase, ClientSession, HANDSHAKE_TIMEOUT, MAX_RETRIES,
};
use landscape_terrain_proto::protocol::{TYPE_AUTH_ACK, TYPE_AUTH_NACK, TYPE_DATA, TYPE_RESP};
use landscape_terrain_proto::transport::{Frame, Link, fmt_mac};
use tokio::sync::mpsc;

mod forward;
mod session;

use forward::{BridgeMsg, Conn};
use session::{SessionEnd, session_loop};

pub const BROADCAST: [u8; 6] = [0xff; 6];
const RETRY_BACKOFF: Duration = Duration::from_secs(3);
const MAX_MISSED_KEEPALIVES: u32 = 3;
// Drive the userspace TCP stack frequently enough to keep bulk transfers
// moving; a 25 ms tick caps a full-duplex relay near the e2e timeout.
const POLL_INTERVAL: Duration = Duration::from_millis(1);
/// Max failed session-frame opens per second (bad tag or replay). Bounds
/// the decrypt work a spoofed-frame flood can force on the session loop; a
/// legitimate session never fails opens.
const MAX_FAILED_OPENS_PER_SEC: u32 = 200;

/// Windowed counter that drops session frames once the failed-open budget
/// for the current second is spent.
struct FailBudget {
    window_start: std::time::Instant,
    fails: u32,
}

impl Default for FailBudget {
    fn default() -> Self {
        Self {
            window_start: std::time::Instant::now(),
            fails: 0,
        }
    }
}

impl FailBudget {
    /// Spend one failed-open token; false once the budget for this second
    /// is drained (until the window rolls over).
    fn allow(&mut self) -> bool {
        let now = std::time::Instant::now();
        if now.duration_since(self.window_start) >= Duration::from_secs(1) {
            self.window_start = now;
            self.fails = 0;
        }
        self.fails += 1;
        self.fails <= MAX_FAILED_OPENS_PER_SEC
    }
}

/// One `--forward LOCAL:DST` rule.
pub type Forward = (u16, u16);

/// A runtime change to the local forwarding listeners.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForwardCommand {
    Add(Forward),
    Remove(Forward),
}

/// Status while the client is establishing or re-establishing a session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionStatus {
    Searching,
    Authenticating,
    AuthRejected(String),
    LinkLost,
    PeerClosed,
}

/// Listener lifecycle state for one forwarding rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForwardStatus {
    Starting,
    Listening,
    Failed,
}

/// A validation error for a runtime forwarding request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ForwardRejection {
    Duplicate,
    DestinationNotAdvertised { port: u16 },
}

/// Events consumed by interactive frontends.
pub enum ClientEvent {
    SessionStatus(SessionStatus),
    SessionReady {
        session_id: u32,
        server_mac: String,
        advertised_ports: Vec<u16>,
    },
    ForwardStatus {
        forward: Forward,
        status: ForwardStatus,
    },
    ForwardRejected {
        forward: Forward,
        reason: ForwardRejection,
    },
}

/// Severity of a client status line.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

/// Where client status lines go: the process's own stdout/stderr (CLI mode)
/// or a channel consumed by the TUI.
#[derive(Clone)]
pub enum LogSink {
    Stdio,
    Chan(tokio::sync::mpsc::UnboundedSender<(LogLevel, String)>),
}

impl LogSink {
    pub fn emit(&self, level: LogLevel, msg: String) {
        match self {
            LogSink::Stdio => match level {
                LogLevel::Info => println!("{msg}"),
                LogLevel::Warn | LogLevel::Error => eprintln!("{msg}"),
            },
            LogSink::Chan(tx) => {
                let _ = tx.send((level, msg));
            }
        }
    }
}

pub struct ClientConfig<'a> {
    pub devs: &'a [String],
    pub ethertype: u16,
    pub mac: Option<[u8; 6]>,
    pub user: &'a str,
    pub psk: &'a str,
    pub client_name: &'a str,
    pub forwards: &'a [Forward],
    /// Discovery token sent in DISCOVER (empty = not sent).
    pub token: &'a str,
    /// Where status lines go.
    pub log: LogSink,
    /// External shutdown trigger (TUI quit key); the client exits the
    /// session loop and tears down when notified.
    pub shutdown: Option<std::sync::Arc<tokio::sync::Notify>>,
    /// Runtime forwarding changes, used by interactive clients.
    pub forward_control: Option<tokio::sync::mpsc::UnboundedReceiver<ForwardCommand>>,
    /// Session and forwarding events for interactive frontends.
    pub events: Option<tokio::sync::mpsc::UnboundedSender<ClientEvent>>,
}

pub async fn run(mut cfg: ClientConfig<'_>) -> Result<(), Box<dyn std::error::Error>> {
    if cfg.psk.len() < 12 {
        cfg.log.emit(
            LogLevel::Warn,
            format!(
                "warning: psk is only {} chars — it is stretched with scrypt at startup, but prefer a long random secret over a passphrase",
                cfg.psk.len()
            ),
        );
    }
    let mut tx = Link::open(cfg.devs, cfg.ethertype, cfg.mac)?;
    // The psk is stretched into a master key once at startup (scrypt); all
    // derivations below feed on it, so a weak psk costs an offline attacker
    // ~32 MiB and ~100 ms per guess instead of a single sha256.
    let master = MasterKey::derive(cfg.psk.as_bytes());
    cfg.log.emit(
        LogLevel::Info,
        format!(
            "client '{}' ready on {} (ethertype 0x{:04x})",
            cfg.client_name,
            tx.names().join(", "),
            cfg.ethertype
        ),
    );
    let notify = cfg
        .shutdown
        .clone()
        .unwrap_or_else(|| std::sync::Arc::new(tokio::sync::Notify::new()));

    // One signal task for the whole client lifetime: a termination signal
    // (or the TUI quit key) must interrupt the handshake and retry backoff
    // too, not just an established session.
    drop(tokio::spawn({
        let n = notify.clone();
        async move {
            wait_for_shutdown().await;
            n.notify_one();
        }
    }));
    let mut forward_control = cfg.forward_control.take();
    let mut active_forwards = cfg.forwards.to_vec();

    loop {
        let mut sess = ClientSession::new();
        emit_event(
            &cfg.events,
            ClientEvent::SessionStatus(SessionStatus::Searching),
        );
        let handshake = tokio::select! {
            r = handshake(&mut tx, &mut sess, &cfg, &master) => r?,
            _ = notify.notified() => return Ok(()),
        };
        let Some((server_mac, advertised_ports)) = handshake else {
            cfg.log.emit(
                LogLevel::Info,
                format!("handshake failed, retrying in {}s", RETRY_BACKOFF.as_secs()),
            );
            tokio::select! {
                _ = tokio::time::sleep(RETRY_BACKOFF) => {}
                _ = notify.notified() => return Ok(()),
            }
            continue;
        };
        let sid = sess.session_id().expect("session id after handshake");
        let keys = sess.keys().expect("session keys after handshake").clone();
        cfg.log.emit(
            LogLevel::Info,
            format!(
                "session {sid} established with {} (encrypted)",
                fmt_mac(&server_mac)
            ),
        );
        emit_event(
            &cfg.events,
            ClientEvent::SessionReady {
                session_id: sid,
                server_mac: fmt_mac(&server_mac),
                advertised_ports: advertised_ports.clone(),
            },
        );

        let end = session_loop(
            &mut tx,
            &server_mac,
            sid,
            keys,
            &cfg,
            notify.clone(),
            &mut forward_control,
            &mut active_forwards,
            &advertised_ports,
        )
        .await;
        let session_failed = end.is_err();
        match end {
            Ok(SessionEnd::Shutdown) => return Ok(()),
            Ok(SessionEnd::LinkLost) => {
                emit_event(
                    &cfg.events,
                    ClientEvent::SessionStatus(SessionStatus::LinkLost),
                );
                cfg.log
                    .emit(LogLevel::Info, "  link lost, restarting handshake".into());
            }
            Ok(SessionEnd::PeerClosed) => {
                emit_event(
                    &cfg.events,
                    ClientEvent::SessionStatus(SessionStatus::PeerClosed),
                );
                cfg.log
                    .emit(LogLevel::Info, "  session closed by peer".into());
            }
            Err(e) => cfg
                .log
                .emit(LogLevel::Error, format!("  session error: {e}")),
        }
        if session_failed {
            emit_event(
                &cfg.events,
                ClientEvent::SessionStatus(SessionStatus::LinkLost),
            );
        }
    }
}

fn emit_event(
    events: &Option<tokio::sync::mpsc::UnboundedSender<ClientEvent>>,
    event: ClientEvent,
) {
    if let Some(tx) = events {
        let _ = tx.send(event);
    }
}

/// Resolves once a termination signal arrives (graceful teardown path).
#[cfg(unix)]
async fn wait_for_shutdown() {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("installing SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}

#[cfg(not(unix))]
async fn wait_for_shutdown() {
    let mut brk = tokio::signal::windows::ctrl_break().expect("installing Ctrl-Break handler");
    let mut close = tokio::signal::windows::ctrl_close().expect("installing Ctrl-Close handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = brk.recv() => {}
        _ = close.recv() => {}
    }
}

async fn handshake(
    tx: &mut Link,
    sess: &mut ClientSession,
    cfg: &ClientConfig<'_>,
    master: &MasterKey,
) -> Result<Option<([u8; 6], Vec<u16>)>, Box<dyn std::error::Error>> {
    let mut saw_resp = false;
    while sess.retransmit_allowed() {
        sess.bump_retry();
        cfg.log.emit(
            LogLevel::Info,
            format!("discover: broadcast (try {}/{MAX_RETRIES})", sess.retries()),
        );
        let token = (!cfg.token.is_empty()).then_some(cfg.token);
        let discover = sess.discover_frame(cfg.client_name, token, master);
        tx.send(&BROADCAST, cfg.ethertype, &discover)?;

        let deadline = tokio::time::Instant::now() + HANDSHAKE_TIMEOUT;
        let Some(resp_frame) = recv_until(
            tx,
            cfg.ethertype,
            |f| frame::decode(&f.payload).is_ok_and(|l| l.msg_type == TYPE_RESP),
            deadline,
        )
        .await?
        else {
            cfg.log.emit(
                LogLevel::Info,
                format!("  no response within {}s", HANDSHAKE_TIMEOUT.as_secs()),
            );
            continue;
        };
        let server_mac = resp_frame.src;
        let resp_proto = frame::decode(&resp_frame.payload)?;
        // Opening the RESP proves the server holds the psk, and the echoed
        // discover_id proves it answers this very attempt: everything else
        // is a rogue or a replay.
        let Some((resp, auth_req)) = sess.on_resp(&resp_proto, cfg.user, master) else {
            cfg.log.emit(
                LogLevel::Warn,
                "  response failed server authentication, rediscovering".into(),
            );
            continue;
        };
        saw_resp = true;
        cfg.log.emit(
            LogLevel::Info,
            format!(
                "  discovered '{}' at {} (forwards: {})",
                resp.device_name,
                fmt_mac(&server_mac),
                if resp.ports.is_empty() {
                    "not advertised".to_string()
                } else {
                    resp.ports
                        .iter()
                        .map(|p| p.to_string())
                        .collect::<Vec<_>>()
                        .join(",")
                }
            ),
        );
        warn_unadvertised_ports(cfg, &resp.ports);
        cfg.log.emit(
            LogLevel::Info,
            format!("  auth request sent for user '{}'", cfg.user),
        );
        emit_event(
            &cfg.events,
            ClientEvent::SessionStatus(SessionStatus::Authenticating),
        );
        tx.send(&server_mac, cfg.ethertype, &auth_req)?;

        let deadline = tokio::time::Instant::now() + HANDSHAKE_TIMEOUT;
        // Auth frames must come from the MAC that answered the RESP: a
        // spoofed lockout NACK from anywhere else is ignored.
        let Some(auth_frame) = recv_until(
            tx,
            cfg.ethertype,
            |f| {
                f.src == server_mac
                    && frame::decode(&f.payload)
                        .is_ok_and(|l| l.msg_type == TYPE_AUTH_ACK || l.msg_type == TYPE_AUTH_NACK)
            },
            deadline,
        )
        .await?
        else {
            cfg.log
                .emit(LogLevel::Info, "  auth timeout, rediscovering".into());
            continue;
        };
        let auth_proto = frame::decode(&auth_frame.payload)?;
        sess.on_auth_frame(&auth_proto, master);
        if sess.session_id().is_some() {
            return Ok(Some((server_mac, resp.ports)));
        }
        if let ClientPhase::Rejected(reason) = &sess.phase {
            emit_event(
                &cfg.events,
                ClientEvent::SessionStatus(SessionStatus::AuthRejected(reason.clone())),
            );
            cfg.log
                .emit(LogLevel::Warn, format!("  auth rejected: {reason}"));
        }
        return Ok(None);
    }
    if !saw_resp {
        cfg.log.emit(
            LogLevel::Warn,
            "  no server response at all — check that psk, token and ethertype match the server"
                .into(),
        );
    }
    Ok(None)
}

/// Warn when a `--forward` destination port is not in the server's
/// advertised list (the server would reject it anyway).
fn warn_unadvertised_ports(cfg: &ClientConfig<'_>, advertised: &[u16]) {
    if advertised.is_empty() {
        return;
    }
    for &(_, dst) in cfg.forwards {
        if !advertised.contains(&dst) {
            cfg.log.emit(
                LogLevel::Warn,
                format!(
                    "  warning: server does not advertise forwarding to port {dst} (may be rejected)"
                ),
            );
        }
    }
}

/// Block until a frame satisfying `pred` arrives, or the deadline passes.
/// The predicate sees the full transport frame (src MAC + Terrain header).
async fn recv_until(
    tx: &mut Link,
    ethertype: u16,
    pred: impl Fn(&Frame) -> bool,
    deadline: tokio::time::Instant,
) -> Result<Option<Frame>, Box<dyn std::error::Error>> {
    loop {
        match tokio::time::timeout_at(deadline, tx.recv(ethertype)).await {
            Ok(Ok(f)) => {
                if pred(&f) {
                    return Ok(Some(f));
                }
            }
            Ok(Err(e)) => return Err(e),
            Err(_) => return Ok(None),
        }
    }
}

/// Pump the stack: send outbound IP packets over the link, move bytes
/// between the kernel sockets and the stack's TCP sockets, and reap closed
/// connections.
#[allow(clippy::too_many_arguments)]
fn pump(
    stack: &mut IpStack,
    tx: &mut Link,
    server_mac: &[u8; 6],
    sid: u32,
    crypto: &mut SessionCrypto,
    ethertype: u16,
    conns: &mut HashMap<SocketHandle, Conn>,
    pending_tx: &mut HashMap<SocketHandle, VecDeque<Vec<u8>>>,
    pending_tx_bytes: &mut usize,
) -> Result<(), Box<dyn std::error::Error>> {
    for pkt in stack.poll() {
        let raw = crypto.seal(TYPE_DATA, sid, &pkt);
        tx.send(server_mac, ethertype, &raw)?;
    }

    let handles: Vec<SocketHandle> = conns.keys().copied().collect();
    let mut reap: Vec<SocketHandle> = Vec::new();
    for h in handles {
        if let Some(q) = pending_tx.get_mut(&h) {
            while let Some(front) = q.front_mut() {
                let n = stack.send_bytes(h, front);
                if n == 0 {
                    break;
                }
                *pending_tx_bytes = pending_tx_bytes.saturating_sub(n);
                front.drain(..n);
                if front.is_empty() {
                    q.pop_front();
                }
            }
            if q.is_empty() {
                pending_tx.remove(&h);
            }
        }
        if !pending_tx.contains_key(&h) && conns[&h].close_after_flush {
            stack.close_socket(h);
            conns.get_mut(&h).unwrap().close_after_flush = false;
        }

        let mut buf = [0u8; 4096];
        loop {
            let Ok(permit) = conns[&h].from_tx.try_reserve() else {
                break;
            };
            let n = stack.recv_bytes(h, &mut buf);
            if n == 0 {
                break;
            }
            permit.send(BridgeMsg::Data(buf[..n].to_vec()));
        }

        let from_drained = conns[&h].from_tx.is_closed()
            || conns[&h].from_tx.capacity() == conns[&h].from_tx.max_capacity();
        if stack.socket_closed(h) && from_drained {
            reap.push(h);
        } else if stack.peer_eof(h) && !conns[&h].peer_eof_sent {
            match conns[&h].from_tx.try_send(BridgeMsg::PeerEof) {
                Ok(()) => conns.get_mut(&h).unwrap().peer_eof_sent = true,
                Err(mpsc::error::TrySendError::Full(_)) => {}
                Err(mpsc::error::TrySendError::Closed(_)) => stack.close_socket(h),
            }
        }
    }
    for h in reap {
        stack.remove_socket(h);
        if let Some(q) = pending_tx.remove(&h) {
            let dropped = q.iter().map(Vec::len).sum::<usize>();
            *pending_tx_bytes = pending_tx_bytes.saturating_sub(dropped);
        }
        conns.remove(&h);
    }
    Ok(())
}
