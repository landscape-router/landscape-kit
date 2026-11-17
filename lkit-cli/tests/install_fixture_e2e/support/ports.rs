use std::net::{TcpListener, UdpSocket};

pub(crate) struct TestPorts {
    pub(crate) dns: u16,
    pub(crate) http: u16,
    pub(crate) https: u16,
    _dns_tcp: TcpListener,
    _dns_udp: UdpSocket,
    _http: TcpListener,
    _https: TcpListener,
}

impl TestPorts {
    pub(crate) fn reserve() -> Self {
        let dns_tcp = TcpListener::bind("127.0.0.1:0").unwrap();
        let dns = dns_tcp.local_addr().unwrap().port();
        let dns_udp = UdpSocket::bind(("127.0.0.1", dns)).unwrap();
        let http = TcpListener::bind("127.0.0.1:0").unwrap();
        let https = TcpListener::bind("127.0.0.1:0").unwrap();
        Self {
            dns,
            http: http.local_addr().unwrap().port(),
            https: https.local_addr().unwrap().port(),
            _dns_tcp: dns_tcp,
            _dns_udp: dns_udp,
            _http: http,
            _https: https,
        }
    }
}
