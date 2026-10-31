//! Userspace IP/TCP stack (smoltcp) bridged over the Terrain link.
//!
//! Every process runs one `IpStack` (the server keeps one per authenticated
//! peer). IP packets travel inside Terrain DATA frames between the two stacks; a
//! fixed point-to-point /31 keeps each pair isolated:
//!
//! - client: `10.13.0.1`
//! - server: `10.13.0.2`
//!
//! TCP connections between the stacks ride on internal port [`INTERNAL_PORT`];
//! the first two bytes of each stream carry the target port (big endian) the
//! server should dial on `127.0.0.1`. Reliability, ordering, connection
//! setup and teardown are all handled by smoltcp's TCP — the Terrain layer only
//! carries raw IP packets.
//!
//! No virtual network interface is created: the device is a pure in-memory
//! queue pair.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use smoltcp::iface::{Config, Interface, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::tcp::{Socket as TcpSocket, SocketBuffer, State as TcpState};
use smoltcp::time::Instant as SmolInstant;
use smoltcp::wire::{HardwareAddress, IpAddress, IpCidr, Ipv4Address, Ipv4Cidr};

pub use smoltcp::iface::SocketHandle;

/// Internal port the smoltcp stacks connect to.
pub const INTERNAL_PORT: u16 = 4444;
/// Client-side stack address (point-to-point /31 with the server).
pub const CLIENT_ADDR: Ipv4Address = Ipv4Address::from_octets([10, 13, 0, 1]);
/// Server-side stack address.
pub const SERVER_ADDR: Ipv4Address = Ipv4Address::from_octets([10, 13, 0, 2]);
/// Virtual link MTU: fits into a standard 1500-byte ethernet frame.
pub const MTU: usize = 1400;
/// Per-socket buffer size (both directions, bounded memory per connection).
pub const SOCKET_BUFFER: usize = 64 * 1024;

/// Capacity of the per-connection channel moving bytes between a kernel
/// socket and its stack socket; bounds buffering per relayed connection.
pub const CONNECTION_CHANNEL_CAPACITY: usize = 16;
/// Server/client-wide cap on bytes buffered toward the stack from the
/// kernel side, so slow links backpressure via the TCP window instead of
/// growing memory without bound on long-running relays.
pub const MAX_PENDING_TO_STACK_BYTES: usize = 32 * 1024 * 1024;

fn peer_eof_state(state: TcpState, can_recv: bool) -> bool {
    !can_recv
        && matches!(
            state,
            TcpState::CloseWait | TcpState::Closing | TcpState::LastAck | TcpState::TimeWait
        )
}

/// A message pushed from a connection task into the stack.
pub enum StackMsg {
    /// Bytes read from the kernel TCP socket, to be sent into the stack.
    Data(Vec<u8>),
    /// Kernel socket EOF: close the stack's TCP socket (sends FIN).
    Close,
}

/// Point-to-point device: IP packets in, IP packets out, nothing else.
struct LndpDevice {
    rx_queue: VecDeque<Vec<u8>>,
    tx_queue: VecDeque<Vec<u8>>,
}

impl LndpDevice {
    fn new() -> Self {
        Self {
            rx_queue: VecDeque::new(),
            tx_queue: VecDeque::new(),
        }
    }
}

struct RxTokenImpl(Vec<u8>);

struct TxTokenImpl<'a> {
    queue: &'a mut VecDeque<Vec<u8>>,
}

impl Device for LndpDevice {
    type RxToken<'a>
        = RxTokenImpl
    where
        Self: 'a;
    type TxToken<'a>
        = TxTokenImpl<'a>
    where
        Self: 'a;

    fn receive(
        &mut self,
        _timestamp: SmolInstant,
    ) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        self.rx_queue.pop_front().map(|pkt| {
            (
                RxTokenImpl(pkt),
                TxTokenImpl {
                    queue: &mut self.tx_queue,
                },
            )
        })
    }

    fn transmit(&mut self, _timestamp: SmolInstant) -> Option<Self::TxToken<'_>> {
        Some(TxTokenImpl {
            queue: &mut self.tx_queue,
        })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ip;
        caps.max_transmission_unit = MTU;
        caps
    }
}

impl RxToken for RxTokenImpl {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(&self.0)
    }
}

impl<'a> TxToken for TxTokenImpl<'a> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut buf = vec![0u8; len];
        let r = f(&mut buf);
        self.queue.push_back(buf);
        r
    }
}

/// A smoltcp stack with an in-memory device and owned (heap) socket buffers.
pub struct IpStack {
    /// Protocol timers use elapsed monotonic time, not the host wall clock.
    clock: Instant,
    iface: Interface,
    device: LndpDevice,
    sockets: SocketSet<'static>,
}

impl IpStack {
    pub fn new(local: Ipv4Address) -> Self {
        let mut config = Config::new(HardwareAddress::Ip);
        config.random_seed = rand::random();
        let mut device = LndpDevice::new();
        let sockets = SocketSet::new(Vec::new());
        let clock = Instant::now();
        let mut iface = Interface::new(config, &mut device, SmolInstant::from_micros(0));
        iface.update_ip_addrs(|addrs| {
            let _ = addrs.push(IpCidr::Ipv4(Ipv4Cidr::new(local, 31)));
        });
        Self {
            clock,
            iface,
            device,
            sockets,
        }
    }

    /// Feed an IP packet received over the Terrain link into the stack.
    pub fn push_packet(&mut self, bytes: &[u8]) {
        self.device.rx_queue.push_back(bytes.to_vec());
    }

    /// Run the stack (timers, TCP state machines, ingress/egress) and return
    /// the outbound IP packets that must be sent over the Terrain link.
    pub fn poll(&mut self) -> Vec<Vec<u8>> {
        self.poll_at(self.now())
    }

    fn now(&self) -> SmolInstant {
        SmolInstant::from_micros(self.clock.elapsed().as_micros() as i64)
    }

    fn poll_at(&mut self, now: SmolInstant) -> Vec<Vec<u8>> {
        let _ = self.iface.poll(now, &mut self.device, &mut self.sockets);
        self.device.tx_queue.drain(..).collect()
    }

    /// Next TCP deadline, or `None` when only an external event can make
    /// progress. Call again after each poll or socket/buffer mutation.
    pub fn poll_delay(&mut self) -> Option<Duration> {
        self.poll_delay_at(self.now())
    }

    fn poll_delay_at(&mut self, now: SmolInstant) -> Option<Duration> {
        if !self.device.rx_queue.is_empty() || !self.device.tx_queue.is_empty() {
            return Some(Duration::ZERO);
        }
        self.iface
            .poll_delay(now, &self.sockets)
            .map(|delay| Duration::from_micros(delay.total_micros()))
    }

    /// Start listening on `port`.
    pub fn add_listener(&mut self, port: u16) -> SocketHandle {
        let mut socket = TcpSocket::new(
            SocketBuffer::new(vec![0u8; SOCKET_BUFFER]),
            SocketBuffer::new(vec![0u8; SOCKET_BUFFER]),
        );
        socket.listen(port).expect("listening on internal port");
        self.sockets.add(socket)
    }

    /// Open a TCP connection to `(remote, remote_port)` from the given local
    /// port. Returns the socket handle once added to the set.
    pub fn connect(
        &mut self,
        remote: Ipv4Address,
        remote_port: u16,
        local_port: u16,
    ) -> SocketHandle {
        let mut socket = TcpSocket::new(
            SocketBuffer::new(vec![0u8; SOCKET_BUFFER]),
            SocketBuffer::new(vec![0u8; SOCKET_BUFFER]),
        );
        socket
            .connect(
                self.iface.context(),
                (IpAddress::from(remote), remote_port),
                local_port,
            )
            .expect("opening internal connection");
        self.sockets.add(socket)
    }

    pub fn socket(&mut self, handle: SocketHandle) -> &mut TcpSocket<'static> {
        self.sockets.get_mut(handle)
    }

    pub fn remove_socket(&mut self, handle: SocketHandle) -> smoltcp::socket::Socket<'static> {
        self.sockets.remove(handle)
    }

    pub fn add_socket(&mut self, socket: TcpSocket<'static>) -> SocketHandle {
        self.sockets.add(socket)
    }

    /// If the listener accepted a connection, move it out of the listener and
    /// return `(established_handle, new_listener_handle)` so more connections
    /// can be accepted.
    pub fn accept(
        &mut self,
        listener: SocketHandle,
        port: u16,
    ) -> Option<(SocketHandle, SocketHandle)> {
        // A smoltcp listener becomes the accepted connection itself (SYN
        // received/established); while it is still in Listen state there is
        // nothing to accept. Calling accept() otherwise would churn the
        // listener every poll: unbounded socket/task growth and multiple
        // listeners on the same port.
        if self.socket(listener).is_listening() {
            return None;
        }
        let established = match self.remove_socket(listener) {
            smoltcp::socket::Socket::Tcp(s) => s,
            _ => unreachable!(),
        };
        let conn = self.add_socket(established);
        let listener = self.add_listener(port);
        Some((conn, listener))
    }

    /// Enqueue bytes into the socket's transmit buffer; returns how many were
    /// accepted (0 when the connection is not established or the buffer is
    /// full — retry later).
    pub fn send_bytes(&mut self, handle: SocketHandle, bytes: &[u8]) -> usize {
        self.socket(handle).send_slice(bytes).unwrap_or(0)
    }

    /// Dequeue bytes from the socket's receive buffer; 0 when empty.
    pub fn recv_bytes(&mut self, handle: SocketHandle, buf: &mut [u8]) -> usize {
        self.socket(handle).recv_slice(buf).unwrap_or(0)
    }

    /// True once the connection is fully closed.
    pub fn socket_closed(&mut self, handle: SocketHandle) -> bool {
        self.socket(handle).state() == TcpState::Closed
    }

    /// True when the peer sent FIN and its data has been fully drained.
    ///
    /// A locally half-closed socket can observe the peer FIN in `TIME-WAIT`
    /// (or the transient `CLOSING`/`LAST-ACK` states), not only in
    /// `CLOSE-WAIT`. Checking the receive half rather than one state keeps
    /// half-close propagation correct for both active and passive closes.
    pub fn peer_eof(&mut self, handle: SocketHandle) -> bool {
        let socket = self.socket(handle);
        peer_eof_state(socket.state(), socket.can_recv())
    }

    /// Close the socket (sends FIN if the connection is established).
    pub fn close_socket(&mut self, handle: SocketHandle) {
        self.socket(handle).close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deliver only queued ingress or work due at `now`; never fixed ticks.
    /// The bound also catches a persistent zero-delay busy loop.
    fn exchange_due(client: &mut IpStack, server: &mut IpStack, now: SmolInstant) {
        for _ in 0..128 {
            let client_due = client.poll_delay_at(now) == Some(Duration::ZERO);
            let server_due = server.poll_delay_at(now) == Some(Duration::ZERO);
            if !client_due && !server_due {
                return;
            }
            if client_due {
                for packet in client.poll_at(now) {
                    server.push_packet(&packet);
                }
            }
            if server.poll_delay_at(now) == Some(Duration::ZERO) {
                for packet in server.poll_at(now) {
                    client.push_packet(&packet);
                }
            }
        }
        panic!("protocol did not quiesce at {now:?}");
    }

    fn connected_pair() -> (IpStack, IpStack, SocketHandle, SocketHandle) {
        let mut client = IpStack::new(CLIENT_ADDR);
        let mut server = IpStack::new(SERVER_ADDR);
        let listener = server.add_listener(INTERNAL_PORT);
        let client_handle = client.connect(SERVER_ADDR, INTERNAL_PORT, 40000);
        exchange_due(&mut client, &mut server, SmolInstant::from_micros(0));
        let (server_handle, _) = server.accept(listener, INTERNAL_PORT).unwrap();
        (client, server, client_handle, server_handle)
    }

    #[test]
    fn idle_and_listening_stacks_have_no_deadline() {
        let mut stack = IpStack::new(SERVER_ADDR);
        assert_eq!(stack.poll_delay(), None);
        for _ in 0..64 {
            stack.add_listener(INTERNAL_PORT);
        }
        assert!(stack.poll().is_empty());
        assert_eq!(stack.poll_delay(), None);

        let (mut client, mut server, _, _) = connected_pair();
        let now = SmolInstant::from_micros(0);
        assert_eq!(client.poll_delay_at(now), None);
        assert_eq!(server.poll_delay_at(now), None);
    }

    #[test]
    fn lost_syn_retransmits_at_deadline_without_ingress() {
        let mut client = IpStack::new(CLIENT_ADDR);
        client.connect(SERVER_ADDR, INTERNAL_PORT, 40000);
        let now = SmolInstant::from_micros(0);
        assert_eq!(client.poll_delay_at(now), Some(Duration::ZERO));
        assert!(!client.poll_at(now).is_empty()); // Drop the SYN.
        let delay = client.poll_delay_at(now).unwrap();
        assert!(delay > Duration::ZERO);
        let deadline = now + smoltcp::time::Duration::from_micros(delay.as_micros() as u64);
        assert!(
            client
                .poll_at(deadline - smoltcp::time::Duration::from_micros(1))
                .is_empty()
        );
        assert!(!client.poll_at(deadline).is_empty());
        assert!(client.poll_delay_at(deadline).unwrap() > Duration::ZERO);
    }

    #[test]
    fn data_delayed_ack_and_fin_advance_using_only_deadlines() {
        let (mut client, mut server, client_handle, server_handle) = connected_pair();
        let mut now = SmolInstant::from_micros(0);
        assert_eq!(client.send_bytes(client_handle, b"hello"), 5);
        assert_eq!(client.poll_delay_at(now), Some(Duration::ZERO));
        exchange_due(&mut client, &mut server, now);
        let mut bytes = [0; 16];
        assert_eq!(server.recv_bytes(server_handle, &mut bytes), 5);
        assert_eq!(&bytes[..5], b"hello");

        // A delayed ACK must fire even if neither application sends again.
        let ack_delay = server.poll_delay_at(now).unwrap();
        now += smoltcp::time::Duration::from_micros(ack_delay.as_micros() as u64);
        exchange_due(&mut client, &mut server, now);
        assert_eq!(client.socket(client_handle).send_queue(), 0);
        assert_eq!(client.poll_delay_at(now), None);
        assert_eq!(server.poll_delay_at(now), None);

        client.close_socket(client_handle);
        assert_eq!(client.poll_delay_at(now), Some(Duration::ZERO));
        exchange_due(&mut client, &mut server, now);
        assert!(server.peer_eof(server_handle));
        server.close_socket(server_handle);
        exchange_due(&mut client, &mut server, now);
        // TIME-WAIT must expire on a timer, without incoming packets.
        let delay = client.poll_delay_at(now).unwrap();
        assert!(delay > Duration::ZERO);
        now += smoltcp::time::Duration::from_micros(delay.as_micros() as u64);
        exchange_due(&mut client, &mut server, now);
        assert!(client.socket_closed(client_handle));
        assert!(server.socket_closed(server_handle));
        assert_eq!(client.poll_delay_at(now), None);
    }

    #[test]
    fn slow_reader_reopens_tcp_window_without_periodic_ticks() {
        let (mut client, mut server, client_handle, server_handle) = connected_pair();
        let mut now = SmolInstant::from_micros(0);
        assert_eq!(
            client.send_bytes(client_handle, &vec![1; SOCKET_BUFFER]),
            SOCKET_BUFFER
        );
        for _ in 0..100 {
            exchange_due(&mut client, &mut server, now);
            if server.socket(server_handle).recv_queue() == SOCKET_BUFFER
                && client.socket(client_handle).send_queue() == 0
            {
                break;
            }
            let delay = [client.poll_delay_at(now), server.poll_delay_at(now)]
                .into_iter()
                .flatten()
                .min()
                .expect("TCP must have pending ACK/retransmit work");
            now += smoltcp::time::Duration::from_micros(delay.as_micros() as u64);
        }
        assert_eq!(server.socket(server_handle).recv_queue(), SOCKET_BUFFER);
        assert_eq!(client.socket(client_handle).send_queue(), 0);

        // No receiver capacity: bytes queue behind a zero TCP window.
        assert_eq!(client.send_bytes(client_handle, &[2; 4096]), 4096);
        exchange_due(&mut client, &mut server, now);
        assert_eq!(server.socket(server_handle).recv_queue(), SOCKET_BUFFER);

        // A consumer-capacity event lets the relay read, which must advertise
        // the reopened window immediately rather than waiting for a tick.
        let mut bytes = [0; 8192];
        assert_eq!(server.recv_bytes(server_handle, &mut bytes), bytes.len());
        assert_eq!(server.poll_delay_at(now), Some(Duration::ZERO));
        exchange_due(&mut client, &mut server, now);
        assert!(server.socket(server_handle).recv_queue() > SOCKET_BUFFER - 8192);
        for _ in 0..100 {
            if server.socket(server_handle).recv_queue() == SOCKET_BUFFER - 8192 + 4096 {
                break;
            }
            let delay = [client.poll_delay_at(now), server.poll_delay_at(now)]
                .into_iter()
                .flatten()
                .min()
                .expect("remaining data must have a TCP deadline");
            now += smoltcp::time::Duration::from_micros(delay.as_micros() as u64);
            exchange_due(&mut client, &mut server, now);
        }
        assert_eq!(
            server.socket(server_handle).recv_queue(),
            SOCKET_BUFFER - 8192 + 4096
        );
    }

    #[test]
    fn concurrent_syns_can_be_distributed_across_listeners() {
        const CONNECTIONS: usize = 32;
        const LISTENERS: usize = 64;

        let mut client = IpStack::new(CLIENT_ADDR);
        let mut server = IpStack::new(SERVER_ADDR);
        let mut listeners: Vec<_> = (0..LISTENERS)
            .map(|_| server.add_listener(INTERNAL_PORT))
            .collect();
        let mut accepted = Vec::new();
        for index in 0..CONNECTIONS {
            client.connect(SERVER_ADDR, INTERNAL_PORT, 40000 + index as u16);
        }

        for _ in 0..100 {
            for packet in client.poll() {
                server.push_packet(&packet);
            }
            for listener in &mut listeners {
                if let Some((handle, replacement)) = server.accept(*listener, INTERNAL_PORT) {
                    *listener = replacement;
                    accepted.push(handle);
                }
            }
            for packet in server.poll() {
                client.push_packet(&packet);
            }
            if accepted.len() == CONNECTIONS {
                break;
            }
        }

        assert_eq!(accepted.len(), CONNECTIONS);
    }

    #[test]
    fn peer_eof_includes_active_close_states() {
        for state in [
            TcpState::CloseWait,
            TcpState::Closing,
            TcpState::LastAck,
            TcpState::TimeWait,
        ] {
            assert!(peer_eof_state(state, false), "{state:?} must report EOF");
            assert!(!peer_eof_state(state, true), "buffered data must delay EOF");
        }
        for state in [
            TcpState::Established,
            TcpState::FinWait1,
            TcpState::FinWait2,
            TcpState::Closed,
        ] {
            assert!(!peer_eof_state(state, false), "{state:?} is not peer EOF");
        }
    }

    #[test]
    fn sustained_transfer_preserves_large_payload() {
        const SIZE: usize = 20 * 1024 * 1024;
        const CHUNK: usize = 8192;
        let payload = vec![0x5a; SIZE];
        let mut client = IpStack::new(CLIENT_ADDR);
        let mut server = IpStack::new(SERVER_ADDR);
        let mut listener = server.add_listener(INTERNAL_PORT);
        let client_handle = client.connect(SERVER_ADDR, INTERNAL_PORT, 40000);
        let mut server_handle = None;
        let mut sent = 0;
        let mut echoed = VecDeque::new();
        let mut received = Vec::with_capacity(SIZE);
        let mut server_closed = false;
        let mut now = SmolInstant::from_micros(0);

        for _ in 0..200_000 {
            if sent < SIZE {
                let end = (sent + CHUNK).min(SIZE);
                let n = client.send_bytes(client_handle, &payload[sent..end]);
                sent += n;
                if sent == SIZE {
                    client.close_socket(client_handle);
                }
            }

            for packet in client.poll_at(now) {
                server.push_packet(&packet);
            }
            if server_handle.is_none()
                && let Some((accepted, replacement)) = server.accept(listener, INTERNAL_PORT)
            {
                listener = replacement;
                server_handle = Some(accepted);
            }
            if let Some(handle) = server_handle {
                let mut buf = [0u8; CHUNK];
                loop {
                    let n = server.recv_bytes(handle, &mut buf);
                    if n == 0 {
                        break;
                    }
                    echoed.push_back(buf[..n].to_vec());
                }
                while let Some(front) = echoed.front_mut() {
                    let n = server.send_bytes(handle, front);
                    if n == 0 {
                        break;
                    }
                    front.drain(..n);
                    if front.is_empty() {
                        echoed.pop_front();
                    }
                }
                if !server_closed && echoed.is_empty() && server.peer_eof(handle) {
                    server.close_socket(handle);
                    server_closed = true;
                }
            }
            for packet in server.poll_at(now) {
                client.push_packet(&packet);
            }
            let mut buf = [0u8; CHUNK];
            loop {
                let n = client.recv_bytes(client_handle, &mut buf);
                if n == 0 {
                    break;
                }
                received.extend_from_slice(&buf[..n]);
            }
            if received.len() == SIZE && client.peer_eof(client_handle) {
                break;
            }
            // Application work is an immediate event. Otherwise advance only
            // to the earliest protocol deadline, never by a fixed tick.
            if (sent < SIZE && client.socket(client_handle).can_send())
                || (server_handle
                    .is_some_and(|h| !echoed.is_empty() && server.socket(h).can_send()))
            {
                continue;
            }
            let delay = [client.poll_delay_at(now), server.poll_delay_at(now)]
                .into_iter()
                .flatten()
                .min()
                .expect("transfer stalled without a deadline");
            now += smoltcp::time::Duration::from_micros(delay.as_micros() as u64);
        }

        assert_eq!(sent, SIZE);
        assert!(server_closed);
        assert_eq!(received.len(), SIZE);
        assert_eq!(received, payload);
    }
}
