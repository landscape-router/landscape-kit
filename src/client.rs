use std::collections::{HashMap, VecDeque};
use std::time::Duration;

use landscape_proto::ipstack::{
    IpStack, SocketHandle, StackMsg, CLIENT_ADDR, INTERNAL_PORT, SERVER_ADDR,
};
use landscape_proto::protocol::crypto::{Dir, SessionCrypto, SessionKeys};
use landscape_proto::protocol::frame;
use landscape_proto::protocol::session::{
    ClientPhase, ClientSession, HANDSHAKE_TIMEOUT, KEEPALIVE_INTERVAL, MAX_RETRIES,
};
use landscape_proto::protocol::{
    TYPE_AUTH_ACK, TYPE_AUTH_NACK, TYPE_DATA, TYPE_KEEPALIVE, TYPE_RESP, TYPE_TEARDOWN,
};
use landscape_proto::transport::{fmt_mac, Frame, Link};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

pub const BROADCAST: [u8; 6] = [0xff; 6];
const RETRY_BACKOFF: Duration = Duration::from_secs(3);
const MAX_MISSED_KEEPALIVES: u32 = 3;
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// One `--forward LOCAL:DST` rule.
pub type Forward = (u16, u16);

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
}

/// Per-connection state on the client side.
struct Conn {
    /// Bytes from the stack to the kernel socket.
    from_tx: mpsc::Sender<Vec<u8>>,
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

pub async fn run(cfg: &ClientConfig<'_>) -> Result<(), Box<dyn std::error::Error>> {
    if cfg.psk.len() < 12 {
        eprintln!(
            "warning: psk is only {} chars — challenge-response keys are derived with a single sha256 pass, so a short psk can be brute-forced offline; use a long random secret",
            cfg.psk.len()
        );
    }
    let mut tx = Link::open(cfg.devs, cfg.ethertype, cfg.mac)?;
    println!(
        "client '{}' ready on {} (ethertype 0x{:04x})",
        cfg.client_name,
        tx.names().join(", "),
        cfg.ethertype
    );

    loop {
        let mut sess = ClientSession::new();
        let Some(server_mac) = handshake(&mut tx, &mut sess, cfg).await? else {
            println!("handshake failed, retrying in {}s", RETRY_BACKOFF.as_secs());
            tokio::time::sleep(RETRY_BACKOFF).await;
            continue;
        };
        let sid = sess.session_id().expect("session id after handshake");
        let keys = sess.keys().expect("session keys after handshake").clone();
        println!(
            "session {sid} established with {} (encrypted)",
            fmt_mac(&server_mac)
        );

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        let sig = tokio::spawn(async move {
            wait_for_shutdown().await;
            let _ = shutdown_tx.send(());
        });
        let end = session_loop(&mut tx, &server_mac, sid, keys, cfg, shutdown_rx).await;
        match end {
            Ok(SessionEnd::Shutdown) => return Ok(()),
            Ok(SessionEnd::LinkLost) => println!("  link lost, restarting handshake"),
            Ok(SessionEnd::PeerClosed) => println!("  session closed by peer"),
            Err(e) => eprintln!("  session error: {e}"),
        }
        sig.abort();
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
) -> Result<Option<[u8; 6]>, Box<dyn std::error::Error>> {
    let mut saw_resp = false;
    while sess.retransmit_allowed() {
        sess.bump_retry();
        println!("discover: broadcast (try {}/{MAX_RETRIES})", sess.retries());
        let token = (!cfg.token.is_empty()).then_some(cfg.token);
        let discover = sess.discover_frame(cfg.client_name, token, cfg.psk.as_bytes());
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
            println!("  no response within {}s", HANDSHAKE_TIMEOUT.as_secs());
            continue;
        };
        let server_mac = resp_frame.src;
        let resp_lndp = frame::decode(&resp_frame.payload)?;
        // Opening the RESP proves the server holds the psk, and the echoed
        // discover_id proves it answers this very attempt: everything else
        // is a rogue or a replay.
        let Some((resp, auth_req)) = sess.on_resp(&resp_lndp, cfg.user, cfg.psk.as_bytes()) else {
            println!("  response failed server authentication, rediscovering");
            continue;
        };
        saw_resp = true;
        println!(
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
        );
        warn_unadvertised_ports(&resp.ports, cfg.forwards);
        println!("  auth request sent for user '{}'", cfg.user);
        tx.send(&server_mac, cfg.ethertype, &auth_req)?;

        let deadline = tokio::time::Instant::now() + HANDSHAKE_TIMEOUT;
        let Some(auth_frame) = recv_until(
            tx,
            cfg.ethertype,
            |f| {
                frame::decode(&f.payload).is_ok_and(|l| {
                    l.msg_type == TYPE_AUTH_ACK || l.msg_type == TYPE_AUTH_NACK
                })
            },
            deadline,
        )
        .await?
        else {
            println!("  auth timeout, rediscovering");
            continue;
        };
        let auth_lndp = frame::decode(&auth_frame.payload)?;
        sess.on_auth_frame(&auth_lndp, cfg.psk.as_bytes());
        if sess.session_id().is_some() {
            return Ok(Some(server_mac));
        }
        if let ClientPhase::Rejected(reason) = &sess.phase {
            eprintln!("  auth rejected: {reason}");
        }
        return Ok(None);
    }
    if !saw_resp {
        eprintln!("  no server response at all — check that psk, token and ethertype match the server");
    }
    Ok(None)
}

/// Warn when a `--forward` destination port is not in the server's
/// advertised list (the server would reject it anyway).
fn warn_unadvertised_ports(advertised: &[u16], forwards: &[Forward]) {
    if advertised.is_empty() {
        return;
    }
    for &(_, dst) in forwards {
        if !advertised.contains(&dst) {
            eprintln!(
                "  warning: server does not advertise forwarding to port {dst} (may be rejected)"
            );
        }
    }
}

/// Block until a frame satisfying `pred` arrives, or the deadline passes.
/// The predicate sees the full transport frame (src MAC + LNDP header).
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
async fn session_loop(
    tx: &mut Link,
    server_mac: &[u8; 6],
    sid: u32,
    keys: SessionKeys,
    cfg: &ClientConfig<'_>,
    mut shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) -> Result<SessionEnd, Box<dyn std::error::Error>> {
    let mut stack = IpStack::new(CLIENT_ADDR);
    let mut crypto = SessionCrypto::new(keys, Dir::C2S);
    let (to_tx, mut to_rx) = mpsc::channel::<(SocketHandle, StackMsg)>(512);
    let (accept_tx, mut accept_rx) = mpsc::channel::<(TcpStream, u16)>(16);
    let mut conns: HashMap<SocketHandle, Conn> = HashMap::new();
    let mut pending_tx: HashMap<SocketHandle, VecDeque<Vec<u8>>> = HashMap::new();
    let mut pending_rx: HashMap<SocketHandle, VecDeque<Vec<u8>>> = HashMap::new();

    let mut accept_tasks = Vec::new();
    for &(listen_port, dst_port) in cfg.forwards {
        let atx = accept_tx.clone();
        accept_tasks.push(tokio::spawn(async move {
            run_listener(listen_port, dst_port, atx).await
        }));
    }

    let mut poll_timer = tokio::time::interval(POLL_INTERVAL);
    let mut last_keepalive = tokio::time::Instant::now();
    let mut missed = 0u32;
    let mut next_local_port: u16 = 40000;

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
                                println!("  teardown from server, closing session");
                                break SessionEnd::PeerClosed;
                            }
                        }
                        TYPE_KEEPALIVE | TYPE_DATA => {
                            let Some(plain) = crypto.open(l.msg_type, l.session_id, l.seq, l.len, l.payload) else {
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
            Some((stream, dst_port)) = accept_rx.recv() => {
                let local_port = next_local_port;
                next_local_port = next_local_port.wrapping_add(1).max(40000);
                let h = stack.connect(SERVER_ADDR, INTERNAL_PORT, local_port);
                let (from_tx, from_rx) = mpsc::channel(512);
                conns.insert(h, Conn { from_tx });
                tokio::spawn(bridge_task(stream, h, to_tx.clone(), from_rx));
                pending_tx.entry(h).or_default().push_back(dst_port.to_be_bytes().to_vec());
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
            _ = poll_timer.tick() => {
                pump(&mut stack, tx, server_mac, sid, &mut crypto, cfg.ethertype, &mut conns, &mut pending_tx, &mut pending_rx)?;
            }
            _ = tokio::time::sleep_until(last_keepalive + KEEPALIVE_INTERVAL) => {
                let raw = crypto.seal(TYPE_KEEPALIVE, sid, &[]);
                tx.send(server_mac, cfg.ethertype, &raw)?;
                last_keepalive = tokio::time::Instant::now();
                missed += 1;
                if missed >= MAX_MISSED_KEEPALIVES {
                    println!("  no keepalive echo for {missed} rounds, link assumed lost");
                    break SessionEnd::LinkLost;
                }
            }
            _ = &mut shutdown_rx => {
                println!("  shutdown requested, sending teardown");
                break SessionEnd::Shutdown;
            }
        }
    };

    // Best-effort teardown so the server drops us immediately instead of
    // waiting for the stale timeout.
    let _ = tx.send(server_mac, cfg.ethertype, &crypto.seal(TYPE_TEARDOWN, sid, &[]));
    for task in accept_tasks {
        task.abort();
    }
    Ok(result)
}

/// Listener for one `--forward` rule: accept local connections and hand them
/// to the session loop, which opens the internal connection.
async fn run_listener(
    listen_port: u16,
    dst_port: u16,
    accept_tx: mpsc::Sender<(TcpStream, u16)>,
) {
    let listener = match TcpListener::bind(("127.0.0.1", listen_port)).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("  cannot listen on 127.0.0.1:{listen_port}: {e}");
            return;
        }
    };
    println!("  forward: 127.0.0.1:{listen_port} -> router 127.0.0.1:{dst_port}");
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                if accept_tx.send((stream, dst_port)).await.is_err() {
                    return;
                }
            }
            Err(e) => eprintln!("  accept error on {listen_port}: {e}"),
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
            pending_rx.entry(h).or_default().push_back(buf[..n].to_vec());
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
