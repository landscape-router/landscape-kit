//! Windows/other link layer: libpcap (Npcap).
//!
//! A background reader thread polls the captures and pushes frames into a
//! tokio channel, so `recv`/`recv_with_meta` are async. Multiple devices are
//! supported (one capture each); `any` (all interfaces) is not available on
//! this platform.

use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pcap::{Capture, Device};
use tokio::sync::mpsc;

use super::{Frame, Interface};

const CAPTURE_TIMEOUT_MS: i32 = 500;
const MULTI_TIMEOUT_MS: i32 = 100;

/// Non-loopback interfaces available on this host (used by the TUI picker).
pub fn list_interface_details() -> Result<Vec<Interface>, Box<dyn std::error::Error>> {
    Ok(Device::list()?
        .into_iter()
        .filter(|d| !d.flags.is_loopback())
        .map(|d| Interface {
            name: d.name,
            description: d.desc,
        })
        .collect())
}

pub struct Link {
    caps: Arc<Mutex<Vec<Capture<pcap::Active>>>>,
    names: Vec<String>,
    local_mac: Option<[u8; 6]>,
    rx: mpsc::Receiver<(Frame, i32)>,
}

impl Link {
    pub fn open(
        devs: &[String],
        ethertype: u16,
        mac_override: Option<[u8; 6]>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        if devs.iter().any(|d| d == "any") {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "'any' (all interfaces) is not supported on this platform, list the devices explicitly",
            )
            .into());
        }
        let names: Vec<String> = if devs.is_empty() {
            let list = Device::list()?;
            let name = Device::lookup()?
                .map(|d| d.name)
                .or_else(|| {
                    list.into_iter()
                        .find(|d| !d.flags.is_loopback())
                        .map(|d| d.name)
                })
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no non-loopback device"))?;
            vec![name]
        } else {
            devs.to_vec()
        };
        let timeout = if names.len() == 1 {
            CAPTURE_TIMEOUT_MS
        } else {
            MULTI_TIMEOUT_MS
        };
        let list = Device::list()?;
        let mut caps = Vec::new();
        for name in &names {
            let dev = list
                .iter()
                .find(|d| d.name == *name)
                .cloned()
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::NotFound,
                        format!("device not found: {name} (run --list to see available devices)"),
                    )
                })?;
            let cap = Capture::from_device(dev)?
                .promisc(true)
                .timeout(timeout)
                .immediate_mode(true)
                .open()?;
            // Non-blocking captures: `next_packet` never blocks. On Windows a
            // blocking `pcap_next_ex` in immediate mode can wait forever, and
            // doing that while holding the capture lock deadlocks the send
            // path, which needs the same lock.
            let mut cap = cap.setnonblock()?;
            cap.filter(&format!("ether proto {:#x}", ethertype), true)?;
            caps.push(cap);
        }
        let local_mac = match mac_override {
            Some(m) => Some(m),
            None => mac_address::get_mac_address()?.map(|m| m.bytes()),
        };
        let caps = Arc::new(Mutex::new(caps));
        let (frame_tx, frame_rx) = mpsc::channel(1024);
        spawn_reader(caps.clone(), local_mac, ethertype, frame_tx);
        Ok(Self {
            caps,
            names,
            local_mac,
            rx: frame_rx,
        })
    }

    pub fn local_mac(&self) -> Option<[u8; 6]> {
        self.local_mac
    }

    /// The device names as given on the command line.
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// Name of the device a received frame came from (index within `names`).
    pub fn ifname(&self, ifindex: i32) -> String {
        self.names
            .get(ifindex as usize)
            .cloned()
            .unwrap_or_else(|| format!("#{ifindex}"))
    }

    pub fn send(
        &mut self,
        dst: &[u8; 6],
        ethertype: u16,
        payload: &[u8],
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.send_on(0, dst, ethertype, payload)
    }

    /// Send on the capture at position `ifindex` in the device list.
    pub fn send_on(
        &mut self,
        ifindex: i32,
        dst: &[u8; 6],
        ethertype: u16,
        payload: &[u8],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut caps = self.caps.lock().unwrap();
        let cap = caps.get_mut(ifindex as usize).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unknown interface index {ifindex}"),
            )
        })?;
        let src = self.local_mac.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "cannot determine local MAC address, use --mac",
            )
        })?;
        let mut buf = Vec::with_capacity(14 + payload.len());
        buf.extend_from_slice(dst);
        buf.extend_from_slice(&src);
        buf.extend_from_slice(&ethertype.to_be_bytes());
        buf.extend_from_slice(payload);
        Ok(cap.sendpacket(buf)?)
    }

    /// Wait for a frame with the given ethertype.
    pub async fn recv(&mut self, ethertype: u16) -> Result<Frame, Box<dyn std::error::Error>> {
        let (f, _) = self.recv_with_meta(ethertype).await?;
        Ok(f)
    }

    /// Like `recv`, but also returns the index of the device the frame came
    /// from within the device list.
    pub async fn recv_with_meta(
        &mut self,
        ethertype: u16,
    ) -> Result<(Frame, i32), Box<dyn std::error::Error>> {
        loop {
            match self.rx.recv().await {
                Some((f, ifindex)) if f.ethertype == ethertype => return Ok((f, ifindex)),
                Some(_) => {}
                None => {
                    return Err(io::Error::new(io::ErrorKind::NotConnected, "link closed").into());
                }
            }
        }
    }
}

/// Background thread: round-robin poll the captures and push matching frames
/// into the channel. Exits when a capture fails (e.g. closed) or the channel
/// is gone.
fn spawn_reader(
    caps: Arc<Mutex<Vec<Capture<pcap::Active>>>>,
    local_mac: Option<[u8; 6]>,
    ethertype: u16,
    tx: mpsc::Sender<(Frame, i32)>,
) {
    // Non-blocking poll loop (the captures were opened with `setnonblock`).
    // The lock is held only for the (never-blocking) `next_packet` calls, so
    // the sender can always acquire it. A short sleep on an empty round
    // bounds CPU use; `idle` skips the sleep while frames are flowing.
    const POLL_INTERVAL: Duration = Duration::from_millis(2);
    std::thread::spawn(move || {
        loop {
            let mut caps = caps.lock().unwrap();
            let mut idle = true;
            for (i, cap) in caps.iter_mut().enumerate() {
                match cap.next_packet() {
                    Ok(pkt) => {
                        idle = false;
                        if let Some(f) = Frame::from_raw(&pkt.data) {
                            if f.ethertype == ethertype && local_mac.is_none_or(|m| f.src != m) {
                                if tx.blocking_send((f, i as i32)).is_err() {
                                    return;
                                }
                            }
                        }
                    }
                    Err(pcap::Error::TimeoutExpired) => {}
                    Err(_) => return,
                }
            }
            drop(caps);
            if idle {
                std::thread::sleep(POLL_INTERVAL);
            }
        }
    });
}
