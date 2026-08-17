//! Transport layer: shared ethernet frame types + platform link implementations.
//!
//! - Linux: raw AF_PACKET sockets, no libpcap at runtime (`linux.rs`)
//! - others (Windows/macOS): libpcap (`windows.rs`)
//!
//! Each platform file exports a `Link` type with the same API, selected here
//! via conditional compilation.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::Link;

#[cfg(not(target_os = "linux"))]
mod windows;
#[cfg(not(target_os = "linux"))]
pub use windows::Link;

#[allow(dead_code)]
pub const ETHERTYPE: u16 = 0x88B6;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub dst: [u8; 6],
    pub src: [u8; 6],
    pub vlan_id: Option<u16>,
    pub ethertype: u16,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn from_raw(raw: &[u8]) -> Option<Self> {
        EthernetFrame::parse(raw).map(|f| f.to_owned())
    }
}

pub struct EthernetFrame<'a> {
    pub dst: &'a [u8],
    pub src: &'a [u8],
    pub vlan_id: Option<u16>,
    pub ethertype: u16,
    pub payload: &'a [u8],
}

impl<'a> EthernetFrame<'a> {
    pub fn parse(raw: &'a [u8]) -> Option<Self> {
        if raw.len() < 14 {
            return None;
        }
        let dst = &raw[0..6];
        let src = &raw[6..12];
        let mut ethertype = u16::from_be_bytes([raw[12], raw[13]]);
        let mut offset = 14;
        let mut vlan_id = None;
        while matches!(ethertype, 0x8100 | 0x88a8 | 0x9100) {
            if raw.len() < offset + 4 {
                return None;
            }
            vlan_id = Some(u16::from_be_bytes([raw[offset], raw[offset + 1]]) & 0x0fff);
            ethertype = u16::from_be_bytes([raw[offset + 2], raw[offset + 3]]);
            offset += 4;
        }
        Some(Self {
            dst,
            src,
            vlan_id,
            ethertype,
            payload: &raw[offset..],
        })
    }

    pub fn to_owned(&self) -> Frame {
        let mut dst = [0u8; 6];
        dst.copy_from_slice(self.dst);
        let mut src = [0u8; 6];
        src.copy_from_slice(self.src);
        Frame {
            dst,
            src,
            vlan_id: self.vlan_id,
            ethertype: self.ethertype,
            payload: self.payload.to_vec(),
        }
    }

    #[allow(dead_code)]
    pub fn is_landscape(&self) -> bool {
        self.ethertype == ETHERTYPE
    }
}

pub fn fmt_mac(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_landscape_frame() {
        let mut raw = Vec::new();
        raw.extend_from_slice(&[0x00, 0x1a, 0x2b, 0x3c, 0x4d, 0x5e]); // dst
        raw.extend_from_slice(&[0x00, 0xaa, 0xbb, 0xcc, 0xdd, 0xee]); // src
        raw.extend_from_slice(&[0x88, 0xb6]); // ethertype landscape
        raw.extend_from_slice(&[1, 2, 3]); // payload

        let frame = EthernetFrame::parse(&raw).expect("frame");
        assert!(frame.is_landscape());
        assert_eq!(frame.vlan_id, None);
        assert_eq!(frame.payload, &[1, 2, 3]);

        let owned = frame.to_owned();
        assert_eq!(owned.payload, vec![1, 2, 3]);
        assert_eq!(
            Frame::from_raw(&raw).expect("owned").src,
            [0x00, 0xaa, 0xbb, 0xcc, 0xdd, 0xee]
        );
    }

    #[test]
    fn parse_vlan_frame() {
        let mut raw = Vec::new();
        raw.extend_from_slice(&[0xff; 6]);
        raw.extend_from_slice(&[0x01; 6]);
        raw.extend_from_slice(&[0x81, 0x00, 0x00, 0x64, 0x88, 0xb6]); // vlan 100 + ethertype
        raw.extend_from_slice(&[0xaa]);

        let frame = EthernetFrame::parse(&raw).expect("frame");
        assert_eq!(frame.vlan_id, Some(100));
        assert!(frame.is_landscape());
        assert_eq!(frame.payload, &[0xaa]);
    }

    #[test]
    fn rejects_short_frames() {
        assert!(EthernetFrame::parse(&[0; 13]).is_none());
    }
}
