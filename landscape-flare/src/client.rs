use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use landscape_terrain_proto::ipstack::{
    CLIENT_ADDR, INTERNAL_PORT, IpStack, SERVER_ADDR, SocketHandle, StackMsg,
};
use landscape_terrain_proto::protocol::crypto::{Dir, MasterKey, SessionCrypto, SessionKeys};
use landscape_terrain_proto::protocol::frame;
use landscape_terrain_proto::protocol::session::{
    ClientPhase, ClientSession, HANDSHAKE_TIMEOUT, KEEPALIVE_INTERVAL, MAX_RETRIES,
};
use landscape_terrain_proto::protocol::{
    TYPE_AUTH_ACK, TYPE_AUTH_NACK, TYPE_DATA, TYPE_KEEPALIVE, TYPE_RESP, TYPE_TEARDOWN,
};
use landscape_terrain_proto::transport::{Frame, Link, fmt_mac};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};

pub const BROADCAST: [u8; 6] = [0xff; 6];
const RETRY_BACKOFF: Duration = Duration::from_secs(3);
const MAX_MISSED_KEEPALIVES: u32 = 3;
const POLL_INTERVAL: Duration = Duration::from_millis(25);
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

/// Events consumed by interactive frontends.
pub enum ClientEvent {
    SessionReady { advertised_ports: Vec<u16> },
    SessionLost,
    ForwardRejected { forward: Forward, reason: String },
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

/// Per-connection state on the client side.
struct Conn {
    /// Bytes from the stack to the kernel socket.
    from_tx: mpsc::Sender<Vec<u8>>,
    forward: Forward,
    close_tx: Option<oneshot::Sender<()>>,
}

/// Why a session loop ended.
enum SessionEnd {
    /// Keepalives went unanswered: the link is gone.
    LinkLost,
    /// The server sent a teardown.
    PeerClosed,
    /// SIGINT/SIGTERM: graceful shutdown (TEARDOWN is sent first).
    Shutdown,
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
                emit_event(&cfg.events, ClientEvent::SessionLost);
                cfg.log
                    .emit(LogLevel::Info, "  link lost, restarting handshake".into());
            }
            Ok(SessionEnd::PeerClosed) => {
                emit_event(&cfg.events, ClientEvent::SessionLost);
                cfg.log
                    .emit(LogLevel::Info, "  session closed by peer".into());
            }
            Err(e) => cfg
                .log
                .emit(LogLevel::Error, format!("  session error: {e}")),
        }
        if session_failed {
            emit_event(&cfg.events, ClientEvent::SessionLost);
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

/// Session loop: keepalive, the userspace stack, and the port-forward
/// listeners.
#[allow(clippy::too_many_arguments)]
async fn session_loop(
    tx: &mut Link,
    server_mac: &[u8; 6],
    sid: u32,
    keys: SessionKeys,
    cfg: &ClientConfig<'_>,
    notify: std::sync::Arc<tokio::sync::Notify>,
    forward_control: &mut Option<tokio::sync::mpsc::UnboundedReceiver<ForwardCommand>>,
    active_forwards: &mut Vec<Forward>,
    advertised_ports: &[u16],
) -> Result<SessionEnd, Box<dyn std::error::Error>> {
    let mut stack = IpStack::new(CLIENT_ADDR);
    let mut crypto = SessionCrypto::new(keys, Dir::C2S);
    let (to_tx, mut to_rx) = mpsc::channel::<(SocketHandle, StackMsg)>(512);
    let (accept_tx, mut accept_rx) = mpsc::channel::<(TcpStream, Forward)>(16);
    let mut conns: HashMap<SocketHandle, Conn> = HashMap::new();
    let mut pending_tx: HashMap<SocketHandle, VecDeque<Vec<u8>>> = HashMap::new();
    let mut pending_rx: HashMap<SocketHandle, VecDeque<Vec<u8>>> = HashMap::new();

    let mut listeners = HashMap::new();
    for &forward in active_forwards.iter() {
        listeners.insert(
            forward,
            spawn_listener(forward, accept_tx.clone(), cfg.log.clone()),
        );
    }

    let mut poll_timer = tokio::time::interval(POLL_INTERVAL);
    let mut last_keepalive = tokio::time::Instant::now();
    let mut missed = 0u32;
    let mut next_local_port: u16 = 40000;
    let mut fail_budget = FailBudget::default();

    let result = loop {
        tokio::select! {
            r = tx.recv(cfg.ethertype) => {
                let f = r?;
                if let Ok(l) = frame::decode(&f.payload) {
                    if l.session_id != sid {
                        continue;
                    }
                    match l.msg_type {
                        TYPE_TEARDOWN => {
                            if crypto.open(l.msg_type, l.session_id, l.seq, l.len, l.payload).is_some() {
                                cfg.log.emit(LogLevel::Info, "  teardown from server, closing session".into());
                                break SessionEnd::PeerClosed;
                            } else if !fail_budget.allow() {
                                continue;
                            }
                        }
                        TYPE_KEEPALIVE | TYPE_DATA => {
                            let Some(plain) = crypto.open(l.msg_type, l.session_id, l.seq, l.len, l.payload) else {
                                if !fail_budget.allow() {
                                    continue;
                                }
                                continue;
                            };
                            if l.msg_type == TYPE_KEEPALIVE {
                                missed = 0;
                            } else {
                                stack.push_packet(&plain);
                                pump(&mut stack, tx, server_mac, sid, &mut crypto, cfg.ethertype, &mut conns, &mut pending_tx, &mut pending_rx)?;
                            }
                        }
                        _ => {}
                    }
                }
            }
            Some((stream, forward)) = accept_rx.recv() => {
                let local_port = next_local_port;
                next_local_port = next_local_port.wrapping_add(1).max(40000);
                let h = stack.connect(SERVER_ADDR, INTERNAL_PORT, local_port);
                let (from_tx, from_rx) = mpsc::channel(512);
                let (close_tx, close_rx) = oneshot::channel();
                conns.insert(
                    h,
                    Conn {
                        from_tx,
                        forward,
                        close_tx: Some(close_tx),
                    },
                );
                tokio::spawn(bridge_task(stream, h, to_tx.clone(), from_rx, close_rx));
                pending_tx
                    .entry(h)
                    .or_default()
                    .push_back(forward.1.to_be_bytes().to_vec());
            }
            Some((h, msg)) = to_rx.recv() => {
                if !conns.contains_key(&h) {
                    continue;
                }
                match msg {
                    StackMsg::Data(b) => pending_tx.entry(h).or_default().push_back(b),
                    StackMsg::Close => stack.close_socket(h),
                }
            }
            command = recv_forward_command(forward_control) => {
                match command {
                    Some(ForwardCommand::Add(forward)) => {
                        if active_forwards.contains(&forward) {
                            emit_event(
                                &cfg.events,
                                ClientEvent::ForwardRejected {
                                    forward,
                                    reason: "该映射已存在".into(),
                                },
                            );
                        } else if !advertised_ports.contains(&forward.1) {
                            let reason = format!(
                                "服务器未允许目标端口 {}，当前会话不能添加该映射",
                                forward.1
                            );
                            cfg.log.emit(LogLevel::Warn, format!("  forward rejected: {reason}"));
                            emit_event(
                                &cfg.events,
                                ClientEvent::ForwardRejected { forward, reason },
                            );
                        } else {
                            active_forwards.push(forward);
                            if let std::collections::hash_map::Entry::Vacant(entry) = listeners.entry(forward) {
                                entry.insert(spawn_listener(forward, accept_tx.clone(), cfg.log.clone()));
                            }
                        }
                    }
                    Some(ForwardCommand::Remove(forward)) => {
                        if let Some(pos) = active_forwards.iter().position(|item| *item == forward) {
                            active_forwards.remove(pos);
                        }
                        if let Some(task) = listeners.remove(&forward) {
                            task.abort();
                        }
                        close_forward_connections(forward, &mut stack, &mut conns);
                    }
                    None => *forward_control = None,
                }
            }
            _ = poll_timer.tick() => {
                pump(&mut stack, tx, server_mac, sid, &mut crypto, cfg.ethertype, &mut conns, &mut pending_tx, &mut pending_rx)?;
            }
            _ = tokio::time::sleep_until(last_keepalive + KEEPALIVE_INTERVAL) => {
                let raw = crypto.seal(TYPE_KEEPALIVE, sid, &[]);
                tx.send(server_mac, cfg.ethertype, &raw)?;
                last_keepalive = tokio::time::Instant::now();
                missed += 1;
                if missed >= MAX_MISSED_KEEPALIVES {
                    cfg.log.emit(
                        LogLevel::Info,
                        format!("  no keepalive echo for {missed} rounds, link assumed lost"),
                    );
                    break SessionEnd::LinkLost;
                }
            }
            _ = notify.notified() => {
                cfg.log.emit(LogLevel::Info, "  shutdown requested, sending teardown".into());
                break SessionEnd::Shutdown;
            }
        }
    };

    // Best-effort teardown so the server drops us immediately instead of
    // waiting for the stale timeout.
    let _ = tx.send(
        server_mac,
        cfg.ethertype,
        &crypto.seal(TYPE_TEARDOWN, sid, &[]),
    );
    for task in listeners.into_values() {
        task.abort();
    }
    Ok(result)
}

async fn recv_forward_command(
    receiver: &mut Option<tokio::sync::mpsc::UnboundedReceiver<ForwardCommand>>,
) -> Option<ForwardCommand> {
    match receiver.as_mut() {
        Some(receiver) => receiver.recv().await,
        None => std::future::pending().await,
    }
}

fn spawn_listener(
    (listen_port, dst_port): Forward,
    accept_tx: mpsc::Sender<(TcpStream, Forward)>,
    log: LogSink,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        run_listener(listen_port, dst_port, accept_tx, log).await;
    })
}

fn close_forward_connections(
    forward: Forward,
    stack: &mut IpStack,
    conns: &mut HashMap<SocketHandle, Conn>,
) {
    let handles: Vec<SocketHandle> = conns
        .iter()
        .filter_map(|(handle, conn)| (conn.forward == forward).then_some(*handle))
        .collect();
    for handle in handles {
        if let Some(conn) = conns.get_mut(&handle)
            && let Some(close_tx) = conn.close_tx.take()
        {
            let _ = close_tx.send(());
        }
        stack.close_socket(handle);
    }
}

/// Listener for one `--forward` rule: accept local connections and hand them
/// to the session loop, which opens the internal connection.
async fn run_listener(
    listen_port: u16,
    dst_port: u16,
    accept_tx: mpsc::Sender<(TcpStream, Forward)>,
    log: LogSink,
) {
    let listener = match TcpListener::bind(("127.0.0.1", listen_port)).await {
        Ok(l) => l,
        Err(e) => {
            log.emit(
                LogLevel::Warn,
                format!("  cannot listen on 127.0.0.1:{listen_port}: {e}"),
            );
            return;
        }
    };
    log.emit(
        LogLevel::Info,
        format!("  forward: 127.0.0.1:{listen_port} -> router 127.0.0.1:{dst_port}"),
    );
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                if accept_tx
                    .send((stream, (listen_port, dst_port)))
                    .await
                    .is_err()
                {
                    return;
                }
            }
            Err(e) => log.emit(
                LogLevel::Warn,
                format!("  accept error on {listen_port}: {e}"),
            ),
        }
    }
}
/// Bridge one kernel TCP socket to the userspace stack: bytes from the
/// kernel socket go into `to_tx` (relayed into the stack), bytes from the
/// stack arrive on `from_rx` and are written to the kernel socket.
async fn bridge_task(
    mut stream: TcpStream,
    handle: SocketHandle,
    to_tx: mpsc::Sender<(SocketHandle, StackMsg)>,
    mut from_rx: mpsc::Receiver<Vec<u8>>,
    mut close_rx: oneshot::Receiver<()>,
) {
    let mut buf = vec![0u8; 8192];
    loop {
        tokio::select! {
            r = stream.read(&mut buf) => {
                match r {
                    Ok(0) => {
                        let _ = to_tx.send((handle, StackMsg::Close)).await;
                        return;
                    }
                    Ok(n) => {
                        if to_tx.send((handle, StackMsg::Data(buf[..n].to_vec()))).await.is_err() {
                            return;
                        }
                    }
                    Err(_) => return,
                }
            }
            msg = from_rx.recv() => {
                match msg {
                    Some(b) => {
                        if stream.write_all(&b).await.is_err() {
                            return;
                        }
                    }
                    None => return,
                }
            }
            _ = &mut close_rx => {
                let _ = stream.shutdown().await;
                let _ = to_tx.send((handle, StackMsg::Close)).await;
                return;
            }
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
    pending_rx: &mut HashMap<SocketHandle, VecDeque<Vec<u8>>>,
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
                front.drain(..n);
                if front.is_empty() {
                    q.pop_front();
                }
            }
            if q.is_empty() {
                pending_tx.remove(&h);
            }
        }

        let mut buf = [0u8; 4096];
        loop {
            let n = stack.recv_bytes(h, &mut buf);
            if n == 0 {
                break;
            }
            pending_rx
                .entry(h)
                .or_default()
                .push_back(buf[..n].to_vec());
        }
        if let Some(q) = pending_rx.get_mut(&h) {
            while let Some(b) = q.front() {
                let b = b.clone();
                match conns[&h].from_tx.try_send(b) {
                    Ok(()) => {
                        q.pop_front();
                    }
                    Err(_) => break,
                }
            }
            if q.is_empty() {
                pending_rx.remove(&h);
            }
        }

        if stack.socket_closed(h) {
            reap.push(h);
        } else if stack.peer_eof(h) {
            stack.close_socket(h);
        }
    }
    for h in reap {
        stack.remove_socket(h);
        pending_tx.remove(&h);
        pending_rx.remove(&h);
        conns.remove(&h);
    }
    Ok(())
}
