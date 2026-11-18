# landscape-terrain-proto

Shared Rust library for the **Terrain** protocol: the L2 emergency-management
channel of the Landscape router. When regular networking is unavailable, a
host speaks Terrain over raw ethernet frames (experimental ethertype, default
`0x88B6`) to the router and gets an encrypted TCP-over-IP tunnel for
management connections.

Two binaries in this workspace are built on it, and it exists so both sides
of the wire share one implementation:

- **`lflare`** (`landscape-flare` crate) — the client. Linux uses AF_PACKET;
  Windows/macOS use libpcap.
- **`lkit flare`** (`lkit-cli` crate) — the server, also run by the `lkit`
  daemon.

## What lives here

| Module | Contents |
| --- | --- |
| `protocol::frame` | 16-byte header wire format (`TERR` magic, version, type, session, len, seq), typed payload codecs |
| `protocol::v6` | Discovery, Noise NX device handshake and authenticated datagram transport |
| `protocol::crypto` | Legacy v5 scrypt master-key stretch, per-phase/per-direction key schedule, ChaCha20-Poly1305 sealing, replay window |
| `protocol::session` | Legacy v5 client/server handshake as pure state machines (frames in, frames out; no I/O) |
| `ipstack` | Userspace TCP/IP stack (smoltcp) bridged over Terrain DATA frames, with monotonic TCP deadlines |
| `transport` | Platform `Link` (AF_PACKET on Linux, libpcap elsewhere), ethernet/VLAN parsing |
| `cli` | Shared argument parsers used by both binaries |
| `concurrency` | Named Tokio tasks, OS threads and runtime workers shared by both binaries |
| `relay` | Deadline-driven waits and coalesced relay-channel capacity/closure notifications |

## Authentication and transport

Terrain v6 discovers routers on the local Ethernet segment. Private discovery wraps requests and responses with a PSK. After selection, a fresh Noise NX handshake proves possession of the chosen device key and establishes the session keys. Clients verify the fingerprint on each connection and submit login credentials over the encrypted channel. A successful login authorizes the configured loopback port mappings. Each mapped application handles its own authentication.

The v5 codec and session state machines support clients connecting to older daemons with the original recovery code. The library has its own crate version. The current wire format is identified by `protocol::v6::VERSION`.

See the [protocol specification](../docs/flare/protocol.md) and [Docker and containerlab tests](../docs/flare/testing.md).

## License

AGPL-3.0 — see [LICENSE](../LICENSE).
