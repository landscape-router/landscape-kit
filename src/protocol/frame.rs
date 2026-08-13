use std::fmt;

use crate::protocol::{MAGIC, TYPE_AUTH_ACK, TYPE_AUTH_NACK, TYPE_AUTH_REQ, TYPE_DATA, TYPE_DISCOVER, TYPE_KEEPALIVE, TYPE_RESP, VERSION};

pub const HEADER_LEN: usize = 10;

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
    Ok(Frame {
        msg_type: data[5],
        session_id: u32::from_be_bytes([data[6], data[7], data[8], data[9]]),
        payload: &data[HEADER_LEN..],
    })
}

pub fn encode(msg_type: u8, session_id: u32, payload: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(HEADER_LEN + payload.len());
    buf.extend_from_slice(&MAGIC.to_be_bytes());
    buf.push(VERSION);
    buf.push(msg_type);
    buf.extend_from_slice(&session_id.to_be_bytes());
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

/// DISCOVER payload: client name
pub fn encode_discover(client_name: &str) -> Vec<u8> {
    let mut p = Vec::new();
    put_str(&mut p, client_name);
    encode(TYPE_DISCOVER, 0, &p)
}

pub fn decode_discover(payload: &[u8]) -> Result<String, FrameError> {
    let mut off = 0;
    get_str(payload, &mut off)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resp {
    pub device_name: String,
    pub nonce: u32,
}

/// RESP payload: device name + nonce for challenge-response auth
pub fn encode_resp(device_name: &str, nonce: u32) -> Vec<u8> {
    let mut p = Vec::new();
    put_str(&mut p, device_name);
    p.extend_from_slice(&nonce.to_be_bytes());
    encode(TYPE_RESP, 0, &p)
}

pub fn decode_resp(payload: &[u8]) -> Result<Resp, FrameError> {
    let mut off = 0;
    let device_name = get_str(payload, &mut off)?;
    if off + 4 > payload.len() {
        return Err(FrameError::BadPayload);
    }
    let nonce = u32::from_be_bytes([payload[off], payload[off + 1], payload[off + 2], payload[off + 3]]);
    Ok(Resp { device_name, nonce })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthReq {
    pub user: String,
    pub hash: [u8; 32],
}

/// AUTH_REQ payload: user name + sha256(psk || nonce)
pub fn encode_auth_req(user: &str, hash: &[u8; 32]) -> Vec<u8> {
    let mut p = Vec::new();
    put_str(&mut p, user);
    p.extend_from_slice(hash);
    encode(TYPE_AUTH_REQ, 0, &p)
}

pub fn decode_auth_req(payload: &[u8]) -> Result<AuthReq, FrameError> {
    let mut off = 0;
    let user = get_str(payload, &mut off)?;
    if off + 32 > payload.len() {
        return Err(FrameError::BadPayload);
    }
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&payload[off..off + 32]);
    Ok(AuthReq { user, hash })
}

/// AUTH_ACK payload: empty; session id is carried in the frame header
pub fn encode_auth_ack(session_id: u32) -> Vec<u8> {
    encode(TYPE_AUTH_ACK, session_id, &[])
}

pub fn encode_auth_nack(reason: &str) -> Vec<u8> {
    let mut p = Vec::new();
    put_str(&mut p, reason);
    encode(TYPE_AUTH_NACK, 0, &p)
}

pub fn decode_auth_nack(payload: &[u8]) -> Result<String, FrameError> {
    let mut off = 0;
    get_str(payload, &mut off)
}

pub fn encode_keepalive(session_id: u32) -> Vec<u8> {
    encode(TYPE_KEEPALIVE, session_id, &[])
}

/// DATA payload: one complete IP packet
#[allow(dead_code)]
pub fn encode_data(session_id: u32, ip_packet: &[u8]) -> Vec<u8> {
    encode(TYPE_DATA, session_id, ip_packet)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_all_messages() {
        let frames = [
            encode_discover("pc-a"),
            encode_resp("landscape-router", 0xDEADBEEF),
            encode_auth_req("admin", &[7u8; 32]),
            encode_auth_ack(42),
            encode_auth_nack("bad token"),
            encode_keepalive(42),
            encode_data(42, &[0x45, 0x00, 0x00, 0x10]),
        ];
        for raw in frames {
            let f = decode(&raw).expect("decode");
            let re = encode(f.msg_type, f.session_id, f.payload);
            assert_eq!(raw, re, "encode(decode(x)) must be identity");
        }
    }

    #[test]
    fn decodes_payloads() {
        assert_eq!(decode_discover(&encode_discover("pc")[HEADER_LEN..]).unwrap(), "pc");
        let r = decode_resp(&encode_resp("router", 1)[HEADER_LEN..]).unwrap();
        assert_eq!(r, Resp { device_name: "router".into(), nonce: 1 });
        let a = decode_auth_req(&encode_auth_req("u", &[9u8; 32])[HEADER_LEN..]).unwrap();
        assert_eq!(a, AuthReq { user: "u".into(), hash: [9u8; 32] });
        assert_eq!(decode_auth_nack(&encode_auth_nack("no")[HEADER_LEN..]).unwrap(), "no");
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(decode(&[]), Err(FrameError::TooShort));
        assert_eq!(decode(&[0x00; HEADER_LEN]), Err(FrameError::BadMagic));
        let mut bad_ver = encode(TYPE_DATA, 0, &[]);
        bad_ver[4] = 0x99;
        assert_eq!(decode(&bad_ver), Err(FrameError::BadVersion));
    }

    #[test]
    fn rejects_truncated_payloads() {
        let raw = encode_resp("router", 5);
        assert!(decode_resp(&raw[HEADER_LEN..raw.len() - 1]).is_err());
        let raw = encode_auth_req("u", &[0u8; 32]);
        assert!(decode_auth_req(&raw[HEADER_LEN..raw.len() - 1]).is_err());
    }

    #[test]
    fn type_names() {
        assert_eq!(type_name(TYPE_DISCOVER), "DISCOVER");
        assert_eq!(type_name(0xEE), "UNKNOWN");
    }
}
