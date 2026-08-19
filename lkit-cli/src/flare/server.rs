use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use landscape_terrain_proto::ipstack::{
    INTERNAL_PORT, IpStack, SERVER_ADDR, SocketHandle, StackMsg,
};
use landscape_terrain_proto::protocol::crypto::{
    Dir, HS_AUTH_ACK, HS_AUTH_NACK, HandshakeKeys, MasterKey, SessionCrypto,
};
use landscape_terrain_proto::protocol::frame;
use landscape_terrain_proto::protocol::session::{self, ServerSession, VerifyResult};
use landscape_terrain_proto::protocol::{
    TYPE_AUTH_REQ, TYPE_DATA, TYPE_DISCOVER, TYPE_KEEPALIVE, TYPE_TEARDOWN,
};
use landscape_terrain_proto::transport::{Link, fmt_mac};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

const POLL_INTERVAL: Duration = Duration::from_millis(25);
const SWEEP_INTERVAL: Duration = Duration::from_secs(5);
const STALE_AFTER: Duration = Duration::from_secs(45);
const CONNECTION_CHANNEL_CAPACITY: usize = 16;
const MAX_PENDING_TO_STACK_BYTES: usize = 4 * 1024 * 1024;
/// smoltcp listeners do not have a kernel-style accept backlog: one listener
/// can hold one SYN/connection. Keep a bounded pool so a burst of local
/// connections does not reset all but the first SYN.
const LISTENER_POOL_SIZE: usize = 32;

/// Max DISCOVER/AUTH_REQ frames per second per source MAC (anti-scanning,
/// brute force and kick attempts). A full token bucket refills at this rate.
const RATE_PER_SEC: f64 = 10.0;

/// Server-wide cap on DISCOVER/AUTH_REQ processing per second. The per-MAC
/// limiter can be bypassed by forging a fresh MAC per frame, so this global
/// bucket bounds CPU and peer-table growth regardless of spoofing.
const GLOBAL_RATE_PER_SEC: f64 = 200.0;

/// Max failed session-frame opens per second per MAC (DATA / KEEPALIVE /
/// TEARDOWN with a bad tag). Bounds the decrypt work a spoofed-frame flood
/// can force on the event loop; a legitimate session never fails opens.
const SESSION_FAIL_PER_SEC: f64 = 200.0;

/// Hard cap on the peer table. DISCOVER frames from unknown MACs are
/// dropped once it is full (spoofed-MAC floods must not grow memory
/// without bound).
const MAX_PEERS: usize = 4096;

/// AUTH_REQ failures before the source MAC is locked out, and the lockout
/// window (brute-force protection). Failures only count against MACs
/// WITHOUT an active session, and the server-wide budget bounds how many
/// MACs a spoofing attacker can lock out at all.
const MAX_AUTH_FAILS: u32 = 5;
const AUTH_WINDOW: Duration = Duration::from_secs(60);
const AUTH_LOCKOUT: Duration = Duration::from_secs(60);
/// Max auth failures the server counts within AUTH_WINDOW before new
/// failures stop being recorded for lockout purposes.
const AUTH_BUDGET: u32 = 25;

pub struct ServerConfig<'a> {
    pub devs: &'a [String],
    pub ethertype: u16,
    pub mac: Option<[u8; 6]>,
    pub psk: &'a str,
    pub device_name: &'a str,
    /// Ports the server is allowed to dial on 127.0.0.1; also advertised in
    /// RESP so clients know what they may forward to.
    pub forward_ports: &'a [u16],
    /// Discovery token: when non-empty, DISCOVER frames without it are
    /// ignored (anti-scanning; the psk challenge-response is the real
    /// security boundary).
    pub discover_token: &'a str,
}

/// Per-peer connection state, keyed by the client's MAC address.
struct Peer {
    sess: ServerSession,
    ifindex: i32,
    stack: Option<IpStack>,
    listeners: Vec<SocketHandle>,
    crypto: Option<SessionCrypto>,
    conns: HashMap<SocketHandle, ServerConn>,
    pending_tx: HashMap<SocketHandle, VecDeque<Vec<u8>>>,
    to_tx: mpsc::Sender<(([u8; 6], SocketHandle), StackMsg)>,
    allowed: Arc<[u16]>,
    last_seen: Instant,
}

/// One relayed connection on the server side.
struct ServerConn {
    /// Bytes from the stack to the kernel socket (dialed service).
    from_tx: mpsc::Sender<Vec<u8>>,
}

/// Token bucket for control frames.
struct RateBucket {
    tokens: f64,
    rate: f64,
    last_refill: Instant,
    last_seen: Instant,
}

impl RateBucket {
    fn new(rate: f64) -> Self {
        let now = Instant::now();
        Self {
            tokens: rate,
            rate,
            last_refill: now,
            last_seen: now,
        }
    }

    fn allow(&mut self) -> bool {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.last_refill = now;
        self.last_seen = now;
        self.tokens = (self.tokens + elapsed * self.rate).min(self.rate);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

impl Default for RateBucket {
    fn default() -> Self {
        Self::new(RATE_PER_SEC)
    }
}

/// Per-MAC auth-failure tracking with lockout.
struct AuthGuard {
    fails: u32,
    window_start: Option<Instant>,
    locked_until: Option<Instant>,
    last_seen: Instant,
}

impl Default for AuthGuard {
    fn default() -> Self {
        Self {
            fails: 0,
            window_start: None,
            locked_until: None,
            last_seen: Instant::now(),
        }
    }
}

impl AuthGuard {
    fn blocked(&mut self) -> bool {
        self.last_seen = Instant::now();
        match self.locked_until {
            Some(t) => Instant::now() < t,
            None => false,
        }
    }

    /// Remaining lockout duration, if any (used for the user-facing NACK).
    fn remaining(&self) -> Option<Duration> {
        self.locked_until
            .map(|t| t.saturating_duration_since(Instant::now()))
    }

    fn record_failure(&mut self) {
        self.last_seen = Instant::now();
        let now = Instant::now();
        if let Some(t) = self.locked_until {
            if now < t {
                // Already locked out: further failures must not extend the
                // lockout forever (a retrying client would never recover).
                return;
            }
            self.locked_until = None;
        }
        match self.window_start {
            Some(start) if now.duration_since(start) <= AUTH_WINDOW => {
                self.fails += 1;
                if self.fails >= MAX_AUTH_FAILS {
                    self.locked_until = Some(now + AUTH_LOCKOUT);
                }
            }
            _ => {
                self.window_start = Some(now);
                self.fails = 1;
            }
        }
    }

    fn record_success(&mut self) {
        self.last_seen = Instant::now();
        self.fails = 0;
        self.window_start = None;
        self.locked_until = None;
    }
}

pub async fn run(
    cfg: &ServerConfig<'_>,
    shutdown: Option<tokio::sync::oneshot::Receiver<()>>,
) -> Result<(), Box<dyn std::error::Error>> {
    if cfg.psk.len() < 12 {
        eprintln!(
            "warning: psk is only {} chars — it is stretched with scrypt at startup, but prefer a long random secret over a passphrase",
            cfg.psk.len()
        );
    }
    let mut tx = Link::open(cfg.devs, cfg.ethertype, cfg.mac)?;
    // The psk is stretched into a master key once at startup (scrypt); all
    // derivations below feed on it, so a weak psk costs an offline attacker
    // ~32 MiB and ~100 ms per guess instead of a single sha256.
    let master = MasterKey::derive(cfg.psk.as_bytes());
    let mut peers: HashMap<[u8; 6], Peer> = HashMap::new();
    let mut rate: HashMap<[u8; 6], RateBucket> = HashMap::new();
    let mut fail_rate: HashMap<[u8; 6], RateBucket> = HashMap::new();
    let mut guards: HashMap<[u8; 6], AuthGuard> = HashMap::new();
    let mut global_rate = RateBucket::new(GLOBAL_RATE_PER_SEC);
    let mut auth_fail_ts: VecDeque<Instant> = VecDeque::new();
    let allowed: Arc<[u16]> = Arc::from(cfg.forward_ports);
    let (to_tx, mut to_rx) = mpsc::channel::<(([u8; 6], SocketHandle), StackMsg)>(512);
    let mut pending_tx_bytes = 0usize;
    println!(
        "server '{}' ready on {} (ethertype 0x{:04x}, forward ports: {})",
        cfg.device_name,
        devs_display(tx.names()),
        cfg.ethertype,
        cfg.forward_ports
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join(",")
    );

    let mut poll_timer = tokio::time::interval(POLL_INTERVAL);
    let mut sweep_timer = tokio::time::interval(SWEEP_INTERVAL);
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel();
    let sig = match shutdown {
        Some(rx) => tokio::spawn(async move {
            let _ = rx.await;
            let _ = shutdown_tx.send(());
        }),
        None => tokio::spawn(async move {
            wait_for_shutdown().await;
            let _ = shutdown_tx.send(());
        }),
    };
    loop {
        tokio::select! {
            biased;
            _ = &mut shutdown_rx => break,
            r = tx.recv_with_meta(cfg.ethertype) => {
                let (f, ifindex) = r?;
                let Ok(l) = frame::decode(&f.payload) else {
                    continue;
                };
                let mac = f.src;
                match l.msg_type {
                    TYPE_DISCOVER => {
                        // Global bucket first: a per-MAC limiter is useless
                        // against per-frame MAC spoofing.
                        if !global_rate.allow() {
                            println!("discover from {} dropped (global rate)", fmt_mac(&mac));
                            continue;
                        }
                        if !rate_allow(&mut rate, &mac) {
                            println!("discover from {} rate-limited", fmt_mac(&mac));
                            continue;
                        }
                        // Sealed with the psk-derived pre-discovery key:
                        // only a psk-holder is even heard, and the client
                        // name/token stay hidden.
                        let Some((discover_id, name, token)) =
                            session::open_discover(&l, &master)
                        else {
                            println!("discover from {} ignored (cannot open)", fmt_mac(&mac));
                            continue;
                        };
                        if !cfg.discover_token.is_empty()
                            && token.as_deref() != Some(cfg.discover_token)
                        {
                            println!(
                                "discover from {} ignored (token mismatch)",
                                fmt_mac(&mac)
                            );
                            continue;
                        }
                        if !peers.contains_key(&mac) && peers.len() >= MAX_PEERS {
                            println!("discover from {} dropped (peer table full)", fmt_mac(&mac));
                            continue;
                        }
                        let peer = peers.entry(mac).or_insert_with(|| new_peer(to_tx.clone(), allowed.clone()));
                        peer.ifindex = ifindex;
                        // Never disturb an active session: only a completed
                        // AUTH_REQ replaces it, so a forged DISCOVER cannot
                        // kick a live client.
                        let resp = peer.sess.begin_discover(
                            discover_id,
                            cfg.device_name,
                            cfg.forward_ports,
                            &master,
                        );
                        tx.send_on(ifindex, &mac, cfg.ethertype, &resp)?;
                        println!("discover from {} '{}'", fmt_mac(&mac), name);
                    }
                    TYPE_AUTH_REQ => {
                        if !global_rate.allow() {
                            println!("auth attempt dropped (global rate)");
                            continue;
                        }
                        let Some(peer) = peers.get_mut(&mac) else {
                            println!("auth attempt from unknown peer {}, ignored", fmt_mac(&mac));
                            continue;
                        };
                        if !rate_allow(&mut rate, &mac) {
                            println!("auth attempt from {} rate-limited", fmt_mac(&mac));
                            continue;
                        }
                        let ifindex = peer.ifindex;
                        let guard = guards.entry(mac).or_default();
                        // MACs with an active session are never locked out:
                        // an unauthenticated attacker could otherwise spoof
                        // the victim's MAC to freeze their re-authentication.
                        if guard.blocked() && peer.sess.session_id().is_none() {
                            if let Some(rem) = guard.remaining() {
                                // Tell the user why: retried too often. Sealed
                                // with the handshake keys when the handshake
                                // is still pending, so spoofed lockout NACKs
                                // are rejected; plaintext only when there is
                                // no pending nonce to seal with.
                                let reason = format!(
                                    "too many auth failures, locked out for {}s",
                                    rem.as_secs()
                                );
                                let nack = match peer.sess.take_server_nonce() {
                                    Some(s_nonce) => {
                                        let hkey = HandshakeKeys::derive(&master, s_nonce);
                                        hkey.seal_frame(
                                            Dir::S2C,
                                            frame::TYPE_AUTH_NACK,
                                            0,
                                            HS_AUTH_NACK,
                                            &frame::encode_auth_nack_payload(&reason),
                                        )
                                    }
                                    None => frame::encode_auth_nack(&reason),
                                };
                                let _ = tx.send_on(ifindex, &mac, cfg.ethertype, &nack);
                            }
                            println!("auth attempt from {} ignored (lockout)", fmt_mac(&mac));
                            continue;
                        }
                        let mut drop_peer = false;
                        match peer.sess.verify_auth(&l, &master) {
                            VerifyResult::Accepted {
                                sid,
                                keys,
                                server_proof,
                                hkey,
                                user,
                            } => {
                                guards.entry(mac).or_default().record_success();
                                teardown_peer(peer, &mut pending_tx_bytes);
                                let mut stack = IpStack::new(SERVER_ADDR);
                                let listeners = (0..LISTENER_POOL_SIZE)
                                    .map(|_| stack.add_listener(INTERNAL_PORT))
                                    .collect();
                                peer.stack = Some(stack);
                                peer.listeners = listeners;
                                peer.crypto = Some(SessionCrypto::new(keys, Dir::S2C));
                                peer.last_seen = Instant::now();
                                // The AUTH_ACK is sealed with the handshake
                                // keys, so only a psk-holder can open it.
                                let ack = hkey.seal_frame(
                                    Dir::S2C,
                                    frame::TYPE_AUTH_ACK,
                                    sid,
                                    HS_AUTH_ACK,
                                    &frame::encode_auth_ack_payload(&server_proof),
                                );
                                tx.send_on(ifindex, &mac, cfg.ethertype, &ack)?;
                                println!(
                                    "client {} '{}' authenticated, session {sid}",
                                    fmt_mac(&mac),
                                    user
                                );
                            }
                            VerifyResult::Rejected { reason, hkey } => {
                                // The client necessarily holds the handshake
                                // keys (its AUTH_REQ opened), so the NACK is
                                // sealed: a spoofed plaintext NACK cannot end
                                // the handshake.
                                let nack = hkey.seal_frame(
                                    Dir::S2C,
                                    frame::TYPE_AUTH_NACK,
                                    0,
                                    HS_AUTH_NACK,
                                    &frame::encode_auth_nack_payload(&reason),
                                );
                                tx.send_on(ifindex, &mac, cfg.ethertype, &nack)?;
                                let active = peer.sess.session_id().is_some();
                                if !active {
                                    // Failures only count against MACs without
                                    // an active session (a spoofing attacker
                                    // must not be able to lock out a live
                                    // client), and the server-wide budget
                                    // bounds how many victims a flood can
                                    // lock out.
                                    let now = Instant::now();
                                    while auth_fail_ts
                                        .front()
                                        .is_some_and(|t| now.duration_since(*t) > AUTH_WINDOW)
                                    {
                                        auth_fail_ts.pop_front();
                                    }
                                    if (auth_fail_ts.len() as u32) < AUTH_BUDGET {
                                        guards.entry(mac).or_default().record_failure();
                                        auth_fail_ts.push_back(now);
                                    }
                                }
                                drop_peer = !active;
                                println!("auth rejected for {}: {reason}", fmt_mac(&mac));
                            }
                            VerifyResult::Unauthentic(reason) => {
                                // Not an auth attempt at all (could not even
                                // be opened): no lockout accounting, no peer
                                // teardown, and the pending nonce survives —
                                // a MAC-spoofed garbage frame can neither
                                // lock out nor interrupt a connecting client.
                                println!("unauthentic auth frame from {} ({reason})", fmt_mac(&mac));
                            }
                        }
                        if drop_peer
                            && let Some(mut peer) = peers.remove(&mac) {
                                teardown_peer(&mut peer, &mut pending_tx_bytes);
                            }
                    }
                    TYPE_KEEPALIVE => {
                        let Some(peer) = peers.get_mut(&mac) else {
                            continue;
                        };
                        if peer.sess.session_id() != Some(l.session_id) {
                            continue;
                        }
                        let Some(crypto) = peer.crypto.as_mut() else {
                            continue;
                        };
                        if crypto.open(l.msg_type, l.session_id, l.seq, l.len, l.payload).is_none() {
                            if !fail_allow(&mut fail_rate, &mac) {
                                continue;
                            }
                            continue;
                        }
                        peer.last_seen = Instant::now();
                        let echo = crypto.seal(TYPE_KEEPALIVE, l.session_id, &[]);
                        tx.send_on(peer.ifindex, &mac, cfg.ethertype, &echo)?;
                    }
                    TYPE_DATA => {
                        let Some(peer) = peers.get_mut(&mac) else {
                            continue;
                        };
                        if peer.sess.session_id() != Some(l.session_id) {
                            continue;
                        }
                        let Some(crypto) = peer.crypto.as_mut() else {
                            continue;
                        };
                        let Some(plain) = crypto.open(l.msg_type, l.session_id, l.seq, l.len, l.payload) else {
                            if !fail_allow(&mut fail_rate, &mac) {
                                continue;
                            }
                            continue;
                        };
                        peer.last_seen = Instant::now();
                        if let Some(stack) = peer.stack.as_mut() {
                            stack.push_packet(&plain);
                            pump_peer(
                                peer,
                                &mac,
                                &mut tx,
                                cfg.ethertype,
                                l.session_id,
                                &mut pending_tx_bytes,
                            )?;
                        }
                    }
                    TYPE_TEARDOWN => {
                        let mut drop_peer = false;
                        if let Some(peer) = peers.get_mut(&mac)
                            && peer.sess.session_id() == Some(l.session_id)
                                && let Some(crypto) = peer.crypto.as_mut() {
                                    if crypto.open(l.msg_type, l.session_id, l.seq, l.len, l.payload).is_some() {
                                        drop_peer = true;
                                    } else if !fail_allow(&mut fail_rate, &mac) {
                                        continue;
                                    }
                                }
                        if drop_peer {
                            println!("client {} sent teardown, dropping session", fmt_mac(&mac));
                            if let Some(mut peer) = peers.remove(&mac) {
                                teardown_peer(&mut peer, &mut pending_tx_bytes);
                            }
                        }
                    }
                    t => println!(
                        "  [server] {} from {} ignored",
                        frame::type_name(t),
                        fmt_mac(&mac)
                    ),
                }
            }
            Some(((mac, h), msg)) = to_rx.recv(), if pending_tx_bytes < MAX_PENDING_TO_STACK_BYTES => {
                if let Some(peer) = peers.get_mut(&mac)
                    && peer.conns.contains_key(&h) {
                        match msg {
                            StackMsg::Data(b) => {
                                pending_tx_bytes += b.len();
                                peer.pending_tx.entry(h).or_default().push_back(b);
                            }
                            StackMsg::Close => {
                                if let Some(stack) = peer.stack.as_mut() {
                                    stack.close_socket(h);
                                }
                            }
                        }
                    }
            }
            _ = poll_timer.tick() => {
                let macs: Vec<[u8; 6]> = peers.keys().copied().collect();
                for mac in macs {
                    if let Some(peer) = peers.get_mut(&mac)
                        && let Some(sid) = peer.sess.session_id() {
                            pump_peer(peer, &mac, &mut tx, cfg.ethertype, sid, &mut pending_tx_bytes)?;
                        }
                }
            }
            _ = sweep_timer.tick() => {
                let now = Instant::now();
                rate.retain(|_, b| now.duration_since(b.last_seen) <= STALE_AFTER);
                fail_rate.retain(|_, b| now.duration_since(b.last_seen) <= STALE_AFTER);
                guards.retain(|_, g| now.duration_since(g.last_seen) <= STALE_AFTER);
                let stale: Vec<[u8; 6]> = peers
                    .iter()
                    .filter(|(_, p)| now.duration_since(p.last_seen) > STALE_AFTER)
                    .map(|(m, _)| *m)
                    .collect();
                for mac in stale {
                    if let Some(peer) = peers.get_mut(&mac)
                        && let (Some(sid), Some(crypto)) = (peer.sess.session_id(), peer.crypto.as_mut()) {
                            let raw = crypto.seal(TYPE_TEARDOWN, sid, &[]);
                            let _ = tx.send_on(peer.ifindex, &mac, cfg.ethertype, &raw);
                        }
                    if let Some(mut peer) = peers.remove(&mac) {
                        teardown_peer(&mut peer, &mut pending_tx_bytes);
                        println!("  peer {} timed out, dropped", fmt_mac(&mac));
                    }
                }
            }
        }
    }
    sig.abort();

    // Graceful shutdown: tell every live peer before we disappear so the
    // clients can reconnect immediately instead of timing out on keepalives.
    // The loop is synchronous (fast sendto syscalls), but bound it anyway:
    // a wedged driver must not hold the exit hostage.
    let peers_left = peers.len();
    let notified = tokio::time::timeout(Duration::from_secs(2), async {
        let mut n = 0;
        for (mac, peer) in peers.iter_mut() {
            if let (Some(sid), Some(crypto)) = (peer.sess.session_id(), peer.crypto.as_mut()) {
                let raw = crypto.seal(TYPE_TEARDOWN, sid, &[]);
                if tx.send_on(peer.ifindex, mac, cfg.ethertype, &raw).is_ok() {
                    n += 1;
                }
            }
        }
        n
    })
    .await
    .unwrap_or(0);
    println!("server shutdown, {notified}/{peers_left} peer(s) notified");
    Ok(())
}

/// Resolves once SIGINT or SIGTERM arrives.
async fn wait_for_shutdown() {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("installing SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}

fn rate_allow(map: &mut HashMap<[u8; 6], RateBucket>, mac: &[u8; 6]) -> bool {
    map.entry(*mac).or_default().allow()
}

/// Spend one failed-open token for a MAC. Once the per-MAC budget is
/// drained, session frames from that MAC are dropped before the decrypt
/// attempt, so a spoofed flood stops costing work on the event loop.
fn fail_allow(map: &mut HashMap<[u8; 6], RateBucket>, mac: &[u8; 6]) -> bool {
    map.entry(*mac)
        .or_insert_with(|| RateBucket::new(SESSION_FAIL_PER_SEC))
        .allow()
}

fn new_peer(to_tx: mpsc::Sender<(([u8; 6], SocketHandle), StackMsg)>, allowed: Arc<[u16]>) -> Peer {
    Peer {
        sess: ServerSession::new(),
        ifindex: 0,
        stack: None,
        listeners: Vec::new(),
        crypto: None,
        conns: HashMap::new(),
        pending_tx: HashMap::new(),
        to_tx,
        allowed,
        last_seen: Instant::now(),
    }
}

/// Drop the peer's stack, session crypto and connections (kernel sockets
/// close via the channels; their tasks exit).
fn teardown_peer(peer: &mut Peer, pending_tx_bytes: &mut usize) {
    let dropped = peer
        .pending_tx
        .values()
        .flatten()
        .map(Vec::len)
        .sum::<usize>();
    *pending_tx_bytes = pending_tx_bytes.saturating_sub(dropped);
    peer.stack = None;
    peer.listeners.clear();
    peer.crypto = None;
    peer.conns.clear();
    peer.pending_tx.clear();
}

/// Pump one peer's stack: accept new internal connections, send outbound IP
/// packets back to the peer (sealed), and move bytes between the stack and
/// the dialed kernel sockets.
fn pump_peer(
    peer: &mut Peer,
    peer_mac: &[u8; 6],
    tx: &mut Link,
    ethertype: u16,
    sid: u32,
    pending_tx_bytes: &mut usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(stack) = peer.stack.as_mut() else {
        return Ok(());
    };
    let Some(crypto) = peer.crypto.as_mut() else {
        return Ok(());
    };

    for listener in &mut peer.listeners {
        if let Some((h, new_listener)) = stack.accept(*listener, INTERNAL_PORT) {
            *listener = new_listener;
            let (from_tx, from_rx) = mpsc::channel(CONNECTION_CHANNEL_CAPACITY);
            peer.conns.insert(h, ServerConn { from_tx });
            tokio::spawn(server_conn_task(
                *peer_mac,
                h,
                peer.to_tx.clone(),
                from_rx,
                peer.allowed.clone(),
            ));
        }
    }

    for pkt in stack.poll() {
        let raw = crypto.seal(TYPE_DATA, sid, &pkt);
        tx.send_on(peer.ifindex, peer_mac, ethertype, &raw)?;
    }

    let handles: Vec<SocketHandle> = peer.conns.keys().copied().collect();
    let mut reap: Vec<SocketHandle> = Vec::new();
    for h in handles {
        if let Some(q) = peer.pending_tx.get_mut(&h) {
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
                peer.pending_tx.remove(&h);
            }
        }

        let mut buf = [0u8; 4096];
        loop {
            let Ok(permit) = peer.conns[&h].from_tx.try_reserve() else {
                break;
            };
            let n = stack.recv_bytes(h, &mut buf);
            if n == 0 {
                break;
            }
            permit.send(buf[..n].to_vec());
        }

        if stack.socket_closed(h) {
            reap.push(h);
        } else if stack.peer_eof(h) {
            stack.close_socket(h);
        }
    }
    for h in reap {
        stack.remove_socket(h);
        if let Some(q) = peer.pending_tx.remove(&h) {
            let dropped = q.iter().map(Vec::len).sum::<usize>();
            *pending_tx_bytes = pending_tx_bytes.saturating_sub(dropped);
        }
        peer.conns.remove(&h);
    }
    Ok(())
}

/// Server side of one relayed connection: read the 2-byte target port from
/// the stream, dial 127.0.0.1:<port>, then bridge both directions.
async fn server_conn_task(
    peer_mac: [u8; 6],
    handle: SocketHandle,
    to_tx: mpsc::Sender<(([u8; 6], SocketHandle), StackMsg)>,
    mut from_rx: mpsc::Receiver<Vec<u8>>,
    allowed: Arc<[u16]>,
) {
    let mut header = Vec::new();
    while header.len() < 2 {
        match from_rx.recv().await {
            Some(b) => header.extend_from_slice(&b),
            None => {
                signal_close(&to_tx, peer_mac, handle).await;
                return;
            }
        }
    }
    let dst = u16::from_be_bytes([header[0], header[1]]);
    if !allowed.contains(&dst) {
        println!("  [server] forward to 127.0.0.1:{dst} not allowed, closing");
        let _ = to_tx.send(((peer_mac, handle), StackMsg::Close)).await;
        return;
    }
    let mut remote = match TcpStream::connect(("127.0.0.1", dst)).await {
        Ok(s) => s,
        Err(e) => {
            println!("  [server] dial 127.0.0.1:{dst} failed: {e}");
            signal_close(&to_tx, peer_mac, handle).await;
            return;
        }
    };
    let extra = &header[2..];
    if !extra.is_empty() && remote.write_all(extra).await.is_err() {
        signal_close(&to_tx, peer_mac, handle).await;
        return;
    }

    let mut buf = vec![0u8; 8192];
    loop {
        tokio::select! {
            msg = from_rx.recv() => match msg {
                Some(b) => {
                    if remote.write_all(&b).await.is_err() {
                        signal_close(&to_tx, peer_mac, handle).await;
                        return;
                    }
                }
                None => {
                    signal_close(&to_tx, peer_mac, handle).await;
                    return;
                }
            },
            r = remote.read(&mut buf) => match r {
                Ok(0) => {
                    signal_close(&to_tx, peer_mac, handle).await;
                    return;
                }
                Ok(n) => {
                    if to_tx.send(((peer_mac, handle), StackMsg::Data(buf[..n].to_vec()))).await.is_err() {
                        return;
                    }
                }
                Err(_) => {
                    signal_close(&to_tx, peer_mac, handle).await;
                    return;
                }
            },
        }
    }
}

async fn signal_close(
    to_tx: &mpsc::Sender<(([u8; 6], SocketHandle), StackMsg)>,
    peer_mac: [u8; 6],
    handle: SocketHandle,
) {
    let _ = to_tx.send(((peer_mac, handle), StackMsg::Close)).await;
}

fn devs_display(names: &[String]) -> String {
    if names.len() == 1 && names[0] == "any" {
        "all interfaces".to_string()
    } else {
        names.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_bucket_allows_burst_then_limits() {
        let mut b = RateBucket::new(RATE_PER_SEC);
        for _ in 0..10 {
            assert!(b.allow(), "initial tokens must be spendable");
        }
        assert!(!b.allow(), "drained bucket must refuse");
        std::thread::sleep(Duration::from_millis(300));
        assert!(b.allow(), "bucket must refill over time");
    }

    #[test]
    fn auth_guard_locks_after_threshold() {
        let mut g = AuthGuard::default();
        for _ in 0..MAX_AUTH_FAILS {
            g.record_failure();
        }
        assert!(g.blocked());
        assert!(g.remaining().is_some());
        // Once unlocked, everything is clean again.
        g.record_success();
        assert!(!g.blocked());
        assert!(g.remaining().is_none());
    }

    #[test]
    fn auth_guard_window_resets_stale_failures() {
        // Failures older than the window must not accumulate.
        let mut g = AuthGuard {
            window_start: Some(Instant::now() - AUTH_WINDOW - Duration::from_secs(1)),
            fails: MAX_AUTH_FAILS - 1,
            ..AuthGuard::default()
        };
        g.record_failure();
        assert_eq!(g.fails, 1);
        assert!(!g.blocked());
    }

    #[test]
    fn auth_guard_does_not_relock_while_locked() {
        let mut g = AuthGuard::default();
        for _ in 0..MAX_AUTH_FAILS {
            g.record_failure();
        }
        let first = g.locked_until;
        // Further failures while locked must not extend the lockout forever.
        for _ in 0..10 {
            g.record_failure();
        }
        assert_eq!(g.locked_until, first);
    }
}
