use super::{ClientConfig, ClientEvent, LogLevel, SessionEnd, emit_event, session_loop};
use landscape_terrain_proto::{
    protocol::{frame, v6},
    transport::{Link, fmt_mac},
};
use std::{collections::BTreeMap, io, time::Duration};
use zeroize::Zeroizing;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Clone)]
pub struct Device {
    pub name: String,
    pub fingerprint: String,
    pub mac: [u8; 6],
    public: [u8; 32],
}

pub async fn discover(
    devs: &[String],
    eth: u16,
    mac: Option<[u8; 6]>,
    gate: &v6::Discovery,
    token: &str,
) -> Result<Vec<Device>> {
    if devs.len() > 1 || devs.iter().any(|d| d == "any") {
        return Err("select one client interface".into());
    }
    let mut link = Link::open(devs, eth, mac)?;
    let id = rand::random::<u64>();
    let mut query = id.to_be_bytes().to_vec();
    query.extend_from_slice(token.as_bytes());
    let request = gate.encode(frame::TYPE_DISCOVER, 0, &query)?;
    let mut devices = BTreeMap::new();
    for _ in 0..2 {
        link.send(&super::BROADCAST, eth, &request)?;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(550);
        while let Ok(result) = tokio::time::timeout_at(deadline, link.recv(eth)).await {
            let response = result?;
            let Ok(f) = v6::decode(&response.payload) else {
                continue;
            };
            if f.msg_type != frame::TYPE_RESP || f.session_id != 0 {
                continue;
            }
            let Some(body) = gate.open(&f) else { continue };
            record_device(&mut devices, id, response.src, &body);
        }
    }
    Ok(devices.into_values().collect())
}

fn record_device(devices: &mut BTreeMap<String, Device>, id: u64, mac: [u8; 6], body: &[u8]) {
    if body.get(..8) != Some(id.to_be_bytes().as_slice()) || body.len() > 256 {
        return;
    }
    let Some(public) = body.get(8..40) else {
        return;
    };
    let Ok(name) = std::str::from_utf8(&body[40..]) else {
        return;
    };
    if name.chars().any(char::is_control) {
        return;
    }
    let fingerprint = v6::fingerprint(public);
    if devices.len() < 256 {
        devices
            .entry(fingerprint.clone())
            .or_insert_with(|| Device {
                name: name.to_string(),
                fingerprint,
                mac,
                public: public.try_into().expect("32 bytes"),
            });
    }
}

pub struct Connection {
    link: Link,
    eth: u16,
    device: Device,
    sid: u32,
    channel: v6::Transport,
}

pub async fn connect(
    devs: &[String],
    eth: u16,
    mac: Option<[u8; 6]>,
    gate: &v6::Discovery,
    mut device: Device,
) -> Result<Connection> {
    let mut link = Link::open(devs, eth, mac)?;
    let sid = rand::random::<u32>().max(1);
    let mut noise = v6::handshake(sid, None)?;
    let mut hello = device.public.to_vec();
    hello.extend(v6::write_handshake(&mut noise)?);
    let message = gate.encode(v6::HELLO, sid, &hello)?;
    for _ in 0..3 {
        link.send(&device.mac, eth, &message)?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        while let Ok(result) = tokio::time::timeout_at(deadline, link.recv(eth)).await {
            let reply = result?;
            let Ok(f) = v6::decode(&reply.payload) else {
                continue;
            };
            if f.session_id != sid || f.msg_type != v6::WELCOME {
                continue;
            }
            v6::read_handshake(&mut noise, f.payload)?;
            if noise.get_remote_static() != Some(device.public.as_slice()) {
                return Err("device fingerprint changed".into());
            }
            device.mac = reply.src;
            return Ok(Connection {
                link,
                eth,
                device,
                sid,
                channel: v6::Transport::new(noise)?,
            });
        }
    }
    Err("selected device did not complete the Noise handshake".into())
}

impl Connection {
    pub async fn run(
        mut self,
        mut cfg: ClientConfig<'_>,
        method: u8,
        password: Zeroizing<String>,
    ) -> Result<()> {
        if cfg.user.is_empty()
            || cfg.user.contains('\0')
            || password.is_empty()
            || password.contains('\0')
        {
            return Err("invalid credentials".into());
        }
        let mut credentials = Zeroizing::new(vec![method]);
        credentials.extend(cfg.user.as_bytes());
        credentials.push(0);
        credentials.extend(password.as_bytes());
        drop(password);
        if credentials.len() > v6::MAX_CREDENTIAL_BYTES {
            return Err("credentials too long".into());
        }
        let count = credentials.len().div_ceil(v6::AUTH_CHUNK) as u8;
        let mut requests = Vec::with_capacity(count.into());
        for (i, part) in credentials.chunks(v6::AUTH_CHUNK).enumerate() {
            let mut data = Zeroizing::new(vec![i as u8, count]);
            data.extend(part);
            let raw = self.channel.seal(frame::TYPE_AUTH_REQ, self.sid, &data)?;
            requests.push(raw);
        }
        drop(credentials);
        let ports = tokio::time::timeout(Duration::from_secs(12), async {
            let mut retry = tokio::time::interval(Duration::from_secs(2));
            loop {
                let reply = tokio::select! {
                    reply = self.link.recv(self.eth) => reply?,
                    _ = retry.tick() => {
                        for packet in &requests {
                            self.link.send(&self.device.mac, self.eth, packet)?;
                        }
                        continue;
                    }
                };
                let Ok(f) = v6::decode(&reply.payload) else {
                    continue;
                };
                if reply.src != self.device.mac
                    || f.session_id != self.sid
                    || !matches!(f.msg_type, frame::TYPE_AUTH_ACK | frame::TYPE_AUTH_NACK)
                {
                    continue;
                }
                let Some(data) = self.channel.open(&f) else {
                    continue;
                };
                if f.msg_type == frame::TYPE_AUTH_NACK {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "authentication failed",
                    )
                    .into());
                }
                if data.len() % 2 != 0 {
                    return Err("invalid port list".into());
                }
                return Ok::<Vec<u16>, Box<dyn std::error::Error>>(
                    data.chunks_exact(2)
                        .map(|p| u16::from_be_bytes([p[0], p[1]]))
                        .collect(),
                );
            }
        })
        .await
        .map_err(|_| "authentication timed out")??;
        cfg.log.emit(
            LogLevel::Info,
            format!(
                "session {} established with {}",
                self.sid, self.device.fingerprint
            ),
        );
        emit_event(
            &cfg.events,
            ClientEvent::SessionReady {
                session_id: self.sid,
                server_mac: fmt_mac(&self.device.mac),
                advertised_ports: ports.clone(),
            },
        );
        let notify = cfg.shutdown.clone().unwrap_or_default();
        let signal = tokio::spawn({
            let notify = notify.clone();
            async move {
                super::wait_for_shutdown().await;
                notify.notify_one();
            }
        });
        let mut controls = cfg.forward_control.take();
        let mut forwards = cfg.forwards.to_vec();
        let end = session_loop(
            &mut self.link,
            &self.device.mac,
            self.sid,
            v6::Channel::Noise(self.channel),
            &cfg,
            notify,
            &mut controls,
            &mut forwards,
            &ports,
        )
        .await;
        signal.abort();
        match end? {
            SessionEnd::Shutdown => Ok(()),
            _ => Err("selected device disconnected; confirm the device to reconnect".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_names_are_distinct_and_duplicate_paths_are_deduplicated() {
        let mut devices = BTreeMap::new();
        let reply =
            |id: u64, key: u8| [id.to_be_bytes().as_slice(), &[key; 32], b"router"].concat();
        record_device(&mut devices, 12, [1; 6], &reply(11, 1));
        assert!(devices.is_empty());
        record_device(&mut devices, 12, [1; 6], &reply(12, 1));
        record_device(&mut devices, 12, [2; 6], &reply(12, 1));
        record_device(&mut devices, 12, [3; 6], &reply(12, 2));
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[&v6::fingerprint(&[1; 32])].mac, [1; 6]);
        assert_eq!(devices[&v6::fingerprint(&[2; 32])].mac, [3; 6]);
    }
}
