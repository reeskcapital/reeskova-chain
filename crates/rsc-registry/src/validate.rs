//! Validación de URL, multiaddr y política de anuncio.
//!
//! No resuelve DNS y no abre conexiones. Una dirección privada, de loopback
//! o de enlace local no se publica como pública. `169.254.169.254` se rechaza
//! aunque el modo de pruebas permita redes privadas.

use crate::types::NodeType;
use libp2p::multiaddr::Protocol;
use libp2p::{Multiaddr, PeerId};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::str::FromStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ValidateError {
    #[error("invalid url")]
    InvalidUrl,
    #[error("invalid address")]
    InvalidAddress,
    #[error("private endpoint")]
    PrivateEndpoint,
    #[error("announce policy")]
    Policy,
    #[error("invalid identity")]
    Identity,
    #[error("invalid request")]
    Request,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpTarget {
    pub scheme: String,
    pub host: String,
    pub port: Option<u16>,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenedEndpoints {
    pub rpc_public_url: Option<String>,
    pub service_url: Option<String>,
    pub primary_rpc: bool,
    pub produces_blocks: bool,
}

pub fn screen_endpoints(
    kind: NodeType,
    rpc: Option<&str>,
    service: Option<&str>,
    allow_private: bool,
    host_allowlist: &[String],
) -> Result<ScreenedEndpoints, ValidateError> {
    let rpc = normalize_optional(rpc);
    let service = normalize_optional(service);
    if !kind.allows_rpc_url() && rpc.is_some() {
        return Err(ValidateError::Policy);
    }
    if !matches!(kind, NodeType::Mobile | NodeType::Community) && service.is_some() {
        return Err(ValidateError::Policy);
    }
    if let Some(url) = &rpc {
        let target = parse_http_target(url)?;
        ensure_host(&target, allow_private, host_allowlist)?;
        if target.path != "/rpc" {
            return Err(ValidateError::Policy);
        }
    }
    if let Some(url) = &service {
        let target = parse_http_target(url)?;
        ensure_host(&target, allow_private, host_allowlist)?;
        let ok = match kind {
            NodeType::Mobile => target.path == "/mobile" || target.path.starts_with("/mobile/"),
            NodeType::Community => {
                target.path == "/community" || target.path.starts_with("/community/")
            }
            _ => false,
        };
        if !ok {
            return Err(ValidateError::Policy);
        }
    }
    Ok(ScreenedEndpoints {
        primary_rpc: kind.allows_primary_rpc() && rpc.is_some(),
        produces_blocks: kind.produces_blocks(),
        rpc_public_url: rpc,
        service_url: service,
    })
}

pub fn validate_p2p_multiaddr(
    text: &str,
    expected: &PeerId,
    allow_private: bool,
) -> Result<(), ValidateError> {
    if text.is_empty() || text.len() > 300 || text.bytes().any(|b| b.is_ascii_whitespace() || b < 0x20)
    {
        return Err(ValidateError::InvalidAddress);
    }
    let addr: Multiaddr = text.parse().map_err(|_| ValidateError::InvalidAddress)?;
    let mut saw_peer = false;
    let mut saw_transport = false;
    for protocol in addr.iter() {
        match protocol {
            Protocol::P2p(id) => {
                if id == *expected {
                    saw_peer = true;
                }
            }
            Protocol::Ip4(ip) => {
                saw_transport = true;
                ensure_v4(ip, allow_private)?;
            }
            Protocol::Ip6(ip) => {
                saw_transport = true;
                ensure_v6(ip, allow_private)?;
            }
            Protocol::Dns(host) | Protocol::Dns4(host) | Protocol::Dns6(host) | Protocol::Dnsaddr(host) => {
                saw_transport = true;
                ensure_dns_name(host.as_ref())?;
            }
            Protocol::Tcp(_) | Protocol::Udp(_) | Protocol::QuicV1 | Protocol::P2pCircuit => {}
            _ => return Err(ValidateError::InvalidAddress),
        }
    }
    if !saw_peer || !saw_transport {
        return Err(ValidateError::InvalidAddress);
    }
    Ok(())
}

/// Backend configurado por el operador. Puede ser loopback. Nunca metadata.
pub fn validate_operator_backend(text: &str, self_port: Option<u16>) -> Result<(), ValidateError> {
    let target = parse_http_target(text)?;
    if target.path != "/" {
        return Err(ValidateError::InvalidUrl);
    }
    ensure_host(&target, true, &[])?;
    if let Some(port) = target.port {
        if Some(port) == self_port {
            return Err(ValidateError::Policy);
        }
    } else {
        let implied = if target.scheme == "https" { 443 } else { 80 };
        if Some(implied) == self_port {
            return Err(ValidateError::Policy);
        }
    }
    Ok(())
}

pub fn parse_http_target(input: &str) -> Result<HttpTarget, ValidateError> {
    if input.is_empty()
        || input.len() > 300
        || input.contains('?')
        || input.contains('#')
        || input.bytes().any(|b| b.is_ascii_whitespace() || b < 0x20)
    {
        return Err(ValidateError::InvalidUrl);
    }
    let (scheme, rest) = if let Some(rest) = input.strip_prefix("https://") {
        ("https", rest)
    } else if let Some(rest) = input.strip_prefix("http://") {
        ("http", rest)
    } else {
        return Err(ValidateError::InvalidUrl);
    };
    if rest.is_empty() || rest.contains('@') || rest.contains("://") {
        return Err(ValidateError::InvalidUrl);
    }
    let (authority, path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, "/"),
    };
    if authority.is_empty() || path.contains("..") {
        return Err(ValidateError::InvalidUrl);
    }
    let (host, port) = split_host_port(authority)?;
    Ok(HttpTarget {
        scheme: scheme.to_string(),
        host,
        port,
        path: path.to_string(),
    })
}

fn normalize_optional(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|text| !text.is_empty() && *text != "-")
        .map(str::to_string)
}

fn split_host_port(authority: &str) -> Result<(String, Option<u16>), ValidateError> {
    if let Some(rest) = authority.strip_prefix('[') {
        let end = rest.find(']').ok_or(ValidateError::InvalidUrl)?;
        let host = rest[..end].to_string();
        if host.is_empty() {
            return Err(ValidateError::InvalidUrl);
        }
        let after = &rest[end + 1..];
        if after.is_empty() {
            return Ok((host, None));
        }
        let port = after.strip_prefix(':').ok_or(ValidateError::InvalidUrl)?;
        return Ok((host, Some(parse_port(port)?)));
    }
    if authority.contains(':') {
        let (host, port) = authority.rsplit_once(':').ok_or(ValidateError::InvalidUrl)?;
        if host.is_empty() || host.contains(':') {
            return Err(ValidateError::InvalidUrl);
        }
        return Ok((host.to_string(), Some(parse_port(port)?)));
    }
    Ok((authority.to_string(), None))
}

fn parse_port(text: &str) -> Result<u16, ValidateError> {
    if text.is_empty() || text.len() > 5 || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ValidateError::InvalidUrl);
    }
    text.parse::<u16>().map_err(|_| ValidateError::InvalidUrl)
}

fn ensure_host(
    target: &HttpTarget,
    allow_private: bool,
    host_allowlist: &[String],
) -> Result<(), ValidateError> {
    if let Some(ip) = target.host.parse::<Ipv4Addr>().ok() {
        ensure_v4(ip, allow_private)?;
    } else if let Some(ip) = target.host.parse::<Ipv6Addr>().ok() {
        ensure_v6(ip, allow_private)?;
    } else {
        ensure_dns_name(&target.host)?;
    }
    if !host_allowlist.is_empty() {
        let host = target.host.to_ascii_lowercase();
        let allowed = host_allowlist.iter().any(|item| item.eq_ignore_ascii_case(&host));
        if !allowed {
            return Err(ValidateError::Policy);
        }
    }
    Ok(())
}

fn ensure_dns_name(host: &str) -> Result<(), ValidateError> {
    if host.is_empty() || host.len() > 253 || host.parse::<Ipv4Addr>().is_ok() {
        return Err(ValidateError::InvalidUrl);
    }
    let lower = host.to_ascii_lowercase();
    if lower == "localhost"
        || lower.ends_with(".localhost")
        || lower.ends_with(".local")
        || lower == "metadata.google.internal"
        || lower.contains("metadata.google")
    {
        return Err(ValidateError::PrivateEndpoint);
    }
    if !lower
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
        || lower.starts_with('.')
        || lower.ends_with('.')
        || lower.contains("..")
    {
        return Err(ValidateError::InvalidUrl);
    }
    Ok(())
}

fn ensure_v4(ip: Ipv4Addr, allow_private: bool) -> Result<(), ValidateError> {
    let octets = ip.octets();
    if ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || octets[0] == 0
        || ip.is_link_local()
    {
        return Err(ValidateError::PrivateEndpoint);
    }
    let private = ip.is_loopback()
        || ip.is_private()
        || (octets[0] == 100 && (octets[1] & 0b1100_0000) == 0b0100_0000);
    if private && !allow_private {
        return Err(ValidateError::PrivateEndpoint);
    }
    Ok(())
}

fn ensure_v6(ip: Ipv6Addr, allow_private: bool) -> Result<(), ValidateError> {
    if ip.is_unspecified() || ip.is_multicast() {
        return Err(ValidateError::PrivateEndpoint);
    }
    let octets = ip.octets();
    let link_local = octets[0] == 0xfe && (octets[1] & 0b1100_0000) == 0b1000_0000;
    if link_local {
        return Err(ValidateError::PrivateEndpoint);
    }
    let unique_local = octets[0] & 0xfe == 0xfc;
    let loopback = ip.is_loopback();
    if (unique_local || loopback) && !allow_private {
        return Err(ValidateError::PrivateEndpoint);
    }
    Ok(())
}

pub fn parse_peer_id(text: &str) -> Result<PeerId, ValidateError> {
    if text.len() > 128 || text.bytes().any(|b| b.is_ascii_whitespace()) {
        return Err(ValidateError::Identity);
    }
    PeerId::from_str(text).map_err(|_| ValidateError::Identity)
}

pub fn valid_network_id(id: &str) -> Result<(), ValidateError> {
    if id.is_empty() || id.len() > 32 {
        return Err(ValidateError::Request);
    }
    if !id
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(ValidateError::Request);
    }
    Ok(())
}

pub fn valid_version(version: &str) -> Result<(), ValidateError> {
    if version.is_empty() || version.len() > 32 {
        return Err(ValidateError::Request);
    }
    if !version
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
    {
        return Err(ValidateError::Request);
    }
    Ok(())
}

pub fn host_allowlist_from_env() -> Vec<String> {
    match std::env::var("RSC_REGISTRY_RPC_HOSTS") {
        Ok(raw) => raw
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(|item| item.to_ascii_lowercase())
            .collect(),
        Err(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libp2p::identity::Keypair;

    fn peer() -> PeerId {
        PeerId::from(Keypair::generate_ed25519().public())
    }

    #[test]
    fn rejects_private_metadata_and_credentials() {
        let cases = [
            "http://127.0.0.1:8545/rpc",
            "http://10.1.1.1/rpc",
            "http://192.168.1.12/rpc",
            "http://169.254.169.254/rpc",
            "http://0.0.0.0/rpc",
            "http://user:pass@rpc.reeskova-chain.xyz/rpc",
            "file:///etc/passwd",
            "http://metadata.google.internal/rpc",
            "https://rpc.reeskova-chain.xyz/rpc?token=abc",
            "http://[::1]/rpc",
        ];
        for case in cases {
            assert!(
                parse_http_target(case).is_err()
                    || screen_endpoints(NodeType::Full, Some(case), None, false, &[]).is_err(),
                "{case}"
            );
        }
        assert_eq!(
            screen_endpoints(
                NodeType::Full,
                Some("http://169.254.169.254/rpc"),
                None,
                true,
                &[],
            ),
            Err(ValidateError::PrivateEndpoint)
        );
    }

    #[test]
    fn accepts_public_rpc_and_rejects_mobile_as_primary() {
        let url = "https://rpc.reeskova-chain.xyz/rpc";
        let screened = screen_endpoints(NodeType::Full, Some(url), None, false, &[]).unwrap();
        assert!(screened.primary_rpc);
        assert!(screened.produces_blocks);
        assert_eq!(screened.rpc_public_url.as_deref(), Some(url));

        assert_eq!(
            screen_endpoints(NodeType::Mobile, Some(url), None, false, &[]),
            Err(ValidateError::Policy)
        );
        let mobile = screen_endpoints(
            NodeType::Mobile,
            None,
            Some("https://rpc.reeskova-chain.xyz/mobile"),
            false,
            &["rpc.reeskova-chain.xyz".into()],
        )
        .unwrap();
        assert!(!mobile.primary_rpc);
        assert!(!mobile.produces_blocks);
        assert!(mobile.rpc_public_url.is_none());

        assert!(screen_endpoints(
            NodeType::Full,
            Some("https://evil.example/rpc"),
            None,
            false,
            &["rpc.reeskova-chain.xyz".into()],
        )
        .is_err());
    }

    #[test]
    fn multiaddr_requires_matching_peer_and_public_ip() {
        let id = peer();
        let other = peer();
        let good = format!("/ip4/203.0.113.10/tcp/9000/p2p/{id}");
        assert!(validate_p2p_multiaddr(&good, &id, false).is_ok());
        let private = format!("/ip4/192.168.1.5/tcp/9000/p2p/{id}");
        assert_eq!(
            validate_p2p_multiaddr(&private, &id, false),
            Err(ValidateError::PrivateEndpoint)
        );
        let mismatch = format!("/ip4/203.0.113.10/tcp/9000/p2p/{other}");
        assert_eq!(
            validate_p2p_multiaddr(&mismatch, &id, false),
            Err(ValidateError::InvalidAddress)
        );
        let loopback = format!("/ip4/127.0.0.1/tcp/9000/p2p/{id}");
        assert!(validate_p2p_multiaddr(&loopback, &id, false).is_err());
        let unspecified = format!("/ip4/0.0.0.0/tcp/9000/p2p/{id}");
        assert!(validate_p2p_multiaddr(&unspecified, &id, true).is_err());
    }

    #[test]
    fn operator_backend_allows_loopback_not_metadata_or_self() {
        assert!(validate_operator_backend("http://127.0.0.1:8545", Some(8544)).is_ok());
        assert!(validate_operator_backend("http://169.254.169.254", Some(8544)).is_err());
        assert!(validate_operator_backend("http://127.0.0.1:8544", Some(8544)).is_err());
        assert!(validate_operator_backend("http://127.0.0.1:8545/rpc", Some(8544)).is_err());
    }
}
