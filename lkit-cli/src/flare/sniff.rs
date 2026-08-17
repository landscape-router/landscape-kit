//! Live and offline capture/decode helpers, used by `lkit flare sniff`.

use std::error::Error;
use std::time::Instant;

use landscape_terrain_proto::protocol;
use landscape_terrain_proto::transport::{Frame, Link, fmt_mac};

pub fn filter_expr(ethertype: u16) -> String {
    format!("ether proto {:#x}", ethertype)
}

pub fn list_devices() -> Result<(), Box<dyn Error>> {
    for (i, e) in std::fs::read_dir("/sys/class/net")?.enumerate() {
        println!("[{}] {}", i, e?.file_name().to_string_lossy());
    }
    Ok(())
}

pub async fn run_live(link: &mut Link, ethertype: u16) -> Result<(), Box<dyn Error>> {
    let start = Instant::now();
    let mut total = 0u64;
    loop {
        match link.recv_with_meta(ethertype).await {
            Ok((f, ifindex)) => {
                total += 1;
                parse_and_print(&f, Some(&link.ifname(ifindex)));
                if total.is_multiple_of(50) {
                    println!(
                        "[stats] {} packets, {:.1}s elapsed",
                        total,
                        start.elapsed().as_secs_f32()
                    );
                }
            }
            Err(e) => return Err(e),
        }
    }
}

pub fn run_offline(path: &std::path::Path, ethertype: u16) -> Result<(), Box<dyn Error>> {
    let start = Instant::now();
    let mut total = 0u64;
    // Minimal .pcap reader: 24-byte global header, then 16-byte record
    // headers (ts_sec ts_usec incl_len orig_len) followed by the frame
    // bytes. No libpcap runtime dependency.
    let data = std::fs::read(path)?;
    if data.len() < 24 {
        return Err("not a pcap file: shorter than the 24-byte global header".into());
    }
    let mut off = 24usize;
    while off + 16 <= data.len() {
        let incl_len = u32::from_be_bytes(data[off + 8..off + 12].try_into().unwrap()) as usize;
        off += 16;
        if incl_len == 0 || off + incl_len > data.len() {
            break;
        }
        if let Some(f) = Frame::from_raw(&data[off..off + incl_len])
            && f.ethertype == ethertype
        {
            total += 1;
            parse_and_print(&f, None);
        }
        off += incl_len;
    }
    println!(
        "done: {} packets, {:.1}s",
        total,
        start.elapsed().as_secs_f32()
    );
    Ok(())
}

pub fn parse_and_print(f: &Frame, dev: Option<&str>) {
    let prefix = match dev {
        Some(d) => format!("[{d}] "),
        None => String::new(),
    };
    println!(
        "{prefix}{} -> {} vlan={}",
        fmt_mac(&f.src),
        fmt_mac(&f.dst),
        f.vlan_id.map_or("none".to_string(), |v| v.to_string()),
    );
    match protocol::frame::decode(&f.payload) {
        Ok(l) => println!(
            "{prefix}    Terrain v{} type={}(0x{:02x}) session=0x{:08x} seq={} payload={}B{}",
            protocol::VERSION,
            protocol::frame::type_name(l.msg_type),
            l.msg_type,
            l.session_id,
            l.seq,
            l.payload.len(),
            if matches!(
                l.msg_type,
                protocol::TYPE_DISCOVER
                    | protocol::TYPE_RESP
                    | protocol::TYPE_DATA
                    | protocol::TYPE_KEEPALIVE
                    | protocol::TYPE_TEARDOWN
                    | protocol::TYPE_AUTH_REQ
                    | protocol::TYPE_AUTH_ACK
                    | protocol::TYPE_AUTH_NACK
            ) {
                " (sealed)"
            } else {
                ""
            }
        ),
        Err(e) => println!("{prefix}    not a Terrain frame: {e}"),
    }
}
