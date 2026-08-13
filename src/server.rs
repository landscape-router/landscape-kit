use std::collections::HashMap;

use landscape_proto::protocol::frame;
use landscape_proto::protocol::session::{ServerSession, VerifyResult};
use landscape_proto::protocol::{
    TYPE_AUTH_REQ, TYPE_DATA, TYPE_DISCOVER, TYPE_KEEPALIVE,
};
use landscape_proto::transport::{fmt_mac, Link};

pub struct ServerConfig<'a> {
    pub devs: &'a [String],
    pub ethertype: u16,
    pub mac: Option<[u8; 6]>,
    pub psk: &'a str,
    pub device_name: &'a str,
}

/// Per-peer connection state, keyed by the client's MAC address.
struct Peer {
    sess: ServerSession,
    ifindex: i32,
}

pub async fn run(cfg: &ServerConfig<'_>) -> Result<(), Box<dyn std::error::Error>> {
    let mut tx = Link::open(cfg.devs, cfg.ethertype, cfg.mac)?;
    let mut peers: HashMap<[u8; 6], Peer> = HashMap::new();
    println!(
        "server '{}' ready on {} (ethertype 0x{:04x})",
        cfg.device_name,
        devs_display(&tx.names()),
        cfg.ethertype
    );

    loop {
        let (f, ifindex) = tx.recv_with_meta(cfg.ethertype).await?;
        let Ok(l) = frame::decode(&f.payload) else {
            continue;
        };
        let mac = f.src;
        match l.msg_type {
            TYPE_DISCOVER => {
                let name = frame::decode_discover(&l.payload).unwrap_or_default();
                let peer = peers.entry(mac).or_insert_with(|| Peer {
                    sess: ServerSession::new(),
                    ifindex: 0,
                });
                peer.sess = ServerSession::new();
                let resp = peer.sess.on_discover(cfg.device_name);
                peer.ifindex = ifindex;
                tx.send_on(ifindex, &mac, cfg.ethertype, &resp)?;
                println!("discover from {} '{}'", fmt_mac(&mac), name);
            }
            TYPE_AUTH_REQ => {
                let Some(peer) = peers.get_mut(&mac) else {
                    println!("auth attempt from unknown peer {}, ignored", fmt_mac(&mac));
                    continue;
                };
                let ifindex = peer.ifindex;
                let Ok(req) = frame::decode_auth_req(&l.payload) else {
                    println!("malformed AUTH_REQ from {}", fmt_mac(&mac));
                    continue;
                };
                match peer.sess.verify_auth(&req, cfg.psk.as_bytes()) {
                    VerifyResult::Accepted(sid) => {
                        tx.send_on(ifindex, &mac, cfg.ethertype, &frame::encode_auth_ack(sid))?;
                        println!(
                            "client {} '{}' authenticated, session {sid}",
                            fmt_mac(&mac),
                            req.user
                        );
                    }
                    VerifyResult::Rejected(reason) => {
                        tx.send_on(
                            ifindex,
                            &mac,
                            cfg.ethertype,
                            &frame::encode_auth_nack(&reason),
                        )?;
                        peers.remove(&mac);
                        println!("auth rejected for {}: {reason}", fmt_mac(&mac));
                    }
                }
            }
            TYPE_KEEPALIVE => {
                if let Some(peer) = peers.get_mut(&mac) {
                    if let Some(sid) = peer.sess.session_id() {
                        tx.send_on(peer.ifindex, &mac, cfg.ethertype, &frame::encode_keepalive(sid))?;
                    }
                }
            }
            TYPE_DATA => {
                if let Some(peer) = peers.get(&mac) {
                    if peer.sess.session_id().is_some() {
                        println!(
                            "  [server] DATA {}B from {} (tunnel not implemented yet)",
                            l.payload.len(),
                            fmt_mac(&mac)
                        );
                    }
                }
            }
            t => println!(
                "  [server] {} from {} ignored",
                frame::type_name(t),
                fmt_mac(&mac)
            ),
        }
    }
}

fn devs_display(names: &[String]) -> String {
    if names.len() == 1 && names[0] == "any" {
        "all interfaces".to_string()
    } else {
        names.join(", ")
    }
}
