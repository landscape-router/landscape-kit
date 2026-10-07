pub mod crypto;
pub mod frame;
pub mod session;

/// Magic value "TERR" = Landscape Terrain Protocol.
/// First 4 bytes of every frame payload; unknown frames are dropped at parse time.
pub const MAGIC: u32 = 0x54455252;
/// v5: the psk is stretched into a master key with scrypt at startup, and
/// every derivation (pre-discovery, handshake, session keys and auth
/// proofs) feeds on the master key; DISCOVER carries a 12-byte nonce so the
/// fixed pre-discovery key has a 2^48 collision bound; AUTH_NACK is sealed
/// with the handshake keys when possible (v4 used a single sha256 over the
/// psk, an 8-byte DISCOVER nonce and plaintext NACKs).
pub const VERSION: u8 = 0x05;

pub const TYPE_DISCOVER: u8 = 0x01;
pub const TYPE_RESP: u8 = 0x02;
pub const TYPE_AUTH_REQ: u8 = 0x03;
pub const TYPE_AUTH_ACK: u8 = 0x04;
pub const TYPE_AUTH_NACK: u8 = 0x05;
pub const TYPE_KEEPALIVE: u8 = 0x06;
pub const TYPE_DATA: u8 = 0x07;
pub const TYPE_TEARDOWN: u8 = 0x08;
/// Plaintext, unsealed error frame a server sends in reply to a frame whose
/// magic matches but whose version byte it does not speak; the header's
/// version field carries the sender's own protocol version. No key
/// compatibility can be assumed across versions, so it cannot be sealed.
/// Unlike every other type it decodes under any version byte — being
/// readable by a differently-versioned peer is its whole purpose. Clients
/// treat it as terminal (retrying cannot fix a version gap); older clients
/// that predate the type still drop it and time out as before.
pub const TYPE_VERSION_MISMATCH: u8 = 0x09;
