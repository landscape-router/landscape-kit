use landscape_terrain_proto::protocol::{frame, v6};
use zeroize::Zeroizing;

pub(super) struct Pending {
    pub sid: u32,
    pub interface: i32,
    pub welcome: Vec<u8>,
    pub channel: v6::Transport,
    pub reply: LoginReply,
    parts: Vec<Option<Zeroizing<Vec<u8>>>>,
}

impl Pending {
    pub fn new(sid: u32, interface: i32, hello: &[u8], identity: &[u8; 32]) -> Option<Self> {
        let mut noise = v6::handshake(sid, Some(identity)).ok()?;
        v6::read_handshake(&mut noise, hello).ok()?;
        let welcome = v6::packet(v6::WELCOME, sid, 0, &v6::write_handshake(&mut noise).ok()?);
        Some(Self {
            sid,
            interface,
            welcome,
            channel: v6::Transport::new(noise).ok()?,
            reply: LoginReply::default(),
            parts: Vec::new(),
        })
    }

    pub fn credentials(&mut self, frame: &frame::Frame<'_>) -> Option<Zeroizing<Vec<u8>>> {
        let plain = Zeroizing::new(self.channel.open(frame)?);
        let (&index, rest) = plain.split_first()?;
        let (&count, part) = rest.split_first()?;
        let count = usize::from(count);
        let index = usize::from(index);
        if count == 0
            || count > v6::MAX_CREDENTIAL_BYTES.div_ceil(v6::AUTH_CHUNK)
            || index >= count
            || part.len() > v6::AUTH_CHUNK
        {
            return None;
        }
        if self.parts.is_empty() {
            self.parts.resize_with(count, || None);
        }
        if self.parts.len() != count {
            return None;
        }
        self.parts[index] = Some(Zeroizing::new(part.to_vec()));
        if self.parts.iter().any(Option::is_none) {
            return None;
        }
        let mut all = Zeroizing::new(Vec::new());
        for part in self.parts.drain(..).flatten() {
            all.extend_from_slice(&part);
        }
        self.reply.seq = frame.seq;
        self.reply.request = frame.payload.to_vec();
        (all.len() <= v6::MAX_CREDENTIAL_BYTES).then_some(all)
    }
}

/// Only an exact retransmission of the final authenticated fragment receives the cached reply.
#[derive(Default)]
pub(super) struct LoginReply {
    seq: u32,
    request: Vec<u8>,
    pub encrypted: Vec<u8>,
    pub interface: i32,
    pub sid: u32,
}
impl LoginReply {
    pub fn matches(&self, frame: &frame::Frame<'_>) -> bool {
        self.sid == frame.session_id && self.seq == frame.seq && self.request == frame.payload
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragmented_login_and_cached_response_survive_loss_and_reordering() {
        let mut noise = v6::handshake(42, None).unwrap();
        let hello = v6::write_handshake(&mut noise).unwrap();
        let mut login = Pending::new(42, 3, &hello, &[4; 32]).unwrap();
        v6::read_handshake(&mut noise, v6::decode(&login.welcome).unwrap().payload).unwrap();
        let mut client = v6::Transport::new(noise).unwrap();
        let end = client.seal(frame::TYPE_AUTH_REQ, 42, &[1, 2, 99]).unwrap();
        let start = client.seal(frame::TYPE_AUTH_REQ, 42, &[0, 2, 11]).unwrap();
        let end = v6::decode(&end).unwrap();
        let start = v6::decode(&start).unwrap();
        assert!(login.credentials(&end).is_none());
        assert!(login.credentials(&end).is_none());
        assert_eq!(&**login.credentials(&start).unwrap(), &[11, 99]);
        login.reply.sid = 42;
        assert!(login.reply.matches(&start));
        assert!(!login.reply.matches(&end));
    }
}
