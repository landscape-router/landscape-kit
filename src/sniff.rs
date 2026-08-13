//! Live and offline capture/decode helpers, used by `lndp-server sniff`.

use std::error::Error;
use std::time::Instant;

use landscape_proto::protocol;
use landscape_proto::transport::{fmt_mac, Frame, Link};

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
                if total % 50 == 0 {
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

pub fn run_offline(
    cap: &mut pcap::Capture<pcap::Offline>,
    ethertype: u16,
) -> Result<(), Box<dyn Error>> {
    let start = Instant::now();
    let mut total = 0u64;
    loop {
        match cap.next_packet() {
            Ok(pkt) => {
                if let Some(f) = Frame::from_raw(&pkt.data) {
                    if f.ethertype == ethertype {
                        total += 1;
                        parse_and_print(&f, None);
                    }
                }
            }
            Err(pcap::Error::TimeoutExpired) => continue,
            Err(_) => break,
        }
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
            "{prefix}    LNDP v{} type={}(0x{:02x}) session=0x{:08x} payload={}B",
            protocol::VERSION,
            protocol::frame::type_name(l.msg_type),
            l.msg_type,
            l.session_id,
            l.payload.len()
        ),
        Err(e) => println!("{prefix}    not an LNDP frame: {e}"),
    }
}
