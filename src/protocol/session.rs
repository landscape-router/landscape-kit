use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::protocol::{
    frame::{self, AuthReq, Frame},
    TYPE_AUTH_ACK, TYPE_AUTH_NACK,
};

pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(3);
pub const MAX_RETRIES: u32 = 5;
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(5);

/// sha256(psk || nonce) — challenge response without sending the psk itself
pub fn auth_hash(psk: &[u8], nonce: u32) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(psk);
    h.update(nonce.to_be_bytes());
    h.finalize().into()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientPhase {
    Discovering,
    AwaitAuth { nonce: u32 },
    Session { session_id: u32 },
    Rejected(String),
}

pub struct ClientSession {
    pub phase: ClientPhase,
    retries: u32,
}

impl ClientSession {
    pub fn new() -> Self {
        Self {
            phase: ClientPhase::Discovering,
            retries: 0,
        }
    }

    pub fn retransmit_allowed(&self) -> bool {
        self.retries < MAX_RETRIES
    }

    pub fn retries(&self) -> u32 {
        self.retries
    }

    pub fn bump_retry(&mut self) {
        self.retries += 1;
    }

    /// Server answered our DISCOVER: build the AUTH_REQ frame (ready to send)
    pub fn on_resp(&mut self, resp: &frame::Resp, user: &str, psk: &[u8]) -> Vec<u8> {
        let hash = auth_hash(psk, resp.nonce);
        self.phase = ClientPhase::AwaitAuth { nonce: resp.nonce };
        frame::encode_auth_req(user, &hash)
    }

    /// Handle an AUTH_ACK / AUTH_NACK frame
    pub fn on_auth_frame(&mut self, frame: &Frame<'_>) {
        match frame.msg_type {
            TYPE_AUTH_ACK => {
                self.phase = ClientPhase::Session {
                    session_id: frame.session_id,
                };
            }
            TYPE_AUTH_NACK => {
                let reason = frame::decode_auth_nack(frame.payload).unwrap_or_else(|_| "unknown".into());
                self.phase = ClientPhase::Rejected(reason);
            }
            _ => {}
        }
    }

    pub fn session_id(&self) -> Option<u32> {
        match self.phase {
            ClientPhase::Session { session_id } => Some(session_id),
            _ => None,
        }
    }
}

pub enum VerifyResult {
    Accepted(u32),
    Rejected(String),
}

pub enum ServerPhase {
    Listening,
    Session { session_id: u32 },
}

pub struct ServerSession {
    pub phase: ServerPhase,
    pending: Option<u32>,
    next_session_id: u32,
}

impl ServerSession {
    pub fn new() -> Self {
        Self {
            phase: ServerPhase::Listening,
            pending: None,
            next_session_id: 1,
        }
    }

    /// Client broadcast DISCOVER: mint a nonce and answer with a RESP frame
    pub fn on_discover(&mut self, device_name: &str) -> Vec<u8> {
        let nonce: u32 = rand::random();
        self.pending = Some(nonce);
        frame::encode_resp(device_name, nonce)
    }

    /// Verify an AUTH_REQ against the pending nonce and the shared psk
    pub fn verify_auth(&mut self, req: &AuthReq, psk: &[u8]) -> VerifyResult {
        match self.pending {
            Some(nonce) => {
                let expect = auth_hash(psk, nonce);
                if req.hash == expect {
                    let sid = self.next_session_id;
                    self.next_session_id += 1;
                    self.pending = None;
                    self.phase = ServerPhase::Session { session_id: sid };
                    VerifyResult::Accepted(sid)
                } else {
                    self.pending = None;
                    VerifyResult::Rejected("authentication failed".into())
                }
            }
            None => VerifyResult::Rejected("no pending discovery".into()),
        }
    }

    pub fn session_id(&self) -> Option<u32> {
        match self.phase {
            ServerPhase::Session { session_id } => Some(session_id),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PSK: &[u8] = b"landscape-secret";

    #[test]
    fn client_handshake_flow() {
        let mut client = ClientSession::new();
        assert!(client.retransmit_allowed());

        let resp = frame::Resp {
            device_name: "router".into(),
            nonce: 1234,
        };
        let auth_req_raw = client.on_resp(&resp, "admin", PSK);
        assert!(matches!(client.phase, ClientPhase::AwaitAuth { nonce: 1234 }));

        let req = frame::decode_auth_req(&auth_req_raw[frame::HEADER_LEN..]).unwrap();
        assert_eq!(req.hash, auth_hash(PSK, 1234));

        let ack_raw = frame::encode_auth_ack(7);
        let ack = frame::decode(&ack_raw).unwrap();
        client.on_auth_frame(&ack);
        assert_eq!(client.session_id(), Some(7));

        let nack_raw = frame::encode_auth_nack("bad token");
        let nack = frame::decode(&nack_raw).unwrap();
        client.on_auth_frame(&nack);
        assert!(matches!(client.phase, ClientPhase::Rejected(_)));
    }

    #[test]
    fn server_accepts_correct_psk() {
        let mut server = ServerSession::new();
        let resp_raw = server.on_discover("router");
        let resp = frame::decode_resp(&resp_raw[frame::HEADER_LEN..]).unwrap();

        let req = frame::AuthReq {
            user: "admin".into(),
            hash: auth_hash(PSK, resp.nonce),
        };
        match server.verify_auth(&req, PSK) {
            VerifyResult::Accepted(sid) => {
                assert_eq!(sid, 1);
                assert_eq!(server.session_id(), Some(1));
            }
            _ => panic!("should accept"),
        }
    }

    #[test]
    fn server_rejects_wrong_psk() {
        let mut server = ServerSession::new();
        let resp_raw = server.on_discover("router");
        let resp = frame::decode_resp(&resp_raw[frame::HEADER_LEN..]).unwrap();

        let req = frame::AuthReq {
            user: "admin".into(),
            hash: auth_hash(b"wrong", resp.nonce),
        };
        assert!(matches!(server.verify_auth(&req, PSK), VerifyResult::Rejected(_)));
        assert!(matches!(server.phase, ServerPhase::Listening));
    }

    #[test]
    fn auth_hash_changes_with_nonce() {
        assert_ne!(auth_hash(PSK, 1), auth_hash(PSK, 2));
    }
}
