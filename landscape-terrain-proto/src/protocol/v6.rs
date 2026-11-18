//! Terrain v6 separates LAN discovery, Noise device authentication and operator login.
use std::io;

use chacha20poly1305::{
    ChaCha20Poly1305, KeyInit,
    aead::{Aead, Payload},
};
use sha2::{Digest, Sha256};
use snow::{Builder, HandshakeState, StatelessTransportState};
use zeroize::Zeroizing;

use super::{crypto::SessionCrypto, frame};

pub const VERSION: u8 = 6;
pub const HELLO: u8 = 10;
pub const WELCOME: u8 = 11;
pub const AUTH_LANDSCAPE: u8 = 1;
pub const AUTH_ROOT: u8 = 2;
pub const AUTH_CHUNK: usize = 900;
pub const MAX_CREDENTIAL_BYTES: usize = 8192;
pub const PATTERN: &str = "Noise_NX_25519_ChaChaPoly_SHA256";

fn invalid() -> io::Error {
    io::Error::other("invalid Terrain v6 message")
}

pub fn packet(kind: u8, sid: u32, seq: u32, body: &[u8]) -> Vec<u8> {
    let mut bytes = frame::encode(kind, sid, seq, body);
    bytes[4] = VERSION;
    bytes
}

pub fn decode(bytes: &[u8]) -> Result<frame::Frame<'_>, frame::FrameError> {
    frame::decode_version(bytes, VERSION)
}

pub fn fingerprint(public: &[u8]) -> String {
    hex::encode(Sha256::digest(public))
}

pub fn public_key(private: &[u8; 32]) -> [u8; 32] {
    x25519_dalek::x25519(*private, x25519_dalek::X25519_BASEPOINT_BYTES)
}

/// Authenticates private discovery and the first unicast handshake packet.
pub struct Discovery(Option<ChaCha20Poly1305>);

impl Discovery {
    pub fn new(psk: Option<&str>) -> Self {
        Self(psk.map(|secret| {
            let mut key = Zeroizing::new([0u8; 32]);
            scrypt::scrypt(
                secret.as_bytes(),
                b"terrain-v6-discovery",
                &scrypt::Params::new(15, 8, 1, 32).expect("fixed scrypt parameters"),
                &mut *key,
            )
            .expect("32 byte output");
            ChaCha20Poly1305::new((&*key).into())
        }))
    }

    pub fn encode(&self, kind: u8, sid: u32, body: &[u8]) -> io::Result<Vec<u8>> {
        let mut payload = Vec::new();
        if let Some(cipher) = &self.0 {
            let nonce = rand::random::<[u8; 12]>();
            payload.push(1);
            payload.extend_from_slice(&nonce);
            let mut aad = vec![VERSION, kind];
            aad.extend_from_slice(&sid.to_be_bytes());
            payload.extend(
                cipher
                    .encrypt(
                        (&nonce).into(),
                        Payload {
                            msg: body,
                            aad: &aad,
                        },
                    )
                    .map_err(|_| invalid())?,
            );
        } else {
            payload.push(0);
            payload.extend_from_slice(body);
        }
        Ok(packet(kind, sid, 0, &payload))
    }

    pub fn open(&self, f: &frame::Frame<'_>) -> Option<Vec<u8>> {
        if f.version != VERSION || f.seq != 0 {
            return None;
        }
        match (&self.0, f.payload.split_first()?) {
            (None, (0, body)) => Some(body.to_vec()),
            (Some(cipher), (1, body)) if body.len() >= 28 => {
                let (nonce, ciphertext) = body.split_at(12);
                let mut aad = vec![VERSION, f.msg_type];
                aad.extend_from_slice(&f.session_id.to_be_bytes());
                cipher
                    .decrypt(
                        nonce.into(),
                        Payload {
                            msg: ciphertext,
                            aad: &aad,
                        },
                    )
                    .ok()
            }
            _ => None,
        }
    }
}

pub fn handshake(sid: u32, private: Option<&[u8; 32]>) -> Result<HandshakeState, snow::Error> {
    let mut prologue = b"TERR/v6".to_vec();
    prologue.extend_from_slice(&sid.to_be_bytes());
    let builder = Builder::new(PATTERN.parse()?).prologue(&prologue)?;
    match private {
        Some(key) => builder.local_private_key(key)?.build_responder(),
        None => builder.build_initiator(),
    }
}

pub fn write_handshake(state: &mut HandshakeState) -> Result<Vec<u8>, snow::Error> {
    let mut buf = vec![0; 128];
    let n = state.write_message(&[], &mut buf)?;
    buf.truncate(n);
    Ok(buf)
}

pub fn read_handshake(state: &mut HandshakeState, bytes: &[u8]) -> Result<(), snow::Error> {
    state.read_message(bytes, &mut [])?;
    Ok(())
}

/// Receive out-of-order Ethernet datagrams once, within a 64-packet window.
#[derive(Default)]
struct Window {
    high: u32,
    bits: u64,
}

impl Window {
    fn accept(&mut self, seq: u32) -> bool {
        if seq > self.high {
            self.bits = self.bits.checked_shl(seq - self.high).unwrap_or(0);
            self.high = seq;
        }
        let Some(mask) = 1u64.checked_shl(self.high - seq) else {
            return false;
        };
        if self.bits & mask != 0 {
            return false;
        }
        self.bits |= mask;
        true
    }
}

pub struct Transport {
    state: StatelessTransportState,
    next: u32,
    window: Window,
}

impl Transport {
    pub fn new(handshake: HandshakeState) -> Result<Self, snow::Error> {
        Ok(Self {
            state: handshake.into_stateless_transport_mode()?,
            next: 0,
            window: Window::default(),
        })
    }

    pub fn seal(&mut self, kind: u8, sid: u32, body: &[u8]) -> io::Result<Vec<u8>> {
        let seq = self.next;
        self.next = seq
            .checked_add(1)
            .ok_or_else(|| io::Error::other("session sequence exhausted"))?;
        let mut plain = Zeroizing::new(vec![kind]);
        plain.extend_from_slice(&sid.to_be_bytes());
        plain.extend_from_slice(body);
        let mut out = vec![0; plain.len() + 16];
        self.state
            .write_message(seq.into(), &plain, &mut out)
            .map_err(|_| invalid())?;
        Ok(packet(kind, sid, seq, &out))
    }

    pub fn open(&mut self, f: &frame::Frame<'_>) -> Option<Vec<u8>> {
        if f.version != VERSION {
            return None;
        }
        let mut plain = Zeroizing::new(vec![0; f.payload.len()]);
        let n = self
            .state
            .read_message(f.seq.into(), f.payload, &mut plain)
            .ok()?;
        if n < 5
            || plain[0] != f.msg_type
            || plain[1..5] != f.session_id.to_be_bytes()
            || !self.window.accept(f.seq)
        {
            return None;
        }
        Some(plain[5..n].to_vec())
    }
}

/// The TCP relay uses the version selected for the authenticated connection.
pub enum Channel {
    Legacy(SessionCrypto),
    Noise(Transport),
}

impl Channel {
    pub fn decode<'a>(&self, bytes: &'a [u8]) -> Result<frame::Frame<'a>, frame::FrameError> {
        match self {
            Self::Legacy(_) => frame::decode(bytes),
            Self::Noise(_) => decode(bytes),
        }
    }

    pub fn seal(&mut self, kind: u8, sid: u32, body: &[u8]) -> io::Result<Vec<u8>> {
        match self {
            Self::Legacy(c) => Ok(c.seal(kind, sid, body)),
            Self::Noise(c) => c.seal(kind, sid, body),
        }
    }

    pub fn open(&mut self, f: &frame::Frame<'_>) -> Option<Vec<u8>> {
        match self {
            Self::Legacy(c) => c.open(f.msg_type, f.session_id, f.seq, f.len, f.payload),
            Self::Noise(c) => c.open(f),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{TYPE_AUTH_REQ, TYPE_DATA};
    use super::*;

    fn pair(private: &[u8; 32]) -> (Transport, Transport) {
        let mut a = handshake(17, None).unwrap();
        let mut b = handshake(17, Some(private)).unwrap();
        read_handshake(&mut b, &write_handshake(&mut a).unwrap()).unwrap();
        read_handshake(&mut a, &write_handshake(&mut b).unwrap()).unwrap();
        assert_eq!(a.get_remote_static(), Some(public_key(private).as_slice()));
        (Transport::new(a).unwrap(), Transport::new(b).unwrap())
    }

    #[test]
    fn each_handshake_uses_fresh_ephemeral_keys_and_proves_the_same_device() {
        let identity = [7; 32];
        let exchange = || {
            let mut initiator = handshake(17, None).unwrap();
            let mut responder = handshake(17, Some(&identity)).unwrap();
            let hello = write_handshake(&mut initiator).unwrap();
            read_handshake(&mut responder, &hello).unwrap();
            let welcome = write_handshake(&mut responder).unwrap();
            read_handshake(&mut initiator, &welcome).unwrap();
            assert_eq!(
                initiator.get_remote_static(),
                Some(public_key(&identity).as_slice())
            );
            assert_ne!(
                initiator.get_remote_static(),
                Some(public_key(&[8; 32]).as_slice())
            );
            (hello, welcome, initiator.get_handshake_hash().to_vec())
        };
        let (a, b, hash) = exchange();
        let (c, d, other) = exchange();
        assert_ne!(a, c);
        assert_ne!(b, d);
        assert_ne!(hash, other);
    }

    #[test]
    fn datagrams_bind_type_and_session_and_accept_reordering_once() {
        let (mut tx, mut rx) = pair(&[37; 32]);
        let first = tx.seal(TYPE_DATA, 17, b"first").unwrap();
        let next = tx.seal(TYPE_DATA, 17, b"next").unwrap();
        for pos in [5, 6, 12] {
            let mut corrupt = next.clone();
            corrupt[pos] ^= 1;
            assert!(rx.open(&decode(&corrupt).unwrap()).is_none());
        }
        assert_eq!(rx.open(&decode(&next).unwrap()).unwrap(), b"next");
        assert_eq!(rx.open(&decode(&first).unwrap()).unwrap(), b"first");
        assert!(rx.open(&decode(&next).unwrap()).is_none());
        let (_, mut later) = pair(&[37; 32]);
        assert!(later.open(&decode(&first).unwrap()).is_none());
    }

    #[test]
    fn private_discovery_authenticates_kind_and_session() {
        // Use fixed test keys to isolate the wire codec from the scrypt work factor.
        let gate = Discovery(Some(ChaCha20Poly1305::new((&[19; 32]).into())));
        let wrong = Discovery(Some(ChaCha20Poly1305::new((&[31; 32]).into())));
        let public = Discovery(None);
        let raw = gate.encode(HELLO, 43, b"message").unwrap();
        let frame = decode(&raw).unwrap();
        assert!(public.open(&frame).is_none());
        assert!(wrong.open(&frame).is_none());
        assert_eq!(gate.open(&frame).unwrap(), b"message");
        let mut other = frame.clone();
        other.msg_type = TYPE_AUTH_REQ;
        assert!(gate.open(&other).is_none());
        let raw = public.encode(HELLO, 43, b"message").unwrap();
        assert!(gate.open(&decode(&raw).unwrap()).is_none());
    }
}
