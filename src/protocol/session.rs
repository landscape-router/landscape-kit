use std::time::Duration;

use crate::protocol::crypto::{auth_proof, SessionKeys, AUTH_LABEL_C2S, AUTH_LABEL_S2C};
use crate::protocol::frame::{self, AuthReq, Frame};
use crate::protocol::{TYPE_AUTH_ACK, TYPE_AUTH_NACK};

pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(3);
pub const MAX_RETRIES: u32 = 5;
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(5);

/// Client->server proof: sha256("lndp-auth-c2s" || psk || s_nonce || c_nonce).
pub fn auth_proof_c2s(psk: &[u8], s_nonce: u64, c_nonce: u64) -> [u8; 32] {
    auth_proof(AUTH_LABEL_C2S, psk, s_nonce, c_nonce)
}

/// Server->client proof: sha256("lndp-auth-s2c" || psk || s_nonce || c_nonce).
/// The client verifies it, so the server must know the psk too (mutual auth).
pub fn auth_proof_s2c(psk: &[u8], s_nonce: u64, c_nonce: u64) -> [u8; 32] {
    auth_proof(AUTH_LABEL_S2C, psk, s_nonce, c_nonce)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientPhase {
    Discovering,
    AwaitAuth {
        server_nonce: u64,
        client_nonce: u64,
    },
    Session {
        session_id: u32,
        keys: SessionKeys,
    },
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

    /// Server answered our DISCOVER: mint a client nonce and build the
    /// AUTH_REQ frame (ready to send).
    pub fn on_resp(&mut self, resp: &frame::Resp, user: &str, psk: &[u8]) -> Vec<u8> {
        let client_nonce: u64 = rand::random();
        let proof = auth_proof_c2s(psk, resp.nonce, client_nonce);
        self.phase = ClientPhase::AwaitAuth {
            server_nonce: resp.nonce,
            client_nonce,
        };
        frame::encode_auth_req(user, client_nonce, &proof)
    }

    /// Handle an AUTH_ACK / AUTH_NACK frame. For AUTH_ACK the server's proof
    /// must match the psk, otherwise the server is not authenticated and the
    /// handshake fails.
    pub fn on_auth_frame(&mut self, frame: &Frame<'_>, psk: &[u8]) {
        match frame.msg_type {
            TYPE_AUTH_ACK => {
                let server_proof = frame::decode_auth_ack(frame.payload);
                match (&self.phase, server_proof) {
                    (
                        ClientPhase::AwaitAuth {
                            server_nonce,
                            client_nonce,
                        },
                        Ok(proof),
                    ) if proof == auth_proof_s2c(psk, *server_nonce, *client_nonce) => {
                        let keys = SessionKeys::derive(psk, *server_nonce, *client_nonce);
                        self.phase = ClientPhase::Session {
                            session_id: frame.session_id,
                            keys,
                        };
                    }
                    _ => {
                        self.phase = ClientPhase::Rejected("server authentication failed".into());
                    }
                }
            }
            TYPE_AUTH_NACK => {
                let reason =
                    frame::decode_auth_nack(frame.payload).unwrap_or_else(|_| "unknown".into());
                self.phase = ClientPhase::Rejected(reason);
            }
            _ => {}
        }
    }

    pub fn session_id(&self) -> Option<u32> {
        match self.phase {
            ClientPhase::Session { session_id, .. } => Some(session_id),
            _ => None,
        }
    }

    pub fn keys(&self) -> Option<&SessionKeys> {
        match &self.phase {
            ClientPhase::Session { keys, .. } => Some(keys),
            _ => None,
        }
    }
}

impl Default for ClientSession {
    fn default() -> Self {
        Self::new()
    }
}

pub enum VerifyResult {
    Accepted {
        sid: u32,
        keys: SessionKeys,
        server_proof: [u8; 32],
    },
    Rejected(String),
}

pub enum ServerPhase {
    Listening,
    Session { session_id: u32 },
}

pub struct ServerSession {
    pub phase: ServerPhase,
    /// Server nonce of the in-flight DISCOVER->AUTH_REQ handshake. A pending
    /// handshake never disturbs an active session: only a successfully
    /// verified AUTH_REQ replaces it.
    pending: Option<u64>,
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

    /// Client broadcast DISCOVER: mint a nonce and answer with a RESP frame,
    /// advertising the forwardable ports.
    pub fn on_discover(&mut self, device_name: &str, ports: &[u16]) -> Vec<u8> {
        let nonce: u64 = rand::random();
        self.pending = Some(nonce);
        frame::encode_resp(device_name, nonce, ports)
    }

    /// Verify an AUTH_REQ against the pending nonce and the shared psk.
    pub fn verify_auth(&mut self, req: &AuthReq, psk: &[u8]) -> VerifyResult {
        match self.pending {
            Some(server_nonce) => {
                let expect = auth_proof_c2s(psk, server_nonce, req.nonce);
                if req.proof == expect {
                    let sid = self.next_session_id;
                    self.next_session_id += 1;
                    self.pending = None;
                    let keys = SessionKeys::derive(psk, server_nonce, req.nonce);
                    let server_proof = auth_proof_s2c(psk, server_nonce, req.nonce);
                    self.phase = ServerPhase::Session { session_id: sid };
                    VerifyResult::Accepted {
                        sid,
                        keys,
                        server_proof,
                    }
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

impl Default for ServerSession {
    fn default() -> Self {
        Self::new()
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
            ports: vec![22, 6443],
        };
        let auth_req_raw = client.on_resp(&resp, "admin", PSK);
        let ClientPhase::AwaitAuth {
            server_nonce,
            client_nonce,
        } = client.phase
        else {
            panic!("expected AwaitAuth");
        };
        assert_eq!(server_nonce, 1234);

        let req = frame::decode_auth_req(&auth_req_raw[frame::HEADER_LEN..]).unwrap();
        assert_eq!(req.nonce, client_nonce);
        assert_eq!(req.proof, auth_proof_c2s(PSK, 1234, client_nonce));

        let server_proof = auth_proof_s2c(PSK, 1234, client_nonce);
        let ack_raw = frame::encode_auth_ack(7, &server_proof);
        let ack = frame::decode(&ack_raw).unwrap();
        client.on_auth_frame(&ack, PSK);
        assert_eq!(client.session_id(), Some(7));
        assert!(client.keys().is_some());

        let nack_raw = frame::encode_auth_nack("bad token");
        let nack = frame::decode(&nack_raw).unwrap();
        client.on_auth_frame(&nack, PSK);
        assert!(matches!(client.phase, ClientPhase::Rejected(_)));
    }

    #[test]
    fn client_rejects_rogue_server() {
        let mut client = ClientSession::new();
        let resp = frame::Resp {
            device_name: "router".into(),
            nonce: 1,
            ports: vec![],
        };
        let _ = client.on_resp(&resp, "admin", PSK);
        // A server that does not know the psk cannot produce a valid proof.
        let ack_raw = frame::encode_auth_ack(7, &[0u8; 32]);
        let ack = frame::decode(&ack_raw).unwrap();
        client.on_auth_frame(&ack, PSK);
        assert!(matches!(client.phase, ClientPhase::Rejected(_)));
        assert_eq!(client.session_id(), None);
    }

    #[test]
    fn server_accepts_correct_psk() {
        let mut server = ServerSession::new();
        let resp_raw = server.on_discover("router", &[22, 6443]);
        let resp = frame::decode_resp(&resp_raw[frame::HEADER_LEN..]).unwrap();
        assert_eq!(resp.ports, [22, 6443]);

        let client_nonce: u64 = 0x1020_3040_5060_7080;
        let req = frame::AuthReq {
            user: "admin".into(),
            nonce: client_nonce,
            proof: auth_proof_c2s(PSK, resp.nonce, client_nonce),
        };
        match server.verify_auth(&req, PSK) {
            VerifyResult::Accepted {
                sid,
                keys,
                server_proof,
            } => {
                assert_eq!(sid, 1);
                assert_eq!(server.session_id(), Some(1));
                assert_eq!(server_proof, auth_proof_s2c(PSK, resp.nonce, client_nonce));
                assert_eq!(keys, SessionKeys::derive(PSK, resp.nonce, client_nonce));
            }
            _ => panic!("should accept"),
        }
    }

    #[test]
    fn server_rejects_wrong_psk() {
        let mut server = ServerSession::new();
        let resp_raw = server.on_discover("router", &[]);
        let resp = frame::decode_resp(&resp_raw[frame::HEADER_LEN..]).unwrap();

        let req = frame::AuthReq {
            user: "admin".into(),
            nonce: 42,
            proof: auth_proof_c2s(b"wrong", resp.nonce, 42),
        };
        assert!(matches!(server.verify_auth(&req, PSK), VerifyResult::Rejected(_)));
        assert!(matches!(server.phase, ServerPhase::Listening));
    }

    #[test]
    fn pending_handshake_does_not_disturb_active_session() {
        let mut server = ServerSession::new();
        let resp_raw = server.on_discover("router", &[]);
        let resp = frame::decode_resp(&resp_raw[frame::HEADER_LEN..]).unwrap();
        let c_nonce = 1u64;
        let req = frame::AuthReq {
            user: "admin".into(),
            nonce: c_nonce,
            proof: auth_proof_c2s(PSK, resp.nonce, c_nonce),
        };
        assert!(matches!(server.verify_auth(&req, PSK), VerifyResult::Accepted { .. }));
        assert_eq!(server.session_id(), Some(1));

        // A new DISCOVER starts a handshake but must not kill the session.
        server.on_discover("router", &[]);
        assert_eq!(server.session_id(), Some(1));

        // An old AUTH_REQ can no longer be verified (pending nonce replaced).
        let stale = frame::AuthReq {
            user: "admin".into(),
            nonce: c_nonce,
            proof: auth_proof_c2s(PSK, resp.nonce, c_nonce),
        };
        assert!(matches!(server.verify_auth(&stale, PSK), VerifyResult::Rejected(_)));
        assert_eq!(server.session_id(), Some(1));
    }
}
