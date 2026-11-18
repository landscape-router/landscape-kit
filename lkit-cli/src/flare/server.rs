use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::login::{LoginReply, Pending};
use landscape_terrain_proto::ipstack::{
    CONNECTION_CHANNEL_CAPACITY, INTERNAL_PORT, IpStack, MAX_PENDING_TO_STACK_BYTES, SERVER_ADDR,
    SocketHandle, StackMsg,
};
use landscape_terrain_proto::protocol::{
    TYPE_AUTH_REQ, TYPE_DATA, TYPE_DISCOVER, TYPE_KEEPALIVE, TYPE_TEARDOWN, frame, v6,
};
use landscape_terrain_proto::relay::{RelayReceiver, RelayWake, wait_for_poll};
use landscape_terrain_proto::transport::{Link, fmt_mac};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

const SWEEP_INTERVAL: Duration = Duration::from_secs(5);
const STALE_AFTER: Duration = Duration::from_secs(45);
/// smoltcp listeners do not have a kernel-style accept backlog: one listener
/// can hold one SYN/connection. Keep a bounded pool so a burst of local
/// connections does not reset all but the first SYN.
const LISTENER_POOL_SIZE: usize = 64;

type ServerConnKey = ([u8; 6], SocketHandle, u64);
type ServerStackMsg = (ServerConnKey, StackMsg);

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

pub struct ServerConfig<'a> {
    pub devs: &'a [String],
    pub ethertype: u16,
    pub mac: Option<[u8; 6]>,
    pub psk: Option<&'a str>,
    pub device_name: &'a str,
    /// Ports returned in the authenticated ACK and allowed for forwarding.
    pub forward_ports: &'a [u16],
    /// Discovery token: when non-empty, DISCOVER frames without it are
    /// ignored.
    pub discover_token: &'a str,
}

/// Per-peer connection state, keyed by the client's MAC address.
struct Peer {
    sid: Option<u32>,
    pending: Option<Pending>,
    login_generation: u64,
    login_sid: Option<u32>,
    login_reply: Option<LoginReply>,
    /// Interface used for the established session (pinned on AUTH success).
    ifindex: i32,
    stack: Option<IpStack>,
    poll_at: Option<tokio::time::Instant>,
    listeners: Vec<SocketHandle>,
    crypto: Option<v6::Transport>,
    conns: HashMap<SocketHandle, ServerConn>,
    pending_tx: HashMap<SocketHandle, VecDeque<Vec<u8>>>,
    next_generation: u64,
    to_tx: mpsc::Sender<ServerStackMsg>,
    allowed: Arc<[u16]>,
    last_seen: Instant,
}

/// One relayed connection on the server side.
struct ServerConn {
    /// Bytes from the stack to the kernel socket (dialed service).
    from_tx: mpsc::Sender<RelayMsg>,
    generation: u64,
    close_after_flush: bool,
    peer_eof_sent: bool,
    /// Session teardown must also cancel relays blocked on the local service,
    /// including ones that have already consumed the peer's half-close.
    relay: Option<tokio::task::AbortHandle>,
}

impl Drop for ServerConn {
    fn drop(&mut self) {
        if let Some(relay) = &self.relay {
            relay.abort();
        }
    }
}

enum RelayMsg {
    Data(Vec<u8>),
    PeerEof,
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

    fn has_token(&mut self) -> bool {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.last_refill = now;
        self.last_seen = now;
        self.tokens = (self.tokens + elapsed * self.rate).min(self.rate);
        self.tokens >= 1.0
    }

    fn allow(&mut self) -> bool {
        if self.has_token() {
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
    let mut tx = Link::open(cfg.devs, cfg.ethertype, cfg.mac)?;
    let private = super::auth::identity()?;
    let public = v6::public_key(&private);
    let gate = v6::Discovery::new(cfg.psk);
    let mut checks = tokio::task::JoinSet::<([u8; 6], u64, Pending, bool)>::new();
    let mut generation = 0u64;
    let mut login_rate = RateBucket::new(4.);
    let mut peers: HashMap<[u8; 6], Peer> = HashMap::new();
    let mut rate: HashMap<[u8; 6], RateBucket> = HashMap::new();
    let mut fail_rate: HashMap<[u8; 6], RateBucket> = HashMap::new();
    let mut guards: HashMap<[u8; 6], AuthGuard> = HashMap::new();
    let mut global_rate = RateBucket::new(GLOBAL_RATE_PER_SEC);

    let allowed: Arc<[u16]> = Arc::from(cfg.forward_ports);
    let (to_tx, mut to_rx) = mpsc::channel::<ServerStackMsg>(512);
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

    let relay_wake = RelayWake::<ServerConnKey>::default();
    let mut sweep_timer = tokio::time::interval(SWEEP_INTERVAL);
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel();
    let sig = match shutdown {
        Some(rx) => landscape_terrain_proto::concurrency::spawn_task(
            crate::task_names::FLARE_SHUTDOWN,
            async move {
                let _ = rx.await;
                let _ = shutdown_tx.send(());
            },
        ),
        None => landscape_terrain_proto::concurrency::spawn_task(
            crate::task_names::FLARE_SHUTDOWN,
            async move {
                wait_for_shutdown().await;
                let _ = shutdown_tx.send(());
            },
        ),
    };
    loop {
        let deadline = peers.values().filter_map(|peer| peer.poll_at).min();
        tokio::select! {
            _ = &mut shutdown_rx => break,
            Some(result) = checks.join_next(), if !checks.is_empty() => {
                let Ok((mac, attempt, mut pending, ok)) = result else {
                    continue;
                };
                let Some(peer) = peers
                    .get_mut(&mac)
                    .filter(|p| p.login_generation == attempt)
                else {
                    continue;
                };
                let (kind, ports) = if ok {
                    guards.entry(mac).or_default().record_success();
                    (
                        frame::TYPE_AUTH_ACK,
                        cfg.forward_ports
                            .iter()
                            .flat_map(|p| p.to_be_bytes())
                            .collect::<Vec<_>>(),
                    )
                } else {
                    guards.entry(mac).or_default().record_failure();
                    (frame::TYPE_AUTH_NACK, Vec::new())
                };
                pending.reply.encrypted = pending.channel.seal(kind, pending.sid, &ports)?;
                pending.reply.sid = pending.sid;
                pending.reply.interface = pending.interface;
                tx.send_on(
                    pending.interface,
                    &mac,
                    cfg.ethertype,
                    &pending.reply.encrypted,
                )?;
                peer.login_reply = Some(pending.reply);
                if !ok {
                    continue;
                }
                teardown_peer(peer, &mut pending_tx_bytes);
                let mut stack = IpStack::new(SERVER_ADDR);
                peer.listeners = (0..LISTENER_POOL_SIZE)
                    .map(|_| stack.add_listener(INTERNAL_PORT))
                    .collect();
                peer.stack = Some(stack);
                peer.poll_at = Some(tokio::time::Instant::now());
                peer.sid = Some(pending.sid);
                peer.crypto = Some(pending.channel);
                peer.ifindex = pending.interface;
                peer.last_seen = Instant::now();
                println!(
                    "client {} authenticated, session {}",
                    fmt_mac(&mac),
                    pending.sid
                );
            }
            r = tx.recv_with_meta(cfg.ethertype) => {
                let (f, ifindex) = r?;
                let Ok(l) = v6::decode(&f.payload) else {
                    continue;
                };
                let mac = f.src;
                match l.msg_type {
                    TYPE_DISCOVER | v6::HELLO => {
                        if !global_rate.allow()
                            || (rate.len() >= MAX_PEERS && !rate.contains_key(&mac))
                            || !rate_allow(&mut rate, &mac)
                        {
                            continue;
                        }
                        let Some(body) = gate.open(&l) else { continue };
                        if l.msg_type == TYPE_DISCOVER {
                            if body.len() < 8 || body[8..] != *cfg.discover_token.as_bytes() {
                                continue;
                            }
                            println!(
                                "discover {} via interface {}",
                                u64::from_be_bytes(body[..8].try_into().expect("8 bytes")),
                                ifindex
                            );
                            let mut answer = body[..8].to_vec();
                            answer.extend_from_slice(&public);
                            answer.extend_from_slice(cfg.device_name.as_bytes());
                            tx.send_on(
                                ifindex,
                                &mac,
                                cfg.ethertype,
                                &gate.encode(frame::TYPE_RESP, 0, &answer)?,
                            )?;
                        } else {
                            if l.session_id == 0
                                || body.get(..32) != Some(public.as_slice())
                                || (peers.len() >= MAX_PEERS && !peers.contains_key(&mac))
                            {
                                continue;
                            }
                            let peer = peers
                                .entry(mac)
                                .or_insert_with(|| new_peer(to_tx.clone(), allowed.clone()));
                            if peer.login_sid != Some(l.session_id) {
                                let Some(p) = Pending::new(l.session_id, ifindex, &body[32..], &private)
                                else {
                                    continue;
                                };
                                generation += 1;
                                peer.login_generation = generation;
                                peer.login_sid = Some(l.session_id);
                                peer.pending = Some(p);
                                peer.login_reply = None;
                                if peer.sid.is_none() {
                                    peer.last_seen = Instant::now();
                                }
                            }
                            if let Some(p) = &peer.pending {
                                tx.send_on(p.interface, &mac, cfg.ethertype, &p.welcome)?;
                            }
                        }
                    }
                    TYPE_AUTH_REQ => {
                        let Some(peer) = peers.get_mut(&mac) else {
                            continue;
                        };
                        if let Some(reply) = peer.login_reply.as_ref().filter(|r| r.matches(&l)) {
                            if global_rate.allow() && rate_allow(&mut rate, &mac) {
                                tx.send_on(reply.interface, &mac, cfg.ethertype, &reply.encrypted)?;
                            }
                            continue;
                        }
                        let Some(p) = peer.pending.as_mut().filter(|p| p.sid == l.session_id) else {
                            continue;
                        };
                        let Some(credentials) = p.credentials(&l) else {
                            continue;
                        };
                        let mut pending = peer.pending.take().expect("just checked");
                        if checks.len() >= 4
                            || !login_rate.allow()
                            || guards.entry(mac).or_default().blocked()
                        {
                            let reply = pending
                                .channel
                                .seal(frame::TYPE_AUTH_NACK, pending.sid, &[])?;
                            tx.send_on(pending.interface, &mac, cfg.ethertype, &reply)?;
                            continue;
                        }
                        let attempt = peer.login_generation;
                        checks.spawn_blocking(move || {
                            (mac, attempt, pending, super::auth::verify(credentials))
                        });
                    }
                    TYPE_KEEPALIVE => {
                        let Some(peer) = peers.get_mut(&mac) else {
                            continue;
                        };
                        if peer.sid != Some(l.session_id) {
                            continue;
                        }
                        let Some(crypto) = peer.crypto.as_mut() else {
                            continue;
                        };
                        if open_session(crypto, &l, &mut fail_rate, mac).is_none() {
                            continue;
                        }
                        peer.last_seen = Instant::now();
                        let echo = crypto.seal(TYPE_KEEPALIVE, l.session_id, &[])?;
                        tx.send_on(peer.ifindex, &mac, cfg.ethertype, &echo)?;
                    }
                    TYPE_DATA => {
                        let Some(peer) = peers.get_mut(&mac) else {
                            continue;
                        };
                        if peer.sid != Some(l.session_id) {
                            continue;
                        }
                        let Some(crypto) = peer.crypto.as_mut() else {
                            continue;
                        };
                        let Some(plain) = open_session(crypto, &l, &mut fail_rate, mac) else {
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
                                &relay_wake,
                            )?;
                        }
                    }
                    TYPE_TEARDOWN => {
                        let mut drop_peer = false;
                        if let Some(peer) = peers.get_mut(&mac)
                            && peer.sid == Some(l.session_id)
                            && let Some(crypto) = peer.crypto.as_mut()
                        {
                            drop_peer = open_session(crypto, &l, &mut fail_rate, mac).is_some();
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
            Some(((mac, h, generation), msg)) = to_rx.recv(), if pending_tx_bytes < MAX_PENDING_TO_STACK_BYTES => {
                if let Some(peer) = peers.get_mut(&mac)
                    && peer
                        .conns
                        .get(&h)
                        .is_some_and(|conn| conn.generation == generation) {
                        match msg {
                            StackMsg::Data(b) => {
                                pending_tx_bytes += b.len();
                                peer.pending_tx.entry(h).or_default().push_back(b);
                            }
                            StackMsg::Close => {
                                if let Some(stack) = peer.stack.as_mut() {
                                    if peer.pending_tx.contains_key(&h) {
                                        peer.conns.get_mut(&h).unwrap().close_after_flush = true;
                                    } else {
                                        stack.close_socket(h);
                                    }
                                }
                            }
                        }
                        if let Some(sid) = peer.sid {
                            pump_peer(peer, &mac, &mut tx, cfg.ethertype, sid, &mut pending_tx_bytes, &relay_wake)?;
                        }
                    }
            }
            keys = relay_wake.recv() => {
                let ready = ready_peers(&peers, keys);
                for mac in ready {
                    if let Some(peer) = peers.get_mut(&mac)
                        && let Some(sid) = peer.sid {
                        pump_peer(peer, &mac, &mut tx, cfg.ethertype, sid, &mut pending_tx_bytes, &relay_wake)?;
                    }
                }
            }
            _ = wait_for_poll(deadline) => {
                let now = tokio::time::Instant::now();
                let due = due_peers(&peers, now);
                for mac in due {
                    if let Some(peer) = peers.get_mut(&mac)
                        && let Some(sid) = peer.sid {
                        pump_peer(peer, &mac, &mut tx, cfg.ethertype, sid, &mut pending_tx_bytes, &relay_wake)?;
                    }
                }
            }
            _ = sweep_timer.tick() => {
                let now = Instant::now();
                rate.retain(|_, b| now.duration_since(b.last_seen) <= STALE_AFTER);
                fail_rate.retain(|_, b| now.duration_since(b.last_seen) <= STALE_AFTER);
                guards.retain(|_, g| now.duration_since(g.last_seen) <= AUTH_WINDOW.max(AUTH_LOCKOUT));
                let stale: Vec<[u8; 6]> = peers
                    .iter()
                    .filter(|(_, p)| now.duration_since(p.last_seen) > if p.pending.is_some() && p.sid.is_none() { Duration::from_secs(300) } else { STALE_AFTER })
                    .map(|(m, _)| *m)
                    .collect();
                for mac in stale {
                    if let Some(peer) = peers.get_mut(&mac)
                        && let (Some(sid), Some(crypto)) = (peer.sid, peer.crypto.as_mut()) {
                            let raw = crypto.seal(TYPE_TEARDOWN, sid, &[])?;
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
            if let (Some(sid), Some(crypto)) = (peer.sid, peer.crypto.as_mut()) {
                let Ok(raw) = crypto.seal(TYPE_TEARDOWN, sid, &[]) else {
                    continue;
                };
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

fn ready_peers(
    peers: &HashMap<[u8; 6], Peer>,
    keys: Vec<ServerConnKey>,
) -> std::collections::HashSet<[u8; 6]> {
    keys.into_iter()
        .filter_map(|(mac, h, generation)| {
            peers
                .get(&mac)?
                .conns
                .get(&h)
                .is_some_and(|conn| conn.generation == generation)
                .then_some(mac)
        })
        .collect()
}

fn due_peers(peers: &HashMap<[u8; 6], Peer>, now: tokio::time::Instant) -> Vec<[u8; 6]> {
    peers
        .iter()
        .filter_map(|(mac, peer)| peer.poll_at.filter(|at| *at <= now).map(|_| *mac))
        .collect()
}

fn rate_allow(map: &mut HashMap<[u8; 6], RateBucket>, mac: &[u8; 6]) -> bool {
    map.entry(*mac).or_default().allow()
}

/// Charge only failed decryptions. Authenticated data is limited by the relay, not this bucket.
fn open_session(
    crypto: &mut v6::Transport,
    frame: &frame::Frame<'_>,
    failures: &mut HashMap<[u8; 6], RateBucket>,
    mac: [u8; 6],
) -> Option<Vec<u8>> {
    let budget = failures
        .entry(mac)
        .or_insert_with(|| RateBucket::new(SESSION_FAIL_PER_SEC));
    if !budget.has_token() {
        return None;
    }
    let plaintext = crypto.open(frame);
    if plaintext.is_none() {
        budget.tokens -= 1.0;
    }
    plaintext
}

fn new_peer(to_tx: mpsc::Sender<ServerStackMsg>, allowed: Arc<[u16]>) -> Peer {
    Peer {
        sid: None,
        pending: None,
        login_generation: 0,
        login_sid: None,
        login_reply: None,
        ifindex: 0,

        stack: None,
        poll_at: None,
        listeners: Vec::new(),
        crypto: None,
        conns: HashMap::new(),
        pending_tx: HashMap::new(),
        next_generation: 1,
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
    peer.sid = None;
    peer.stack = None;
    peer.poll_at = None;
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
    relay_wake: &RelayWake<ServerConnKey>,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(stack) = peer.stack.as_mut() else {
        return Ok(());
    };
    let Some(crypto) = peer.crypto.as_mut() else {
        return Ok(());
    };

    // Process ingress before accepting sockets or draining their buffers.
    for pkt in stack.poll() {
        let raw = crypto.seal(TYPE_DATA, sid, &pkt)?;
        tx.send_on(peer.ifindex, peer_mac, ethertype, &raw)?;
    }

    for listener in &mut peer.listeners {
        if let Some((h, new_listener)) = stack.accept(*listener, INTERNAL_PORT) {
            *listener = new_listener;
            let (from_tx, from_rx) = mpsc::channel(CONNECTION_CHANNEL_CAPACITY);
            let generation = peer.next_generation;
            peer.next_generation = peer.next_generation.wrapping_add(1).max(1);
            let relay = landscape_terrain_proto::concurrency::spawn_task(
                crate::task_names::FLARE_CONNECTION,
                server_conn_task(
                    *peer_mac,
                    h,
                    generation,
                    peer.to_tx.clone(),
                    relay_wake.watch((*peer_mac, h, generation), from_rx),
                    peer.allowed.clone(),
                ),
            );
            peer.conns.insert(
                h,
                ServerConn {
                    from_tx,
                    generation,
                    close_after_flush: false,
                    peer_eof_sent: false,
                    relay: Some(relay.abort_handle()),
                },
            );
        }
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
        if !peer.pending_tx.contains_key(&h) && peer.conns[&h].close_after_flush {
            stack.close_socket(h);
            peer.conns.get_mut(&h).unwrap().close_after_flush = false;
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
            permit.send(RelayMsg::Data(buf[..n].to_vec()));
        }

        let from_drained = peer.conns[&h].from_tx.is_closed()
            || peer.conns[&h].from_tx.capacity() == peer.conns[&h].from_tx.max_capacity();
        if stack.socket_closed(h) && from_drained {
            reap.push(h);
        } else if stack.peer_eof(h) && !peer.conns[&h].peer_eof_sent {
            match peer.conns[&h].from_tx.try_send(RelayMsg::PeerEof) {
                Ok(()) => peer.conns.get_mut(&h).unwrap().peer_eof_sent = true,
                Err(mpsc::error::TrySendError::Full(_)) => {}
                Err(mpsc::error::TrySendError::Closed(_)) => stack.close_socket(h),
            }
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
    // Flush writes, FINs and receive-window updates in this turn, rather
    // than relying on a later periodic tick to advance the stack.
    for pkt in stack.poll() {
        let raw = crypto.seal(TYPE_DATA, sid, &pkt)?;
        tx.send_on(peer.ifindex, peer_mac, ethertype, &raw)?;
    }
    peer.poll_at = stack
        .poll_delay()
        .map(|delay| tokio::time::Instant::now() + delay);
    Ok(())
}

/// Server side of one relayed connection: read the 2-byte target port from
/// the stream, dial 127.0.0.1:<port>, then bridge both directions.
async fn server_conn_task(
    peer_mac: [u8; 6],
    handle: SocketHandle,
    generation: u64,
    to_tx: mpsc::Sender<ServerStackMsg>,
    mut from_rx: RelayReceiver<RelayMsg, ServerConnKey>,
    allowed: Arc<[u16]>,
) {
    let mut header = Vec::new();
    while header.len() < 2 {
        match from_rx.recv().await {
            Some(RelayMsg::Data(b)) => header.extend_from_slice(&b),
            Some(RelayMsg::PeerEof) | None => {
                signal_close(&to_tx, peer_mac, handle, generation).await;
                return;
            }
        }
    }
    let dst = u16::from_be_bytes([header[0], header[1]]);
    if !allowed.contains(&dst) {
        println!("  [server] forward to 127.0.0.1:{dst} not allowed, closing");
        let _ = to_tx
            .send(((peer_mac, handle, generation), StackMsg::Close))
            .await;
        return;
    }
    let remote = match TcpStream::connect(("127.0.0.1", dst)).await {
        Ok(s) => s,
        Err(e) => {
            println!("  [server] dial 127.0.0.1:{dst} failed: {e}");
            signal_close(&to_tx, peer_mac, handle, generation).await;
            return;
        }
    };
    let (mut remote_read, mut remote_write) = remote.into_split();
    let extra = &header[2..];
    if !extra.is_empty() && remote_write.write_all(extra).await.is_err() {
        signal_close(&to_tx, peer_mac, handle, generation).await;
        return;
    }

    let mut buf = vec![0u8; 8192];
    let mut writer_done = false;
    let writer = relay_remote_writer(from_rx, remote_write);
    tokio::pin!(writer);
    loop {
        tokio::select! {
            result = &mut writer, if !writer_done => {
                writer_done = true;
                if result.is_err() {
                    signal_close(&to_tx, peer_mac, handle, generation).await;
                    return;
                }
            },
            r = remote_read.read(&mut buf) => match r {
                Ok(0) => {
                    signal_close(&to_tx, peer_mac, handle, generation).await;
                    return;
                }
                Ok(n) => {
                    if to_tx
                        .send(((peer_mac, handle, generation), StackMsg::Data(buf[..n].to_vec())))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                Err(_) => {
                    signal_close(&to_tx, peer_mac, handle, generation).await;
                    return;
                }
            },
        }
    }
}

async fn relay_remote_writer(
    mut from_rx: RelayReceiver<RelayMsg, ServerConnKey>,
    mut remote_write: tokio::net::tcp::OwnedWriteHalf,
) -> Result<(), ()> {
    while let Some(message) = from_rx.recv().await {
        match message {
            RelayMsg::Data(bytes) => remote_write.write_all(&bytes).await.map_err(|_| ())?,
            RelayMsg::PeerEof => {
                remote_write.shutdown().await.map_err(|_| ())?;
                return Ok(());
            }
        }
    }
    // A missing producer means the session/connection disappeared, not a TCP
    // half-close. Only the explicit PeerEof message permits waiting for replies.
    Err(())
}

async fn signal_close(
    to_tx: &mpsc::Sender<ServerStackMsg>,
    peer_mac: [u8; 6],
    handle: SocketHandle,
    generation: u64,
) {
    let _ = to_tx
        .send(((peer_mac, handle, generation), StackMsg::Close))
        .await;
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

    struct RelayFixture {
        from_tx: mpsc::Sender<RelayMsg>,
        task: tokio::task::JoinHandle<()>,
        remote: TcpStream,
        to_rx: mpsc::Receiver<ServerStackMsg>,
        handle: SocketHandle,
    }

    async fn relay_fixture() -> RelayFixture {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = IpStack::new(SERVER_ADDR).add_listener(INTERNAL_PORT);
        let (from_tx, from_rx) = mpsc::channel(4);
        let (to_tx, to_rx) = mpsc::channel(4);
        from_tx
            .send(RelayMsg::Data(port.to_be_bytes().to_vec()))
            .await
            .unwrap();
        let task = tokio::spawn(server_conn_task(
            [1; 6],
            handle,
            1,
            to_tx,
            RelayWake::default().watch(([1; 6], handle, 1), from_rx),
            Arc::from([port]),
        ));
        let (remote, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
            .await
            .expect("relay must connect to the ephemeral local service")
            .unwrap();
        RelayFixture {
            from_tx,
            task,
            remote,
            to_rx,
            handle,
        }
    }

    #[tokio::test]
    async fn disappearing_relay_channel_closes_the_local_socket() {
        let RelayFixture {
            from_tx,
            task,
            mut remote,
            mut to_rx,
            ..
        } = relay_fixture().await;
        drop(from_tx);
        // Keep the local service open and silent: its EOF must not be needed
        // for the relay to terminate after losing the session's channel.
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("a vanished session must terminate its relay")
            .unwrap();
        assert!(matches!(to_rx.recv().await, Some((_, StackMsg::Close))));
        assert_eq!(remote.read(&mut [0]).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn session_teardown_cancels_half_closed_relays() {
        // Explicit peer teardown and dropping the entire server both own the
        // lifetime of the relay, including after a successful TCP half-close.
        for drop_server in [false, true] {
            tokio::time::timeout(Duration::from_secs(2), async {
                let RelayFixture {
                    from_tx,
                    task,
                    mut remote,
                    mut to_rx,
                    handle,
                } = relay_fixture().await;
                let (to_tx, _) = mpsc::channel(1);
                let mut peer = new_peer(to_tx, Arc::from([]));
                peer.conns.insert(
                    handle,
                    ServerConn {
                        from_tx,
                        generation: 1,
                        close_after_flush: false,
                        peer_eof_sent: false,
                        relay: Some(task.abort_handle()),
                    },
                );
                peer.conns[&handle]
                    .from_tx
                    .send(RelayMsg::PeerEof)
                    .await
                    .unwrap();
                assert_eq!(remote.read(&mut [0]).await.unwrap(), 0);
                assert!(!task.is_finished(), "half-close must preserve replies");
                remote.write_all(b"reply after EOF").await.unwrap();
                let mut reply = Vec::new();
                while reply.len() < b"reply after EOF".len() {
                    let (_, StackMsg::Data(bytes)) = to_rx.recv().await.unwrap() else {
                        panic!("half-close must forward the local service's reply");
                    };
                    reply.extend(bytes);
                }
                assert_eq!(reply, b"reply after EOF");
                if drop_server {
                    drop(HashMap::from([([1; 6], peer)]));
                } else {
                    teardown_peer(&mut peer, &mut 0);
                }
                assert!(task.await.unwrap_err().is_cancelled());
                // The remote service remains open until after the cancelled
                // relay is joined, so service cooperation cannot hide leaks.
                drop(remote);
            })
            .await
            .expect("session teardown must release half-closed relays");
        }
    }

    #[test]
    fn deadlines_select_only_due_peers_and_teardown_clears_them() {
        let (tx, _) = mpsc::channel(1);
        let mut peers = HashMap::new();
        let now = tokio::time::Instant::now();
        for (mac, deadline) in [
            (1, None),
            (2, Some(now)),
            (3, Some(now + Duration::from_secs(30))),
        ] {
            let mut peer = new_peer(tx.clone(), Arc::from([]));
            peer.poll_at = deadline;
            peers.insert([mac; 6], peer);
        }
        assert_eq!(due_peers(&peers, now), vec![[2; 6]]);
        let mut pending_bytes = 0;
        teardown_peer(peers.get_mut(&[2; 6]).unwrap(), &mut pending_bytes);
        assert!(due_peers(&peers, now).is_empty());
    }

    #[test]
    fn capacity_wakes_ignore_stale_connections_and_coalesce_peers() {
        let (tx, _) = mpsc::channel(1);
        let mut peer = new_peer(tx, Arc::from([]));
        let mut stack = IpStack::new(SERVER_ADDR);
        let h = stack.add_listener(INTERNAL_PORT);
        let (from_tx, _) = mpsc::channel(1);
        peer.conns.insert(
            h,
            ServerConn {
                from_tx,
                generation: 2,
                close_after_flush: false,
                peer_eof_sent: false,
                relay: None,
            },
        );
        let peers = HashMap::from([([1; 6], peer)]);
        assert!(ready_peers(&peers, vec![([1; 6], h, 1), ([2; 6], h, 2)]).is_empty());
        assert_eq!(
            ready_peers(&peers, vec![([1; 6], h, 2), ([1; 6], h, 2)]),
            std::collections::HashSet::from([[1; 6]])
        );
    }

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
        assert!(g.locked_until.is_some());
        // Once unlocked, everything is clean again.
        g.record_success();
        assert!(!g.blocked());
        assert!(g.locked_until.is_none());
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
