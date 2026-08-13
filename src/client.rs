use std::time::Duration;

use landscape_proto::protocol::frame::{self, Frame as LndpFrame};
use landscape_proto::protocol::session::{
    ClientSession, HANDSHAKE_TIMEOUT, KEEPALIVE_INTERVAL, MAX_RETRIES,
};
use landscape_proto::protocol::{TYPE_AUTH_ACK, TYPE_AUTH_NACK, TYPE_DATA, TYPE_KEEPALIVE, TYPE_RESP};
use landscape_proto::transport::{fmt_mac, Frame, Link};

pub const BROADCAST: [u8; 6] = [0xff; 6];
const RETRY_BACKOFF: Duration = Duration::from_secs(3);
const MAX_MISSED_KEEPALIVES: u32 = 3;

pub struct ClientConfig<'a> {
    pub devs: &'a [String],
    pub ethertype: u16,
    pub mac: Option<[u8; 6]>,
    pub user: &'a str,
    pub psk: &'a str,
    pub client_name: &'a str,
}

pub async fn run(cfg: &ClientConfig<'_>) -> Result<(), Box<dyn std::error::Error>> {
    let mut tx = Link::open(cfg.devs, cfg.ethertype, cfg.mac)?;
    println!(
        "client '{}' ready on {} (ethertype 0x{:04x})",
        cfg.client_name,
        tx.names().join(", "),
        cfg.ethertype
    );

    loop {
        let mut sess = ClientSession::new();
        let Some(server_mac) = handshake(&mut tx, &mut sess, cfg).await? else {
            println!("handshake failed, retrying in {}s", RETRY_BACKOFF.as_secs());
            tokio::time::sleep(RETRY_BACKOFF).await;
            continue;
        };
        let sid = sess.session_id().expect("session id after handshake");
        println!("session {sid} established with {}", fmt_mac(&server_mac));

        match session_loop(&mut tx, &server_mac, sid, cfg.ethertype).await {
            Ok(true) => println!("  link lost, restarting handshake"),
            Ok(false) => println!("  session closed by peer"),
            Err(e) => eprintln!("  session error: {e}"),
        }
    }
}

async fn handshake(
    tx: &mut Link,
    sess: &mut ClientSession,
    cfg: &ClientConfig<'_>,
) -> Result<Option<[u8; 6]>, Box<dyn std::error::Error>> {
    while sess.retransmit_allowed() {
        sess.bump_retry();
        println!("discover: broadcast (try {}/{MAX_RETRIES})", sess.retries());
        tx.send(&BROADCAST, cfg.ethertype, &frame::encode_discover(cfg.client_name))?;

        let deadline = tokio::time::Instant::now() + HANDSHAKE_TIMEOUT;
        let Some(resp_frame) =
            recv_until(tx, cfg.ethertype, |l| l.msg_type == TYPE_RESP, deadline).await?
        else {
            println!("  no response within {}s", HANDSHAKE_TIMEOUT.as_secs());
            continue;
        };
        let server_mac = resp_frame.src;
        let resp_lndp = frame::decode(&resp_frame.payload)?;
        let resp = frame::decode_resp(&resp_lndp.payload)?;
        println!(
            "  discovered '{}' at {} (nonce=0x{:08x})",
            resp.device_name,
            fmt_mac(&server_mac),
            resp.nonce
        );

        let auth_req = sess.on_resp(&resp, cfg.user, cfg.psk.as_bytes());
        tx.send(&server_mac, cfg.ethertype, &auth_req)?;
        println!("  auth request sent for user '{}'", cfg.user);

        let deadline = tokio::time::Instant::now() + HANDSHAKE_TIMEOUT;
        let Some(auth_frame) = recv_until(
            tx,
            cfg.ethertype,
            |l| l.msg_type == TYPE_AUTH_ACK || l.msg_type == TYPE_AUTH_NACK,
            deadline,
        )
        .await?
        else {
            println!("  auth timeout, rediscovering");
            continue;
        };
        let auth_lndp = frame::decode(&auth_frame.payload)?;
        sess.on_auth_frame(&auth_lndp);
        if auth_lndp.msg_type == TYPE_AUTH_ACK {
            return Ok(Some(server_mac));
        }
        let reason = frame::decode_auth_nack(&auth_lndp.payload).unwrap_or_default();
        eprintln!("  auth rejected: {reason}");
        return Ok(None);
    }
    Ok(None)
}

/// Block until a frame whose LNDP header satisfies `pred` arrives, or the
/// deadline passes.
async fn recv_until(
    tx: &mut Link,
    ethertype: u16,
    pred: impl Fn(&LndpFrame) -> bool,
    deadline: tokio::time::Instant,
) -> Result<Option<Frame>, Box<dyn std::error::Error>> {
    loop {
        match tokio::time::timeout_at(deadline, tx.recv(ethertype)).await {
            Ok(Ok(f)) => {
                if let Ok(l) = frame::decode(&f.payload) {
                    if pred(&l) {
                        return Ok(Some(f));
                    }
                }
            }
            Ok(Err(e)) => return Err(e),
            Err(_) => return Ok(None),
        }
    }
}

/// Session loop: keepalive every KEEPALIVE_INTERVAL; Ok(true) = link lost,
/// Ok(false) = peer said goodbye.
async fn session_loop(
    tx: &mut Link,
    server_mac: &[u8; 6],
    sid: u32,
    ethertype: u16,
) -> Result<bool, Box<dyn std::error::Error>> {
    let mut last_keepalive = tokio::time::Instant::now();
    let mut missed = 0u32;
    loop {
        tokio::select! {
            r = tx.recv(ethertype) => {
                let f = r?;
                if let Ok(l) = frame::decode(&f.payload) {
                    if l.session_id != sid {
                        println!("  [client] foreign frame from {}", fmt_mac(&f.src));
                        continue;
                    }
                    match l.msg_type {
                        TYPE_KEEPALIVE => missed = 0,
                        TYPE_DATA => println!(
                            "  [client] DATA {}B (tunnel not implemented yet)",
                            l.payload.len()
                        ),
                        t => println!("  [client] {} from {}", frame::type_name(t), fmt_mac(&f.src)),
                    }
                }
            }
            _ = tokio::time::sleep_until(last_keepalive + KEEPALIVE_INTERVAL) => {
                tx.send(server_mac, ethertype, &frame::encode_keepalive(sid))?;
                last_keepalive = tokio::time::Instant::now();
                missed += 1;
                if missed >= MAX_MISSED_KEEPALIVES {
                    println!("  no keepalive echo for {missed} rounds, link assumed lost");
                    return Ok(true);
                }
            }
        }
    }
}
