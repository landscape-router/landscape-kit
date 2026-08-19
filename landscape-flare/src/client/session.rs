use std::collections::{HashMap, VecDeque};

use landscape_terrain_proto::ipstack::{
    CLIENT_ADDR, INTERNAL_PORT, IpStack, SERVER_ADDR, SocketHandle, StackMsg,
};
use landscape_terrain_proto::protocol::crypto::{Dir, SessionCrypto, SessionKeys};
use landscape_terrain_proto::protocol::frame;
use landscape_terrain_proto::protocol::session::KEEPALIVE_INTERVAL;
use landscape_terrain_proto::protocol::{TYPE_DATA, TYPE_KEEPALIVE, TYPE_TEARDOWN};
use landscape_terrain_proto::transport::Link;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};

use super::forward::{Conn, ConnKey, bridge_task, close_connections, spawn_listener};
use super::{
    ClientConfig, ClientEvent, FailBudget, Forward, ForwardCommand, ForwardRejection, LogLevel,
    MAX_MISSED_KEEPALIVES, POLL_INTERVAL, emit_event, pump,
};

const FIRST_LOCAL_PORT: u16 = 40000;
const CONNECTION_CHANNEL_CAPACITY: usize = 16;
const MAX_PENDING_TO_STACK_BYTES: usize = 4 * 1024 * 1024;

/// Allocate an internal source port without colliding with a live or
/// TIME-WAIT socket. The old monotonic allocator eventually wrapped and
/// handed a still-used port to smoltcp, making long-running HTTP clients
/// fail after enough short-lived requests.
fn allocate_local_port(next: &mut u16, is_used: impl Fn(u16) -> bool) -> Option<u16> {
    let start = *next;
    loop {
        let candidate = *next;
        *next = if candidate == u16::MAX {
            FIRST_LOCAL_PORT
        } else {
            candidate + 1
        };
        if !is_used(candidate) {
            return Some(candidate);
        }
        if *next == start {
            return None;
        }
    }
}

/// Why a session loop ended.
pub(super) enum SessionEnd {
    /// Keepalives went unanswered: the link is gone.
    LinkLost,
    /// The server sent a teardown.
    PeerClosed,
    /// SIGINT/SIGTERM: graceful shutdown (TEARDOWN is sent first).
    Shutdown,
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn session_loop(
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
    let (to_tx, mut to_rx) = mpsc::channel::<(ConnKey, StackMsg)>(512);
    let (accept_tx, mut accept_rx) = mpsc::channel::<(TcpStream, Forward)>(16);
    let mut conns: HashMap<SocketHandle, Conn> = HashMap::new();
    let mut pending_tx: HashMap<SocketHandle, VecDeque<Vec<u8>>> = HashMap::new();
    let mut pending_tx_bytes = 0usize;

    let mut listeners = HashMap::new();
    for &forward in active_forwards.iter() {
        listeners.insert(
            forward,
            spawn_listener(
                forward,
                accept_tx.clone(),
                cfg.log.clone(),
                cfg.events.clone(),
            ),
        );
    }

    let mut poll_timer = tokio::time::interval(POLL_INTERVAL);
    let mut last_keepalive = tokio::time::Instant::now();
    let mut missed = 0u32;
    let mut next_local_port = FIRST_LOCAL_PORT;
    let mut next_generation = 1u64;
    let mut fail_budget = FailBudget::default();

    let result: Result<SessionEnd, Box<dyn std::error::Error>> = 'session: loop {
        tokio::select! {
            r = tx.recv(cfg.ethertype) => {
                let f = match r {
                    Ok(frame) => frame,
                    Err(error) => break 'session Err(error),
                };
                if let Ok(l) = frame::decode(&f.payload) {
                    if l.session_id != sid {
                        continue;
                    }
                    match l.msg_type {
                        TYPE_TEARDOWN => {
                            if crypto.open(l.msg_type, l.session_id, l.seq, l.len, l.payload).is_some() {
                                cfg.log.emit(LogLevel::Info, "  teardown from server, closing session".into());
                                break 'session Ok(SessionEnd::PeerClosed);
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
                            // Any authenticated frame proves the peer and link
                            // are alive. Under heavy forwarding traffic a
                            // keepalive echo may be delayed or dropped even
                            // while DATA is still arriving.
                            missed = 0;
                            if l.msg_type == TYPE_DATA {
                                stack.push_packet(&plain);
                                if let Err(error) = pump(&mut stack, tx, server_mac, sid, &mut crypto, cfg.ethertype, &mut conns, &mut pending_tx, &mut pending_tx_bytes) {
                                    break 'session Err(error);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            Some((stream, forward)) = accept_rx.recv() => {
                let Some(local_port) = allocate_local_port(
                    &mut next_local_port,
                    |port| conns.values().any(|conn| conn.local_port == port),
                ) else {
                    cfg.log.emit(
                        LogLevel::Warn,
                        "  all internal forwarding ports are busy; dropping local connection".into(),
                    );
                    drop(stream);
                    continue;
                };
                let h = stack.connect(SERVER_ADDR, INTERNAL_PORT, local_port);
                let generation = next_generation;
                next_generation = next_generation.wrapping_add(1).max(1);
                let (from_tx, from_rx) = mpsc::channel(CONNECTION_CHANNEL_CAPACITY);
                let (close_tx, close_rx) = oneshot::channel();
                conns.insert(
                    h,
                    Conn {
                        from_tx,
                        forward,
                        local_port,
                        generation,
                        close_tx: Some(close_tx),
                    },
                );
                tokio::spawn(bridge_task(
                    stream,
                    (h, generation),
                    to_tx.clone(),
                    from_rx,
                    close_rx,
                ));
                pending_tx
                    .entry(h)
                    .or_default()
                    .push_back(forward.1.to_be_bytes().to_vec());
                pending_tx_bytes += std::mem::size_of::<u16>();
            }
            Some(((h, generation), msg)) = to_rx.recv(), if pending_tx_bytes < MAX_PENDING_TO_STACK_BYTES => {
                if conns.get(&h).is_none_or(|conn| conn.generation != generation) {
                    continue;
                }
                match msg {
                    StackMsg::Data(b) => {
                        pending_tx_bytes += b.len();
                        pending_tx.entry(h).or_default().push_back(b);
                    }
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
                                    reason: ForwardRejection::Duplicate,
                                },
                            );
                        } else if !advertised_ports.contains(&forward.1) {
                            cfg.log.emit(
                                LogLevel::Warn,
                                format!(
                                    "  forward rejected: server does not advertise destination port {}",
                                    forward.1
                                ),
                            );
                            emit_event(
                                &cfg.events,
                                ClientEvent::ForwardRejected {
                                    forward,
                                    reason: ForwardRejection::DestinationNotAdvertised {
                                        port: forward.1,
                                    },
                                },
                            );
                        } else {
                            active_forwards.push(forward);
                            if let std::collections::hash_map::Entry::Vacant(entry) = listeners.entry(forward) {
                                entry.insert(spawn_listener(
                                    forward,
                                    accept_tx.clone(),
                                    cfg.log.clone(),
                                    cfg.events.clone(),
                                ));
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
                        close_connections(forward, &mut stack, &mut conns);
                    }
                    None => *forward_control = None,
                }
            }
            _ = poll_timer.tick() => {
                if let Err(error) = pump(&mut stack, tx, server_mac, sid, &mut crypto, cfg.ethertype, &mut conns, &mut pending_tx, &mut pending_tx_bytes) {
                    break 'session Err(error);
                }
            }
            _ = tokio::time::sleep_until(last_keepalive + KEEPALIVE_INTERVAL) => {
                let raw = crypto.seal(TYPE_KEEPALIVE, sid, &[]);
                if let Err(error) = tx.send(server_mac, cfg.ethertype, &raw) {
                    break 'session Err(error);
                }
                last_keepalive = tokio::time::Instant::now();
                missed += 1;
                if missed >= MAX_MISSED_KEEPALIVES {
                    cfg.log.emit(
                        LogLevel::Info,
                        format!("  no keepalive echo for {missed} rounds, link assumed lost"),
                    );
                    break 'session Ok(SessionEnd::LinkLost);
                }
            }
            _ = notify.notified() => {
                cfg.log.emit(LogLevel::Info, "  shutdown requested, sending teardown".into());
                break 'session Ok(SessionEnd::Shutdown);
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
    result
}

async fn recv_forward_command(
    receiver: &mut Option<tokio::sync::mpsc::UnboundedReceiver<ForwardCommand>>,
) -> Option<ForwardCommand> {
    match receiver.as_mut() {
        Some(receiver) => receiver.recv().await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_port_allocator_skips_ports_in_use() {
        let mut next = FIRST_LOCAL_PORT;
        let used = [FIRST_LOCAL_PORT, FIRST_LOCAL_PORT + 1];
        assert_eq!(
            allocate_local_port(&mut next, |port| used.contains(&port)),
            Some(FIRST_LOCAL_PORT + 2)
        );
        assert_eq!(next, FIRST_LOCAL_PORT + 3);
    }

    #[test]
    fn local_port_allocator_wraps_without_reusing_busy_ports() {
        let mut next = u16::MAX;
        assert_eq!(
            allocate_local_port(&mut next, |port| port == u16::MAX),
            Some(FIRST_LOCAL_PORT)
        );
        assert_eq!(next, FIRST_LOCAL_PORT + 1);
    }

    #[test]
    fn local_port_allocator_reports_exhaustion() {
        let mut next = FIRST_LOCAL_PORT;
        assert_eq!(allocate_local_port(&mut next, |_| true), None);
        assert_eq!(next, FIRST_LOCAL_PORT);
    }
}
