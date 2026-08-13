//! Linux link layer: AF_PACKET raw sockets, no libpcap at runtime.
//!
//! A `Link` can cover:
//! - one device (default), one socket bound to that interface;
//! - a set of devices (`--dev eth0,eth1`), one socket per interface, polled
//!   together;
//! - `any`, one socket bound to ifindex 0 which receives from every interface.
//!
//! A background reader thread polls the sockets and pushes frames into a tokio
//! channel, so `recv`/`recv_with_meta` are async. Sending stays synchronous
//! (fast syscalls) and can route replies out a specific interface (`send_on`).

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::io::{self, ErrorKind};
use std::mem;
use std::os::unix::io::RawFd;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

use super::Frame;

const POLL_INTERVAL_MS: i32 = 200;

pub struct Link {
    fds: Vec<RawFd>,
    ifindexes: Vec<i32>,
    names: Vec<String>,
    macs: Arc<Mutex<HashMap<i32, [u8; 6]>>>,
    mac_override: Option<[u8; 6]>,
    rx: mpsc::Receiver<(Frame, i32)>,
}

impl Link {
    pub fn open(
        devs: &[String],
        ethertype: u16,
        mac_override: Option<[u8; 6]>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let devs = if devs.is_empty() {
            vec![default_interface()?]
        } else {
            devs.to_vec()
        };
        if devs.iter().any(|d| d == "any") && devs.len() != 1 {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "'any' cannot be combined with other devices",
            )
            .into());
        }

        let mut fds = Vec::new();
        let mut ifindexes = Vec::new();
        for dev in &devs {
            let ifindex = if dev == "any" {
                0
            } else {
                let cname = CString::new(dev.as_str())?;
                let idx = unsafe { libc::if_nametoindex(cname.as_ptr()) };
                if idx == 0 {
                    return Err(io::Error::new(
                        ErrorKind::NotFound,
                        format!("interface not found: {dev}"),
                    )
                    .into());
                }
                idx as i32
            };
            let fd = unsafe {
                libc::socket(
                    libc::AF_PACKET,
                    libc::SOCK_RAW,
                    (libc::ETH_P_ALL as u16).to_be() as libc::c_int,
                )
            };
            if fd < 0 {
                return Err(io::Error::last_os_error().into());
            }
            let sockaddr = libc::sockaddr_ll {
                sll_family: libc::AF_PACKET as u16,
                sll_protocol: (libc::ETH_P_ALL as u16).to_be(),
                sll_ifindex: ifindex,
                sll_hatype: 0,
                sll_pkttype: 0,
                sll_halen: 0,
                sll_addr: [0; 8],
            };
            let res = unsafe {
                libc::bind(
                    fd,
                    &sockaddr as *const libc::sockaddr_ll as *const libc::sockaddr,
                    mem::size_of::<libc::sockaddr_ll>() as libc::socklen_t,
                )
            };
            if res != 0 {
                let err = io::Error::last_os_error();
                unsafe { libc::close(fd) };
                return Err(err.into());
            }
            let tv = libc::timeval {
                tv_sec: 0,
                tv_usec: 500_000,
            };
            unsafe {
                libc::setsockopt(
                    fd,
                    libc::SOL_SOCKET,
                    libc::SO_RCVTIMEO,
                    &tv as *const libc::timeval as *const libc::c_void,
                    mem::size_of::<libc::timeval>() as libc::socklen_t,
                );
            }
            fds.push(fd);
            ifindexes.push(ifindex);
        }

        let mut macs = HashMap::new();
        if let Some(m) = mac_override {
            for idx in &ifindexes {
                macs.insert(*idx, m);
            }
        }
        for idx in &ifindexes {
            if *idx != 0 && !macs.contains_key(idx) {
                if let Ok(m) = read_mac_from_ifindex(*idx) {
                    macs.insert(*idx, m);
                }
            }
        }
        let macs = Arc::new(Mutex::new(macs));

        let (frame_tx, frame_rx) = mpsc::channel(1024);
        spawn_reader(fds.clone(), macs.clone(), ethertype, frame_tx);

        Ok(Self {
            fds,
            ifindexes,
            names: devs,
            macs,
            mac_override,
            rx: frame_rx,
        })
    }

    pub fn local_mac(&self) -> Option<[u8; 6]> {
        self.macs.lock().unwrap().get(&self.ifindexes[0]).copied()
    }

    /// The device names as given on the command line (`any`, `eth0`, ...).
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// Name of the interface a received frame came from.
    pub fn ifname(&self, ifindex: i32) -> String {
        if let Ok(name) = ifname_from_ifindex(ifindex) {
            return name;
        }
        self.names
            .get(self.ifindexes.iter().position(|&i| i == ifindex).unwrap_or(0))
            .cloned()
            .unwrap_or_else(|| format!("#{ifindex}"))
    }

    /// Send on the primary device (ifindex 0 = `any` is rejected).
    pub fn send(
        &mut self,
        dst: &[u8; 6],
        ethertype: u16,
        payload: &[u8],
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.send_on(self.ifindexes[0], dst, ethertype, payload)
    }

    /// Send a frame out the given interface. Used to reply on the interface a
    /// request arrived on (`any` mode and multi-device mode).
    pub fn send_on(
        &mut self,
        ifindex: i32,
        dst: &[u8; 6],
        ethertype: u16,
        payload: &[u8],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let pos = self
            .ifindexes
            .iter()
            .position(|&i| i == ifindex)
            .or_else(|| if self.ifindexes == [0] { Some(0) } else { None })
            .ok_or_else(|| {
                io::Error::new(
                    ErrorKind::InvalidInput,
                    format!("unknown interface index {ifindex}"),
                )
            })?;
        if ifindex == 0 {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "cannot send without a source interface (use a concrete device)",
            )
            .into());
        }
        let src = {
            let mut macs = self.macs.lock().unwrap();
            match macs.get(&ifindex).copied() {
                Some(m) => m,
                None => {
                    let m = self
                        .mac_override
                        .or_else(|| read_mac_from_ifindex(ifindex).ok())
                        .ok_or_else(|| {
                            io::Error::new(
                                ErrorKind::NotFound,
                                format!(
                                    "cannot determine local MAC of {}, use --mac",
                                    self.ifname(ifindex)
                                ),
                            )
                        })?;
                    macs.insert(ifindex, m);
                    m
                }
            }
        };
        let mut frame = Vec::with_capacity(14 + payload.len());
        frame.extend_from_slice(dst);
        frame.extend_from_slice(&src);
        frame.extend_from_slice(&ethertype.to_be_bytes());
        frame.extend_from_slice(payload);
        let sockaddr = libc::sockaddr_ll {
            sll_family: libc::AF_PACKET as u16,
            sll_protocol: ethertype.to_be(),
            sll_ifindex: ifindex,
            sll_hatype: 0,
            sll_pkttype: 0,
            sll_halen: 6,
            sll_addr: {
                let mut a = [0u8; 8];
                a[..6].copy_from_slice(dst);
                a
            },
        };
        let n = unsafe {
            libc::sendto(
                self.fds[pos],
                frame.as_ptr() as *const libc::c_void,
                frame.len(),
                0,
                &sockaddr as *const libc::sockaddr_ll as *const libc::sockaddr,
                mem::size_of::<libc::sockaddr_ll>() as libc::socklen_t,
            )
        };
        if n < 0 {
            return Err(io::Error::last_os_error().into());
        }
        Ok(())
    }

    /// Wait for a frame with the given ethertype.
    pub async fn recv(
        &mut self,
        ethertype: u16,
    ) -> Result<Frame, Box<dyn std::error::Error>> {
        let (f, _) = self.recv_with_meta(ethertype).await?;
        Ok(f)
    }

    /// Like `recv`, but also returns the ifindex of the interface the frame
    /// arrived on. Self-sent frames are skipped by the reader thread.
    pub async fn recv_with_meta(
        &mut self,
        ethertype: u16,
    ) -> Result<(Frame, i32), Box<dyn std::error::Error>> {
        loop {
            match self.rx.recv().await {
                Some((f, ifindex)) if f.ethertype == ethertype => return Ok((f, ifindex)),
                Some(_) => {}
                None => {
                    return Err(io::Error::new(ErrorKind::NotConnected, "link closed").into());
                }
            }
        }
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        for &fd in &self.fds {
            unsafe { libc::close(fd) };
        }
    }
}

/// Background thread: poll all sockets and push matching frames into the
/// channel. Exits when the fds are closed (POLLNVAL) or the channel is gone.
fn spawn_reader(
    fds: Vec<RawFd>,
    macs: Arc<Mutex<HashMap<i32, [u8; 6]>>>,
    ethertype: u16,
    tx: mpsc::Sender<(Frame, i32)>,
) {
    std::thread::spawn(move || {
        let mut pfds: Vec<libc::pollfd> = fds
            .iter()
            .map(|&fd| libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            })
            .collect();
        loop {
            let n = unsafe { libc::poll(pfds.as_mut_ptr(), pfds.len() as libc::nfds_t, POLL_INTERVAL_MS) };
            if n < 0 {
                if io::Error::last_os_error().kind() == ErrorKind::Interrupted {
                    continue;
                }
                return;
            }
            if n == 0 {
                continue;
            }
            for (i, pfd) in pfds.iter().enumerate() {
                if pfd.revents & libc::POLLNVAL != 0 {
                    return;
                }
                if pfd.revents & libc::POLLIN == 0 {
                    continue;
                }
                let own = macs.lock().unwrap();
                match recv_one(fds[i], &own, ethertype) {
                    Ok(Some((f, ifindex))) => {
                        drop(own);
                        if tx.blocking_send((f, ifindex)).is_err() {
                            return;
                        }
                    }
                    Ok(None) => {}
                    Err(_) => {}
                }
            }
        }
    });
}

/// Receive one frame from `fd`, skipping wrong ethertypes, self-sent frames
/// (`PACKET_OUTGOING`) and frames whose src MAC is one of our own interfaces
/// (re-entrant copies, e.g. via a veth peer or a switch port).
fn recv_one(
    fd: RawFd,
    own_macs: &HashMap<i32, [u8; 6]>,
    ethertype: u16,
) -> Result<Option<(Frame, i32)>, Box<dyn std::error::Error>> {
    let mut buf = [0u8; 65536];
    let mut sa: libc::sockaddr_ll = unsafe { mem::zeroed() };
    let mut slen = mem::size_of::<libc::sockaddr_ll>() as libc::socklen_t;
    let n = unsafe {
        libc::recvfrom(
            fd,
            buf.as_mut_ptr() as *mut libc::c_void,
            buf.len(),
            0,
            &mut sa as *mut libc::sockaddr_ll as *mut libc::sockaddr,
            &mut slen,
        )
    };
    if n < 0 {
        return match io::Error::last_os_error().kind() {
            ErrorKind::WouldBlock | ErrorKind::TimedOut => Ok(None),
            kind => Err(io::Error::new(kind, "af_packet recv failed").into()),
        };
    }
    let Some(f) = Frame::from_raw(&buf[..n as usize]) else {
        return Ok(None);
    };
    if f.ethertype != ethertype {
        return Ok(None);
    }
    if sa.sll_pkttype == libc::PACKET_OUTGOING {
        return Ok(None);
    }
    if own_macs.values().any(|m| *m == f.src) {
        return Ok(None);
    }
    Ok(Some((f, sa.sll_ifindex)))
}

/// First interface with a default route, else the first non-loopback one.
fn default_interface() -> io::Result<String> {
    let route = std::fs::read_to_string("/proc/net/route")?;
    for line in route.lines().skip(1) {
        let mut fields = line.split_whitespace();
        let name = fields.next().unwrap_or("").to_string();
        let dest = fields.next().unwrap_or("");
        if dest == "00000000" && !name.is_empty() {
            return Ok(name);
        }
    }
    for entry in std::fs::read_dir("/sys/class/net")? {
        let name = entry?.file_name().to_string_lossy().into_owned();
        if name != "lo" {
            return Ok(name);
        }
    }
    Err(io::Error::new(ErrorKind::NotFound, "no network interface found"))
}

fn ifname_from_ifindex(ifindex: i32) -> io::Result<String> {
    let mut buf = [0u8; libc::IFNAMSIZ];
    let p = buf.as_mut_ptr() as *mut libc::c_char;
    if unsafe { libc::if_indextoname(ifindex as libc::c_uint, p) }.is_null() {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
}

fn read_mac_from_ifindex(ifindex: i32) -> io::Result<[u8; 6]> {
    read_mac_from_sysfs(&ifname_from_ifindex(ifindex)?)
}

fn read_mac_from_sysfs(dev: &str) -> io::Result<[u8; 6]> {
    let raw = std::fs::read_to_string(format!("/sys/class/net/{dev}/address"))?;
    let parts: Vec<&str> = raw.trim().split(':').collect();
    if parts.len() != 6 {
        return Err(io::Error::new(ErrorKind::InvalidData, "bad mac in sysfs"));
    }
    let mut mac = [0u8; 6];
    for (i, p) in parts.iter().enumerate() {
        mac[i] = u8::from_str_radix(p, 16)
            .map_err(|_| io::Error::new(ErrorKind::InvalidData, "bad mac in sysfs"))?;
    }
    Ok(mac)
}
