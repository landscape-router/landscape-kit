use std::net::{TcpListener, UdpSocket};

pub(crate) struct TestPorts {
    pub(crate) dns: u16,
    pub(crate) http: u16,
    pub(crate) https: u16,
}

impl TestPorts {
    pub(crate) fn reserve() -> Self {
        let dns_tcp = TcpListener::bind("127.0.0.1:0").unwrap();
        let dns = dns_tcp.local_addr().unwrap().port();
        let dns_udp = UdpSocket::bind(("127.0.0.1", dns)).unwrap();
        let http = free_tcp_port();
        let https = free_tcp_port();
        drop(dns_udp);
        drop(dns_tcp);
        Self { dns, http, https }
    }
}

fn free_tcp_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}
