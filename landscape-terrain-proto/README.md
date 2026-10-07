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
| `protocol::crypto` | scrypt master-key stretch, per-phase/per-direction key schedule, ChaCha20-Poly1305 sealing, replay window |
| `protocol::session` | Client/server handshake as pure state machines (frames in, frames out; no I/O) |
| `ipstack` | Userspace TCP/IP stack (smoltcp) bridged over Terrain DATA frames, plus the relay tuning constants |
| `transport` | Platform `Link` (AF_PACKET on Linux, libpcap elsewhere), ethernet/VLAN parsing |
| `cli` | Shared argument parsers used by both binaries |

## Security model in one paragraph

The psk is stretched with scrypt into a master key; every key and auth proof
is domain-separated from it. DISCOVER is sealed with a pre-discovery key, so
the server stays silent for anyone without the psk (an optional discovery
token adds anti-scanning). Each direction of the handshake and the session
uses its own key; the cleartext header is AEAD-associated data; sequence
numbers feed both the nonce and a strict replay window. There is no forward
secrecy — acceptable for a shared-secret LAN protocol. The server bounds
brute-force with per-MAC lockout plus global rate budgets.

The full specification (frame tables, key schedule, handshake flow,
version-mismatch behavior, server defenses) lives in
[`docs/flare/protocol.md`](../docs/flare/protocol.md); the Docker L2
end-to-end test topology is described in
[`docs/flare/testing.md`](../docs/flare/testing.md).

This crate carries its own version line, independent of the workspace
version, because the protocol version (`VERSION` const) evolves with it.

## License

AGPL-3.0 — see [LICENSE](../LICENSE).
