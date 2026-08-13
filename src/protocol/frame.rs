use std::fmt;

pub use crate::protocol::{
    MAGIC, TYPE_AUTH_ACK, TYPE_AUTH_NACK, TYPE_AUTH_REQ, TYPE_DATA, TYPE_DISCOVER, TYPE_KEEPALIVE,
    TYPE_RESP, TYPE_TEARDOWN, VERSION,
};

/// 16-byte header: magic(4) version(1) type(1) session(4) len(2) seq(4).
///
/// `len` is the exact payload length, so ethernet min-frame padding is never
/// mistaken for payload. `seq` is the per-session counter used for the AEAD
/// nonce and replay protection; handshake frames always carry seq = 0.
pub const HEADER_LEN: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    TooShort,
    BadMagic,
    BadVersion,
    BadPayload,
    BadUtf8,
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FrameError::TooShort => write!(f, "frame shorter than header"),
            FrameError::BadMagic => write!(f, "bad magic, not an LNDP frame"),
            FrameError::BadVersion => write!(f, "unsupported protocol version"),
            FrameError::BadPayload => write!(f, "malformed payload"),
            FrameError::BadUtf8 => write!(f, "non-utf8 string in payload"),
        }
    }
}

impl std::error::Error for FrameError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame<'a> {
    pub msg_type: u8,
    pub session_id: u32,
    /// Per-session sequence number (0 for handshake frames).
    pub seq: u32,
    /// Payload length as carried in the header (padding is not included).
    pub len: u16,
    pub payload: &'a [u8],
}

pub fn decode(data: &[u8]) -> Result<Frame<'_>, FrameError> {
    if data.len() < HEADER_LEN {
        return Err(FrameError::TooShort);
    }
    if u32::from_be_bytes([data[0], data[1], data[2], data[3]]) != MAGIC {
        return Err(FrameError::BadMagic);
    }
    if data[4] != VERSION {
        return Err(FrameError::BadVersion);
    }
    let len = u16::from_be_bytes([data[10], data[11]]) as usize;
    if data.len() < HEADER_LEN + len {
        return Err(FrameError::BadPayload);
    }
    Ok(Frame {
        msg_type: data[5],
        session_id: u32::from_be_bytes([data[6], data[7], data[8], data[9]]),
        seq: u32::from_be_bytes([data[12], data[13], data[14], data[15]]),
        len: len as u16,
        payload: &data[HEADER_LEN..HEADER_LEN + len],
    })
}

/// The 16-byte frame header. Session frames use it as the AEAD associated
/// data, so any tampering with the header is detected at decrypt time.
pub fn encode_header(msg_type: u8, session_id: u32, seq: u32, payload_len: u16) -> [u8; HEADER_LEN] {
    let mut buf = [0u8; HEADER_LEN];
    buf[0..4].copy_from_slice(&MAGIC.to_be_bytes());
    buf[4] = VERSION;
    buf[5] = msg_type;
    buf[6..10].copy_from_slice(&session_id.to_be_bytes());
    buf[10..12].copy_from_slice(&payload_len.to_be_bytes());
    buf[12..16].copy_from_slice(&seq.to_be_bytes());
    buf
}

/// Encode a plaintext frame (handshake messages). Session frames must go
/// through `crypto::SessionCrypto` instead — never call this for DATA,
/// KEEPALIVE or TEARDOWN.
pub fn encode(msg_type: u8, session_id: u32, seq: u32, payload: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(HEADER_LEN + payload.len());
    buf.extend_from_slice(&encode_header(msg_type, session_id, seq, payload.len() as u16));
    buf.extend_from_slice(payload);
    buf
}

pub fn type_name(t: u8) -> &'static str {
    match t {
        TYPE_DISCOVER => "DISCOVER",
        TYPE_RESP => "RESP",
        TYPE_AUTH_REQ => "AUTH_REQ",
        TYPE_AUTH_ACK => "AUTH_ACK",
        TYPE_AUTH_NACK => "AUTH_NACK",
        TYPE_KEEPALIVE => "KEEPALIVE",
        TYPE_DATA => "DATA",
        TYPE_TEARDOWN => "TEARDOWN",
        _ => "UNKNOWN",
    }
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    let b = s.as_bytes();
    out.extend_from_slice(&(b.len() as u16).to_be_bytes());
    out.extend_from_slice(b);
}

fn get_str(data: &[u8], off: &mut usize) -> Result<String, FrameError> {
    if *off + 2 > data.len() {
        return Err(FrameError::BadPayload);
    }
    let len = u16::from_be_bytes([data[*off], data[*off + 1]]) as usize;
    *off += 2;
    if *off + len > data.len() {
        return Err(FrameError::BadPayload);
    }
    let s = std::str::from_utf8(&data[*off..*off + len]).map_err(|_| FrameError::BadUtf8)?;
    *off += len;
    Ok(s.to_string())
}

fn put_u64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn get_u64(data: &[u8], off: &mut usize) -> Result<u64, FrameError> {
    if *off + 8 > data.len() {
        return Err(FrameError::BadPayload);
    }
    let v = u64::from_be_bytes(data[*off..*off + 8].try_into().unwrap());
    *off += 8;
    Ok(v)
}

/// DISCOVER payload: client name + optional discovery token (anti-scanning).
/// The server stays silent when a token is configured and not carried.
pub fn encode_discover(client_name: &str, token: Option<&str>) -> Vec<u8> {
    let mut p = Vec::new();
    put_str(&mut p, client_name);
    if let Some(t) = token.filter(|t| !t.is_empty()) {
        put_str(&mut p, t);
    }
    encode(TYPE_DISCOVER, 0, 0, &p)
}

pub fn decode_discover(payload: &[u8]) -> Result<(String, Option<String>), FrameError> {
    let mut off = 0;
    let name = get_str(payload, &mut off)?;
    let token = if off < payload.len() {
        Some(get_str(payload, &mut off)?)
    } else {
        None
    };
    Ok((name, token))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resp {
    pub device_name: String,
    pub nonce: u64,
    /// Ports the server allows forwarding to (capability info, optional).
    pub ports: Vec<u16>,
}

/// RESP payload: device name + server nonce for challenge-response auth,
/// followed by the advertised forward ports (capability negotiation).
pub fn encode_resp(device_name: &str, nonce: u64, ports: &[u16]) -> Vec<u8> {
    let mut p = Vec::new();
    put_str(&mut p, device_name);
    put_u64(&mut p, nonce);
    for port in ports {
        p.extend_from_slice(&port.to_be_bytes());
    }
    encode(TYPE_RESP, 0, 0, &p)
}

pub fn decode_resp(payload: &[u8]) -> Result<Resp, FrameError> {
    let mut off = 0;
    let device_name = get_str(payload, &mut off)?;
    let nonce = get_u64(payload, &mut off)?;
    let mut ports = Vec::new();
    while off + 2 <= payload.len() {
        ports.push(u16::from_be_bytes([payload[off], payload[off + 1]]));
        off += 2;
    }
    if off != payload.len() {
        return Err(FrameError::BadPayload);
    }
    Ok(Resp {
        device_name,
        nonce,
        ports,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthReq {
    pub user: String,
    /// Client nonce for the mutual challenge.
    pub nonce: u64,
    /// sha256("lndp-auth-c2s" || psk || server_nonce || client_nonce)
    pub proof: [u8; 32],
}

pub fn encode_auth_req(user: &str, nonce: u64, proof: &[u8; 32]) -> Vec<u8> {
    let mut p = Vec::new();
    put_str(&mut p, user);
    put_u64(&mut p, nonce);
    p.extend_from_slice(proof);
    encode(TYPE_AUTH_REQ, 0, 0, &p)
}

pub fn decode_auth_req(payload: &[u8]) -> Result<AuthReq, FrameError> {
    let mut off = 0;
    let user = get_str(payload, &mut off)?;
    let nonce = get_u64(payload, &mut off)?;
    if off + 32 > payload.len() {
        return Err(FrameError::BadPayload);
    }
    let mut proof = [0u8; 32];
    proof.copy_from_slice(&payload[off..off + 32]);
    Ok(AuthReq { user, nonce, proof })
}

/// AUTH_ACK payload: the server's proof of psk knowledge
/// (sha256("lndp-auth-s2c" || psk || server_nonce || client_nonce));
/// the session id is carried in the frame header.
pub fn encode_auth_ack(session_id: u32, server_proof: &[u8; 32]) -> Vec<u8> {
    let mut p = Vec::new();
    p.extend_from_slice(server_proof);
    encode(TYPE_AUTH_ACK, session_id, 0, &p)
}

pub fn decode_auth_ack(payload: &[u8]) -> Result<[u8; 32], FrameError> {
    if payload.len() != 32 {
        return Err(FrameError::BadPayload);
    }
    let mut proof = [0u8; 32];
    proof.copy_from_slice(payload);
    Ok(proof)
}

pub fn encode_auth_nack(reason: &str) -> Vec<u8> {
    let mut p = Vec::new();
    put_str(&mut p, reason);
    encode(TYPE_AUTH_NACK, 0, 0, &p)
}

pub fn decode_auth_nack(payload: &[u8]) -> Result<String, FrameError> {
    let mut off = 0;
    get_str(payload, &mut off)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_all_messages() {
        let frames = [
            encode_discover("pc-a", None),
            encode_discover("pc-b", Some("landscape-token")),
            encode_resp("landscape-router", 0xDEADBEEF_DEADBEEF, &[22, 6443]),
            encode_auth_req("admin", 0x0102_0304_0506_0708, &[7u8; 32]),
            encode_auth_ack(42, &[9u8; 32]),
            encode_auth_nack("bad token"),
        ];
        for raw in frames {
            let f = decode(&raw).expect("decode");
            let re = encode(f.msg_type, f.session_id, f.seq, f.payload);
            assert_eq!(raw, re, "encode(decode(x)) must be identity");
        }
    }

    #[test]
    fn decodes_payloads() {
        assert_eq!(
            decode_discover(&encode_discover("pc", None)[HEADER_LEN..]).unwrap(),
            ("pc".to_string(), None)
        );
        assert_eq!(
            decode_discover(&encode_discover("pc", Some("tok"))[HEADER_LEN..]).unwrap(),
            ("pc".to_string(), Some("tok".to_string()))
        );
        assert_eq!(
            decode_discover(&encode_discover("pc", Some(""))[HEADER_LEN..]).unwrap(),
            ("pc".to_string(), None)
        );
        let r = decode_resp(&encode_resp("router", 1, &[])[HEADER_LEN..]).unwrap();
        assert_eq!(
            r,
            Resp {
                device_name: "router".into(),
                nonce: 1,
                ports: vec![]
            }
        );
        let r = decode_resp(&encode_resp("router", 1, &[22, 6443])[HEADER_LEN..]).unwrap();
        assert_eq!(r.ports, [22, 6443]);
        let a = decode_auth_req(&encode_auth_req("u", 5, &[9u8; 32])[HEADER_LEN..]).unwrap();
        assert_eq!(
            a,
            AuthReq {
                user: "u".into(),
                nonce: 5,
                proof: [9u8; 32]
            }
        );
        assert_eq!(
            decode_auth_ack(&encode_auth_ack(7, &[3u8; 32])[HEADER_LEN..]).unwrap(),
            [3u8; 32]
        );
        assert_eq!(decode_auth_nack(&encode_auth_nack("no")[HEADER_LEN..]).unwrap(), "no");
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(decode(&[]), Err(FrameError::TooShort));
        assert_eq!(decode(&[0x00; HEADER_LEN]), Err(FrameError::BadMagic));
        let mut bad_ver = encode(TYPE_DATA, 0, 0, &[]);
        bad_ver[4] = 0x99;
        assert_eq!(decode(&bad_ver), Err(FrameError::BadVersion));
    }

    #[test]
    fn rejects_truncated_payloads() {
        let raw = encode_resp("router", 5, &[]);
        assert!(decode_resp(&raw[HEADER_LEN..raw.len() - 1]).is_err());
        let raw = encode_auth_req("u", 1, &[0u8; 32]);
        assert!(decode_auth_req(&raw[HEADER_LEN..raw.len() - 1]).is_err());
    }

    #[test]
    fn strips_ethernet_padding() {
        let raw = encode_resp("router", 5, &[]);
        let mut padded = raw.clone();
        padded.extend_from_slice(&[0x00; 40]); // min-frame padding
        let f = decode(&padded).unwrap();
        assert_eq!(f.payload, &raw[HEADER_LEN..]);
        assert_eq!(f.len as usize, f.payload.len());
    }

    #[test]
    fn rejects_oversized_len() {
        let mut bad = encode_resp("router", 5, &[]);
        bad[10] = 0xff;
        bad[11] = 0xff;
        assert_eq!(decode(&bad), Err(FrameError::BadPayload));
    }

    #[test]
    fn type_names() {
        assert_eq!(type_name(TYPE_DISCOVER), "DISCOVER");
        assert_eq!(type_name(TYPE_TEARDOWN), "TEARDOWN");
        assert_eq!(type_name(0xEE), "UNKNOWN");
    }
}
