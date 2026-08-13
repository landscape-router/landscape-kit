pub mod crypto;
pub mod frame;
pub mod session;

/// Magic value "LNDP" = Landscape Net Data Protocol.
/// First 4 bytes of every frame payload; unknown frames are dropped at parse time.
pub const MAGIC: u32 = 0x4C4E4450;
/// v3: AUTH_REQ/AUTH_ACK/AUTH_NACK are sealed with handshake keys derived
/// from the psk and the server nonce (v2 left them in cleartext).
pub const VERSION: u8 = 0x03;

pub const TYPE_DISCOVER: u8 = 0x01;
pub const TYPE_RESP: u8 = 0x02;
pub const TYPE_AUTH_REQ: u8 = 0x03;
pub const TYPE_AUTH_ACK: u8 = 0x04;
pub const TYPE_AUTH_NACK: u8 = 0x05;
pub const TYPE_KEEPALIVE: u8 = 0x06;
pub const TYPE_DATA: u8 = 0x07;
pub const TYPE_TEARDOWN: u8 = 0x08;
