//! Capa de transporte P2P de RSC Chain v1.
//!
//! `rsc-p2p` mueve bytes entre nodos. No interpreta consenso, no valida
//! bloques, no ejecuta WASM y no escribe storage.
//!
//! `PeerId` es identidad de **red**. No es account, validator ni voting power.

#![forbid(unsafe_code)]

pub mod discovery;
pub mod ingress;
pub mod network;
pub mod peer;
pub mod presence;
pub mod sync;

pub use discovery::{BootstrapPeer, DiscoveryConfig, DiscoveryEvent, DiscoveryManager};
pub use ingress::{classify_ingress, Ingress};
pub use network::{
    GossipAcceptance, GossipTopic, NetworkCommand, NetworkConfig, NetworkEvent, NetworkHandle,
    NetworkTimeouts, RscNetwork, RscRequest, RscRequestId, RscResponse,
};
pub use peer::{
    BanReason, IdentityError, LocalPeerIdentity, PeerInfo, PeerPolicy, PeerRegistry, PeerState,
};
pub use sync::{apply_in_order, range_len};

pub use libp2p::identity::Keypair;
pub use libp2p::{Multiaddr, PeerId};
pub use rsc_core::ChainId;

/// Versión del protocolo de control RSC-P2P.
pub const RSC_P2P_PROTOCOL_VERSION: &str = "1.0.0";
pub const HANDSHAKE_PROTOCOL: &str = "/rsc/handshake/1.0.0";
pub const BLOCK_SYNC_PROTOCOL: &str = "/rsc/block-sync/1.0.0";
pub const GOSSIPSUB_PROTOCOL: &str = "/rsc/gossipsub/1.1.0";

pub const TOPIC_BLOCKS: &str = "blocks";
pub const TOPIC_TRANSACTIONS: &str = "transactions";
pub const TOPIC_CONSENSUS: &str = "consensus";
pub const TOPIC_EVIDENCE: &str = "evidence";

/// Límites de red. Todos son finitos; no hay colas unbounded.
///
/// Protege memoria y CPU frente a peers no confiables. No se usan como
/// peso de consenso ni scoring de validators.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct P2pLimits {
    pub max_peers: u32,
    pub max_inbound_connections: u32,
    pub max_outbound_connections: u32,
    pub max_gossip_message_size: usize,
    pub max_block_gossip_size: usize,
    pub max_tx_gossip_size: usize,
    pub max_consensus_gossip_size: usize,
    pub max_evidence_gossip_size: usize,
    pub max_request_size: usize,
    pub max_response_size: usize,
    pub max_concurrent_requests_per_peer: u32,
    pub max_pending_responses: u32,
    pub max_command_queue: usize,
    pub max_event_queue: usize,
}

impl Default for P2pLimits {
    fn default() -> Self {
        Self {
            max_peers: 50,
            max_inbound_connections: 32,
            max_outbound_connections: 32,
            max_gossip_message_size: 1_048_576,
            max_block_gossip_size: 1_048_576,
            max_tx_gossip_size: 131_072,
            max_consensus_gossip_size: 65_536,
            max_evidence_gossip_size: 65_536,
            max_request_size: 65_536,
            max_response_size: 1_048_576,
            max_concurrent_requests_per_peer: 8,
            max_pending_responses: 64,
            max_command_queue: 256,
            max_event_queue: 256,
        }
    }
}

/// Error público sanitizado. Sin claves, credenciales ni payloads sensibles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum P2pError {
    #[error("invalid configuration")]
    Configuration,
    #[error("transport error")]
    Transport,
    #[error("listen failed")]
    Listen,
    #[error("dial failed")]
    Dial,
    #[error("handshake failed")]
    HandshakeFailed,
    #[error("network mismatch")]
    NetworkMismatch,
    #[error("chain mismatch")]
    ChainMismatch,
    #[error("protocol version mismatch")]
    ProtocolVersionMismatch,
    #[error("invalid address")]
    InvalidAddress,
    #[error("invalid message")]
    InvalidMessage,
    #[error("message too large")]
    MessageTooLarge,
    #[error("peer banned")]
    PeerBanned,
    #[error("request timeout")]
    RequestTimeout,
    #[error("response too large")]
    ResponseTooLarge,
    #[error("discovery error")]
    Discovery,
    #[error("publish failed")]
    Publish,
    #[error("subscribe failed")]
    Subscribe,
    #[error("request-response error")]
    RequestResponse,
    #[error("channel closed")]
    ChannelClosed,
    #[error("channel full")]
    Backpressure,
    #[error("shutdown")]
    Shutdown,
    #[error("unsupported protocol")]
    UnsupportedProtocol,
    #[error("unsupported topic")]
    UnsupportedTopic,
}

/// `network_id` seguro para names de protocolo y topics.
pub fn validate_network_id(id: &str) -> Result<(), P2pError> {
    if id.is_empty() || id.len() > 32 {
        return Err(P2pError::Configuration);
    }
    if !id
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(P2pError::Configuration);
    }
    Ok(())
}

pub fn kad_protocol(network_id: &str) -> Result<String, P2pError> {
    validate_network_id(network_id)?;
    Ok(format!("/rsc/{network_id}/kad/1.0.0"))
}

pub fn topic_name(network_id: &str, kind: &str) -> Result<String, P2pError> {
    validate_network_id(network_id)?;
    match kind {
        TOPIC_BLOCKS | TOPIC_TRANSACTIONS | TOPIC_CONSENSUS | TOPIC_EVIDENCE => {
            Ok(format!("/rsc/{network_id}/{kind}/1"))
        }
        _ => Err(P2pError::UnsupportedTopic),
    }
}

pub fn gossip_size_limit(kind: GossipTopic, limits: &P2pLimits) -> usize {
    let specific = match kind {
        GossipTopic::Blocks => limits.max_block_gossip_size,
        GossipTopic::Transactions => limits.max_tx_gossip_size,
        GossipTopic::Consensus => limits.max_consensus_gossip_size,
        GossipTopic::Evidence => limits.max_evidence_gossip_size,
    };
    specific.min(limits.max_gossip_message_size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use libp2p::identity;

    #[test]
    fn test_generate_identity() {
        let a = identity::Keypair::generate_ed25519();
        let b = identity::Keypair::generate_ed25519();
        assert_ne!(PeerId::from(a.public()), PeerId::from(b.public()));
    }

    #[test]
    fn test_peer_id_deterministic_for_same_key() {
        let kp = identity::Keypair::generate_ed25519();
        let id1 = PeerId::from(kp.public());
        let id2 = PeerId::from(kp.public());
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_identity_is_not_account_identity() {
        let peer = PeerId::from(identity::Keypair::generate_ed25519().public());
        let _account = rsc_core::AccountId::ZERO;
        assert_ne!(format!("{peer}"), rsc_core::AccountId::ZERO.to_hex());
    }

    #[test]
    fn test_network_id_validation() {
        assert!(validate_network_id("mainnet").is_ok());
        assert!(validate_network_id("").is_err());
        assert!(validate_network_id("MAIN").is_err());
        assert!(validate_network_id("bad/id").is_err());
    }

    #[test]
    fn test_topic_builders() {
        assert_eq!(
            topic_name("devnet", "blocks").unwrap(),
            "/rsc/devnet/blocks/1"
        );
        assert!(topic_name("devnet", "state").is_err());
        assert_eq!(kad_protocol("devnet").unwrap(), "/rsc/devnet/kad/1.0.0");
    }

    #[test]
    fn test_no_secret_in_error_display() {
        let s = P2pError::HandshakeFailed.to_string();
        assert!(!s.contains("private"));
        assert!(!s.contains("mnemonic"));
        assert!(!s.contains("seed"));
    }

    #[test]
    fn test_limits_are_finite() {
        let l = P2pLimits::default();
        assert!(l.max_peers > 0);
        assert!(l.max_command_queue > 0);
        assert!(l.max_event_queue > 0);
        assert!(l.max_gossip_message_size > 0);
        assert!(l.max_request_size > 0);
        assert!(l.max_response_size > 0);
    }
}
