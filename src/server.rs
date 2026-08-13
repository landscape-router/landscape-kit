use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use landscape_proto::ipstack::{
    IpStack, SocketHandle, StackMsg, INTERNAL_PORT, SERVER_ADDR,
};
use landscape_proto::protocol::frame;
use landscape_proto::protocol::session::{ServerSession, VerifyResult};
use landscape_proto::protocol::{
    TYPE_AUTH_REQ, TYPE_DATA, TYPE_DISCOVER, TYPE_KEEPALIVE,
};
use landscape_proto::transport::{fmt_mac, Link};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

const POLL_INTERVAL: Duration = Duration::from_millis(25);
const SWEEP_INTERVAL: Duration = Duration::from_secs(5);
const STALE_AFTER: Duration = Duration::from_secs(45);

pub struct ServerConfig<'a> {
    pub devs: &'a [String],
    pub ethertype: u16,
    pub mac: Option<[u8; 6]>,
    pub psk: &'a str,
    pub device_name: &'a str,
    /// Ports the server is allowed to dial on 127.0.0.1.
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
    listener: Option<SocketHandle>,
    conns: HashMap<SocketHandle, ServerConn>,
    pending_tx: HashMap<SocketHandle, VecDeque<Vec<u8>>>,
    pending_rx: HashMap<SocketHandle, VecDeque<Vec<u8>>>,
    to_tx: mpsc::Sender<(([u8; 6], SocketHandle), StackMsg)>,
    allowed: Arc<[u16]>,
    last_seen: Instant,
}

/// One relayed connection on the server side.
struct ServerConn {
    /// Bytes from the stack to the kernel socket (dialed service).
    from_tx: mpsc::Sender<Vec<u8>>,
}

pub async fn run(cfg: &ServerConfig<'_>) -> Result<(), Box<dyn std::error::Error>> {
    let mut tx = Link::open(cfg.devs, cfg.ethertype, cfg.mac)?;
    let mut peers: HashMap<[u8; 6], Peer> = HashMap::new();
    let allowed: Arc<[u16]> = Arc::from(cfg.forward_ports);
    let (to_tx, mut to_rx) = mpsc::channel::<(([u8; 6], SocketHandle), StackMsg)>(512);
    println!(
        "server '{}' ready on {} (ethertype 0x{:04x}, forward ports: {})",
        cfg.device_name,
        devs_display(&tx.names()),
        cfg.ethertype,
        cfg.forward_ports
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join(",")
    );

    let mut poll_timer = tokio::time::interval(POLL_INTERVAL);
    let mut sweep_timer = tokio::time::interval(SWEEP_INTERVAL);
    loop {
        tokio::select! {
            r = tx.recv_with_meta(cfg.ethertype) => {
                let (f, ifindex) = r?;
                let Ok(l) = frame::decode(&f.payload) else {
                    continue;
                };
                let mac = f.src;
                match l.msg_type {
                    TYPE_DISCOVER => {
                        let Ok((name, token)) = frame::decode_discover(&l.payload) else {
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
                        if let Some(old) = peers.get_mut(&mac) {
                            teardown_peer(old);
                        }
                        let peer = peers.entry(mac).or_insert_with(|| new_peer(to_tx.clone(), allowed.clone()));
                        peer.sess = ServerSession::new();
                        let resp = peer.sess.on_discover(cfg.device_name);
                        peer.ifindex = ifindex;
                        tx.send_on(ifindex, &mac, cfg.ethertype, &resp)?;
                        println!("discover from {} '{}'", fmt_mac(&mac), name);
                    }
                    TYPE_AUTH_REQ => {
                        let Some(peer) = peers.get_mut(&mac) else {
                            println!("auth attempt from unknown peer {}, ignored", fmt_mac(&mac));
                            continue;
                        };
                        let ifindex = peer.ifindex;
                        let Ok(req) = frame::decode_auth_req(&l.payload) else {
                            println!("malformed AUTH_REQ from {}", fmt_mac(&mac));
                            continue;
                        };
                        match peer.sess.verify_auth(&req, cfg.psk.as_bytes()) {
                            VerifyResult::Accepted(sid) => {
                                let mut stack = IpStack::new(SERVER_ADDR);
                                let listener = stack.add_listener(INTERNAL_PORT);
                                peer.stack = Some(stack);
                                peer.listener = Some(listener);
                                peer.last_seen = Instant::now();
                                tx.send_on(ifindex, &mac, cfg.ethertype, &frame::encode_auth_ack(sid))?;
                                println!(
                                    "client {} '{}' authenticated, session {sid}",
                                    fmt_mac(&mac),
                                    req.user
                                );
                            }
                            VerifyResult::Rejected(reason) => {
                                tx.send_on(ifindex, &mac, cfg.ethertype, &frame::encode_auth_nack(&reason))?;
                                peers.remove(&mac);
                                println!("auth rejected for {}: {reason}", fmt_mac(&mac));
                            }
                        }
                    }
                    TYPE_KEEPALIVE => {
                        if let Some(peer) = peers.get_mut(&mac) {
                            if let Some(sid) = peer.sess.session_id() {
                                tx.send_on(ifindex, &mac, cfg.ethertype, &frame::encode_keepalive(sid))?;
                            }
                            peer.last_seen = Instant::now();
                        }
                    }
                    TYPE_DATA => {
                        if let Some(peer) = peers.get_mut(&mac) {
                            if peer.sess.session_id() == Some(l.session_id) {
                                peer.last_seen = Instant::now();
                                if let Some(stack) = peer.stack.as_mut() {
                                    stack.push_packet(l.payload);
                                    pump_peer(peer, &mac, &mut tx, cfg.ethertype, l.session_id)?;
                                }
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
            Some(((mac, h), msg)) = to_rx.recv() => {
                if let Some(peer) = peers.get_mut(&mac) {
                    if peer.conns.contains_key(&h) {
                        match msg {
                            StackMsg::Data(b) => peer.pending_tx.entry(h).or_default().push_back(b),
                            StackMsg::Close => {
                                if let Some(stack) = peer.stack.as_mut() {
                                    stack.close_socket(h);
                                }
                            }
                        }
                    }
                }
            }
            _ = poll_timer.tick() => {
                let macs: Vec<[u8; 6]> = peers.keys().copied().collect();
                for mac in macs {
                    if let Some(peer) = peers.get_mut(&mac) {
                        if let Some(sid) = peer.sess.session_id() {
                            pump_peer(peer, &mac, &mut tx, cfg.ethertype, sid)?;
                        }
                    }
                }
            }
            _ = sweep_timer.tick() => {
                let now = Instant::now();
                let stale: Vec<[u8; 6]> = peers
                    .iter()
                    .filter(|(_, p)| now.duration_since(p.last_seen) > STALE_AFTER)
                    .map(|(m, _)| *m)
                    .collect();
                for mac in stale {
                    if let Some(mut peer) = peers.remove(&mac) {
                        teardown_peer(&mut peer);
                        println!("  peer {} timed out, dropped", fmt_mac(&mac));
                    }
                }
            }
        }
    }
}

fn new_peer(
    to_tx: mpsc::Sender<(([u8; 6], SocketHandle), StackMsg)>,
    allowed: Arc<[u16]>,
) -> Peer {
    Peer {
        sess: ServerSession::new(),
        ifindex: 0,
        stack: None,
        listener: None,
        conns: HashMap::new(),
        pending_tx: HashMap::new(),
        pending_rx: HashMap::new(),
        to_tx,
        allowed,
        last_seen: Instant::now(),
    }
}

/// Drop the peer's stack and connections (kernel sockets close via the
/// channels; their tasks exit).
fn teardown_peer(peer: &mut Peer) {
    peer.stack = None;
    peer.listener = None;
    peer.conns.clear();
    peer.pending_tx.clear();
    peer.pending_rx.clear();
}

/// Pump one peer's stack: accept new internal connections, send outbound IP
/// packets back to the peer, and move bytes between the stack and the dialed
/// kernel sockets.
fn pump_peer(
    peer: &mut Peer,
    peer_mac: &[u8; 6],
    tx: &mut Link,
    ethertype: u16,
    sid: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(stack) = peer.stack.as_mut() else {
        return Ok(());
    };

    if let Some(listener) = peer.listener {
        if let Some((h, new_listener)) = stack.accept(listener, INTERNAL_PORT) {
            peer.listener = Some(new_listener);
            let (from_tx, from_rx) = mpsc::channel(512);
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
        tx.send_on(peer.ifindex, peer_mac, ethertype, &frame::encode_data(sid, &pkt))?;
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
            let n = stack.recv_bytes(h, &mut buf);
            if n == 0 {
                break;
            }
            peer.pending_rx.entry(h).or_default().push_back(buf[..n].to_vec());
        }
        if let Some(q) = peer.pending_rx.get_mut(&h) {
            while let Some(b) = q.front() {
                let b = b.clone();
                match peer.conns[&h].from_tx.try_send(b) {
                    Ok(()) => {
                        q.pop_front();
                    }
                    Err(_) => break,
                }
            }
            if q.is_empty() {
                peer.pending_rx.remove(&h);
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
        peer.pending_tx.remove(&h);
        peer.pending_rx.remove(&h);
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
            None => return,
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
            let _ = to_tx.send(((peer_mac, handle), StackMsg::Close)).await;
            return;
        }
    };
    let extra = &header[2..];
    if !extra.is_empty() && remote.write_all(extra).await.is_err() {
        return;
    }

    let mut buf = vec![0u8; 8192];
    loop {
        tokio::select! {
            msg = from_rx.recv() => match msg {
                Some(b) => {
                    if remote.write_all(&b).await.is_err() {
                        return;
                    }
                }
                None => return,
            },
            r = remote.read(&mut buf) => match r {
                Ok(0) => {
                    let _ = to_tx.send(((peer_mac, handle), StackMsg::Close)).await;
                    return;
                }
                Ok(n) => {
                    if to_tx.send(((peer_mac, handle), StackMsg::Data(buf[..n].to_vec()))).await.is_err() {
                        return;
                    }
                }
                Err(_) => return,
            },
        }
    }
}

fn devs_display(names: &[String]) -> String {
    if names.len() == 1 && names[0] == "any" {
        "all interfaces".to_string()
    } else {
        names.join(", ")
    }
}
