use std::time::Duration;

use crate::protocol::crypto::{
    auth_proof, ct_eq, HandshakeKeys, SessionKeys, AUTH_LABEL_C2S, AUTH_LABEL_S2C, HS_AUTH_ACK,
    HS_AUTH_NACK, HS_AUTH_REQ,
};
use crate::protocol::frame::{self, Frame};
use crate::protocol::{TYPE_AUTH_ACK, TYPE_AUTH_NACK, TYPE_AUTH_REQ};

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
        hkey: HandshakeKeys,
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
    /// sealed AUTH_REQ frame (ready to send). The payload is encrypted with
    /// handshake keys derived from the psk and the server nonce.
    pub fn on_resp(&mut self, resp: &frame::Resp, user: &str, psk: &[u8]) -> Vec<u8> {
        let client_nonce: u64 = rand::random();
        let proof = auth_proof_c2s(psk, resp.nonce, client_nonce);
        let hkey = HandshakeKeys::derive(psk, resp.nonce);
        self.phase = ClientPhase::AwaitAuth {
            server_nonce: resp.nonce,
            client_nonce,
            hkey: hkey.clone(),
        };
        let payload = frame::encode_auth_req_payload(user, client_nonce, &proof);
        hkey.seal_frame(TYPE_AUTH_REQ, 0, HS_AUTH_REQ, &payload)
    }

    /// Handle an AUTH_ACK / AUTH_NACK frame. The AUTH_ACK payload is sealed
    /// with the handshake keys; once opened, the server's proof must match
    /// the psk, otherwise the server is not authenticated and the handshake
    /// fails. AUTH_NACK may be plaintext (the server cannot assume shared
    /// handshake keys when rejecting).
    pub fn on_auth_frame(&mut self, frame: &Frame<'_>, psk: &[u8]) {
        match frame.msg_type {
            TYPE_AUTH_ACK => {
                let opened = match &self.phase {
                    ClientPhase::AwaitAuth {
                        server_nonce,
                        client_nonce,
                        hkey,
                    } => hkey.open_frame(HS_AUTH_ACK, frame).and_then(|plain| {
                        let server_proof = frame::decode_auth_ack_payload(&plain).ok()?;
                        (ct_eq(&server_proof, &auth_proof_s2c(psk, *server_nonce, *client_nonce)))
                            .then_some(SessionKeys::derive(psk, *server_nonce, *client_nonce))
                    }),
                    _ => None,
                };
                match opened {
                    Some(keys) => {
                        self.phase = ClientPhase::Session {
                            session_id: frame.session_id,
                            keys,
                        };
                    }
                    None => {
                        self.phase = ClientPhase::Rejected("server authentication failed".into());
                    }
                }
            }
            TYPE_AUTH_NACK => {
                let reason = match &self.phase {
                    ClientPhase::AwaitAuth { hkey, .. } => hkey
                        .open_frame(HS_AUTH_NACK, frame)
                        .and_then(|p| frame::decode_auth_nack_payload(&p).ok())
                        .unwrap_or_else(|| "unknown".into()),
                    _ => "unknown".into(),
                };
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
        /// Handshake keys for sealing the AUTH_ACK back to the client.
        hkey: HandshakeKeys,
        user: String,
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

    /// Verify a sealed AUTH_REQ frame against the pending nonce and the
    /// shared psk: open it with the handshake keys, then check the proof.
    pub fn verify_auth(&mut self, frame: &Frame<'_>, psk: &[u8]) -> VerifyResult {
        match self.pending {
            Some(server_nonce) => {
                let hkey = HandshakeKeys::derive(psk, server_nonce);
                let plain = match hkey.open_frame(HS_AUTH_REQ, frame) {
                    Some(p) => p,
                    None => {
                        self.pending = None;
                        return VerifyResult::Rejected("bad handshake frame".into());
                    }
                };
                let Ok(req) = frame::decode_auth_req_payload(&plain) else {
                    self.pending = None;
                    return VerifyResult::Rejected("bad handshake frame".into());
                };
                let expect = auth_proof_c2s(psk, server_nonce, req.nonce);
                if ct_eq(&req.proof, &expect) {
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
                        hkey,
                        user: req.user,
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

    fn sealed_auth_req(user: &str, psk: &[u8], server_nonce: u64, client_nonce: u64) -> Vec<u8> {
        let hkey = HandshakeKeys::derive(psk, server_nonce);
        let proof = auth_proof_c2s(psk, server_nonce, client_nonce);
        let payload = frame::encode_auth_req_payload(user, client_nonce, &proof);
        hkey.seal_frame(TYPE_AUTH_REQ, 0, HS_AUTH_REQ, &payload)
    }

    fn sealed_auth_ack(psk: &[u8], server_nonce: u64, client_nonce: u64, sid: u32) -> Vec<u8> {
        let hkey = HandshakeKeys::derive(psk, server_nonce);
        let proof = auth_proof_s2c(psk, server_nonce, client_nonce);
        let payload = frame::encode_auth_ack_payload(&proof);
        hkey.seal_frame(TYPE_AUTH_ACK, sid, HS_AUTH_ACK, &payload)
    }

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
            hkey,
        } = &client.phase
        else {
            panic!("expected AwaitAuth");
        };
        assert_eq!(*server_nonce, 1234);

        // The wire payload must be sealed: opening it yields user + nonce + proof.
        let l = frame::decode(&auth_req_raw).unwrap();
        assert_eq!(l.msg_type, TYPE_AUTH_REQ);
        let plain = hkey.open_frame(HS_AUTH_REQ, &l).unwrap();
        let req = frame::decode_auth_req_payload(&plain).unwrap();
        assert_eq!(req.user, "admin");
        assert_eq!(req.nonce, *client_nonce);
        assert_eq!(req.proof, auth_proof_c2s(PSK, 1234, *client_nonce));

        // A NACK (encrypted with the same handshake keys) moves to Rejected.
        let nack_hkey = HandshakeKeys::derive(PSK, 1234);
        let nack_raw = nack_hkey.seal_frame(
            TYPE_AUTH_NACK,
            0,
            HS_AUTH_NACK,
            &frame::encode_auth_nack_payload("bad token"),
        );
        let nack = frame::decode(&nack_raw).unwrap();
        client.on_auth_frame(&nack, PSK);
        assert!(matches!(client.phase, ClientPhase::Rejected(_)));
    }

    #[test]
    fn client_accepts_server_ack() {
        let mut client = ClientSession::new();
        let resp = frame::Resp {
            device_name: "router".into(),
            nonce: 99,
            ports: vec![],
        };
        let _ = client.on_resp(&resp, "admin", PSK);
        let ClientPhase::AwaitAuth {
            server_nonce,
            client_nonce,
            ..
        } = &client.phase
        else {
            panic!("expected AwaitAuth");
        };
        let ack_raw = sealed_auth_ack(PSK, *server_nonce, *client_nonce, 7);
        let ack = frame::decode(&ack_raw).unwrap();
        client.on_auth_frame(&ack, PSK);
        assert_eq!(client.session_id(), Some(7));
        assert!(client.keys().is_some());
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
        let ClientPhase::AwaitAuth {
            server_nonce,
            client_nonce,
            ..
        } = &client.phase
        else {
            panic!("expected AwaitAuth");
        };
        // A server that does not know the psk cannot seal a valid ACK.
        let hkey = HandshakeKeys::derive(b"other-psk", *server_nonce);
        let ack_raw = hkey.seal_frame(
            TYPE_AUTH_ACK,
            7,
            HS_AUTH_ACK,
            &frame::encode_auth_ack_payload(&[0u8; 32]),
        );
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
        let auth_raw = sealed_auth_req("admin", PSK, resp.nonce, client_nonce);
        let l = frame::decode(&auth_raw).unwrap();
        match server.verify_auth(&l, PSK) {
            VerifyResult::Accepted {
                sid,
                keys,
                server_proof,
                hkey,
                user,
            } => {
                assert_eq!(sid, 1);
                assert_eq!(user, "admin");
                assert_eq!(hkey, HandshakeKeys::derive(PSK, resp.nonce));
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

        // Sealed with a different psk: cannot be opened.
        let auth_raw = sealed_auth_req("admin", b"wrong", resp.nonce, 42);
        let l = frame::decode(&auth_raw).unwrap();
        assert!(matches!(server.verify_auth(&l, PSK), VerifyResult::Rejected(_)));

        // Sealed with the right psk but a wrong proof: opened, then rejected.
        let mut server2 = ServerSession::new();
        let resp_raw2 = server2.on_discover("router", &[]);
        let resp2 = frame::decode_resp(&resp_raw2[frame::HEADER_LEN..]).unwrap();
        let hkey = HandshakeKeys::derive(PSK, resp2.nonce);
        let payload = frame::encode_auth_req_payload("admin", 42, &[0u8; 32]);
        let auth_raw2 = hkey.seal_frame(TYPE_AUTH_REQ, 0, HS_AUTH_REQ, &payload);
        let l2 = frame::decode(&auth_raw2).unwrap();
        assert!(matches!(server2.verify_auth(&l2, PSK), VerifyResult::Rejected(_)));
        assert!(matches!(server2.phase, ServerPhase::Listening));
    }

    #[test]
    fn server_rejects_garbage_handshake() {
        let mut server = ServerSession::new();
        server.on_discover("router", &[]);
        let raw = frame::encode(TYPE_AUTH_REQ, 0, 0, b"not sealed");
        let l = frame::decode(&raw).unwrap();
        assert!(matches!(server.verify_auth(&l, PSK), VerifyResult::Rejected(_)));
    }

    #[test]
    fn pending_handshake_does_not_disturb_active_session() {
        let mut server = ServerSession::new();
        let resp_raw = server.on_discover("router", &[]);
        let resp = frame::decode_resp(&resp_raw[frame::HEADER_LEN..]).unwrap();
        let c_nonce = 1u64;
        let auth_raw = sealed_auth_req("admin", PSK, resp.nonce, c_nonce);
        let l = frame::decode(&auth_raw).unwrap();
        assert!(matches!(server.verify_auth(&l, PSK), VerifyResult::Accepted { .. }));
        assert_eq!(server.session_id(), Some(1));

        // A new DISCOVER starts a handshake but must not kill the session.
        server.on_discover("router", &[]);
        assert_eq!(server.session_id(), Some(1));

        // An old AUTH_REQ can no longer be verified (pending nonce replaced).
        let stale = frame::decode(&auth_raw).unwrap();
        assert!(matches!(server.verify_auth(&stale, PSK), VerifyResult::Rejected(_)));
        assert_eq!(server.session_id(), Some(1));
    }
}
