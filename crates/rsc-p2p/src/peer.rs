//! Estado de peers de **red**. Sin voting power, stake ni cuentas RSC.

use crate::P2pError;
use libp2p::{identity, Multiaddr, PeerId};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Un archivo de identidad que no se puede usar. No se reemplaza.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityError {
    /// El archivo existe y no se pudo leer.
    Unreadable,
    /// El archivo existe y no es una clave Ed25519.
    Invalid,
    /// No había archivo y no se pudo crear.
    Persist,
}

impl std::fmt::Display for IdentityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreadable => write!(f, "p2p.key ilegible"),
            Self::Invalid => write!(f, "p2p.key inválida"),
            Self::Persist => write!(f, "no se pudo guardar p2p.key"),
        }
    }
}

/// Identidad local de red. El keypair no se serializa ni se loguea.
pub struct LocalPeerIdentity {
    peer_id: PeerId,
}

impl LocalPeerIdentity {
    pub fn from_keypair(keypair: &identity::Keypair) -> Self {
        Self {
            peer_id: PeerId::from(keypair.public()),
        }
    }

    pub fn generate() -> (Self, identity::Keypair) {
        let keypair = identity::Keypair::generate_ed25519();
        (Self::from_keypair(&keypair), keypair)
    }

    /// Identidad estable en disco. La misma computadora vuelve a ser el mismo nodo.
    ///
    /// Si el archivo existe y no decodifica como Ed25519, se conserva y se
    /// devuelve error. No se genera otra clave encima.
    pub fn load_or_create(
        path: &std::path::Path,
    ) -> Result<(Self, identity::Keypair), IdentityError> {
        if path.exists() {
            let bytes = std::fs::read(path).map_err(|_| IdentityError::Unreadable)?;
            let keypair = identity::Keypair::from_protobuf_encoding(&bytes)
                .map_err(|_| IdentityError::Invalid)?;
            if keypair.key_type() != identity::KeyType::Ed25519 {
                return Err(IdentityError::Invalid);
            }
            return Ok((Self::from_keypair(&keypair), keypair));
        }
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|_| IdentityError::Persist)?;
            }
        }
        let keypair = identity::Keypair::generate_ed25519();
        let bytes = keypair
            .to_protobuf_encoding()
            .map_err(|_| IdentityError::Persist)?;
        let tmp = path.with_extension("tmp");
        let _ = std::fs::remove_file(&tmp);
        {
            use std::io::Write;
            let mut opts = std::fs::OpenOptions::new();
            opts.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                opts.mode(0o600);
            }
            let mut file = opts.open(&tmp).map_err(|_| IdentityError::Persist)?;
            file.write_all(&bytes).map_err(|_| IdentityError::Persist)?;
            file.sync_all().map_err(|_| IdentityError::Persist)?;
        }
        std::fs::rename(&tmp, path).map_err(|_| IdentityError::Persist)?;
        let stored = std::fs::read(path).map_err(|_| IdentityError::Persist)?;
        if stored != bytes {
            return Err(IdentityError::Persist);
        }
        Ok((Self::from_keypair(&keypair), keypair))
    }

    pub fn peer_id(&self) -> PeerId {
        self.peer_id
    }
}

impl std::fmt::Debug for LocalPeerIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalPeerIdentity")
            .field("peer_id", &self.peer_id)
            .finish()
    }
}

/// Estado de conexión de un peer de red.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeerState {
    Disconnected,
    Connecting,
    Connected,
    Banned,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BanReason {
    ProtocolViolation,
    Flooding,
    MalformedMessage,
    ExcessiveRequests,
    HandshakeFailed,
}

/// Información operacional de un peer.
///
/// `last_seen` y `latency` son métricas de red. No son voting power.
#[derive(Clone, Debug)]
pub struct PeerInfo {
    pub peer_id: PeerId,
    pub addresses: Vec<Multiaddr>,
    pub connection_state: PeerState,
    pub protocols: Vec<String>,
    pub agent_version: Option<String>,
    pub rsc_protocol_version: Option<String>,
    pub last_seen: Option<Instant>,
    pub latency: Option<Duration>,
    pub banned_until: Option<Instant>,
    pub ban_reason: Option<BanReason>,
}

impl PeerInfo {
    pub fn new(peer_id: PeerId) -> Self {
        Self {
            peer_id,
            addresses: Vec::new(),
            connection_state: PeerState::Disconnected,
            protocols: Vec::new(),
            agent_version: None,
            rsc_protocol_version: None,
            last_seen: None,
            latency: None,
            banned_until: None,
            ban_reason: None,
        }
    }

    pub fn is_banned(&self, now: Instant) -> bool {
        match self.banned_until {
            Some(until) => until > now,
            None => self.connection_state == PeerState::Banned,
        }
    }
}

/// Registro en memoria de peers conocidos. No es storage persistente.
#[derive(Debug, Default)]
pub struct PeerRegistry {
    peers: HashMap<PeerId, PeerInfo>,
}

impl PeerRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, info: PeerInfo) {
        self.peers.insert(info.peer_id, info);
    }

    pub fn get(&self, id: &PeerId) -> Option<&PeerInfo> {
        self.peers.get(id)
    }

    pub fn get_mut(&mut self, id: &PeerId) -> Option<&mut PeerInfo> {
        self.peers.get_mut(id)
    }

    pub fn entry(&mut self, id: PeerId) -> &mut PeerInfo {
        self.peers.entry(id).or_insert_with(|| PeerInfo::new(id))
    }

    pub fn remove(&mut self, id: &PeerId) -> Option<PeerInfo> {
        self.peers.remove(id)
    }

    pub fn list(&self) -> impl Iterator<Item = &PeerInfo> {
        self.peers.values()
    }

    pub fn connected_count(&self) -> usize {
        self.peers
            .values()
            .filter(|p| p.connection_state == PeerState::Connected)
            .count()
    }

    pub fn ban(&mut self, id: PeerId, reason: BanReason, until: Instant) {
        let info = self.entry(id);
        info.connection_state = PeerState::Banned;
        info.ban_reason = Some(reason);
        info.banned_until = Some(until);
    }

    pub fn unban(&mut self, id: &PeerId) {
        if let Some(info) = self.peers.get_mut(id) {
            info.banned_until = None;
            info.ban_reason = None;
            if info.connection_state == PeerState::Banned {
                info.connection_state = PeerState::Disconnected;
            }
        }
    }

    pub fn ensure_not_banned(&self, id: &PeerId, now: Instant) -> Result<(), P2pError> {
        match self.peers.get(id) {
            Some(p) if p.is_banned(now) => Err(P2pError::PeerBanned),
            _ => Ok(()),
        }
    }
}

/// Política local de conexiones. No es scoring de consenso.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeerPolicy {
    pub max_peers: u32,
    pub max_inbound: u32,
    pub max_outbound: u32,
}

impl From<&crate::P2pLimits> for PeerPolicy {
    fn from(l: &crate::P2pLimits) -> Self {
        Self {
            max_peers: l.max_peers,
            max_inbound: l.max_inbound_connections,
            max_outbound: l.max_outbound_connections,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_peer_info() {
        let id = PeerId::random();
        let info = PeerInfo::new(id);
        assert_eq!(info.connection_state, PeerState::Disconnected);
        assert!(info.addresses.is_empty());
    }

    #[test]
    fn test_registry_ban() {
        let mut reg = PeerRegistry::new();
        let id = PeerId::random();
        let now = Instant::now();
        reg.ban(id, BanReason::Flooding, now + Duration::from_secs(60));
        assert_eq!(reg.ensure_not_banned(&id, now), Err(P2pError::PeerBanned));
        reg.unban(&id);
        assert!(reg.ensure_not_banned(&id, now).is_ok());
    }

    #[test]
    fn test_load_or_create_keeps_peer_id() {
        let dir = std::env::temp_dir().join(format!(
            "rsc-peer-id-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("p2p.key");
        let (first, _) = LocalPeerIdentity::load_or_create(&path).unwrap();
        let id = first.peer_id();
        let bytes = std::fs::read(&path).unwrap();
        drop(first);
        let (second, _) = LocalPeerIdentity::load_or_create(&path).unwrap();
        assert_eq!(second.peer_id(), id);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_invalid_identity_is_preserved() {
        let dir = std::env::temp_dir().join(format!(
            "rsc-peer-invalid-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("p2p.key");
        let original = b"not-an-ed25519-key".to_vec();
        std::fs::write(&path, &original).unwrap();
        let err = LocalPeerIdentity::load_or_create(&path).unwrap_err();
        assert_eq!(err, IdentityError::Invalid);
        assert_eq!(std::fs::read(&path).unwrap(), original);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_local_identity_debug_has_no_secret() {
        let (local, _) = LocalPeerIdentity::generate();
        let dbg = format!("{local:?}");
        assert!(dbg.contains("PeerId"));
        assert!(!dbg.to_ascii_lowercase().contains("secret"));
        assert!(!dbg.to_ascii_lowercase().contains("private"));
    }
}
