use std::time::Duration;

use crate::protocol::crypto::{
    AUTH_LABEL_C2S, AUTH_LABEL_S2C, Dir, HS_AUTH_ACK, HS_AUTH_NACK, HS_AUTH_REQ, HS_RESP,
    HandshakeKeys, MasterKey, PreSharedKey, SessionKeys, TAG_LEN, auth_proof, ct_eq,
};
use crate::protocol::frame::{self, Frame, Resp};
use crate::protocol::{TYPE_AUTH_ACK, TYPE_AUTH_NACK, TYPE_AUTH_REQ, TYPE_RESP};

pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(3);
pub const MAX_RETRIES: u32 = 5;
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(5);

/// Client->server proof: sha256("lndp-auth-c2s" || master || s_nonce || c_nonce).
pub fn auth_proof_c2s(master: &MasterKey, s_nonce: u64, c_nonce: u64) -> [u8; 32] {
    auth_proof(AUTH_LABEL_C2S, master.as_bytes(), s_nonce, c_nonce)
}

/// Server->client proof: sha256("lndp-auth-s2c" || master || s_nonce || c_nonce).
/// The client verifies it, so the server must know the psk too (mutual auth).
pub fn auth_proof_s2c(master: &MasterKey, s_nonce: u64, c_nonce: u64) -> [u8; 32] {
    auth_proof(AUTH_LABEL_S2C, master.as_bytes(), s_nonce, c_nonce)
}

/// Open a sealed DISCOVER frame (no peer state needed): returns the
/// discover_id, client name and token. None for frames a master-key holder
/// could not have produced — the server stays silent for those.
pub fn open_discover(l: &Frame<'_>, master: &MasterKey) -> Option<(u64, String, Option<String>)> {
    let plain = PreSharedKey::derive(master).open_discover(l)?;
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
    /// derived from the master key, so only the intended server can open
    /// it. Each frame carries a fresh 12-byte nonce.
    pub fn discover_frame(
        &mut self,
        client_name: &str,
        token: Option<&str>,
        master: &MasterKey,
    ) -> Vec<u8> {
        let discover_id: u64 = rand::random();
        let nonce: [u8; 12] = rand::random();
        self.phase = ClientPhase::Discovering { discover_id };
        let payload = frame::encode_discover_payload(discover_id, client_name, token);
        PreSharedKey::derive(master).seal_discover(0, nonce, &payload)
    }

    /// Handle a RESP frame: open it with the handshake keys derived from
    /// the server nonce (proves the server holds the psk), verify the
    /// discover_id echo, then build the sealed AUTH_REQ (ready to send).
    /// None = unauthentic or replayed response, caller should retry.
    pub fn on_resp(
        &mut self,
        l: &Frame<'_>,
        user: &str,
        master: &MasterKey,
    ) -> Option<(Resp, Vec<u8>)> {
        let ClientPhase::Discovering { discover_id } = self.phase else {
            return None;
        };
        if l.payload.len() < 8 + TAG_LEN {
            return None;
        }
        let server_nonce = u64::from_be_bytes(l.payload[..8].try_into().ok()?);
        let hkey = HandshakeKeys::derive(master, server_nonce);
        let plain = hkey.open_prefixed(Dir::S2C, HS_RESP, l, 8)?;
        let resp = frame::decode_resp_payload(&plain).ok()?;
        if resp.discover_id != discover_id {
            return None;
        }
        let client_nonce: u64 = rand::random();
        let proof = auth_proof_c2s(master, server_nonce, client_nonce);
        self.phase = ClientPhase::AwaitAuth {
            server_nonce,
            client_nonce,
            hkey: hkey.clone(),
        };
        let payload = frame::encode_auth_req_payload(user, client_nonce, &proof);
        let auth_req = hkey.seal_frame(Dir::C2S, TYPE_AUTH_REQ, 0, HS_AUTH_REQ, &payload);
        Some((resp, auth_req))
    }

    /// Handle an AUTH_ACK / AUTH_NACK frame. The AUTH_ACK payload is sealed
    /// with the handshake keys; once opened, the server's proof must match
    /// the psk, otherwise the server is not authenticated and the handshake
    /// fails. AUTH_NACK is tried sealed first (v5), then as a plaintext
    /// fallback for servers without a pending handshake nonce (e.g. lockout
    /// messages); the caller should additionally filter auth frames by the
    /// RESP source MAC.
    pub fn on_auth_frame(&mut self, frame: &Frame<'_>, master: &MasterKey) {
        match frame.msg_type {
            TYPE_AUTH_ACK => {
                let opened = match &self.phase {
                    ClientPhase::AwaitAuth {
                        server_nonce,
                        client_nonce,
                        hkey,
                    } => hkey.open_frame(Dir::S2C, HS_AUTH_ACK, frame).and_then(|plain| {
                        let server_proof = frame::decode_auth_ack_payload(&plain).ok()?;
                        (ct_eq(
                            &server_proof,
                            &auth_proof_s2c(master, *server_nonce, *client_nonce),
                        ))
                        .then_some(SessionKeys::derive(
                            master,
                            *server_nonce,
                            *client_nonce,
                        ))
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
                        .open_frame(Dir::S2C, HS_AUTH_NACK, frame)
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

#[derive(Debug)]
pub enum VerifyResult {
    Accepted {
        sid: u32,
        keys: SessionKeys,
        server_proof: [u8; 32],
        /// Handshake keys for sealing the AUTH_ACK back to the client.
        hkey: HandshakeKeys,
        user: String,
    },
    /// A tag-valid AUTH_REQ whose proof did not match: a genuine (failed)
    /// auth attempt, so it counts toward the per-MAC lockout. Carries the
    /// handshake keys so the caller can seal the AUTH_NACK (the sender
    /// necessarily holds them too).
    Rejected {
        reason: String,
        hkey: HandshakeKeys,
    },
    /// A frame that failed to even open (wrong tag) or was malformed: not
    /// an auth attempt at all. It does not count toward lockout and the
    /// pending nonce is preserved, so a MAC-spoofed garbage frame can
    /// neither interrupt a real handshake nor lock out a connecting client.
    Unauthentic(String),
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
        master: &MasterKey,
    ) -> Vec<u8> {
        let server_nonce: u64 = rand::random();
        self.pending = Some(server_nonce);
        let hkey = HandshakeKeys::derive(master, server_nonce);
        let payload = frame::encode_resp_payload(discover_id, device_name, ports);
        hkey.seal_prefixed(
            Dir::S2C,
            TYPE_RESP,
            0,
            HS_RESP,
            &server_nonce.to_be_bytes(),
            &payload,
        )
    }

    /// The server nonce of the in-flight handshake, if any, clearing it.
    /// Lets the caller seal an AUTH_NACK with the handshake keys (e.g.
    /// lockout messages); being a take, at most one NACK is ever sealed per
    /// handshake, so the (key, nonce) pair of the NACK counter is never
    /// reused with a different plaintext (later lockout NACKs fall back to
    /// plaintext).
    pub fn take_server_nonce(&mut self) -> Option<u64> {
        self.pending.take()
    }

    /// Verify a sealed AUTH_REQ frame against the pending nonce and the
    /// shared psk: open it with the handshake keys, then check the proof.
    pub fn verify_auth(&mut self, frame: &Frame<'_>, master: &MasterKey) -> VerifyResult {
        match self.pending {
            Some(server_nonce) => {
                let hkey = HandshakeKeys::derive(master, server_nonce);
                // A frame that cannot be opened is not an auth attempt at
                // all: keep the pending nonce, so a spoofed garbage frame
                // can neither interrupt a real handshake nor lock out a
                // connecting client.
                let Some(plain) = hkey.open_frame(Dir::C2S, HS_AUTH_REQ, frame) else {
                    return VerifyResult::Unauthentic("bad handshake frame".into());
                };
                let Ok(req) = frame::decode_auth_req_payload(&plain) else {
                    return VerifyResult::Unauthentic("malformed auth request".into());
                };
                let expect = auth_proof_c2s(master, server_nonce, req.nonce);
                if ct_eq(&req.proof, &expect) {
                    let sid = self.next_session_id;
                    self.next_session_id += 1;
                    self.pending = None;
                    let keys = SessionKeys::derive(master, server_nonce, req.nonce);
                    let server_proof = auth_proof_s2c(master, server_nonce, req.nonce);
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
                    VerifyResult::Rejected {
                        reason: "authentication failed".into(),
                        hkey,
                    }
                }
            }
            None => VerifyResult::Unauthentic("no pending discovery".into()),
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
    use crate::protocol::TYPE_DISCOVER;

    /// Scrypt-stretched test master, computed once (N=2^15 takes ~100ms).
    fn master() -> &'static MasterKey {
        static M: std::sync::OnceLock<MasterKey> = std::sync::OnceLock::new();
        M.get_or_init(|| MasterKey::derive(b"landscape-secret"))
    }

    /// Client seals a DISCOVER frame; returns the raw frame.
    fn client_discover(client: &mut ClientSession) -> Vec<u8> {
        client.discover_frame("pc", Some("tok"), master())
    }

    /// Server answers a discover_id with a sealed RESP; returns the server
    /// nonce (the plaintext prefix of the payload) and the raw frame.
    fn server_resp(server: &mut ServerSession, discover_id: u64, m: &MasterKey) -> (u64, Vec<u8>) {
        let raw = server.begin_discover(discover_id, "router", &[22, 6443], m);
        let l = frame::decode(&raw).unwrap();
        let nonce = u64::from_be_bytes(l.payload[..8].try_into().unwrap());
        (nonce, raw)
    }

    fn sealed_auth_req(user: &str, m: &MasterKey, server_nonce: u64, client_nonce: u64) -> Vec<u8> {
        let hkey = HandshakeKeys::derive(m, server_nonce);
        let proof = auth_proof_c2s(m, server_nonce, client_nonce);
        let payload = frame::encode_auth_req_payload(user, client_nonce, &proof);
        hkey.seal_frame(Dir::C2S, TYPE_AUTH_REQ, 0, HS_AUTH_REQ, &payload)
    }

    fn sealed_auth_ack(m: &MasterKey, server_nonce: u64, client_nonce: u64, sid: u32) -> Vec<u8> {
        let hkey = HandshakeKeys::derive(m, server_nonce);
        let proof = auth_proof_s2c(m, server_nonce, client_nonce);
        let payload = frame::encode_auth_ack_payload(&proof);
        hkey.seal_frame(Dir::S2C, TYPE_AUTH_ACK, sid, HS_AUTH_ACK, &payload)
    }

    fn sealed_nack(reason: &str, m: &MasterKey, server_nonce: u64) -> Vec<u8> {
        let hkey = HandshakeKeys::derive(m, server_nonce);
        hkey.seal_frame(
            Dir::S2C,
            TYPE_AUTH_NACK,
            0,
            HS_AUTH_NACK,
            &frame::encode_auth_nack_payload(reason),
        )
    }

    /// Drive the client into AwaitAuth; returns (server_nonce, client_nonce).
    fn await_auth(client: &mut ClientSession, server: &mut ServerSession) -> (u64, u64) {
        let discover_raw = client_discover(client);
        let d = frame::decode(&discover_raw).unwrap();
        let (did, _, _) = open_discover(&d, master()).unwrap();
        let (_, resp_raw) = server_resp(server, did, master());
        let r = frame::decode(&resp_raw).unwrap();
        let _ = client
            .on_resp(&r, "admin", master())
            .expect("authentic RESP");
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
        let discover_raw = client_discover(&mut client);
        let d = frame::decode(&discover_raw).unwrap();
        assert_eq!(d.msg_type, TYPE_DISCOVER);
        let (did, name, token) = open_discover(&d, master()).unwrap();
        assert_eq!(name, "pc");
        assert_eq!(token.as_deref(), Some("tok"));

        let mut server = ServerSession::new();
        let (s_nonce, resp_raw) = server_resp(&mut server, did, master());
        let r = frame::decode(&resp_raw).unwrap();
        let (resp, auth_req_raw) = client
            .on_resp(&r, "admin", master())
            .expect("server authenticated");
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
        let plain = hkey.open_frame(Dir::C2S, HS_AUTH_REQ, &a).unwrap();
        let req = frame::decode_auth_req_payload(&plain).unwrap();
        assert_eq!(req.user, "admin");
        assert_eq!(req.proof, auth_proof_c2s(master(), s_nonce, *client_nonce));

        match server.verify_auth(&a, master()) {
            VerifyResult::Accepted {
                sid,
                keys,
                server_proof,
                hkey,
                user,
            } => {
                assert_eq!(sid, 1);
                assert_eq!(user, "admin");
                assert_eq!(hkey, HandshakeKeys::derive(master(), s_nonce));
                assert_eq!(server.session_id(), Some(1));
                assert_eq!(
                    server_proof,
                    auth_proof_s2c(master(), s_nonce, *client_nonce)
                );
                assert_eq!(keys, SessionKeys::derive(master(), s_nonce, *client_nonce));
            }
            _ => panic!("should accept"),
        }

        let ack_raw = sealed_auth_ack(master(), s_nonce, *client_nonce, 1);
        let ack = frame::decode(&ack_raw).unwrap();
        client.on_auth_frame(&ack, master());
        assert_eq!(client.session_id(), Some(1));
        assert!(client.keys().is_some());
    }

    #[test]
    fn server_stays_silent_for_wrong_psk_discover() {
        let other = MasterKey::derive(b"wrong");
        let mut client = ClientSession::new();
        let raw = client.discover_frame("pc", Some("tok"), &other);
        let l = frame::decode(&raw).unwrap();
        assert!(open_discover(&l, master()).is_none());
    }

    #[test]
    fn client_rejects_replayed_resp() {
        let mut client = ClientSession::new();
        let discover_raw = client_discover(&mut client);
        let d = frame::decode(&discover_raw).unwrap();
        let (did, _, _) = open_discover(&d, master()).unwrap();
        let mut server = ServerSession::new();
        let (_, resp_raw) = server_resp(&mut server, did, master());
        let r = frame::decode(&resp_raw).unwrap();

        // A fresh attempt has a new discover_id: the old RESP must not open.
        let _ = client.discover_frame("pc", None, master());
        assert!(client.on_resp(&r, "admin", master()).is_none());
    }

    #[test]
    fn client_rejects_rogue_resp() {
        let mut client = ClientSession::new();
        let discover_raw = client_discover(&mut client);
        let d = frame::decode(&discover_raw).unwrap();
        let (did, _, _) = open_discover(&d, master()).unwrap();
        // A rogue without the psk cannot seal a RESP the client accepts.
        let rogue_master = MasterKey::derive(b"other");
        let mut rogue = ServerSession::new();
        let (_, resp_raw) = server_resp(&mut rogue, did, &rogue_master);
        let r = frame::decode(&resp_raw).unwrap();
        assert!(client.on_resp(&r, "admin", master()).is_none());
    }

    #[test]
    fn client_reads_sealed_nack() {
        let mut client = ClientSession::new();
        let mut server = ServerSession::new();
        let (s_nonce, _) = await_auth(&mut client, &mut server);
        let nack_raw = sealed_nack("bad token", master(), s_nonce);
        let nack = frame::decode(&nack_raw).unwrap();
        client.on_auth_frame(&nack, master());
        assert_eq!(client.phase, ClientPhase::Rejected("bad token".into()));
    }

    #[test]
    fn client_reads_plaintext_nack() {
        // Plaintext NACKs are still accepted as a fallback when the server
        // has no pending handshake nonce: the reason must surface to the
        // user. Callers filter these frames by the RESP source MAC.
        let mut client = ClientSession::new();
        let mut server = ServerSession::new();
        let _ = await_auth(&mut client, &mut server);
        let reason = "too many auth failures, locked out for 59s";
        let nack_raw = frame::encode_auth_nack(reason);
        let nack = frame::decode(&nack_raw).unwrap();
        client.on_auth_frame(&nack, master());
        assert_eq!(client.phase, ClientPhase::Rejected(reason.into()));
        assert_eq!(client.session_id(), None);
    }

    #[test]
    fn server_nonce_enables_sealed_lockout_nack() {
        let mut client = ClientSession::new();
        let mut server = ServerSession::new();
        let (s_nonce, _) = await_auth(&mut client, &mut server);
        // The server can seal a NACK while the handshake is pending...
        assert_eq!(server.take_server_nonce(), Some(s_nonce));
        // ...but only once: the nonce is consumed, so a repeated lockout
        // NACK can never reuse the same (key, nonce) with new plaintext.
        assert_eq!(server.take_server_nonce(), None);
        let nack_raw = sealed_nack("locked out for 59s", master(), s_nonce);
        let nack = frame::decode(&nack_raw).unwrap();
        client.on_auth_frame(&nack, master());
        assert_eq!(client.phase, ClientPhase::Rejected("locked out for 59s".into()));
    }

    #[test]
    fn server_rejects_garbage_and_wrong_proof() {
        // Garbage AUTH_REQ against a valid pending: unauthentic, and the
        // pending nonce survives (a spoofed garbage frame must not be able
        // to interrupt the handshake).
        let mut server = ServerSession::new();
        let (s_nonce, _) = server_resp(&mut server, 1, master());
        let raw = frame::encode(TYPE_AUTH_REQ, 0, 0, b"not sealed");
        let l = frame::decode(&raw).unwrap();
        assert!(matches!(
            server.verify_auth(&l, master()),
            VerifyResult::Unauthentic(_)
        ));
        assert_eq!(server.take_server_nonce(), Some(s_nonce));

        // Valid seal, wrong proof: opened, then rejected (counts toward
        // lockout), carrying the handshake keys to seal the NACK.
        let mut server2 = ServerSession::new();
        let (s_nonce2, _) = server_resp(&mut server2, 1, master());
        let hkey = HandshakeKeys::derive(master(), s_nonce2);
        let payload = frame::encode_auth_req_payload("admin", 42, &[0u8; 32]);
        let auth_raw2 = hkey.seal_frame(Dir::C2S, TYPE_AUTH_REQ, 0, HS_AUTH_REQ, &payload);
        let l2 = frame::decode(&auth_raw2).unwrap();
        match server2.verify_auth(&l2, master()) {
            VerifyResult::Rejected { reason, hkey } => {
                assert_eq!(reason, "authentication failed");
                // The client holds the same handshake keys, so the sealed
                // NACK opens on its side.
                let nack = hkey.seal_frame(
                    Dir::S2C,
                    TYPE_AUTH_NACK,
                    0,
                    HS_AUTH_NACK,
                    &frame::encode_auth_nack_payload(&reason),
                );
                let n = frame::decode(&nack).unwrap();
                assert_eq!(
                    hkey.open_frame(Dir::S2C, HS_AUTH_NACK, &n),
                    Some(frame::encode_auth_nack_payload("authentication failed"))
                );
            }
            r => panic!("expected Rejected, got {r:?}"),
        }
        assert!(matches!(server2.phase, ServerPhase::Listening));
        // The nonce is gone: a repeated attempt is unauthentic, not a
        // lockout-relevant failure.
        assert_eq!(server2.take_server_nonce(), None);
    }

    #[test]
    fn garbage_auth_req_does_not_consume_pending() {
        // A MAC-spoofed garbage AUTH_REQ must not break the victim's
        // handshake: the pending nonce survives, so the real AUTH_REQ
        // (sealed under the same nonce) still verifies afterwards.
        let mut server = ServerSession::new();
        let (s_nonce, _) = server_resp(&mut server, 7, master());
        for i in 0..5 {
            let garbage = frame::encode(TYPE_AUTH_REQ, 0, i, b"garbage");
            let g = frame::decode(&garbage).unwrap();
            assert!(matches!(
                server.verify_auth(&g, master()),
                VerifyResult::Unauthentic(_)
            ));
        }
        // The real AUTH_REQ (sealed under the same nonce) still verifies.
        let auth_raw = sealed_auth_req("admin", master(), s_nonce, 42);
        let a = frame::decode(&auth_raw).unwrap();
        assert!(matches!(
            server.verify_auth(&a, master()),
            VerifyResult::Accepted { .. }
        ));
        // ...and the successful verify consumed the nonce exactly once.
        assert_eq!(server.take_server_nonce(), None);
    }

    #[test]
    fn pending_handshake_does_not_disturb_active_session() {
        let mut server = ServerSession::new();
        let (s_nonce, _) = server_resp(&mut server, 1, master());
        let c_nonce = 1u64;
        let auth_raw = sealed_auth_req("admin", master(), s_nonce, c_nonce);
        let l = frame::decode(&auth_raw).unwrap();
        assert!(matches!(
            server.verify_auth(&l, master()),
            VerifyResult::Accepted { .. }
        ));
        assert_eq!(server.session_id(), Some(1));

        // A new DISCOVER starts a handshake but must not kill the session.
        server.begin_discover(2, "router", &[], master());
        assert_eq!(server.session_id(), Some(1));

        // An old AUTH_REQ under the replaced nonce no longer opens: it is
        // unauthentic, and the fresh pending nonce survives untouched.
        let stale = frame::decode(&auth_raw).unwrap();
        assert!(matches!(
            server.verify_auth(&stale, master()),
            VerifyResult::Unauthentic(_)
        ));
        assert_eq!(server.session_id(), Some(1));
    }
}
