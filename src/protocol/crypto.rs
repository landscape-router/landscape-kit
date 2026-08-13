//! Session cryptography for LNDP v2.
//!
//! The psk is never sent on the wire; it only feeds the challenge-response
//! proofs and the key derivation of the session phase. All post-handshake
//! frames (DATA, KEEPALIVE, TEARDOWN) are sealed with ChaCha20-Poly1305:
//!
//! - the AEAD nonce is the 8-byte session salt followed by the frame's
//!   4-byte sequence number (unique per direction and per session);
//! - the 16-byte cleartext frame header is bound as associated data, so
//!   tampering with the header is detected as well;
//! - each direction has its own key, so salt+seq reuse across directions
//!   is harmless.
//!
//! The auth proofs use distinct domain-separation labels from the keys, so
//! the (cleartext) handshake proofs never leak session key material.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use sha2::{Digest, Sha256};

use crate::protocol::frame;

/// AEAD tag length appended to every sealed payload.
pub const TAG_LEN: usize = 16;
const SALT_LEN: usize = 8;
const NONCE_LEN: usize = 12;

/// Direction of a session frame; each direction uses its own key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    /// Client -> server
    C2S,
    /// Server -> client
    S2C,
}

impl Dir {
    fn key_label(self) -> &'static [u8] {
        match self {
            Dir::C2S => b"lndp-key-c2s",
            Dir::S2C => b"lndp-key-s2c",
        }
    }

    fn rx(self) -> Self {
        match self {
            Dir::C2S => Dir::S2C,
            Dir::S2C => Dir::C2S,
        }
    }
}

pub const AUTH_LABEL_C2S: &[u8] = b"lndp-auth-c2s";
pub const AUTH_LABEL_S2C: &[u8] = b"lndp-auth-s2c";

/// sha256(label || psk || server_nonce || client_nonce)
fn h(label: &[u8], psk: &[u8], server_nonce: u64, client_nonce: u64) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(label);
    hasher.update(psk);
    hasher.update(server_nonce.to_be_bytes());
    hasher.update(client_nonce.to_be_bytes());
    hasher.finalize().into()
}

/// Challenge-response proof for the handshake.
pub fn auth_proof(label: &[u8], psk: &[u8], server_nonce: u64, client_nonce: u64) -> [u8; 32] {
    h(label, psk, server_nonce, client_nonce)
}

/// Constant-time equality for proof comparisons (no early exit on the
/// first differing byte).
pub fn ct_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Fixed counters inside the handshake nonce domain. They are distinct
/// from the session-frame sequence numbers (which start at 0 under the
/// session keys), so nonces can never collide across the two phases.
pub const HS_AUTH_REQ: u32 = 0;
pub const HS_AUTH_ACK: u32 = 1;
pub const HS_AUTH_NACK: u32 = 2;

/// Key material for sealing the AUTH_REQ / AUTH_ACK / AUTH_NACK handshake
/// frames. Derived from the psk and the server nonce — both sides know
/// these after RESP — so the user name and proofs are never visible on the
/// wire, and the session keys (which depend on the client nonce too) stay
/// independent of this key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandshakeKeys {
    key: [u8; 32],
    salt: [u8; SALT_LEN],
}

impl HandshakeKeys {
    pub fn derive(psk: &[u8], server_nonce: u64) -> Self {
        let salt = h(b"lndp-hsalt", psk, server_nonce, 0);
        Self {
            key: h(b"lndp-hkey", psk, server_nonce, 0),
            salt: salt[..SALT_LEN].try_into().unwrap(),
        }
    }

    fn nonce(&self, counter: u32) -> Nonce {
        let mut raw = [0u8; NONCE_LEN];
        raw[..SALT_LEN].copy_from_slice(&self.salt);
        raw[SALT_LEN..].copy_from_slice(&counter.to_be_bytes());
        raw.into()
    }

    fn seal(&self, counter: u32, header: &[u8], plaintext: &[u8]) -> Vec<u8> {
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&self.key));
        cipher
            .encrypt(
                &self.nonce(counter),
                Payload {
                    msg: plaintext,
                    aad: header,
                },
            )
            .expect("aead seal cannot fail")
    }

    fn open(&self, counter: u32, header: &[u8], sealed: &[u8]) -> Option<Vec<u8>> {
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&self.key));
        cipher
            .decrypt(
                &self.nonce(counter),
                Payload {
                    msg: sealed,
                    aad: header,
                },
            )
            .ok()
    }

    /// Build a complete sealed handshake frame (header + ciphertext + tag).
    pub fn seal_frame(
        &self,
        msg_type: u8,
        session_id: u32,
        counter: u32,
        plaintext: &[u8],
    ) -> Vec<u8> {
        let header = frame::encode_header(
            msg_type,
            session_id,
            0,
            (plaintext.len() + TAG_LEN) as u16,
        );
        let body = self.seal(counter, &header, plaintext);
        let mut raw = Vec::with_capacity(header.len() + body.len());
        raw.extend_from_slice(&header);
        raw.extend_from_slice(&body);
        raw
    }

    /// Verify and decrypt a sealed handshake frame (already decoded).
    pub fn open_frame(&self, counter: u32, l: &frame::Frame<'_>) -> Option<Vec<u8>> {
        let header = frame::encode_header(l.msg_type, l.session_id, l.seq, l.len);
        self.open(counter, &header, l.payload)
    }
}

/// Per-session keys derived from the psk and both handshake nonces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionKeys {
    c2s: [u8; 32],
    s2c: [u8; 32],
    salt: [u8; SALT_LEN],
}

impl SessionKeys {
    pub fn derive(psk: &[u8], server_nonce: u64, client_nonce: u64) -> Self {
        let salt = h(b"lndp-salt", psk, server_nonce, client_nonce);
        Self {
            c2s: h(Dir::C2S.key_label(), psk, server_nonce, client_nonce),
            s2c: h(Dir::S2C.key_label(), psk, server_nonce, client_nonce),
            salt: salt[..SALT_LEN].try_into().unwrap(),
        }
    }

    fn key(&self, dir: Dir) -> &[u8; 32] {
        match dir {
            Dir::C2S => &self.c2s,
            Dir::S2C => &self.s2c,
        }
    }

    fn nonce(&self, seq: u32) -> Nonce {
        let mut raw = [0u8; NONCE_LEN];
        raw[..SALT_LEN].copy_from_slice(&self.salt);
        raw[SALT_LEN..].copy_from_slice(&seq.to_be_bytes());
        raw.into()
    }

    /// Seal a payload with AAD = the 16-byte frame header; returns
    /// ciphertext || tag.
    pub fn seal(&self, dir: Dir, seq: u32, header: &[u8], plaintext: &[u8]) -> Vec<u8> {
        let cipher = ChaCha20Poly1305::new(Key::from_slice(self.key(dir)));
        cipher
            .encrypt(
                &self.nonce(seq),
                Payload {
                    msg: plaintext,
                    aad: header,
                },
            )
            .expect("aead seal cannot fail")
    }

    /// Open a sealed payload, authenticating `header` and the tag.
    pub fn open(&self, dir: Dir, seq: u32, header: &[u8], sealed: &[u8]) -> Option<Vec<u8>> {
        let cipher = ChaCha20Poly1305::new(Key::from_slice(self.key(dir)));
        cipher
            .decrypt(
                &self.nonce(seq),
                Payload {
                    msg: sealed,
                    aad: header,
                },
            )
            .ok()
    }

    #[cfg(test)]
    fn keys(&self) -> ([u8; 32], [u8; 32]) {
        (self.c2s, self.s2c)
    }
}

/// Anti-replay window for one direction of a session. L2 links preserve
/// per-sender ordering, so a strictly increasing sequence number is
/// accepted; anything older (including the u32 wrap-around case) is a
/// replay or a desynced stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplayWindow {
    last: Option<u32>,
}

impl ReplayWindow {
    pub fn new() -> Self {
        Self { last: None }
    }

    /// Record and accept `seq` if it is newer than anything seen before.
    pub fn accept(&mut self, seq: u32) -> bool {
        if let Some(last) = self.last {
            let diff = seq.wrapping_sub(last) as i32;
            if diff <= 0 {
                return false;
            }
        }
        self.last = Some(seq);
        true
    }
}

impl Default for ReplayWindow {
    fn default() -> Self {
        Self::new()
    }
}

/// Incremental encoder/decoder for the encrypted session phase: owns the
/// transmit sequence counter and the receive anti-replay window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCrypto {
    keys: SessionKeys,
    tx_dir: Dir,
    tx_seq: u32,
    rx: ReplayWindow,
}

impl SessionCrypto {
    pub fn new(keys: SessionKeys, tx_dir: Dir) -> Self {
        Self {
            keys,
            tx_dir,
            tx_seq: 0,
            rx: ReplayWindow::new(),
        }
    }

    /// Build a complete sealed frame: 16-byte header + ciphertext + tag.
    pub fn seal(&mut self, msg_type: u8, session_id: u32, payload: &[u8]) -> Vec<u8> {
        let seq = self.tx_seq;
        self.tx_seq = self.tx_seq.wrapping_add(1);
        let header =
            frame::encode_header(msg_type, session_id, seq, (payload.len() + TAG_LEN) as u16);
        let body = self.keys.seal(self.tx_dir, seq, &header, payload);
        let mut raw = Vec::with_capacity(header.len() + body.len());
        raw.extend_from_slice(&header);
        raw.extend_from_slice(&body);
        raw
    }

    /// Verify and decrypt one received session frame. The frame must already
    /// be decoded; the header is rebuilt from its fields and used as AAD.
    /// Returns the plaintext, or None on authentication failure or replay.
    pub fn open(
        &mut self,
        msg_type: u8,
        session_id: u32,
        seq: u32,
        len: u16,
        sealed: &[u8],
    ) -> Option<Vec<u8>> {
        let header = frame::encode_header(msg_type, session_id, seq, len);
        let plain = self.keys.open(self.tx_dir.rx(), seq, &header, sealed)?;
        self.rx.accept(seq).then_some(plain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::frame;

    const PSK: &[u8] = b"landscape-secret";

    #[test]
    fn derive_is_stable_and_sensitive() {
        let k1 = SessionKeys::derive(PSK, 1, 2);
        let k2 = SessionKeys::derive(PSK, 1, 2);
        assert_eq!(k1, k2);
        assert_ne!(k1, SessionKeys::derive(PSK, 1, 3));
        assert_ne!(k1, SessionKeys::derive(PSK, 0, 2));
        assert_ne!(k1, SessionKeys::derive(b"other", 1, 2));
        let (c2s, s2c) = k1.keys();
        assert_ne!(c2s, s2c);
    }

    #[test]
    fn proofs_do_not_leak_keys() {
        let s_nonce = 0x0102_0304_0506_0708u64;
        let c_nonce = 0x1112_1314_1516_1718u64;
        let keys = SessionKeys::derive(PSK, s_nonce, c_nonce);
        let (c2s, s2c) = keys.keys();
        let p_c2s = auth_proof(AUTH_LABEL_C2S, PSK, s_nonce, c_nonce);
        let p_s2c = auth_proof(AUTH_LABEL_S2C, PSK, s_nonce, c_nonce);
        assert_ne!(p_c2s, c2s);
        assert_ne!(p_s2c, s2c);
        assert_ne!(p_c2s, p_s2c);
    }

    #[test]
    fn seal_open_roundtrip() {
        let keys = SessionKeys::derive(PSK, 7, 9);
        let mut c = SessionCrypto::new(keys.clone(), Dir::C2S);
        let raw = c.seal(frame::TYPE_DATA, 42, b"hello");
        let l = frame::decode(&raw).unwrap();
        assert_eq!(l.msg_type, frame::TYPE_DATA);
        assert_eq!(l.seq, 0);
        let mut s = SessionCrypto::new(keys, Dir::S2C);
        assert_eq!(
            s.open(l.msg_type, l.session_id, l.seq, l.len, l.payload),
            Some(b"hello".to_vec())
        );
    }

    #[test]
    fn tampered_ciphertext_rejected() {
        let keys = SessionKeys::derive(PSK, 1, 1);
        let mut c = SessionCrypto::new(keys.clone(), Dir::C2S);
        let mut raw = c.seal(frame::TYPE_DATA, 42, b"secret");
        let n = raw.len();
        raw[n - 1] ^= 0xff;
        let l = frame::decode(&raw).unwrap();
        let mut s = SessionCrypto::new(keys, Dir::S2C);
        assert_eq!(s.open(l.msg_type, l.session_id, l.seq, l.len, l.payload), None);
    }

    #[test]
    fn tampered_header_rejected() {
        let keys = SessionKeys::derive(PSK, 1, 1);
        let mut c = SessionCrypto::new(keys.clone(), Dir::C2S);
        let mut raw = c.seal(frame::TYPE_DATA, 42, b"secret");
        raw[6] ^= 0x01; // session id lives in the cleartext header
        let l = frame::decode(&raw).unwrap();
        let mut s = SessionCrypto::new(keys, Dir::S2C);
        assert_eq!(s.open(l.msg_type, l.session_id, l.seq, l.len, l.payload), None);
    }

    #[test]
    fn replay_rejected() {
        let keys = SessionKeys::derive(PSK, 1, 1);
        let mut c = SessionCrypto::new(keys.clone(), Dir::C2S);
        let mut s = SessionCrypto::new(keys, Dir::S2C);
        let mut frames = Vec::new();
        for i in 0..3 {
            frames.push(c.seal(frame::TYPE_DATA, 42, &[i]));
        }
        for f in &frames {
            let l = frame::decode(f).unwrap();
            assert!(s.open(l.msg_type, l.session_id, l.seq, l.len, l.payload).is_some());
        }
        let l = frame::decode(&frames[0]).unwrap();
        assert_eq!(s.open(l.msg_type, l.session_id, l.seq, l.len, l.payload), None);
    }

    #[test]
    fn wrong_direction_rejected() {
        let keys = SessionKeys::derive(PSK, 1, 1);
        let mut c = SessionCrypto::new(keys.clone(), Dir::C2S);
        let raw = c.seal(frame::TYPE_DATA, 42, b"x");
        let l = frame::decode(&raw).unwrap();
        // Receiver using the same tx direction would open with the wrong key.
        let mut s = SessionCrypto::new(keys, Dir::C2S);
        assert_eq!(s.open(l.msg_type, l.session_id, l.seq, l.len, l.payload), None);
    }

    #[test]
    fn handshake_keys_domain() {
        // Different server nonces yield different handshake keys.
        assert_ne!(HandshakeKeys::derive(PSK, 1), HandshakeKeys::derive(PSK, 2));
        // Handshake keys must not equal the session keys.
        let hk = HandshakeKeys::derive(PSK, 7);
        let sk = SessionKeys::derive(PSK, 7, 9);
        let (c2s, s2c) = sk.keys();
        assert_ne!(hk.key, c2s);
        assert_ne!(hk.key, s2c);

        // Sealed frames open with the right counter only.
        let hk = HandshakeKeys::derive(PSK, 3);
        let raw = hk.seal_frame(frame::TYPE_AUTH_REQ, 0, HS_AUTH_REQ, b"hello");
        let l = frame::decode(&raw).unwrap();
        assert_eq!(
            hk.open_frame(HS_AUTH_REQ, &l),
            Some(b"hello".to_vec())
        );
        assert_eq!(hk.open_frame(HS_AUTH_ACK, &l), None);
    }

    #[test]
    fn replay_window_wraps() {
        let mut w = ReplayWindow::new();
        assert!(w.accept(1));
        assert!(w.accept(2));
        assert!(!w.accept(2));
        assert!(!w.accept(0));
        assert!(!w.accept(u32::MAX)); // 4 billion frames ahead: not a wrap
        let mut w = ReplayWindow::new();
        assert!(w.accept(u32::MAX - 1));
        assert!(w.accept(u32::MAX));
        assert!(w.accept(0)); // wrapped around to a newer value
        assert!(!w.accept(u32::MAX));
        assert!(!w.accept(0));
    }
}
