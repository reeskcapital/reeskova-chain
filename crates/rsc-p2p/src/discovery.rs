//! Discovery: bootstrap seeds + Kademlia. Los bootnodes no son autoridades.

use crate::{validate_network_id, P2pError};
use libp2p::multiaddr::Protocol;
use libp2p::{Multiaddr, PeerId};
use std::time::Duration;

/// Semilla de discovery. No implica autoridad de consenso ni de estado.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BootstrapPeer {
    pub peer_id: PeerId,
    pub addresses: Vec<Multiaddr>,
}

impl BootstrapPeer {
    pub fn new(peer_id: PeerId, addresses: Vec<Multiaddr>) -> Result<Self, P2pError> {
        let peer = Self { peer_id, addresses };
        peer.validate()?;
        Ok(peer)
    }

    pub fn validate(&self) -> Result<(), P2pError> {
        if self.addresses.is_empty() {
            return Err(P2pError::InvalidAddress);
        }
        for addr in &self.addresses {
            if addr.is_empty() {
                return Err(P2pError::InvalidAddress);
            }
            if let Some(id) = peer_id_from_multiaddr(addr) {
                if id != self.peer_id {
                    return Err(P2pError::InvalidAddress);
                }
            }
        }
        Ok(())
    }
}

pub fn peer_id_from_multiaddr(addr: &Multiaddr) -> Option<PeerId> {
    addr.iter().find_map(|p| match p {
        Protocol::P2p(id) => Some(id),
        _ => None,
    })
}

/// Configuración de discovery. Kademlia usa MemoryStore en V1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveryConfig {
    pub bootstrap_peers: Vec<BootstrapPeer>,
    pub kad_protocol: String,
    pub bootstrap_timeout: Duration,
    pub periodic_bootstrap_interval: Duration,
}

impl DiscoveryConfig {
    pub fn for_network(
        network_id: &str,
        bootstrap_peers: Vec<BootstrapPeer>,
    ) -> Result<Self, P2pError> {
        validate_network_id(network_id)?;
        for p in &bootstrap_peers {
            p.validate()?;
        }
        Ok(Self {
            bootstrap_peers,
            kad_protocol: crate::kad_protocol(network_id)?,
            bootstrap_timeout: Duration::from_secs(30),
            periodic_bootstrap_interval: Duration::from_secs(300),
        })
    }
}

/// Eventos de discovery. Bootstrap no otorga confianza extra.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiscoveryEvent {
    BootstrapStarted,
    BootstrapSucceeded,
    BootstrapFailed,
    PeerDiscovered { peer_id: PeerId },
    AddressDiscovered { peer_id: PeerId, address: Multiaddr },
    AddressRemoved { peer_id: PeerId, address: Multiaddr },
}

/// Orquesta bootstrap e integración Identify → Kademlia.
///
/// No modifica consenso. Las direcciones se filtran para que `/p2p/<id>`
/// coincida con el PeerId anunciado.
#[derive(Debug)]
pub struct DiscoveryManager {
    config: DiscoveryConfig,
    bootstrap_started: bool,
}

impl DiscoveryManager {
    pub fn new(config: DiscoveryConfig) -> Self {
        Self {
            config,
            bootstrap_started: false,
        }
    }

    pub fn config(&self) -> &DiscoveryConfig {
        &self.config
    }

    pub fn mark_bootstrap_started(&mut self) -> DiscoveryEvent {
        self.bootstrap_started = true;
        DiscoveryEvent::BootstrapStarted
    }

    pub fn filter_identify_addrs(&self, peer: PeerId, addrs: &[Multiaddr]) -> Vec<Multiaddr> {
        addrs
            .iter()
            .filter(|a| !a.is_empty())
            .filter(|a| match peer_id_from_multiaddr(a) {
                Some(id) => id == peer,
                None => true,
            })
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_invalid_bootstrap_peerid_mismatch() {
        let a = PeerId::random();
        let b = PeerId::random();
        let addr: Multiaddr = format!("/ip4/127.0.0.1/tcp/1/p2p/{b}").parse().unwrap();
        assert_eq!(
            BootstrapPeer::new(a, vec![addr]).unwrap_err(),
            P2pError::InvalidAddress
        );
    }

    #[test]
    fn test_valid_bootstrap() {
        let id = PeerId::random();
        let addr: Multiaddr = format!("/ip4/127.0.0.1/tcp/1/p2p/{id}").parse().unwrap();
        assert!(BootstrapPeer::new(id, vec![addr]).is_ok());
    }

    #[test]
    fn test_empty_bootstrap_rejected() {
        assert_eq!(
            BootstrapPeer::new(PeerId::random(), vec![]).unwrap_err(),
            P2pError::InvalidAddress
        );
    }

    #[test]
    fn test_identify_addr_filters_wrong_p2p() {
        let mgr = DiscoveryManager::new(DiscoveryConfig::for_network("devnet", vec![]).unwrap());
        let peer = PeerId::random();
        let other = PeerId::random();
        let good: Multiaddr = "/ip4/1.2.3.4/tcp/1".parse().unwrap();
        let bad: Multiaddr = format!("/ip4/1.2.3.4/tcp/1/p2p/{other}").parse().unwrap();
        let kept = mgr.filter_identify_addrs(peer, &[good.clone(), bad]);
        assert_eq!(kept, vec![good]);
    }
}
