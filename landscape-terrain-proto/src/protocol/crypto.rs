//! Session cryptography for Terrain v5.
//!
//! The psk is never used directly: at startup both sides stretch it into a
//! 32-byte master key with scrypt (a memory-hard KDF), so an offline
//! attacker pays ~32 MiB and ~100 ms per psk guess instead of one sha256.
//! All challenge-response proofs and key derivations feed on the master
//! key, never on the raw psk.
//!
//! Key schedule:
//!
//! - the pre-discovery key (seals DISCOVER) is derived from the master key
//!   alone; each DISCOVER frame carries its own random 12-byte nonce, so
//!   the fixed-key nonce-collision bound is 2^48 frames — unreachable;
//! - the handshake keys (RESP / AUTH_REQ / AUTH_ACK / AUTH_NACK) are
//!   derived from the master key and the server nonce, one key per
//!   direction (the fixed per-message counters in the nonce must not be
//!   reused across directions under a shared key);
//! - the session keys are derived from the master key and both handshake
//!   nonces; each direction has its own key, so salt+seq reuse across
//!   directions is harmless.
//!
//! All post-handshake frames (DATA, KEEPALIVE, TEARDOWN) are sealed with
//! ChaCha20-Poly1305:
//!
//! - the AEAD nonce is the 8-byte session salt followed by the frame's
//!   4-byte sequence number (unique per direction and per session);
//! - the 16-byte cleartext frame header is bound as associated data, so
//!   tampering with the header is detected as well;
//! - each direction has its own key, so salt+seq reuse across directions
//!   is harmless.
//!
//! The auth proofs use distinct domain-separation labels from the keys, so
//! the handshake proofs never leak session key material. There is no
//! forward secrecy: any master-key holder can re-derive session keys from
//! captured nonces — acceptable for a shared-secret LAN protocol.
//!
//! Threat model note: without forward secrecy and with a deterministic key
//! schedule, a compromised psk lets an attacker decrypt and inject traffic
//! for any session; keep the psk long and random, or use scrypt-stretched
//! passphrases (the startup derivation below).

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use scrypt::{Params, scrypt};
use sha2::{Digest, Sha256};

use crate::protocol::frame;

/// AEAD tag length appended to every sealed payload.
pub const TAG_LEN: usize = 16;
const SALT_LEN: usize = 8;
const NONCE_LEN: usize = 12;
/// Random nonce prefix carried in cleartext by every DISCOVER frame.
const DISCOVER_NONCE_LEN: usize = 12;

/// scrypt cost parameters for the master-key derivation (run once per
/// process at startup): 2^15 blocks, r=8, p=1 ≈ 32 MiB / ~100 ms on
/// desktop hardware. Override the exponent with the `LANDSCAPE_TERRAIN_SCRYPT_LOG_N`
/// environment variable (clamped to 10..=20) for constrained devices or
/// fast test suites — both peers must agree on it, since the derived key
/// depends on it.
pub const SCRYPT_LOG_N: u8 = 15;
pub const SCRYPT_R: u32 = 8;
pub const SCRYPT_P: u32 = 1;

/// Parse the `LANDSCAPE_TERRAIN_SCRYPT_LOG_N` override: missing, unparseable or
/// out-of-range values fall back to the default exponent.
fn parse_scrypt_log_n(raw: Option<&str>) -> u8 {
    raw.and_then(|v| v.parse::<u8>().ok())
        .filter(|n| (10..=20).contains(n))
        .unwrap_or(SCRYPT_LOG_N)
}

/// 32-byte key stretched from the psk with scrypt at startup. Every other
/// derivation in this module feeds on it; the raw psk never reaches the
/// wire or the key schedule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MasterKey([u8; 32]);

impl MasterKey {
    /// scrypt(psk, "terrain-v5-master", log_n, r=8, p=1) → 32 bytes.
    /// Call once per process; the cost is paid by the peer once, and by an
    /// offline attacker once per psk guess.
    pub fn derive(psk: &[u8]) -> Self {
        let log_n = parse_scrypt_log_n(
            std::env::var("LANDSCAPE_TERRAIN_SCRYPT_LOG_N")
                .ok()
                .as_deref(),
        );
        let params = Params::new(log_n, SCRYPT_R, SCRYPT_P, 32).expect("valid scrypt params");
        let mut out = [0u8; 32];
        scrypt(psk, b"terrain-v5-master", &params, &mut out)
            .expect("scrypt derivation cannot fail");
        Self(out)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

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
            Dir::C2S => b"terrain-key-c2s",
            Dir::S2C => b"terrain-key-s2c",
        }
    }

    fn rx(self) -> Self {
        match self {
            Dir::C2S => Dir::S2C,
            Dir::S2C => Dir::C2S,
        }
    }
}

pub const AUTH_LABEL_C2S: &[u8] = b"terrain-auth-c2s";
pub const AUTH_LABEL_S2C: &[u8] = b"terrain-auth-s2c";

/// sha256(label || key || server_nonce || client_nonce)
fn h(label: &[u8], key: &[u8], server_nonce: u64, client_nonce: u64) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(label);
    hasher.update(key);
    hasher.update(server_nonce.to_be_bytes());
    hasher.update(client_nonce.to_be_bytes());
    hasher.finalize().into()
}

/// Challenge-response proof for the handshake.
pub fn auth_proof(label: &[u8], key: &[u8], server_nonce: u64, client_nonce: u64) -> [u8; 32] {
    h(label, key, server_nonce, client_nonce)
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
pub const HS_RESP: u32 = 3;

/// Pre-discovery key, derived from the master key alone (no nonces exist
/// yet). Seals the DISCOVER frame, so only a master-key holder can even be
/// heard: the server stays silent for everyone else, and the client
/// name/token are never visible on the wire. Each frame carries its own
/// random 12-byte nonce in the clear (only the key is secret), keeping the
/// fixed-key collision bound at 2^48 sealed frames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreSharedKey([u8; 32]);

impl PreSharedKey {
    pub fn derive(master: &MasterKey) -> Self {
        Self(h(b"terrain-hkey0", master.as_bytes(), 0, 0))
    }

    /// Build a sealed DISCOVER frame: nonce(12) || ciphertext || tag.
    pub fn seal_discover(&self, session_id: u32, nonce: [u8; 12], plaintext: &[u8]) -> Vec<u8> {
        let header = frame::encode_header(
            frame::TYPE_DISCOVER,
            session_id,
            0,
            (DISCOVER_NONCE_LEN + plaintext.len() + TAG_LEN) as u16,
        );
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&self.0));
        let body = cipher
            .encrypt(
                &nonce.into(),
                Payload {
                    msg: plaintext,
                    aad: &header,
                },
            )
            .expect("aead seal cannot fail");
        let mut raw = Vec::with_capacity(header.len() + DISCOVER_NONCE_LEN + body.len());
        raw.extend_from_slice(&header);
        raw.extend_from_slice(&nonce);
        raw.extend_from_slice(&body);
        raw
    }

    /// Verify and open a sealed DISCOVER frame (already decoded): the
    /// nonce is the 12-byte prefix of the payload, the rest is ciphertext
    /// || tag.
    pub fn open_discover(&self, l: &frame::Frame<'_>) -> Option<Vec<u8>> {
        if l.payload.len() < DISCOVER_NONCE_LEN + TAG_LEN {
            return None;
        }
        let mut nonce = [0u8; NONCE_LEN];
        nonce.copy_from_slice(&l.payload[..DISCOVER_NONCE_LEN]);
        let header = frame::encode_header(l.msg_type, l.session_id, l.seq, l.len);
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&self.0));
        cipher
            .decrypt(
                &nonce.into(),
                Payload {
                    msg: &l.payload[DISCOVER_NONCE_LEN..],
                    aad: &header,
                },
            )
            .ok()
    }
}

/// Key material for sealing the AUTH_REQ / AUTH_ACK / AUTH_NACK handshake
/// frames. Derived from the master key and the server nonce — both sides
/// know these after RESP — so the user name and proofs are never visible on
/// the wire, and the session keys (which depend on the client nonce too)
/// stay independent of this key.
///
/// Like the session phase, each direction has its own key: the AEAD nonce
/// is `salt(8) || counter(4)` with a fixed counter per message type, so a
/// counter that repeats in the other direction would otherwise reuse the
/// nonce under the same key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandshakeKeys {
    c2s: [u8; 32],
    s2c: [u8; 32],
    salt: [u8; SALT_LEN],
}

impl HandshakeKeys {
    pub fn derive(master: &MasterKey, server_nonce: u64) -> Self {
        let salt = h(b"terrain-hsalt", master.as_bytes(), server_nonce, 0);
        Self {
            c2s: h(b"terrain-hkey-c2s", master.as_bytes(), server_nonce, 0),
            s2c: h(b"terrain-hkey-s2c", master.as_bytes(), server_nonce, 0),
            salt: salt[..SALT_LEN].try_into().unwrap(),
        }
    }

    fn key(&self, dir: Dir) -> &[u8; 32] {
        match dir {
            Dir::C2S => &self.c2s,
            Dir::S2C => &self.s2c,
        }
    }

    fn nonce(&self, counter: u32) -> Nonce {
        let mut raw = [0u8; NONCE_LEN];
        raw[..SALT_LEN].copy_from_slice(&self.salt);
        raw[SALT_LEN..].copy_from_slice(&counter.to_be_bytes());
        raw.into()
    }

    fn seal(&self, dir: Dir, counter: u32, header: &[u8], plaintext: &[u8]) -> Vec<u8> {
        let cipher = ChaCha20Poly1305::new(Key::from_slice(self.key(dir)));
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

    fn open(&self, dir: Dir, counter: u32, header: &[u8], sealed: &[u8]) -> Option<Vec<u8>> {
        let cipher = ChaCha20Poly1305::new(Key::from_slice(self.key(dir)));
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
        dir: Dir,
        msg_type: u8,
        session_id: u32,
        counter: u32,
        plaintext: &[u8],
    ) -> Vec<u8> {
        let header =
            frame::encode_header(msg_type, session_id, 0, (plaintext.len() + TAG_LEN) as u16);
        let body = self.seal(dir, counter, &header, plaintext);
        let mut raw = Vec::with_capacity(header.len() + body.len());
        raw.extend_from_slice(&header);
        raw.extend_from_slice(&body);
        raw
    }

    /// Verify and decrypt a sealed handshake frame (already decoded).
    pub fn open_frame(&self, dir: Dir, counter: u32, l: &frame::Frame<'_>) -> Option<Vec<u8>> {
        let header = frame::encode_header(l.msg_type, l.session_id, l.seq, l.len);
        self.open(dir, counter, &header, l.payload)
    }

    /// Build a frame with a plaintext prefix before the ciphertext
    /// (RESP: the server nonce must be readable to derive the handshake
    /// keys; the rest is sealed).
    pub fn seal_prefixed(
        &self,
        dir: Dir,
        msg_type: u8,
        session_id: u32,
        counter: u32,
        prefix: &[u8],
        plaintext: &[u8],
    ) -> Vec<u8> {
        let header = frame::encode_header(
            msg_type,
            session_id,
            0,
            (prefix.len() + plaintext.len() + TAG_LEN) as u16,
        );
        let body = self.seal(dir, counter, &header, plaintext);
        let mut raw = Vec::with_capacity(header.len() + prefix.len() + body.len());
        raw.extend_from_slice(&header);
        raw.extend_from_slice(prefix);
        raw.extend_from_slice(&body);
        raw
    }

    /// Open a frame that carries a plaintext prefix before the ciphertext
    /// (RESP). The prefix bytes are part of the AAD via the header.
    pub fn open_prefixed(
        &self,
        dir: Dir,
        counter: u32,
        l: &frame::Frame<'_>,
        prefix_len: usize,
    ) -> Option<Vec<u8>> {
        if l.payload.len() < prefix_len + TAG_LEN {
            return None;
        }
        let header = frame::encode_header(l.msg_type, l.session_id, l.seq, l.len);
        self.open(dir, counter, &header, &l.payload[prefix_len..])
    }
}

/// Per-session keys derived from the master key and both handshake nonces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionKeys {
    c2s: [u8; 32],
    s2c: [u8; 32],
    salt: [u8; SALT_LEN],
}

impl SessionKeys {
    pub fn derive(master: &MasterKey, server_nonce: u64, client_nonce: u64) -> Self {
        let salt = h(
            b"terrain-salt",
            master.as_bytes(),
            server_nonce,
            client_nonce,
        );
        Self {
            c2s: h(
                Dir::C2S.key_label(),
                master.as_bytes(),
                server_nonce,
                client_nonce,
            ),
            s2c: h(
                Dir::S2C.key_label(),
                master.as_bytes(),
                server_nonce,
                client_nonce,
            ),
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

    /// Scrypt-stretched test master, computed once (N=2^15 takes ~100ms).
    fn master() -> &'static MasterKey {
        static M: std::sync::OnceLock<MasterKey> = std::sync::OnceLock::new();
        M.get_or_init(|| MasterKey::derive(b"landscape-secret"))
    }

    #[test]
    fn scrypt_cost_override_is_parsed_and_clamped() {
        assert_eq!(parse_scrypt_log_n(None), SCRYPT_LOG_N);
        assert_eq!(parse_scrypt_log_n(Some("10")), 10);
        assert_eq!(parse_scrypt_log_n(Some("20")), 20);
        assert_eq!(parse_scrypt_log_n(Some("9")), SCRYPT_LOG_N);
        assert_eq!(parse_scrypt_log_n(Some("21")), SCRYPT_LOG_N);
        assert_eq!(parse_scrypt_log_n(Some("abc")), SCRYPT_LOG_N);
        assert_eq!(parse_scrypt_log_n(Some("")), SCRYPT_LOG_N);
    }

    #[test]
    fn master_derivation_is_scrypt() {
        // Stable, sensitive, and 32 bytes.
        assert_eq!(
            MasterKey::derive(b"landscape-secret"),
            MasterKey::derive(b"landscape-secret")
        );
        assert_ne!(
            MasterKey::derive(b"landscape-secret"),
            MasterKey::derive(b"other-secret")
        );
        assert_eq!(master().as_bytes().len(), 32);
        // The master must not equal the raw single-pass sha256 of the psk.
        let raw = Sha256::digest(b"landscape-secret");
        assert_ne!(master().as_bytes(), &raw[..]);
    }

    #[test]
    fn pre_discovery_seal_open_roundtrip() {
        let key = PreSharedKey::derive(master());
        let nonce = [7u8; 12];
        let raw = key.seal_discover(0, nonce, b"payload");
        let l = frame::decode(&raw).unwrap();
        assert_eq!(l.msg_type, frame::TYPE_DISCOVER);
        assert_eq!(key.open_discover(&l), Some(b"payload".to_vec()));
    }

    #[test]
    fn pre_discovery_tampered_nonce_rejected() {
        // The nonce prefix is in the clear: flipping it must fail the tag.
        let key = PreSharedKey::derive(master());
        let mut raw = key.seal_discover(0, [0u8; 12], b"payload");
        raw[frame::HEADER_LEN] ^= 0x01;
        let l = frame::decode(&raw).unwrap();
        assert_eq!(key.open_discover(&l), None);
    }

    #[test]
    fn derive_is_stable_and_sensitive() {
        let k1 = SessionKeys::derive(master(), 1, 2);
        let k2 = SessionKeys::derive(master(), 1, 2);
        assert_eq!(k1, k2);
        assert_ne!(k1, SessionKeys::derive(master(), 1, 3));
        assert_ne!(k1, SessionKeys::derive(master(), 0, 2));
        let other = MasterKey::derive(b"other-secret");
        assert_ne!(k1, SessionKeys::derive(&other, 1, 2));
        let (c2s, s2c) = k1.keys();
        assert_ne!(c2s, s2c);
    }

    #[test]
    fn proofs_do_not_leak_keys() {
        let s_nonce = 0x0102_0304_0506_0708u64;
        let c_nonce = 0x1112_1314_1516_1718u64;
        let keys = SessionKeys::derive(master(), s_nonce, c_nonce);
        let (c2s, s2c) = keys.keys();
        let p_c2s = auth_proof(AUTH_LABEL_C2S, master().as_bytes(), s_nonce, c_nonce);
        let p_s2c = auth_proof(AUTH_LABEL_S2C, master().as_bytes(), s_nonce, c_nonce);
        assert_ne!(p_c2s, c2s);
        assert_ne!(p_s2c, s2c);
        assert_ne!(p_c2s, p_s2c);
    }

    #[test]
    fn seal_open_roundtrip() {
        let keys = SessionKeys::derive(master(), 7, 9);
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
        let keys = SessionKeys::derive(master(), 1, 1);
        let mut c = SessionCrypto::new(keys.clone(), Dir::C2S);
        let mut raw = c.seal(frame::TYPE_DATA, 42, b"secret");
        let n = raw.len();
        raw[n - 1] ^= 0xff;
        let l = frame::decode(&raw).unwrap();
        let mut s = SessionCrypto::new(keys, Dir::S2C);
        assert_eq!(
            s.open(l.msg_type, l.session_id, l.seq, l.len, l.payload),
            None
        );
    }

    #[test]
    fn tampered_header_rejected() {
        let keys = SessionKeys::derive(master(), 1, 1);
        let mut c = SessionCrypto::new(keys.clone(), Dir::C2S);
        let mut raw = c.seal(frame::TYPE_DATA, 42, b"secret");
        raw[6] ^= 0x01; // session id lives in the cleartext header
        let l = frame::decode(&raw).unwrap();
        let mut s = SessionCrypto::new(keys, Dir::S2C);
        assert_eq!(
            s.open(l.msg_type, l.session_id, l.seq, l.len, l.payload),
            None
        );
    }

    #[test]
    fn replay_rejected() {
        let keys = SessionKeys::derive(master(), 1, 1);
        let mut c = SessionCrypto::new(keys.clone(), Dir::C2S);
        let mut s = SessionCrypto::new(keys, Dir::S2C);
        let mut frames = Vec::new();
        for i in 0..3 {
            frames.push(c.seal(frame::TYPE_DATA, 42, &[i]));
        }
        for f in &frames {
            let l = frame::decode(f).unwrap();
            assert!(
                s.open(l.msg_type, l.session_id, l.seq, l.len, l.payload)
                    .is_some()
            );
        }
        let l = frame::decode(&frames[0]).unwrap();
        assert_eq!(
            s.open(l.msg_type, l.session_id, l.seq, l.len, l.payload),
            None
        );
    }

    #[test]
    fn wrong_direction_rejected() {
        let keys = SessionKeys::derive(master(), 1, 1);
        let mut c = SessionCrypto::new(keys.clone(), Dir::C2S);
        let raw = c.seal(frame::TYPE_DATA, 42, b"x");
        let l = frame::decode(&raw).unwrap();
        // Receiver using the same tx direction would open with the wrong key.
        let mut s = SessionCrypto::new(keys, Dir::C2S);
        assert_eq!(
            s.open(l.msg_type, l.session_id, l.seq, l.len, l.payload),
            None
        );
    }

    #[test]
    fn handshake_keys_domain() {
        // Different server nonces yield different handshake keys.
        assert_ne!(
            HandshakeKeys::derive(master(), 1),
            HandshakeKeys::derive(master(), 2)
        );
        // Handshake keys must not equal the session keys.
        let hk = HandshakeKeys::derive(master(), 7);
        let sk = SessionKeys::derive(master(), 7, 9);
        let (c2s, s2c) = sk.keys();
        assert_ne!(hk.c2s, c2s);
        assert_ne!(hk.s2c, s2c);
        // Each direction has its own handshake key.
        assert_ne!(hk.c2s, hk.s2c);

        // Sealed frames open with the right counter and direction only.
        let hk = HandshakeKeys::derive(master(), 3);
        let raw = hk.seal_frame(Dir::C2S, frame::TYPE_AUTH_REQ, 0, HS_AUTH_REQ, b"hello");
        let l = frame::decode(&raw).unwrap();
        assert_eq!(
            hk.open_frame(Dir::C2S, HS_AUTH_REQ, &l),
            Some(b"hello".to_vec())
        );
        assert_eq!(hk.open_frame(Dir::S2C, HS_AUTH_REQ, &l), None);
        assert_eq!(hk.open_frame(Dir::C2S, HS_AUTH_ACK, &l), None);
    }

    #[test]
    fn handshake_nonce_not_reused_across_directions() {
        // The nonce is salt(8) || counter(4) with a fixed counter per
        // message type: if both directions shared one key, a counter
        // repeated in the other direction would reuse the nonce. The
        // per-direction keys make that harmless.
        let hk = HandshakeKeys::derive(master(), 5);
        let raw = hk.seal_frame(Dir::C2S, frame::TYPE_AUTH_REQ, 0, HS_AUTH_REQ, b"c2s");
        let l = frame::decode(&raw).unwrap();
        assert_eq!(hk.open_frame(Dir::S2C, HS_AUTH_REQ, &l), None);
        let raw = hk.seal_frame(Dir::S2C, frame::TYPE_AUTH_ACK, 0, HS_AUTH_ACK, b"s2c");
        let l = frame::decode(&raw).unwrap();
        assert_eq!(hk.open_frame(Dir::C2S, HS_AUTH_ACK, &l), None);
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
