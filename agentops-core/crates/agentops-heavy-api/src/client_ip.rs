//! The client address used for rate limiting and for the Active Sessions
//! list. Only a hop we control may vouch for it: `X-Real-IP` is trusted
//! when the TCP peer is loopback or private (the in-container Next.js
//! server, or a reverse proxy on the host or a private network), and
//! ignored from anyone else. The deployment's proxies must set
//! `X-Real-IP` to the real client address and overwrite any value the
//! client sent; nginx's `proxy_set_header X-Real-IP $remote_addr` does.
//!
//! The first `X-Forwarded-For` hop is never used: behind a proxy that
//! appends to the header, that hop is whatever the client sent.

use std::net::{IpAddr, SocketAddr};

use axum::extract::ConnectInfo;
use axum::http::HeaderMap;

/// The best-known client address for a request. `peer` is `None` when the
/// server wasn't started with connect info (tests, `Router::oneshot`); the
/// result is then `None` too unless no header is needed.
pub(crate) fn client_ip(peer: Option<SocketAddr>, headers: &HeaderMap) -> Option<IpAddr> {
    let peer = peer?;
    if is_trusted_proxy(peer.ip()) {
        if let Some(real) = headers.get("x-real-ip").and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<IpAddr>().ok()) {
            return Some(real);
        }
    }
    Some(peer.ip())
}

/// Convenience for handlers taking `Option<Extension<ConnectInfo<SocketAddr>>>`.
pub(crate) fn peer_of(info: &Option<axum::Extension<ConnectInfo<SocketAddr>>>) -> Option<SocketAddr> {
    info.as_ref().map(|axum::Extension(ConnectInfo(addr))| *addr)
}

fn is_trusted_proxy(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private(),
        IpAddr::V6(v6) => v6.is_loopback() || (v6.segments()[0] & 0xfe00) == 0xfc00 || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback() || v4.is_private()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(real_ip: Option<&str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        if let Some(ip) = real_ip {
            h.insert("x-real-ip", ip.parse().unwrap());
        }
        h.insert("x-forwarded-for", "6.6.6.6, 198.51.100.1".parse().unwrap());
        h
    }

    #[test]
    fn a_private_peer_may_vouch_for_the_client_with_x_real_ip() {
        let peer: SocketAddr = "127.0.0.1:40000".parse().unwrap();
        assert_eq!(client_ip(Some(peer), &headers(Some("203.0.113.7"))), Some("203.0.113.7".parse().unwrap()));
        let docker_bridge: SocketAddr = "172.17.0.1:40000".parse().unwrap();
        assert_eq!(client_ip(Some(docker_bridge), &headers(Some("203.0.113.7"))), Some("203.0.113.7".parse().unwrap()));
    }

    #[test]
    fn a_public_peer_cannot_spoof_its_address() {
        let peer: SocketAddr = "203.0.113.9:40000".parse().unwrap();
        assert_eq!(client_ip(Some(peer), &headers(Some("10.0.0.1"))), Some("203.0.113.9".parse().unwrap()));
    }

    #[test]
    fn a_missing_or_bad_header_falls_back_to_the_peer_and_forwarded_for_is_ignored() {
        let peer: SocketAddr = "127.0.0.1:40000".parse().unwrap();
        assert_eq!(client_ip(Some(peer), &headers(None)), Some("127.0.0.1".parse().unwrap()));
        assert_eq!(client_ip(Some(peer), &headers(Some("not-an-ip"))), Some("127.0.0.1".parse().unwrap()));
        assert_eq!(client_ip(None, &headers(Some("203.0.113.7"))), None);
    }
}
