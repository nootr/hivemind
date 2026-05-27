use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket as StdUdpSocket};

pub fn inferred_node_url(bind_addr: SocketAddr, target: SocketAddr) -> String {
    let port = bind_addr.port();
    if bind_addr.ip().is_loopback() {
        return format!("http://127.0.0.1:{port}");
    }
    let ip = outbound_ip_for(target).unwrap_or(bind_addr.ip());
    let host = match ip {
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => format!("[{ip}]"),
    };
    format!("http://{host}:{port}")
}

pub fn outbound_ip_for(peer: SocketAddr) -> Option<IpAddr> {
    let socket = StdUdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect(peer).ok()?;
    Some(socket.local_addr().ok()?.ip())
}

pub fn local_node_url(bind_addr: SocketAddr) -> String {
    format!(
        "http://{}:{}",
        public_host(bind_addr.ip()),
        bind_addr.port()
    )
}

pub fn public_host(bind_ip: IpAddr) -> String {
    let ip = if bind_ip.is_unspecified() {
        default_lan_ip().unwrap_or(IpAddr::V4(Ipv4Addr::LOCALHOST))
    } else {
        bind_ip
    };
    match ip {
        IpAddr::V4(ip) => ip.to_string(),
        IpAddr::V6(ip) => format!("[{ip}]"),
    }
}

fn default_lan_ip() -> Option<IpAddr> {
    outbound_ip_for(SocketAddr::from((Ipv4Addr::new(1, 1, 1, 1), 80)))
        .filter(|ip| !ip.is_loopback() && !ip.is_unspecified())
}

pub fn valid_node_url(url: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return false;
    };
    matches!(parsed.scheme(), "http" | "https")
        && parsed.host().is_some()
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.fragment().is_none()
        && !url.bytes().any(|byte| byte.is_ascii_whitespace())
}

pub fn normalized_node_url(url: &str) -> String {
    url.trim_end_matches('/').to_owned()
}
