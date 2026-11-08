//! Presencia de red. Clasifica direcciones, traduce UPnP y AutoNAT,
//! y firma la ficha de presencia del PeerId persistente.
//! El mismo canon se vuelve a leer. No abre sockets.

use crate::{
    validate_network_id, BLOCK_SYNC_PROTOCOL, GOSSIPSUB_PROTOCOL, HANDSHAKE_PROTOCOL,
    RSC_P2P_PROTOCOL_VERSION,
};
use libp2p::{identity, Multiaddr, PeerId};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};

/// Clase de una dirección observada. `Global` es candidata, no alcance público.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddrClass {
    Loopback,
    Private,
    Cgnat,
    LinkLocal,
    Global,
    Unspecified,
}

/// Lo que AutoNAT informó. `Unknown` no es público.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NatView {
    Unknown,
    Private,
    Public,
}

/// Camino de alcance. Una multiaddr con `/p2p-circuit` es `Relay`, no IPv4 pública.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    Unknown,
    Local,
    PublicDirect,
    Relay,
}

/// Modo Kademlia que corresponde a [`NatView`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KadRole {
    Server,
    Client,
}

/// Efecto sobre el swarm. `ConfirmExternal` solo sale de AutoNAT `Public`
/// con una dirección global.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PresenceAction {
    ConfirmExternal(Multiaddr),
    WithdrawExternal(Multiaddr),
    Kad(KadRole),
    Note(PresenceNote),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PresenceNote {
    Class { address: Multiaddr, class: AddrClass },
    Candidate(Multiaddr),
    Public(Multiaddr),
    Private,
    Unknown,
    Relay(Multiaddr),
    UpnpGatewayMissing,
    UpnpNotRoutable,
    Ignored(Multiaddr),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Presence {
    view: NatView,
    path: Reach,
    candidates: Vec<Multiaddr>,
    confirmed: Option<Multiaddr>,
}

impl Presence {
    pub fn new() -> Self {
        Self {
            view: NatView::Unknown,
            path: Reach::Unknown,
            candidates: Vec::new(),
            confirmed: None,
        }
    }

    pub fn view(&self) -> NatView {
        self.view
    }

    pub fn path(&self) -> Reach {
        self.path
    }

    pub fn confirmed(&self) -> Option<&Multiaddr> {
        self.confirmed.as_ref()
    }

    /// `Unknown` conserva el modo servidor del DHT local y no publica dirección.
    pub fn on_listen(&mut self, address: Multiaddr) -> Vec<PresenceAction> {
        if is_circuit(&address) {
            return Vec::new();
        }
        let class = classify(&address);
        vec![PresenceAction::Note(PresenceNote::Class { address, class })]
    }

    /// Reserva de relay. No confirma una IPv4 pública directa.
    pub fn on_relay(&mut self, address: Multiaddr) -> Vec<PresenceAction> {
        if self.path != Reach::PublicDirect {
            self.path = Reach::Relay;
        }
        vec![PresenceAction::Note(PresenceNote::Relay(address))]
    }

    /// La reserva cayó. No queda un estado directo.
    pub fn on_relay_lost(&mut self) -> Vec<PresenceAction> {
        if self.path != Reach::Relay {
            return Vec::new();
        }
        self.path = Reach::Unknown;
        vec![PresenceAction::Note(PresenceNote::Unknown)]
    }

    /// UPnP `NewExternalAddr` es candidata. No confirma alcance.
    pub fn on_upnp_candidate(&mut self, external: Multiaddr) -> Vec<PresenceAction> {
        if is_circuit(&external) {
            return self.on_relay(external);
        }
        let mut actions = Vec::new();
        if classify(&external) != AddrClass::Global {
            actions.push(PresenceAction::WithdrawExternal(external.clone()));
            actions.push(PresenceAction::Note(PresenceNote::Ignored(external)));
            return actions;
        }
        if !self.candidates.iter().any(|addr| addr == &external) {
            self.candidates.push(external.clone());
        }
        if self.confirmed.as_ref() != Some(&external) {
            actions.push(PresenceAction::WithdrawExternal(external.clone()));
        }
        actions.push(PresenceAction::Note(PresenceNote::Candidate(external)));
        actions
    }

    pub fn on_upnp_expired(&mut self, external: Multiaddr) -> Vec<PresenceAction> {
        self.candidates.retain(|addr| addr != &external);
        let mut actions = vec![PresenceAction::WithdrawExternal(external.clone())];
        if self.confirmed.as_ref() == Some(&external) {
            self.confirmed = None;
            self.view = NatView::Unknown;
            actions.push(PresenceAction::Kad(kad_role(self.view)));
            actions.push(PresenceAction::Note(PresenceNote::Unknown));
        }
        actions
    }

    pub fn on_upnp_gateway_missing(&mut self) -> Vec<PresenceAction> {
        vec![PresenceAction::Note(PresenceNote::UpnpGatewayMissing)]
    }

    pub fn on_upnp_not_routable(&mut self) -> Vec<PresenceAction> {
        vec![PresenceAction::Note(PresenceNote::UpnpNotRoutable)]
    }

    pub fn on_autonat_unknown(&mut self) -> Vec<PresenceAction> {
        self.set_view(NatView::Unknown, None)
    }

    pub fn on_autonat_private(&mut self) -> Vec<PresenceAction> {
        self.set_view(NatView::Private, None)
    }

    /// Solo una dirección global pasa a `add_external_address`.
    pub fn on_autonat_public(&mut self, address: Multiaddr) -> Vec<PresenceAction> {
        if is_circuit(&address) {
            return self.on_relay(address);
        }
        if classify(&address) != AddrClass::Global {
            let mut actions = vec![PresenceAction::Note(PresenceNote::Ignored(address.clone()))];
            actions.extend(self.set_view(NatView::Unknown, None));
            return actions;
        }
        self.set_view(NatView::Public, Some(address))
    }

    fn set_view(&mut self, view: NatView, confirmed: Option<Multiaddr>) -> Vec<PresenceAction> {
        let previous = self.confirmed.clone();
        self.view = view;
        self.path = match (view, self.path) {
            (NatView::Public, _) => Reach::PublicDirect,
            (_, Reach::Relay) => Reach::Relay,
            (NatView::Private, _) => Reach::Local,
            (NatView::Unknown, _) => Reach::Unknown,
        };
        self.confirmed = confirmed.clone();
        let mut actions = Vec::new();
        if let Some(old) = previous {
            if Some(&old) != confirmed.as_ref() {
                actions.push(PresenceAction::WithdrawExternal(old));
            }
        }
        if let Some(addr) = confirmed {
            actions.push(PresenceAction::ConfirmExternal(addr.clone()));
            actions.push(PresenceAction::Note(PresenceNote::Public(addr)));
        } else {
            actions.push(PresenceAction::Note(match view {
                NatView::Private => PresenceNote::Private,
                NatView::Unknown | NatView::Public => PresenceNote::Unknown,
            }));
        }
        actions.push(PresenceAction::Kad(kad_role(view)));
        actions
    }
}

impl Default for Presence {
    fn default() -> Self {
        Self::new()
    }
}

pub fn kad_role(view: NatView) -> KadRole {
    match view {
        NatView::Unknown | NatView::Public => KadRole::Server,
        NatView::Private => KadRole::Client,
    }
}

pub fn is_circuit(address: &Multiaddr) -> bool {
    address
        .iter()
        .any(|protocol| matches!(protocol, libp2p::multiaddr::Protocol::P2pCircuit))
}

pub fn classify(address: &Multiaddr) -> AddrClass {
    for protocol in address.iter() {
        match protocol {
            libp2p::multiaddr::Protocol::Ip4(ip) => return class_v4(ip),
            libp2p::multiaddr::Protocol::Ip6(ip) => return class_v6(ip),
            _ => {}
        }
    }
    AddrClass::Unspecified
}

fn class_v4(ip: Ipv4Addr) -> AddrClass {
    let octets = ip.octets();
    if ip.is_loopback() {
        return AddrClass::Loopback;
    }
    if ip.is_unspecified() || octets[0] == 0 {
        return AddrClass::Unspecified;
    }
    if octets[0] == 100 && (octets[1] & 0b1100_0000) == 0b0100_0000 {
        return AddrClass::Cgnat;
    }
    if ip.is_link_local() {
        return AddrClass::LinkLocal;
    }
    if ip.is_private() {
        return AddrClass::Private;
    }
    if ip.is_multicast() || ip.is_broadcast() {
        return AddrClass::Unspecified;
    }
    AddrClass::Global
}

fn class_v6(ip: Ipv6Addr) -> AddrClass {
    if ip.is_loopback() {
        return AddrClass::Loopback;
    }
    if ip.is_unspecified() {
        return AddrClass::Unspecified;
    }
    let octets = ip.octets();
    if octets[0] == 0xfe && (octets[1] & 0b1100_0000) == 0b1000_0000 {
        return AddrClass::LinkLocal;
    }
    if octets[0] & 0xfe == 0xfc {
        return AddrClass::Private;
    }
    if octets[0] & 0xe0 == 0x20 {
        return AddrClass::Global;
    }
    AddrClass::Unspecified
}

pub fn autonomous_listen() -> Vec<Multiaddr> {
    vec![
        "/ip4/0.0.0.0/tcp/0".parse().expect("tcp4"),
        "/ip6/::/tcp/0".parse().expect("tcp6"),
        "/ip4/0.0.0.0/udp/0/quic-v1".parse().expect("quic4"),
        "/ip6/::/udp/0/quic-v1".parse().expect("quic6"),
    ]
}

pub fn format_note(note: &PresenceNote) -> String {
    match note {
        PresenceNote::Class { address, class } => format!("class {address} {}", class_name(*class)),
        PresenceNote::Candidate(addr) => format!("presence candidate {addr}"),
        PresenceNote::Public(addr) => format!("presence public {addr}"),
        PresenceNote::Private => "presence private".to_string(),
        PresenceNote::Unknown => "presence unknown".to_string(),
        PresenceNote::UpnpGatewayMissing => "presence upnp gateway-not-found".to_string(),
        PresenceNote::UpnpNotRoutable => "presence upnp non-routable".to_string(),
        PresenceNote::Ignored(addr) => format!("presence ignored {addr}"),
        PresenceNote::Relay(addr) => format!("presence relay {addr}"),
    }
}

const PRESENCE_MAGIC: &[u8] = b"RSC-NP";
const PRESENCE_VERSION: u8 = 1;

/// Error de una ficha de presencia. No es un error de bloque, transacción ni QC.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PresenceError {
    #[error("invalid signature")]
    InvalidSignature,
    #[error("identity mismatch")]
    IdentityMismatch,
    #[error("expired presence")]
    Expired,
    #[error("stale sequence")]
    StaleSequence,
    #[error("network mismatch")]
    NetworkMismatch,
    #[error("chain mismatch")]
    ChainMismatch,
    #[error("not publishable")]
    NotPublishable,
    #[error("not relay address")]
    NotRelayAddress,
    #[error("not public")]
    NotPublic,
    #[error("invalid presence")]
    Invalid,
}

/// Vista de cadena que el nodo ya conoce. Un relay sin cadena la omite.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChainView {
    pub latest_height: u64,
    pub latest_block_hash: [u8; 32],
    pub finalized_height: u64,
    pub epoch: u64,
}

/// Lo que se firma. La clave pública va dentro porque el `PeerId` no la devuelve.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PresenceRecord {
    pub network_id: String,
    pub chain_id: u32,
    pub peer_id: PeerId,
    pub public_key: Vec<u8>,
    pub protocols: [String; 4],
    pub chain: Option<ChainView>,
    pub reach: Reach,
    pub addresses: Vec<Multiaddr>,
    pub issued_at: u64,
    pub expires_at: u64,
    pub sequence: u64,
}

/// Ficha y firma Ed25519 de 64 bytes sobre [`PresenceRecord::canonical_bytes`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedPresence {
    pub record: PresenceRecord,
    pub signature: [u8; 64],
}

/// Datos que el nodo rellena antes de firmar. `nat` no entra en el canon.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PresenceDraft {
    pub network_id: String,
    pub chain_id: u32,
    pub chain: Option<ChainView>,
    pub reach: Reach,
    pub nat: NatView,
    pub addresses: Vec<Multiaddr>,
    pub issued_at: u64,
    pub expires_at: u64,
    pub sequence: u64,
}

impl PresenceDraft {
    /// Firma con la clave Ed25519 de `--persist-peer`.
    pub fn sign(self, keypair: &identity::Keypair) -> Result<SignedPresence, PresenceError> {
        if keypair.key_type() != identity::KeyType::Ed25519 {
            return Err(PresenceError::IdentityMismatch);
        }
        validate_network_id(&self.network_id).map_err(|_| PresenceError::Invalid)?;
        screen_addresses(self.reach, self.nat, &self.addresses)?;
        let public = keypair.public();
        let record = PresenceRecord {
            network_id: self.network_id,
            chain_id: self.chain_id,
            peer_id: PeerId::from(&public),
            public_key: public.encode_protobuf(),
            protocols: protocol_list(),
            chain: self.chain,
            reach: self.reach,
            addresses: self.addresses,
            issued_at: self.issued_at,
            expires_at: self.expires_at,
            sequence: self.sequence,
        };
        let canon = record.canonical_bytes()?;
        let raw = keypair
            .sign(&canon)
            .map_err(|_| PresenceError::InvalidSignature)?;
        let signature = <[u8; 64]>::try_from(raw.as_slice())
            .map_err(|_| PresenceError::InvalidSignature)?;
        Ok(SignedPresence { record, signature })
    }
}

impl SignedPresence {
    /// La clave protobuf es Ed25519 y su `PeerId` es el anunciado. La firma cubre el canon.
    pub fn verify(&self) -> Result<(), PresenceError> {
        let key = identity::PublicKey::try_decode_protobuf(&self.record.public_key)
            .map_err(|_| PresenceError::IdentityMismatch)?;
        if key.key_type() != identity::KeyType::Ed25519 {
            return Err(PresenceError::IdentityMismatch);
        }
        if PeerId::from(&key) != self.record.peer_id {
            return Err(PresenceError::IdentityMismatch);
        }
        let canon = self.record.canonical_bytes()?;
        if key.verify(&canon, &self.signature) {
            Ok(())
        } else {
            Err(PresenceError::InvalidSignature)
        }
    }

    /// Tercero: firma, red, cadena, vencimiento, secuencia y direcciones.
    pub fn accept(
        &self,
        now: u64,
        network_id: &str,
        chain_id: u32,
        latest_sequence: u64,
    ) -> Result<(), PresenceError> {
        self.verify()?;
        if self.record.network_id != network_id {
            return Err(PresenceError::NetworkMismatch);
        }
        if self.record.chain_id != chain_id {
            return Err(PresenceError::ChainMismatch);
        }
        if self.record.protocols != protocol_list() {
            return Err(PresenceError::Invalid);
        }
        if self.record.expires_at <= now {
            return Err(PresenceError::Expired);
        }
        if self.record.sequence < latest_sequence {
            return Err(PresenceError::StaleSequence);
        }
        published_addresses(self.record.reach, &self.record.addresses)?;
        Ok(())
    }

    /// Canon seguido de la firma de 64 bytes. No reordena campos.
    pub fn to_wire(&self) -> Result<Vec<u8>, PresenceError> {
        let mut wire = self.record.canonical_bytes()?;
        wire.extend_from_slice(&self.signature);
        Ok(wire)
    }

    /// Una sola ficha. Bytes de más después de la firma son `invalid presence`.
    pub fn from_wire(bytes: &[u8]) -> Result<Self, PresenceError> {
        let (signed, consumed) = read_signed(bytes)?;
        if consumed != bytes.len() {
            return Err(PresenceError::Invalid);
        }
        Ok(signed)
    }

    /// Varias fichas seguidas, cada una con su canon y su firma.
    pub fn read_list(bytes: &[u8]) -> Result<Vec<Self>, PresenceError> {
        let mut out = Vec::new();
        let mut rest = bytes;
        while !rest.is_empty() {
            let (signed, consumed) = read_signed(rest)?;
            out.push(signed);
            rest = &rest[consumed..];
        }
        Ok(out)
    }
}

impl PresenceRecord {
    /// Bytes deterministas. La firma no forma parte de este buffer.
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, PresenceError> {
        let mut out = Vec::new();
        out.extend_from_slice(PRESENCE_MAGIC);
        out.push(PRESENCE_VERSION);
        push_str(&mut out, &self.network_id)?;
        out.extend_from_slice(&self.chain_id.to_be_bytes());
        push_bytes(&mut out, &self.peer_id.to_bytes())?;
        push_blob(&mut out, &self.public_key)?;
        out.push(u8::try_from(self.protocols.len()).map_err(|_| PresenceError::Invalid)?);
        for protocol in &self.protocols {
            push_str(&mut out, protocol)?;
        }
        match self.chain {
            None => out.push(0),
            Some(chain) => {
                out.push(1);
                out.extend_from_slice(&chain.latest_height.to_be_bytes());
                out.extend_from_slice(&chain.latest_block_hash);
                out.extend_from_slice(&chain.finalized_height.to_be_bytes());
                out.extend_from_slice(&chain.epoch.to_be_bytes());
            }
        }
        out.push(reach_byte(self.reach));
        if self.addresses.len() > u16::MAX as usize {
            return Err(PresenceError::Invalid);
        }
        out.extend_from_slice(&(self.addresses.len() as u16).to_be_bytes());
        for address in &self.addresses {
            push_blob(&mut out, &address.to_vec())?;
        }
        out.extend_from_slice(&self.issued_at.to_be_bytes());
        out.extend_from_slice(&self.expires_at.to_be_bytes());
        out.extend_from_slice(&self.sequence.to_be_bytes());
        Ok(out)
    }

    /// Lee el canon. El prefijo releído tiene que ser idéntico a estos bytes.
    pub fn decode_prefix(bytes: &[u8]) -> Result<(Self, usize), PresenceError> {
        let mut cur = Cursor { bytes, at: 0 };
        if cur.take(PRESENCE_MAGIC.len())? != PRESENCE_MAGIC {
            return Err(PresenceError::Invalid);
        }
        if cur.be_u8()? != PRESENCE_VERSION {
            return Err(PresenceError::Invalid);
        }
        let network_id = cur.text_u8()?;
        validate_network_id(&network_id).map_err(|_| PresenceError::Invalid)?;
        let chain_id = cur.be_u32()?;
        let peer_raw = cur.vec_u8()?;
        let peer_id = PeerId::from_bytes(&peer_raw).map_err(|_| PresenceError::Invalid)?;
        let public_key = cur.vec_u16()?;
        if cur.be_u8()? != 4 {
            return Err(PresenceError::Invalid);
        }
        let mut protocols = [String::new(), String::new(), String::new(), String::new()];
        for slot in &mut protocols {
            *slot = cur.text_u8()?;
        }
        let chain = match cur.be_u8()? {
            0 => None,
            1 => {
                let latest_height = cur.be_u64()?;
                let mut latest_block_hash = [0u8; 32];
                latest_block_hash.copy_from_slice(cur.take(32)?);
                Some(ChainView {
                    latest_height,
                    latest_block_hash,
                    finalized_height: cur.be_u64()?,
                    epoch: cur.be_u64()?,
                })
            }
            _ => return Err(PresenceError::Invalid),
        };
        let reach = reach_from(cur.be_u8()?)?;
        let addr_count = cur.be_u16()? as usize;
        let mut addresses = Vec::with_capacity(addr_count);
        for _ in 0..addr_count {
            let raw = cur.vec_u16()?;
            addresses.push(Multiaddr::try_from(raw).map_err(|_| PresenceError::Invalid)?);
        }
        let record = PresenceRecord {
            network_id,
            chain_id,
            peer_id,
            public_key,
            protocols,
            chain,
            reach,
            addresses,
            issued_at: cur.be_u64()?,
            expires_at: cur.be_u64()?,
            sequence: cur.be_u64()?,
        };
        let consumed = cur.at;
        if record.canonical_bytes()?.as_slice() != &bytes[..consumed] {
            return Err(PresenceError::Invalid);
        }
        Ok((record, consumed))
    }
}

fn read_signed(bytes: &[u8]) -> Result<(SignedPresence, usize), PresenceError> {
    let (record, canon_len) = PresenceRecord::decode_prefix(bytes)?;
    let end = canon_len.checked_add(64).ok_or(PresenceError::Invalid)?;
    if end > bytes.len() {
        return Err(PresenceError::Invalid);
    }
    let mut signature = [0u8; 64];
    signature.copy_from_slice(&bytes[canon_len..end]);
    Ok((SignedPresence { record, signature }, end))
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn take(&mut self, len: usize) -> Result<&[u8], PresenceError> {
        let end = self.at.checked_add(len).ok_or(PresenceError::Invalid)?;
        let out = self.bytes.get(self.at..end).ok_or(PresenceError::Invalid)?;
        self.at = end;
        Ok(out)
    }

    fn be_u8(&mut self) -> Result<u8, PresenceError> {
        Ok(self.take(1)?[0])
    }

    fn be_u16(&mut self) -> Result<u16, PresenceError> {
        let mut buf = [0u8; 2];
        buf.copy_from_slice(self.take(2)?);
        Ok(u16::from_be_bytes(buf))
    }

    fn be_u32(&mut self) -> Result<u32, PresenceError> {
        let mut buf = [0u8; 4];
        buf.copy_from_slice(self.take(4)?);
        Ok(u32::from_be_bytes(buf))
    }

    fn be_u64(&mut self) -> Result<u64, PresenceError> {
        let mut buf = [0u8; 8];
        buf.copy_from_slice(self.take(8)?);
        Ok(u64::from_be_bytes(buf))
    }

    fn vec_u8(&mut self) -> Result<Vec<u8>, PresenceError> {
        let len = self.be_u8()? as usize;
        Ok(self.take(len)?.to_vec())
    }

    fn vec_u16(&mut self) -> Result<Vec<u8>, PresenceError> {
        let len = self.be_u16()? as usize;
        Ok(self.take(len)?.to_vec())
    }

    fn text_u8(&mut self) -> Result<String, PresenceError> {
        String::from_utf8(self.vec_u8()?).map_err(|_| PresenceError::Invalid)
    }
}

/// Direcciones que el nodo puede poner en la ficha.
pub fn screen_addresses(
    reach: Reach,
    nat: NatView,
    addresses: &[Multiaddr],
) -> Result<(), PresenceError> {
    if reach == Reach::PublicDirect && nat != NatView::Public {
        return Err(PresenceError::NotPublic);
    }
    published_addresses(reach, addresses)
}

fn published_addresses(reach: Reach, addresses: &[Multiaddr]) -> Result<(), PresenceError> {
    match reach {
        Reach::Unknown | Reach::Local => {
            if addresses.is_empty() {
                Ok(())
            } else {
                Err(PresenceError::NotPublishable)
            }
        }
        Reach::PublicDirect => {
            if addresses.is_empty() {
                return Err(PresenceError::NotPublishable);
            }
            for address in addresses {
                if is_circuit(address) || classify(address) != AddrClass::Global {
                    return Err(PresenceError::NotPublishable);
                }
            }
            Ok(())
        }
        Reach::Relay => {
            if addresses.is_empty() || addresses.iter().any(|address| !is_circuit(address)) {
                Err(PresenceError::NotRelayAddress)
            } else {
                Ok(())
            }
        }
    }
}

fn protocol_list() -> [String; 4] {
    [
        RSC_P2P_PROTOCOL_VERSION.to_string(),
        HANDSHAKE_PROTOCOL.to_string(),
        BLOCK_SYNC_PROTOCOL.to_string(),
        GOSSIPSUB_PROTOCOL.to_string(),
    ]
}

fn reach_byte(reach: Reach) -> u8 {
    match reach {
        Reach::Unknown => 0,
        Reach::Local => 1,
        Reach::PublicDirect => 2,
        Reach::Relay => 3,
    }
}

fn reach_from(byte: u8) -> Result<Reach, PresenceError> {
    match byte {
        0 => Ok(Reach::Unknown),
        1 => Ok(Reach::Local),
        2 => Ok(Reach::PublicDirect),
        3 => Ok(Reach::Relay),
        _ => Err(PresenceError::Invalid),
    }
}

fn push_str(out: &mut Vec<u8>, value: &str) -> Result<(), PresenceError> {
    push_bytes(out, value.as_bytes())
}

fn push_bytes(out: &mut Vec<u8>, value: &[u8]) -> Result<(), PresenceError> {
    let len = u8::try_from(value.len()).map_err(|_| PresenceError::Invalid)?;
    out.push(len);
    out.extend_from_slice(value);
    Ok(())
}

fn push_blob(out: &mut Vec<u8>, value: &[u8]) -> Result<(), PresenceError> {
    let len = u16::try_from(value.len()).map_err(|_| PresenceError::Invalid)?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(value);
    Ok(())
}

/// Archivo de secuencia junto a la clave. No modifica los bytes Ed25519.
pub fn sequence_path(key_path: &Path) -> PathBuf {
    let mut name = key_path.as_os_str().to_owned();
    name.push(".seq");
    PathBuf::from(name)
}

/// `0` si todavía no hay ficha. Cualquier otro tamaño es inválido.
pub fn read_sequence(key_path: &Path) -> Result<u64, PresenceError> {
    let path = sequence_path(key_path);
    match std::fs::read(&path) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Ok(bytes) if bytes.len() == 8 => {
            let mut buf = [0u8; 8];
            buf.copy_from_slice(&bytes);
            Ok(u64::from_be_bytes(buf))
        }
        _ => Err(PresenceError::Invalid),
    }
}

/// Escribe `value` en `{clave}.seq` con modo `0600` (Unix). La clave no se toca.
pub fn write_sequence(key_path: &Path, value: u64) -> Result<(), PresenceError> {
    use std::io::Write;
    let path = sequence_path(key_path);
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(PresenceError::Invalid)?;
    let parent = path.parent().filter(|dir| dir.is_dir()).ok_or(PresenceError::Invalid)?;
    let tmp = parent.join(format!(".{name}.tmp"));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(&tmp).map_err(|_| PresenceError::Invalid)?;
    file.write_all(&value.to_be_bytes())
        .map_err(|_| PresenceError::Invalid)?;
    file.sync_all().map_err(|_| PresenceError::Invalid)?;
    drop(file);
    std::fs::rename(&tmp, &path).map_err(|_| PresenceError::Invalid)?;
    Ok(())
}

/// Siguiente secuencia persistida. Sobrevive al reinicio del proceso.
pub fn advance_sequence(key_path: &Path) -> Result<u64, PresenceError> {
    let next = read_sequence(key_path)?
        .checked_add(1)
        .ok_or(PresenceError::Invalid)?;
    write_sequence(key_path, next)?;
    Ok(next)
}

fn class_name(class: AddrClass) -> &'static str {
    match class {
        AddrClass::Loopback => "loopback",
        AddrClass::Private => "private",
        AddrClass::Cgnat => "cgnat",
        AddrClass::LinkLocal => "link-local",
        AddrClass::Global => "global",
        AddrClass::Unspecified => "unspecified",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip4(text: &str) -> Multiaddr {
        format!("/ip4/{text}/tcp/1").parse().unwrap()
    }

    fn confirms(actions: &[PresenceAction]) -> Vec<&Multiaddr> {
        actions
            .iter()
            .filter_map(|action| match action {
                PresenceAction::ConfirmExternal(addr) => Some(addr),
                _ => None,
            })
            .collect()
    }

    fn role(actions: &[PresenceAction]) -> Option<KadRole> {
        actions.iter().rev().find_map(|action| match action {
            PresenceAction::Kad(role) => Some(*role),
            _ => None,
        })
    }

    #[test]
    fn unit_loopback_is_not_external() {
        let addr = ip4("127.0.0.1");
        assert_eq!(classify(&addr), AddrClass::Loopback);
        let mut presence = Presence::new();
        assert!(confirms(&presence.on_listen(addr)).is_empty());
        assert!(presence.confirmed().is_none());
    }

    #[test]
    fn unit_private_ranges_are_not_external() {
        for text in ["10.0.0.1", "172.16.0.1", "192.168.1.12"] {
            let addr = ip4(text);
            assert_eq!(classify(&addr), AddrClass::Private, "{text}");
            let mut presence = Presence::new();
            assert!(confirms(&presence.on_listen(addr.clone())).is_empty());
            assert!(confirms(&presence.on_upnp_candidate(addr)).is_empty());
            assert!(presence.confirmed().is_none());
        }
    }

    #[test]
    fn unit_cgnat_is_not_external() {
        let addr = ip4("100.64.0.1");
        assert_eq!(classify(&addr), AddrClass::Cgnat);
        let mut presence = Presence::new();
        assert!(confirms(&presence.on_listen(addr.clone())).is_empty());
        assert!(confirms(&presence.on_upnp_candidate(addr)).is_empty());
    }

    #[test]
    fn unit_link_local_is_not_external() {
        assert_eq!(classify(&ip4("169.254.1.1")), AddrClass::LinkLocal);
        let v6: Multiaddr = "/ip6/fe80::1/tcp/1".parse().unwrap();
        assert_eq!(classify(&v6), AddrClass::LinkLocal);
    }

    #[test]
    fn unit_global_listen_stays_candidate() {
        let addr = ip4("1.2.3.4");
        assert_eq!(classify(&addr), AddrClass::Global);
        let mut presence = Presence::new();
        let actions = presence.on_listen(addr);
        assert!(confirms(&actions).is_empty());
        assert_eq!(presence.view(), NatView::Unknown);
    }

    #[test]
    fn unit_autonat_unknown_does_not_publish() {
        let mut presence = Presence::new();
        let actions = presence.on_autonat_unknown();
        assert!(confirms(&actions).is_empty());
        assert_eq!(role(&actions), Some(KadRole::Server));
        assert_eq!(presence.view(), NatView::Unknown);
        assert!(presence.confirmed().is_none());
    }

    #[test]
    fn unit_autonat_private_is_client() {
        let mut presence = Presence::new();
        let actions = presence.on_autonat_private();
        assert!(confirms(&actions).is_empty());
        assert_eq!(role(&actions), Some(KadRole::Client));
        assert_eq!(presence.view(), NatView::Private);
    }

    #[test]
    fn unit_autonat_public_confirms_global_and_server() {
        let addr = ip4("203.0.113.10");
        let mut presence = Presence::new();
        let actions = presence.on_autonat_public(addr.clone());
        assert_eq!(confirms(&actions), vec![&addr]);
        assert_eq!(role(&actions), Some(KadRole::Server));
        assert_eq!(presence.view(), NatView::Public);
        assert_eq!(presence.confirmed(), Some(&addr));
    }

    #[test]
    fn unit_autonat_public_rejects_private_address() {
        let addr = ip4("10.1.1.1");
        let mut presence = Presence::new();
        let actions = presence.on_autonat_public(addr);
        assert!(confirms(&actions).is_empty());
        assert_ne!(presence.view(), NatView::Public);
        assert!(presence.confirmed().is_none());
    }

    #[test]
    fn unit_upnp_gateway_events_do_not_confirm() {
        let mut presence = Presence::new();
        assert!(confirms(&presence.on_upnp_gateway_missing()).is_empty());
        assert!(confirms(&presence.on_upnp_not_routable()).is_empty());
        assert_eq!(presence.view(), NatView::Unknown);
    }

    #[test]
    fn unit_upnp_candidate_is_withdrawn_until_autonat() {
        let addr = ip4("198.51.100.8");
        let mut presence = Presence::new();
        let actions = presence.on_upnp_candidate(addr.clone());
        assert!(confirms(&actions).is_empty());
        assert!(actions.iter().any(|action| matches!(
            action,
            PresenceAction::WithdrawExternal(got) if got == &addr
        )));
        assert!(presence.confirmed().is_none());
    }

    #[test]
    fn unit_circuit_address_is_relay_not_public() {
        let peer = libp2p::PeerId::random();
        let addr: Multiaddr = "/ip4/203.0.113.10/tcp/4001"
            .parse::<Multiaddr>()
            .unwrap()
            .with(libp2p::multiaddr::Protocol::P2p(peer))
            .with(libp2p::multiaddr::Protocol::P2pCircuit);
        assert!(is_circuit(&addr));
        let mut presence = Presence::new();
        let actions = presence.on_autonat_public(addr.clone());
        assert!(confirms(&actions).is_empty());
        assert_eq!(presence.view(), NatView::Unknown);
        assert_eq!(presence.path(), Reach::Relay);
        assert!(presence.confirmed().is_none());
        let note = actions.iter().find_map(|action| match action {
            PresenceAction::Note(note) => Some(format_note(note)),
            _ => None,
        });
        let note = note.unwrap();
        assert!(note.starts_with("presence relay "));
        assert!(note.contains("p2p-circuit"));
        assert!(!note.contains("presence public"));
        let lost = presence.on_relay_lost();
        assert_eq!(presence.path(), Reach::Unknown);
        assert_eq!(
            lost.iter().find_map(|action| match action {
                PresenceAction::Note(note) => Some(format_note(note)),
                _ => None,
            }),
            Some("presence unknown".to_string())
        );
    }

    fn draft(reach: Reach, nat: NatView, addresses: Vec<Multiaddr>) -> PresenceDraft {
        PresenceDraft {
            network_id: "devnet".into(),
            chain_id: 3,
            chain: Some(ChainView {
                latest_height: 35,
                latest_block_hash: [0x11; 32],
                finalized_height: 35,
                epoch: 3,
            }),
            reach,
            nat,
            addresses,
            issued_at: 1_000,
            expires_at: 1_300,
            sequence: 4,
        }
    }

    fn signed() -> SignedPresence {
        let key = libp2p::identity::Keypair::generate_ed25519();
        draft(
            Reach::PublicDirect,
            NatView::Public,
            vec!["/ip4/203.0.113.10/tcp/4001".parse().unwrap()],
        )
        .sign(&key)
        .unwrap()
    }

    #[test]
    fn unit_canonical_bytes_are_deterministic() {
        let key = libp2p::identity::Keypair::generate_ed25519();
        let left = draft(Reach::Local, NatView::Private, vec![])
            .sign(&key)
            .unwrap();
        let right = draft(Reach::Local, NatView::Private, vec![])
            .sign(&key)
            .unwrap();
        assert_eq!(left.record.canonical_bytes().unwrap(), right.record.canonical_bytes().unwrap());
        assert_eq!(left.signature, right.signature);
        let mut other = left.record.clone();
        other.sequence += 1;
        assert_ne!(
            left.record.canonical_bytes().unwrap(),
            other.canonical_bytes().unwrap()
        );
    }

    #[test]
    fn unit_valid_signature_roundtrip() {
        let card = signed();
        assert!(card.verify().is_ok());
        assert!(card.accept(1_100, "devnet", 3, 4).is_ok());
        assert!(card.record.chain.is_some());
    }

    #[test]
    fn unit_relay_without_chain_omits_chain_fields() {
        let key = libp2p::identity::Keypair::generate_ed25519();
        let peer = libp2p::PeerId::random();
        let circuit = "/ip4/198.51.100.8/tcp/4001"
            .parse::<Multiaddr>()
            .unwrap()
            .with(libp2p::multiaddr::Protocol::P2p(peer))
            .with(libp2p::multiaddr::Protocol::P2pCircuit);
        let mut input = draft(Reach::Relay, NatView::Private, vec![circuit]);
        input.chain = None;
        let card = input.sign(&key).unwrap();
        assert!(card.record.chain.is_none());
        assert!(card.verify().is_ok());
        let with_chain = draft(
            Reach::Relay,
            NatView::Private,
            card.record.addresses.clone(),
        )
        .sign(&key)
        .unwrap();
        assert_ne!(
            card.record.canonical_bytes().unwrap(),
            with_chain.record.canonical_bytes().unwrap()
        );
    }

    #[test]
    fn unit_altered_signature_is_rejected() {
        let mut card = signed();
        card.signature[63] ^= 0x01;
        assert_eq!(card.verify(), Err(PresenceError::InvalidSignature));
    }

    #[test]
    fn unit_altered_peer_id_is_identity_mismatch() {
        let mut card = signed();
        card.record.peer_id = libp2p::PeerId::random();
        assert_eq!(card.verify(), Err(PresenceError::IdentityMismatch));
    }

    #[test]
    fn unit_other_network_and_chain_are_rejected() {
        let card = signed();
        assert_eq!(
            card.accept(1_100, "other", 3, 4),
            Err(PresenceError::NetworkMismatch)
        );
        assert_eq!(
            card.accept(1_100, "devnet", 1, 4),
            Err(PresenceError::ChainMismatch)
        );
    }

    #[test]
    fn unit_altered_issued_at_breaks_signature() {
        let mut card = signed();
        card.record.issued_at += 1;
        assert_eq!(card.verify(), Err(PresenceError::InvalidSignature));
    }

    #[test]
    fn unit_expired_presence_is_rejected() {
        let card = signed();
        assert_eq!(
            card.accept(1_300, "devnet", 3, 4),
            Err(PresenceError::Expired)
        );
        assert!(card.accept(1_299, "devnet", 3, 4).is_ok());
    }

    #[test]
    fn unit_lower_sequence_is_stale() {
        let card = signed();
        assert_eq!(
            card.accept(1_100, "devnet", 3, 5),
            Err(PresenceError::StaleSequence)
        );
        assert!(card.accept(1_100, "devnet", 3, 4).is_ok());
    }

    #[test]
    fn unit_sequence_survives_restart_and_keeps_key() {
        let dir = std::env::temp_dir().join(format!(
            "rsc-presence-seq-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let key_path = dir.join("peer.key");
        let key = libp2p::identity::Keypair::generate_ed25519();
        let bytes = key.to_protobuf_encoding().unwrap();
        std::fs::write(&key_path, &bytes).unwrap();
        assert_eq!(advance_sequence(&key_path).unwrap(), 1);
        assert_eq!(std::fs::read(&key_path).unwrap(), bytes);
        assert_eq!(read_sequence(&key_path).unwrap(), 1);
        assert_eq!(advance_sequence(&key_path).unwrap(), 2);
        assert_eq!(std::fs::read(&key_path).unwrap(), bytes);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unit_direct_addresses_follow_class_and_autonat() {
        let cases = [
            ("10.1.1.1", PresenceError::NotPublishable),
            ("100.64.1.1", PresenceError::NotPublishable),
            ("127.0.0.1", PresenceError::NotPublishable),
            ("169.254.1.1", PresenceError::NotPublishable),
        ];
        for (ip, expected) in cases {
            let addr = ip4(ip);
            assert_eq!(
                screen_addresses(Reach::PublicDirect, NatView::Public, &[addr]),
                Err(expected),
                "{ip}"
            );
        }
        let global = ip4("203.0.113.10");
        assert_eq!(
            screen_addresses(
                Reach::PublicDirect,
                NatView::Unknown,
                std::slice::from_ref(&global),
            ),
            Err(PresenceError::NotPublic)
        );
        assert!(screen_addresses(
            Reach::PublicDirect,
            NatView::Public,
            std::slice::from_ref(&global),
        )
        .is_ok());
    }

    #[test]
    fn unit_relay_requires_circuit() {
        let plain = ip4("203.0.113.10");
        assert_eq!(
            screen_addresses(Reach::Relay, NatView::Private, &[plain]),
            Err(PresenceError::NotRelayAddress)
        );
        let peer = libp2p::PeerId::random();
        let circuit = ip4("203.0.113.10")
            .with(libp2p::multiaddr::Protocol::P2p(peer))
            .with(libp2p::multiaddr::Protocol::P2pCircuit);
        assert!(screen_addresses(Reach::Relay, NatView::Private, &[circuit]).is_ok());
    }

    #[test]
    fn unit_wire_roundtrip_rejects_trailing_bytes() {
        let card = signed();
        let wire = card.to_wire().unwrap();
        assert_eq!(SignedPresence::from_wire(&wire).unwrap(), card);
        assert!(SignedPresence::from_wire(&wire[..wire.len() - 1]).is_err());
        let mut extra = wire.clone();
        extra.push(0);
        assert_eq!(
            SignedPresence::from_wire(&extra),
            Err(PresenceError::Invalid)
        );
        let mut both = wire.clone();
        both.extend_from_slice(&wire);
        let list = SignedPresence::read_list(&both).unwrap();
        assert_eq!(list, vec![card.clone(), card]);
        assert!(SignedPresence::from_wire(b"RSC-NP").is_err());
        let key = libp2p::identity::Keypair::generate_ed25519();
        let peer = libp2p::PeerId::random();
        let circuit = "/ip4/198.51.100.8/tcp/4001"
            .parse::<Multiaddr>()
            .unwrap()
            .with(libp2p::multiaddr::Protocol::P2p(peer))
            .with(libp2p::multiaddr::Protocol::P2pCircuit);
        let mut relay = draft(Reach::Relay, NatView::Private, vec![circuit]);
        relay.chain = None;
        let relay = relay.sign(&key).unwrap();
        assert_eq!(
            SignedPresence::from_wire(&relay.to_wire().unwrap()).unwrap(),
            relay
        );
    }
}
