//! Outbound fetching of user-supplied documentation URLs. A `docs_url`
//! comes from whoever called `register_library` (or from a package
//! registry via `discover_library`), and on a hosted server the response
//! is stored where that caller can read it back. Fetching it with a plain
//! `ureq::get` let any tenant make the server read loopback, private-network
//! or cloud-metadata addresses (engineering docs finding F35).
//!
//! `DOC_AGENT` refuses non-public addresses at DNS-resolution time, not by
//! looking at the URL text. ureq resolves again for every connection, so
//! redirects and DNS rebinding are covered too.
//!
//! The `Resolver` trait lives in ureq's `unversioned` module, which ureq
//! says does not follow semver (ureq 3.4.0 docs). Recheck this file when
//! upgrading ureq.

use std::net::IpAddr;
use std::sync::LazyLock;

use anyhow::{bail, Context, Result};
use ureq::config::Config;
use ureq::unversioned::resolver::{DefaultResolver, ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};
use ureq::Agent;

/// Set to `1` or `true` to allow fetching docs from non-public addresses,
/// e.g. intranet docs on a trusted self-host.
const ALLOW_PRIVATE_ENV: &str = "AGENTOPS_ALLOW_PRIVATE_DOC_URLS";

/// The agent every fetch of a user-supplied documentation URL goes
/// through. Fixed-host API calls (registries, GitHub, Firecrawl) keep plain
/// `ureq` calls.
pub(crate) static DOC_AGENT: LazyLock<Agent> = LazyLock::new(|| {
    let config = Agent::config_builder().build();
    if matches!(std::env::var(ALLOW_PRIVATE_ENV).as_deref(), Ok("1") | Ok("true")) {
        Agent::new_with_config(config)
    } else {
        Agent::with_parts(config, DefaultConnector::default(), PublicOnlyResolver::default())
    }
});

/// Wraps ureq's resolver and fails if any resolved address is not public.
#[derive(Debug, Default)]
struct PublicOnlyResolver(DefaultResolver);

impl Resolver for PublicOnlyResolver {
    fn resolve(&self, uri: &ureq::http::Uri, config: &Config, timeout: NextTimeout) -> Result<ResolvedSocketAddrs, ureq::Error> {
        let addrs = self.0.resolve(uri, config, timeout)?;
        if let Some(blocked) = addrs.iter().find(|a| !is_public_ip(a.ip())) {
            return Err(ureq::Error::BadUri(format!(
                "refusing to fetch {uri}: it resolves to the non-public address {} (set {ALLOW_PRIVATE_ENV}=1 to allow)",
                blocked.ip()
            )));
        }
        Ok(addrs)
    }
}

/// True only for globally routable unicast addresses. Loopback, private,
/// link-local (which covers cloud metadata at 169.254.169.254),
/// shared/CGNAT, unspecified, broadcast, multicast, IPv6 unique-local and
/// link-local, and IPv4-mapped or -compatible forms of these are all false.
pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_documentation()
                || a == 0
                || (a == 100 && (64..128).contains(&b)))
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_ip(IpAddr::V4(v4));
            }
            let first = v6.segments()[0];
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (first & 0xfe00) == 0xfc00
                || (first & 0xffc0) == 0xfe80
                // Deprecated IPv4-compatible form (::a.b.c.d).
                || v6.segments()[..6].iter().all(|s| *s == 0))
        }
    }
}

/// A documentation URL must be absolute `http` or `https`. Checked where a
/// `docs_url` is stored and again before it is fetched.
pub fn validate_docs_url(url: &str) -> Result<()> {
    let parsed = url::Url::parse(url).with_context(|| format!("docs_url {url:?} is not a valid URL"))?;
    match parsed.scheme() {
        "http" | "https" => Ok(()),
        other => bail!("docs_url must use http or https, not {other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_public_addresses_are_refused() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.10",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "255.255.255.255",
            "::1",
            "::",
            "fc00::1",
            "fd12:3456::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
        ] {
            assert!(!is_public_ip(ip.parse().unwrap()), "{ip} must not be treated as public");
        }
    }

    #[test]
    fn public_addresses_are_allowed() {
        for ip in ["93.184.216.34", "1.1.1.1", "2606:4700:4700::1111", "::ffff:93.184.216.34"] {
            assert!(is_public_ip(ip.parse().unwrap()), "{ip} must be treated as public");
        }
    }

    #[test]
    fn only_http_and_https_docs_urls_are_valid() {
        assert!(validate_docs_url("https://docs.rs/ureq").is_ok());
        assert!(validate_docs_url("http://example.com/docs").is_ok());
        for bad in ["ftp://example.com/docs", "file:///etc/passwd", "gopher://x", "not a url"] {
            assert!(validate_docs_url(bad).is_err(), "{bad} must be rejected");
        }
    }

    #[test]
    fn a_fetch_to_loopback_is_refused_by_the_resolver() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://127.0.0.1:{}/", listener.local_addr().unwrap().port());
        let agent = Agent::with_parts(Agent::config_builder().build(), DefaultConnector::default(), PublicOnlyResolver::default());
        let err = agent.get(&url).call().expect_err("loopback must be refused");
        assert!(err.to_string().contains("non-public"), "{err}");
    }
}
