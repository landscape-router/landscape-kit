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
        let mut iface = Interface::new(config, &mut device, SmolInstant::now());
        iface.update_ip_addrs(|addrs| {
            let _ = addrs.push(IpCidr::Ipv4(Ipv4Cidr::new(local, 31)));
        });
        Self {
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
        let _ = self
            .iface
            .poll(SmolInstant::now(), &mut self.device, &mut self.sockets);
        self.device.tx_queue.drain(..).collect()
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

        for _ in 0..200_000 {
            if sent < SIZE {
                let end = (sent + CHUNK).min(SIZE);
                let n = client.send_bytes(client_handle, &payload[sent..end]);
                sent += n;
                if sent == SIZE {
                    client.close_socket(client_handle);
                }
            }

            for packet in client.poll() {
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
            for packet in server.poll() {
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
        }

        assert_eq!(sent, SIZE);
        assert!(server_closed);
        assert_eq!(received.len(), SIZE);
        assert_eq!(received, payload);
    }
}
