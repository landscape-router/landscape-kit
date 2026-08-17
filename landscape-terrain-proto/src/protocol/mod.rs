pub mod crypto;
pub mod frame;
pub mod session;

/// Magic value "LNDP" = Landscape Net Data Protocol.
/// First 4 bytes of every frame payload; unknown frames are dropped at parse time.
pub const MAGIC: u32 = 0x4C4E4450;
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
