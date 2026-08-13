use std::time::Duration;

use crate::protocol::crypto::{
    auth_proof, ct_eq, HandshakeKeys, PreSharedKey, SessionKeys, AUTH_LABEL_C2S, AUTH_LABEL_S2C,
    HS_AUTH_ACK, HS_AUTH_NACK, HS_AUTH_REQ, HS_RESP, TAG_LEN,
};
use crate::protocol::frame::{self, Frame, Resp};
use crate::protocol::{
    TYPE_AUTH_ACK, TYPE_AUTH_NACK, TYPE_AUTH_REQ, TYPE_DISCOVER, TYPE_RESP,
};

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

/// Open a sealed DISCOVER frame (no peer state needed): returns the
/// discover_id, client name and token. None for frames a psk-holder could
/// not have produced — the server stays silent for those.
pub fn open_discover(l: &Frame<'_>, psk: &[u8]) -> Option<(u64, String, Option<String>)> {
    let plain = PreSharedKey::derive(psk).open_discover(l)?;
    frame::decode_discover_payload(&plain).ok()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientPhase {
    Discovering {
        /// Random per-attempt id echoed by the server in the sealed RESP;
        /// anything else is a replayed or raced response.
        discover_id: u64,
    },
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
            phase: ClientPhase::Discovering { discover_id: 0 },
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

    /// Build the sealed DISCOVER frame: sealed with the pre-discovery key
    /// derived from the psk, so only the intended server can open it.
    pub fn discover_frame(
        &mut self,
        client_name: &str,
        token: Option<&str>,
        psk: &[u8],
    ) -> Vec<u8> {
        let discover_id: u64 = rand::random();
        let salt: [u8; 8] = rand::random();
        self.phase = ClientPhase::Discovering { discover_id };
        let payload = frame::encode_discover_payload(discover_id, client_name, token);
        PreSharedKey::derive(psk).seal_discover(0, salt, &payload)
    }

    /// Handle a RESP frame: open it with the handshake keys derived from
    /// the server nonce (proves the server holds the psk), verify the
    /// discover_id echo, then build the sealed AUTH_REQ (ready to send).
    /// None = unauthentic or replayed response, caller should retry.
    pub fn on_resp(&mut self, l: &Frame<'_>, user: &str, psk: &[u8]) -> Option<(Resp, Vec<u8>)> {
        let ClientPhase::Discovering { discover_id } = self.phase else {
            return None;
        };
        if l.payload.len() < 8 + TAG_LEN {
            return None;
        }
        let server_nonce = u64::from_be_bytes(l.payload[..8].try_into().ok()?);
        let hkey = HandshakeKeys::derive(psk, server_nonce);
        let plain = hkey.open_prefixed(HS_RESP, l, 8)?;
        let resp = frame::decode_resp_payload(&plain).ok()?;
        if resp.discover_id != discover_id {
            return None;
        }
        let client_nonce: u64 = rand::random();
        let proof = auth_proof_c2s(psk, server_nonce, client_nonce);
        self.phase = ClientPhase::AwaitAuth {
            server_nonce,
            client_nonce,
            hkey: hkey.clone(),
        };
        let payload = frame::encode_auth_req_payload(user, client_nonce, &proof);
        let auth_req = hkey.seal_frame(TYPE_AUTH_REQ, 0, HS_AUTH_REQ, &payload);
        Some((resp, auth_req))
    }

    /// Handle an AUTH_ACK / AUTH_NACK frame. The AUTH_ACK payload is sealed
    /// with the handshake keys; once opened, the server's proof must match
    /// the psk, otherwise the server is not authenticated and the handshake
    /// fails. AUTH_NACK may be plaintext (e.g. lockout messages, or from a
    /// server that cannot assume shared handshake keys).
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
                        .or_else(|| frame::decode_auth_nack_payload(frame.payload).ok())
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

    /// A verified DISCOVER wants to start a handshake: mint a server nonce
    /// and answer with the sealed RESP frame (echoing the client's
    /// discover_id, advertising the forwardable ports). The DISCOVER frame
    /// itself is opened by the stateless `open_discover` helper.
    pub fn begin_discover(
        &mut self,
        discover_id: u64,
        device_name: &str,
        ports: &[u16],
        psk: &[u8],
    ) -> Vec<u8> {
        let server_nonce: u64 = rand::random();
        self.pending = Some(server_nonce);
        let hkey = HandshakeKeys::derive(psk, server_nonce);
        let payload = frame::encode_resp_payload(discover_id, device_name, ports);
        hkey.seal_prefixed(
            TYPE_RESP,
            0,
            HS_RESP,
            &server_nonce.to_be_bytes(),
            &payload,
        )
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

    /// Client seals a DISCOVER frame; returns the raw frame.
    fn client_discover(client: &mut ClientSession, psk: &[u8]) -> Vec<u8> {
        client.discover_frame("pc", Some("tok"), psk)
    }

    /// Server answers a discover_id with a sealed RESP; returns the server
    /// nonce (the plaintext prefix of the payload) and the raw frame.
    fn server_resp(server: &mut ServerSession, discover_id: u64, psk: &[u8]) -> (u64, Vec<u8>) {
        let raw = server.begin_discover(discover_id, "router", &[22, 6443], psk);
        let l = frame::decode(&raw).unwrap();
        let nonce = u64::from_be_bytes(l.payload[..8].try_into().unwrap());
        (nonce, raw)
    }

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

    fn sealed_nack(reason: &str, server_nonce: u64) -> Vec<u8> {
        let hkey = HandshakeKeys::derive(PSK, server_nonce);
        hkey.seal_frame(
            TYPE_AUTH_NACK,
            0,
            HS_AUTH_NACK,
            &frame::encode_auth_nack_payload(reason),
        )
    }

    /// Drive the client into AwaitAuth; returns (server_nonce, client_nonce).
    fn await_auth(client: &mut ClientSession, server: &mut ServerSession) -> (u64, u64) {
        let discover_raw = client_discover(client, PSK);
        let d = frame::decode(&discover_raw).unwrap();
        let (did, _, _) = open_discover(&d, PSK).unwrap();
        let (s_nonce, resp_raw) = server_resp(server, did, PSK);
        let r = frame::decode(&resp_raw).unwrap();
        let _ = client.on_resp(&r, "admin", PSK).expect("authentic RESP");
        let ClientPhase::AwaitAuth {
            server_nonce,
            client_nonce,
            ..
        } = &client.phase
        else {
            panic!("expected AwaitAuth");
        };
        (*server_nonce, *client_nonce)
    }

    #[test]
    fn full_handshake_flow() {
        let mut client = ClientSession::new();
        let discover_raw = client_discover(&mut client, PSK);
        let d = frame::decode(&discover_raw).unwrap();
        assert_eq!(d.msg_type, TYPE_DISCOVER);
        let (did, name, token) = open_discover(&d, PSK).unwrap();
        assert_eq!(name, "pc");
        assert_eq!(token.as_deref(), Some("tok"));

        let mut server = ServerSession::new();
        let (s_nonce, resp_raw) = server_resp(&mut server, did, PSK);
        let r = frame::decode(&resp_raw).unwrap();
        let (resp, auth_req_raw) = client.on_resp(&r, "admin", PSK).expect("server authenticated");
        assert_eq!(resp.device_name, "router");
        assert_eq!(resp.ports, [22, 6443]);
        assert_eq!(resp.discover_id, did);
        let ClientPhase::AwaitAuth {
            server_nonce,
            client_nonce,
            hkey,
        } = &client.phase
        else {
            panic!("expected AwaitAuth");
        };
        assert_eq!(*server_nonce, s_nonce);

        let a = frame::decode(&auth_req_raw).unwrap();
        let plain = hkey.open_frame(HS_AUTH_REQ, &a).unwrap();
        let req = frame::decode_auth_req_payload(&plain).unwrap();
        assert_eq!(req.user, "admin");
        assert_eq!(req.proof, auth_proof_c2s(PSK, s_nonce, *client_nonce));

        match server.verify_auth(&a, PSK) {
            VerifyResult::Accepted {
                sid,
                keys,
                server_proof,
                hkey,
                user,
            } => {
                assert_eq!(sid, 1);
                assert_eq!(user, "admin");
                assert_eq!(hkey, HandshakeKeys::derive(PSK, s_nonce));
                assert_eq!(server.session_id(), Some(1));
                assert_eq!(server_proof, auth_proof_s2c(PSK, s_nonce, *client_nonce));
                assert_eq!(keys, SessionKeys::derive(PSK, s_nonce, *client_nonce));
            }
            _ => panic!("should accept"),
        }

        let ack_raw = sealed_auth_ack(PSK, s_nonce, *client_nonce, 1);
        let ack = frame::decode(&ack_raw).unwrap();
        client.on_auth_frame(&ack, PSK);
        assert_eq!(client.session_id(), Some(1));
        assert!(client.keys().is_some());
    }

    #[test]
    fn server_stays_silent_for_wrong_psk_discover() {
        let mut client = ClientSession::new();
        let raw = client_discover(&mut client, b"wrong");
        let l = frame::decode(&raw).unwrap();
        assert!(open_discover(&l, PSK).is_none());
    }

    #[test]
    fn client_rejects_replayed_resp() {
        let mut client = ClientSession::new();
        let discover_raw = client_discover(&mut client, PSK);
        let d = frame::decode(&discover_raw).unwrap();
        let (did, _, _) = open_discover(&d, PSK).unwrap();
        let mut server = ServerSession::new();
        let (_, resp_raw) = server_resp(&mut server, did, PSK);
        let r = frame::decode(&resp_raw).unwrap();

        // A fresh attempt has a new discover_id: the old RESP must not open.
        let _ = client.discover_frame("pc", None, PSK);
        assert!(client.on_resp(&r, "admin", PSK).is_none());
    }

    #[test]
    fn client_rejects_rogue_resp() {
        let mut client = ClientSession::new();
        let discover_raw = client_discover(&mut client, PSK);
        let d = frame::decode(&discover_raw).unwrap();
        let (did, _, _) = open_discover(&d, PSK).unwrap();
        // A rogue without the psk cannot seal a RESP the client accepts.
        let mut rogue = ServerSession::new();
        let (_, resp_raw) = server_resp(&mut rogue, did, b"other");
        let r = frame::decode(&resp_raw).unwrap();
        assert!(client.on_resp(&r, "admin", PSK).is_none());
    }

    #[test]
    fn client_reads_sealed_nack() {
        let mut client = ClientSession::new();
        let mut server = ServerSession::new();
        let (s_nonce, _) = await_auth(&mut client, &mut server);
        let nack_raw = sealed_nack("bad token", s_nonce);
        let nack = frame::decode(&nack_raw).unwrap();
        client.on_auth_frame(&nack, PSK);
        assert_eq!(client.phase, ClientPhase::Rejected("bad token".into()));
    }

    #[test]
    fn client_reads_plaintext_nack() {
        // Lockout NACKs are plaintext (the server cannot assume shared
        // handshake keys): the reason must still surface to the user.
        let mut client = ClientSession::new();
        let mut server = ServerSession::new();
        let _ = await_auth(&mut client, &mut server);
        let reason = "too many auth failures, locked out for 59s";
        let nack_raw = frame::encode_auth_nack(reason);
        let nack = frame::decode(&nack_raw).unwrap();
        client.on_auth_frame(&nack, PSK);
        assert_eq!(client.phase, ClientPhase::Rejected(reason.into()));
        assert_eq!(client.session_id(), None);
    }

    #[test]
    fn server_rejects_garbage_and_wrong_proof() {
        // Garbage AUTH_REQ against a valid pending: rejected.
        let mut server = ServerSession::new();
        server.begin_discover(1, "router", &[], PSK);
        let raw = frame::encode(TYPE_AUTH_REQ, 0, 0, b"not sealed");
        let l = frame::decode(&raw).unwrap();
        assert!(matches!(server.verify_auth(&l, PSK), VerifyResult::Rejected(_)));

        // Valid seal, wrong proof: opened, then rejected.
        let mut server2 = ServerSession::new();
        let (s_nonce, _) = server_resp(&mut server2, 1, PSK);
        let hkey = HandshakeKeys::derive(PSK, s_nonce);
        let payload = frame::encode_auth_req_payload("admin", 42, &[0u8; 32]);
        let auth_raw2 = hkey.seal_frame(TYPE_AUTH_REQ, 0, HS_AUTH_REQ, &payload);
        let l2 = frame::decode(&auth_raw2).unwrap();
        assert!(matches!(server2.verify_auth(&l2, PSK), VerifyResult::Rejected(_)));
        assert!(matches!(server2.phase, ServerPhase::Listening));
    }

    #[test]
    fn pending_handshake_does_not_disturb_active_session() {
        let mut server = ServerSession::new();
        let (s_nonce, _) = server_resp(&mut server, 1, PSK);
        let c_nonce = 1u64;
        let auth_raw = sealed_auth_req("admin", PSK, s_nonce, c_nonce);
        let l = frame::decode(&auth_raw).unwrap();
        assert!(matches!(server.verify_auth(&l, PSK), VerifyResult::Accepted { .. }));
        assert_eq!(server.session_id(), Some(1));

        // A new DISCOVER starts a handshake but must not kill the session.
        server.begin_discover(2, "router", &[], PSK);
        assert_eq!(server.session_id(), Some(1));

        // An old AUTH_REQ can no longer be verified (pending nonce replaced).
        let stale = frame::decode(&auth_raw).unwrap();
        assert!(matches!(server.verify_auth(&stale, PSK), VerifyResult::Rejected(_)));
        assert_eq!(server.session_id(), Some(1));
    }
}
