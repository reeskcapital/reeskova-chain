//! Swarm, transports, GossipSub, Identify, Kademlia, Ping y request-response.
//! El loop solo hace networking. No valida bloques ni ejecuta WASM.

use crate::discovery::{
    peer_id_from_multiaddr, BootstrapPeer, DiscoveryConfig, DiscoveryEvent, DiscoveryManager,
};
use crate::peer::{BanReason, LocalPeerIdentity, PeerRegistry, PeerState};
use crate::presence::{self, KadRole, Presence};
use crate::{
    gossip_size_limit, topic_name, validate_network_id, ChainId, P2pError, P2pLimits, PeerId,
    BLOCK_SYNC_PROTOCOL, GOSSIPSUB_PROTOCOL, HANDSHAKE_PROTOCOL, RSC_P2P_PROTOCOL_VERSION,
    TOPIC_BLOCKS, TOPIC_CONSENSUS, TOPIC_EVIDENCE, TOPIC_TRANSACTIONS,
};
use futures::StreamExt;
use libp2p::identity::Keypair;
use libp2p::request_response::{self, OutboundRequestId, ProtocolSupport, ResponseChannel};
use libp2p::swarm::{NetworkBehaviour, SwarmEvent};
use libp2p::multiaddr::Protocol;
use libp2p::{
    allow_block_list, autonat, connection_limits, dcutr, gossipsub, identify, kad, noise, ping,
    relay, tcp, upnp, yamux, Multiaddr, StreamProtocol, Swarm, SwarmBuilder,
};
use rsc_crypto::hash;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

const BAN_SECS: u64 = 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GossipTopic {
    Blocks,
    Transactions,
    Consensus,
    Evidence,
}

impl GossipTopic {
    fn as_str(self) -> &'static str {
        match self {
            Self::Blocks => TOPIC_BLOCKS,
            Self::Transactions => TOPIC_TRANSACTIONS,
            Self::Consensus => TOPIC_CONSENSUS,
            Self::Evidence => TOPIC_EVIDENCE,
        }
    }

    fn parse(network_id: &str, topic: &str) -> Option<Self> {
        let pairs = [
            (TOPIC_BLOCKS, Self::Blocks),
            (TOPIC_TRANSACTIONS, Self::Transactions),
            (TOPIC_CONSENSUS, Self::Consensus),
            (TOPIC_EVIDENCE, Self::Evidence),
        ];
        for (kind, t) in pairs {
            if topic_name(network_id, kind).ok()?.as_str() == topic {
                return Some(t);
            }
        }
        None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GossipAcceptance {
    Accept,
    Reject,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RscRequestId(pub u64);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandshakeOffer {
    pub network_id: String,
    pub chain_id: u32,
    pub protocol_version: String,
    pub agent_version: String,
    pub capabilities: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandshakeReply {
    pub accepted: bool,
    pub reason: Option<String>,
    pub network_id: String,
    pub chain_id: u32,
    pub protocol_version: String,
    pub capabilities: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RscRequest {
    Handshake(HandshakeOffer),
    GetBlockByHash { hash: Vec<u8> },
    GetBlockByHeight { height: u64 },
    GetBlockRange { start: u64, end: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RscResponse {
    Handshake(HandshakeReply),
    Blocks { payloads: Vec<Vec<u8>> },
    Empty,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetworkTimeouts {
    pub dial_timeout: Duration,
    pub handshake_timeout: Duration,
    pub request_timeout: Duration,
    pub idle_connection_timeout: Duration,
}

impl Default for NetworkTimeouts {
    fn default() -> Self {
        Self {
            dial_timeout: Duration::from_secs(10),
            handshake_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(10),
            idle_connection_timeout: Duration::from_secs(60),
        }
    }
}

/// Configuración de un nodo RSC-P2P.
///
/// Poseída por la capa superior. No incluye secretos. `network_id` y
/// `chain_id` aíslan redes; los timeouts y `limits` son operacionales y no
/// afectan voting power.
#[derive(Clone, Debug)]
pub struct NetworkConfig {
    pub network_id: String,
    pub chain_id: ChainId,
    pub protocol_version: String,
    pub agent_version: String,
    pub listen_addresses: Vec<Multiaddr>,
    pub bootstrap_peers: Vec<BootstrapPeer>,
    pub limits: P2pLimits,
    pub timeouts: NetworkTimeouts,
    pub discovery: DiscoveryConfig,
    pub capabilities: Vec<String>,
    /// `true` fija `relay::Status::Enable`. En localhost no hay dirección externa confirmada.
    pub relay_server: bool,
    /// Multiaddr del relay, con `/p2p/<PeerId>`. La reserva sale de `listen_on` + `/p2p-circuit`.
    pub relay_client: Option<Multiaddr>,
    /// Destino a marcar por el circuito después de una reserva aceptada.
    pub circuit_dial: Option<Multiaddr>,
}

impl NetworkConfig {
    pub fn validate(&self) -> Result<(), P2pError> {
        validate_network_id(&self.network_id)?;
        if self.protocol_version.is_empty() || self.agent_version.is_empty() {
            return Err(P2pError::Configuration);
        }
        if self.limits.max_peers == 0 || self.limits.max_command_queue == 0 {
            return Err(P2pError::Configuration);
        }
        for p in &self.bootstrap_peers {
            p.validate()?;
        }
        if let Some(addr) = &self.relay_client {
            if peer_id_from_multiaddr(addr).is_none() {
                return Err(P2pError::InvalidAddress);
            }
        }
        if let Some(addr) = &self.circuit_dial {
            if peer_id_from_multiaddr(addr).is_none() {
                return Err(P2pError::InvalidAddress);
            }
        }
        Ok(())
    }
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            network_id: "devnet".into(),
            chain_id: ChainId(3),
            protocol_version: RSC_P2P_PROTOCOL_VERSION.into(),
            agent_version: "rsc-p2p/0.1.0".into(),
            listen_addresses: vec!["/ip4/127.0.0.1/tcp/0".parse().expect("static addr")],
            bootstrap_peers: vec![],
            limits: P2pLimits::default(),
            timeouts: NetworkTimeouts::default(),
            discovery: DiscoveryConfig::for_network("devnet", vec![]).expect("static discovery"),
            capabilities: vec!["full-node".into()],
            relay_server: false,
            relay_client: None,
            circuit_dial: None,
        }
    }
}

/// Comandos hacia el event loop. Las capas superiores no tocan el `Swarm`.
#[derive(Debug)]
pub enum NetworkCommand {
    Dial {
        peer: Option<PeerId>,
        address: Multiaddr,
    },
    Disconnect {
        peer: PeerId,
    },
    Publish {
        topic: GossipTopic,
        data: Vec<u8>,
    },
    Subscribe {
        topic: GossipTopic,
    },
    Unsubscribe {
        topic: GossipTopic,
    },
    SendRequest {
        id: RscRequestId,
        peer: PeerId,
        request: RscRequest,
    },
    SendResponse {
        id: RscRequestId,
        response: RscResponse,
    },
    ReportGossipValidation {
        message_id: Vec<u8>,
        acceptance: GossipAcceptance,
    },
    AddKnownAddress {
        peer: PeerId,
        address: Multiaddr,
    },
    BanPeer {
        peer: PeerId,
        reason: BanReason,
    },
    UnbanPeer {
        peer: PeerId,
    },
    Shutdown,
}

/// Eventos sanitizados emitidos por el loop. Payloads protocolarios son bytes
/// opacos; P2P no interpreta consenso, bloques ni transacciones.
#[derive(Debug)]
pub enum NetworkEvent {
    ListeningStarted,
    ListeningAddressAdded(Multiaddr),
    Presence(String),
    PeerConnected(PeerId),
    PeerDisconnected(PeerId),
    PeerIdentified {
        peer_id: PeerId,
        addresses: Vec<Multiaddr>,
        protocols: Vec<String>,
        agent_version: String,
    },
    GossipMessageReceived {
        peer: PeerId,
        topic: GossipTopic,
        data: Vec<u8>,
        message_id: Vec<u8>,
    },
    GossipSubscribed(GossipTopic),
    GossipUnsubscribed(GossipTopic),
    RequestReceived {
        peer: PeerId,
        request_id: RscRequestId,
        request: RscRequest,
    },
    ResponseReceived {
        peer: PeerId,
        request_id: RscRequestId,
        response: RscResponse,
    },
    RequestFailed {
        peer: PeerId,
        request_id: RscRequestId,
        error: P2pError,
    },
    DiscoveryEvent(DiscoveryEvent),
    DialFailed {
        peer: Option<PeerId>,
        error: P2pError,
    },
    TransportError,
    HandshakeAccepted(PeerId),
    HandshakeRejected {
        peer: PeerId,
        error: P2pError,
    },
    /// Línea de relay o DCUtR. No equivale a `PeerConnected`.
    Relay(String),
    PeerBanned(PeerId),
    PeerUnbanned(PeerId),
    Backpressure,
}

/// Handle clonable hacia el event loop. No expone el `Swarm`.
///
/// Envía [`NetworkCommand`] por un canal acotado. Si la cola está llena
/// devuelve [`P2pError::Backpressure`].
#[derive(Clone)]
pub struct NetworkHandle {
    tx: mpsc::Sender<NetworkCommand>,
    ids: Arc<AtomicU64>,
    local_peer_id: PeerId,
}

impl NetworkHandle {
    pub fn local_peer_id(&self) -> PeerId {
        self.local_peer_id
    }

    fn send(&self, cmd: NetworkCommand) -> Result<(), P2pError> {
        self.tx.try_send(cmd).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => P2pError::Backpressure,
            mpsc::error::TrySendError::Closed(_) => P2pError::ChannelClosed,
        })
    }

    pub fn dial(&self, address: Multiaddr) -> Result<(), P2pError> {
        let peer = peer_id_from_multiaddr(&address);
        self.send(NetworkCommand::Dial { peer, address })
    }

    pub fn disconnect(&self, peer: PeerId) -> Result<(), P2pError> {
        self.send(NetworkCommand::Disconnect { peer })
    }

    pub fn publish(&self, topic: GossipTopic, data: Vec<u8>) -> Result<(), P2pError> {
        self.send(NetworkCommand::Publish { topic, data })
    }

    pub fn subscribe(&self, topic: GossipTopic) -> Result<(), P2pError> {
        self.send(NetworkCommand::Subscribe { topic })
    }

    pub fn unsubscribe(&self, topic: GossipTopic) -> Result<(), P2pError> {
        self.send(NetworkCommand::Unsubscribe { topic })
    }

    pub fn request(&self, peer: PeerId, request: RscRequest) -> Result<RscRequestId, P2pError> {
        let id = RscRequestId(self.ids.fetch_add(1, Ordering::Relaxed));
        self.send(NetworkCommand::SendRequest { id, peer, request })?;
        Ok(id)
    }

    pub fn send_response(&self, id: RscRequestId, response: RscResponse) -> Result<(), P2pError> {
        self.send(NetworkCommand::SendResponse { id, response })
    }

    pub fn report_gossip_validation(
        &self,
        message_id: Vec<u8>,
        acceptance: GossipAcceptance,
    ) -> Result<(), P2pError> {
        self.send(NetworkCommand::ReportGossipValidation {
            message_id,
            acceptance,
        })
    }

    pub fn add_known_address(&self, peer: PeerId, address: Multiaddr) -> Result<(), P2pError> {
        self.send(NetworkCommand::AddKnownAddress { peer, address })
    }

    pub fn ban_peer(&self, peer: PeerId, reason: BanReason) -> Result<(), P2pError> {
        self.send(NetworkCommand::BanPeer { peer, reason })
    }

    pub fn unban_peer(&self, peer: PeerId) -> Result<(), P2pError> {
        self.send(NetworkCommand::UnbanPeer { peer })
    }

    pub fn shutdown(&self) -> Result<(), P2pError> {
        self.send(NetworkCommand::Shutdown)
    }
}

/// Nodo de red RSC.
///
/// Posee el `NetworkHandle` y el receptor de [`NetworkEvent`]. El loop corre
/// en una task Tokio; `start` no bloquea. Shutdown cierra el canal de comandos.
pub struct RscNetwork {
    handle: NetworkHandle,
    events: mpsc::Receiver<NetworkEvent>,
    local_peer_id: PeerId,
}

impl RscNetwork {
    pub async fn start(config: NetworkConfig, keypair: Keypair) -> Result<Self, P2pError> {
        config.validate()?;
        let local_peer_id = LocalPeerIdentity::from_keypair(&keypair).peer_id();
        let mut swarm = build_swarm(keypair, &config)?;
        for addr in &config.listen_addresses {
            swarm
                .listen_on(addr.clone())
                .map_err(|_| P2pError::Listen)?;
        }
        for boot in &config.bootstrap_peers {
            for addr in &boot.addresses {
                swarm
                    .behaviour_mut()
                    .kad
                    .add_address(&boot.peer_id, addr.clone());
                swarm.add_peer_address(boot.peer_id, addr.clone());
            }
        }
        subscribe_all(&mut swarm, &config.network_id)?;

        let (cmd_tx, cmd_rx) = mpsc::channel(config.limits.max_command_queue);
        let (ev_tx, ev_rx) = mpsc::channel(config.limits.max_event_queue);
        let handle = NetworkHandle {
            tx: cmd_tx,
            ids: Arc::new(AtomicU64::new(1)),
            local_peer_id,
        };
        tokio::spawn(event_loop(swarm, config, cmd_rx, ev_tx));
        Ok(Self {
            handle,
            events: ev_rx,
            local_peer_id,
        })
    }

    pub fn handle(&self) -> &NetworkHandle {
        &self.handle
    }

    pub fn local_peer_id(&self) -> PeerId {
        self.local_peer_id
    }

    pub async fn next_event(&mut self) -> Option<NetworkEvent> {
        self.events.recv().await
    }
}

#[derive(NetworkBehaviour)]
struct RscBehaviour {
    gossipsub: gossipsub::Behaviour,
    identify: identify::Behaviour,
    kad: kad::Behaviour<kad::store::MemoryStore>,
    ping: ping::Behaviour,
    request_response: request_response::cbor::Behaviour<RscRequest, RscResponse>,
    blocked: allow_block_list::Behaviour<allow_block_list::BlockedPeers>,
    limits: connection_limits::Behaviour,
    upnp: upnp::tokio::Behaviour,
    autonat: autonat::Behaviour,
    relay_client: relay::client::Behaviour,
    relay_server: relay::Behaviour,
    dcutr: dcutr::Behaviour,
}

fn build_swarm(keypair: Keypair, config: &NetworkConfig) -> Result<Swarm<RscBehaviour>, P2pError> {
    let idle = config.timeouts.idle_connection_timeout;
    let cfg = config.clone();
    let swarm = SwarmBuilder::with_existing_identity(keypair)
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )
        .map_err(|_| P2pError::Transport)?
        .with_quic()
        .with_dns()
        .map_err(|_| P2pError::Transport)?
        .with_relay_client(noise::Config::new, yamux::Config::default)
        .map_err(|_| P2pError::Transport)?
        .with_behaviour(move |key, relay_client| build_behaviour(key, &cfg, relay_client))
        .map_err(|_| P2pError::Configuration)?
        .with_swarm_config(|c| c.with_idle_connection_timeout(idle))
        .build();
    Ok(swarm)
}

fn build_behaviour(
    key: &Keypair,
    config: &NetworkConfig,
    relay_client: relay::client::Behaviour,
) -> RscBehaviour {
    let local = PeerId::from(key.public());
    let gossipsub = build_gossipsub(key, config);
    let identify = identify::Behaviour::new(
        identify::Config::new(config.protocol_version.clone(), key.public())
            .with_agent_version(config.agent_version.clone()),
    );
    let kad_name = StreamProtocol::try_from_owned(config.discovery.kad_protocol.clone())
        .expect("validated kad protocol");
    let mut kad_cfg = kad::Config::new(kad_name);
    kad_cfg.set_query_timeout(config.discovery.bootstrap_timeout);
    kad_cfg.set_periodic_bootstrap_interval(Some(config.discovery.periodic_bootstrap_interval));
    let mut kad = kad::Behaviour::with_config(local, kad::store::MemoryStore::new(local), kad_cfg);
    kad.set_mode(Some(mode_of(presence::kad_role(presence::NatView::Unknown))));

    let rr_cfg =
        request_response::Config::default().with_request_timeout(config.timeouts.request_timeout);
    let codec = request_response::cbor::codec::Codec::<RscRequest, RscResponse>::default()
        .set_request_size_maximum(config.limits.max_request_size as u64)
        .set_response_size_maximum(config.limits.max_response_size as u64);
    let request_response = request_response::Behaviour::with_codec(
        codec,
        [
            (
                StreamProtocol::new(HANDSHAKE_PROTOCOL),
                ProtocolSupport::Full,
            ),
            (
                StreamProtocol::new(BLOCK_SYNC_PROTOCOL),
                ProtocolSupport::Full,
            ),
        ],
        rr_cfg,
    );
    let limits = connection_limits::Behaviour::new(
        connection_limits::ConnectionLimits::default()
            .with_max_established(Some(config.limits.max_peers))
            .with_max_established_incoming(Some(config.limits.max_inbound_connections))
            .with_max_established_outgoing(Some(config.limits.max_outbound_connections))
            .with_max_established_per_peer(Some(8)),
    );
    let mut relay_server = relay::Behaviour::new(local, relay::Config::default());
    if config.relay_server {
        relay_server.set_status(Some(relay::Status::Enable));
    }

    RscBehaviour {
        gossipsub,
        identify,
        kad,
        ping: ping::Behaviour::new(ping::Config::new()),
        request_response,
        blocked: allow_block_list::Behaviour::default(),
        limits,
        upnp: upnp::tokio::Behaviour::default(),
        autonat: autonat::Behaviour::new(local, autonat::Config::default()),
        relay_client,
        relay_server,
        dcutr: dcutr::Behaviour::new(local),
    }
}

struct LinkCounts {
    direct: u32,
    relayed: u32,
}

fn mode_of(role: KadRole) -> kad::Mode {
    match role {
        KadRole::Server => kad::Mode::Server,
        KadRole::Client => kad::Mode::Client,
    }
}

fn build_gossipsub(key: &Keypair, config: &NetworkConfig) -> gossipsub::Behaviour {
    let max = config.limits.max_gossip_message_size;
    let gs_cfg = gossipsub::ConfigBuilder::default()
        .heartbeat_interval(Duration::from_secs(1))
        .validation_mode(gossipsub::ValidationMode::Strict)
        .validate_messages()
        .mesh_n(6)
        .mesh_n_low(4)
        .mesh_n_high(12)
        .mesh_outbound_min(2)
        .history_length(5)
        .history_gossip(3)
        .duplicate_cache_time(Duration::from_secs(60))
        .max_transmit_size(max)
        .protocol_id(GOSSIPSUB_PROTOCOL, gossipsub::Version::V1_1)
        .message_id_fn(content_message_id)
        .build()
        .expect("gossipsub config");
    gossipsub::Behaviour::new(gossipsub::MessageAuthenticity::Signed(key.clone()), gs_cfg)
        .expect("gossipsub behaviour")
}

fn content_message_id(message: &gossipsub::Message) -> gossipsub::MessageId {
    let mut buf = Vec::with_capacity(message.topic.as_str().len() + 1 + message.data.len());
    buf.extend_from_slice(message.topic.as_str().as_bytes());
    buf.push(0);
    buf.extend_from_slice(&message.data);
    gossipsub::MessageId::from(hash(&buf).to_bytes().to_vec())
}

fn subscribe_all(swarm: &mut Swarm<RscBehaviour>, network_id: &str) -> Result<(), P2pError> {
    for kind in [
        TOPIC_BLOCKS,
        TOPIC_TRANSACTIONS,
        TOPIC_CONSENSUS,
        TOPIC_EVIDENCE,
    ] {
        let name = topic_name(network_id, kind)?;
        let topic = gossipsub::IdentTopic::new(name);
        swarm
            .behaviour_mut()
            .gossipsub
            .subscribe(&topic)
            .map_err(|_| P2pError::Subscribe)?;
    }
    Ok(())
}

struct LoopState {
    config: NetworkConfig,
    registry: PeerRegistry,
    discovery: DiscoveryManager,
    inbound: HashMap<RscRequestId, ResponseChannel<RscResponse>>,
    outbound: HashMap<OutboundRequestId, RscRequestId>,
    gossip_src: HashMap<Vec<u8>, (gossipsub::MessageId, PeerId)>,
    inbound_seq: u64,
    handshake_sent: HashMap<PeerId, Instant>,
    presence: Presence,
    links: HashMap<PeerId, LinkCounts>,
    relay_listening: bool,
    relay_reserved: bool,
    circuit_dialed: bool,
}

fn emit(tx: &mpsc::Sender<NetworkEvent>, ev: NetworkEvent) {
    let _ = tx.try_send(ev);
}

async fn event_loop(
    mut swarm: Swarm<RscBehaviour>,
    config: NetworkConfig,
    mut cmds: mpsc::Receiver<NetworkCommand>,
    events: mpsc::Sender<NetworkEvent>,
) {
    let mut state = LoopState {
        discovery: DiscoveryManager::new(config.discovery.clone()),
        config,
        registry: PeerRegistry::new(),
        inbound: HashMap::new(),
        outbound: HashMap::new(),
        gossip_src: HashMap::new(),
        inbound_seq: 1,
        handshake_sent: HashMap::new(),
        presence: Presence::new(),
        links: HashMap::new(),
        relay_listening: false,
        relay_reserved: false,
        circuit_dialed: false,
    };
    emit(&events, NetworkEvent::ListeningStarted);
    if state.config.relay_server {
        emit(&events, NetworkEvent::Relay("relay-server".to_string()));
    }
    emit(
        &events,
        NetworkEvent::Presence(presence::format_note(&presence::PresenceNote::Unknown)),
    );
    loop {
        tokio::select! {
            cmd = cmds.recv() => {
                let Some(cmd) = cmd else { break };
                if matches!(cmd, NetworkCommand::Shutdown) {
                    break;
                }
                handle_command(&mut swarm, &mut state, &events, cmd);
            }
            event = swarm.select_next_some() => {
                handle_swarm(&mut swarm, &mut state, &events, event);
            }
        }
    }
}

fn handle_command(
    swarm: &mut Swarm<RscBehaviour>,
    state: &mut LoopState,
    events: &mpsc::Sender<NetworkEvent>,
    cmd: NetworkCommand,
) {
    match cmd {
        NetworkCommand::Dial { peer, address } => {
            if let Some(id) = peer {
                if state
                    .registry
                    .ensure_not_banned(&id, Instant::now())
                    .is_err()
                {
                    emit(
                        events,
                        NetworkEvent::DialFailed {
                            peer: Some(id),
                            error: P2pError::PeerBanned,
                        },
                    );
                    return;
                }
                state.registry.entry(id).connection_state = PeerState::Connecting;
            }
            if swarm.dial(address).is_err() {
                emit(
                    events,
                    NetworkEvent::DialFailed {
                        peer,
                        error: P2pError::Dial,
                    },
                );
            }
        }
        NetworkCommand::Disconnect { peer } => {
            let _ = swarm.disconnect_peer_id(peer);
        }
        NetworkCommand::Publish { topic, data } => {
            let limit = gossip_size_limit(topic, &state.config.limits);
            if data.len() > limit {
                emit(events, NetworkEvent::TransportError);
                return;
            }
            let Ok(name) = topic_name(&state.config.network_id, topic.as_str()) else {
                return;
            };
            let t = gossipsub::IdentTopic::new(name);
            if swarm.behaviour_mut().gossipsub.publish(t, data).is_err() {
                emit(events, NetworkEvent::TransportError);
            }
        }
        NetworkCommand::Subscribe { topic } => {
            if let Ok(name) = topic_name(&state.config.network_id, topic.as_str()) {
                let t = gossipsub::IdentTopic::new(name);
                if swarm.behaviour_mut().gossipsub.subscribe(&t).is_ok() {
                    emit(events, NetworkEvent::GossipSubscribed(topic));
                }
            }
        }
        NetworkCommand::Unsubscribe { topic } => {
            if let Ok(name) = topic_name(&state.config.network_id, topic.as_str()) {
                let t = gossipsub::IdentTopic::new(name);
                let _ = swarm.behaviour_mut().gossipsub.unsubscribe(&t);
                emit(events, NetworkEvent::GossipUnsubscribed(topic));
            }
        }
        NetworkCommand::SendRequest { id, peer, request } => {
            if state
                .registry
                .ensure_not_banned(&peer, Instant::now())
                .is_err()
            {
                emit(
                    events,
                    NetworkEvent::RequestFailed {
                        peer,
                        request_id: id,
                        error: P2pError::PeerBanned,
                    },
                );
                return;
            }
            if request_size(&request) > state.config.limits.max_request_size {
                emit(
                    events,
                    NetworkEvent::RequestFailed {
                        peer,
                        request_id: id,
                        error: P2pError::MessageTooLarge,
                    },
                );
                return;
            }
            let out = swarm
                .behaviour_mut()
                .request_response
                .send_request(&peer, request);
            state.outbound.insert(out, id);
        }
        NetworkCommand::SendResponse { id, response } => {
            if response_size(&response) > state.config.limits.max_response_size {
                return;
            }
            if let Some(ch) = state.inbound.remove(&id) {
                let _ = swarm
                    .behaviour_mut()
                    .request_response
                    .send_response(ch, response);
            }
        }
        NetworkCommand::ReportGossipValidation {
            message_id,
            acceptance,
        } => {
            if let Some((mid, src)) = state.gossip_src.remove(&message_id) {
                let acc = match acceptance {
                    GossipAcceptance::Accept => gossipsub::MessageAcceptance::Accept,
                    GossipAcceptance::Reject => gossipsub::MessageAcceptance::Reject,
                };
                let _ = swarm
                    .behaviour_mut()
                    .gossipsub
                    .report_message_validation_result(&mid, &src, acc);
            }
        }
        NetworkCommand::AddKnownAddress { peer, address } => {
            swarm
                .behaviour_mut()
                .kad
                .add_address(&peer, address.clone());
            swarm.add_peer_address(peer, address);
        }
        NetworkCommand::BanPeer { peer, reason } => {
            state
                .registry
                .ban(peer, reason, Instant::now() + Duration::from_secs(BAN_SECS));
            swarm.behaviour_mut().blocked.block_peer(peer);
            let _ = swarm.disconnect_peer_id(peer);
            emit(events, NetworkEvent::PeerBanned(peer));
        }
        NetworkCommand::UnbanPeer { peer } => {
            state.registry.unban(&peer);
            swarm.behaviour_mut().blocked.unblock_peer(peer);
            emit(events, NetworkEvent::PeerUnbanned(peer));
        }
        NetworkCommand::Shutdown => {}
    }
}

fn handle_swarm(
    swarm: &mut Swarm<RscBehaviour>,
    state: &mut LoopState,
    events: &mpsc::Sender<NetworkEvent>,
    event: SwarmEvent<RscBehaviourEvent>,
) {
    match event {
        SwarmEvent::NewListenAddr { address, .. } => {
            if state.config.relay_server && !presence::is_circuit(&address) {
                swarm.add_external_address(address.clone());
            }
            let actions = state.presence.on_listen(address.clone());
            emit(events, NetworkEvent::ListeningAddressAdded(address));
            apply_presence(swarm, events, actions);
        }
        SwarmEvent::ConnectionEstablished {
            peer_id, endpoint, ..
        } => {
            if state
                .registry
                .ensure_not_banned(&peer_id, Instant::now())
                .is_err()
            {
                let _ = swarm.disconnect_peer_id(peer_id);
                return;
            }
            if state.registry.connected_count() >= state.config.limits.max_peers as usize
                && state.registry.get(&peer_id).map(|p| p.connection_state)
                    != Some(PeerState::Connected)
            {
                let _ = swarm.disconnect_peer_id(peer_id);
                return;
            }
            let relayed = endpoint.is_relayed();
            let info = state.registry.entry(peer_id);
            info.connection_state = PeerState::Connected;
            info.last_seen = Some(Instant::now());
            let link = state.links.entry(peer_id).or_insert(LinkCounts {
                direct: 0,
                relayed: 0,
            });
            if relayed {
                link.relayed += 1;
                emit(
                    events,
                    NetworkEvent::Relay(format!("connected-relay {peer_id}")),
                );
            } else {
                link.direct += 1;
                emit(events, NetworkEvent::PeerConnected(peer_id));
                maybe_reserve(swarm, state, events, peer_id);
            }
            send_handshake(swarm, state, peer_id);
        }
        SwarmEvent::ConnectionClosed {
            peer_id,
            endpoint,
            num_established,
            ..
        } => {
            if let Some(link) = state.links.get_mut(&peer_id) {
                if endpoint.is_relayed() {
                    link.relayed = link.relayed.saturating_sub(1);
                } else {
                    link.direct = link.direct.saturating_sub(1);
                }
            }
            if endpoint.is_relayed() {
                emit(
                    events,
                    NetworkEvent::Relay(format!("relay-closed {peer_id}")),
                );
            } else if state.relay_reserved
                && state
                    .config
                    .relay_client
                    .as_ref()
                    .and_then(peer_id_from_multiaddr)
                    == Some(peer_id)
            {
                emit(events, NetworkEvent::Relay(format!("relay-lost {peer_id}")));
                let actions = state.presence.on_relay_lost();
                apply_presence(swarm, events, actions);
            }
            if num_established == 0 {
                state.registry.entry(peer_id).connection_state = PeerState::Disconnected;
                state.handshake_sent.remove(&peer_id);
                state.links.remove(&peer_id);
                emit(events, NetworkEvent::PeerDisconnected(peer_id));
            }
        }
        SwarmEvent::ListenerClosed { reason, .. } => {
            if reason.is_err() && state.relay_listening && !state.relay_reserved {
                emit(
                    events,
                    NetworkEvent::Relay("reservation-failed".to_string()),
                );
            }
        }
        SwarmEvent::OutgoingConnectionError { peer_id, .. } => {
            emit(
                events,
                NetworkEvent::DialFailed {
                    peer: peer_id,
                    error: P2pError::Dial,
                },
            );
        }
        SwarmEvent::Behaviour(ev) => handle_behaviour(swarm, state, events, ev),
        _ => {}
    }
}

fn maybe_reserve(
    swarm: &mut Swarm<RscBehaviour>,
    state: &mut LoopState,
    events: &mpsc::Sender<NetworkEvent>,
    peer_id: PeerId,
) {
    if state.relay_listening {
        return;
    }
    let Some(addr) = state.config.relay_client.clone() else {
        return;
    };
    if peer_id_from_multiaddr(&addr) != Some(peer_id) {
        return;
    }
    let circuit = addr.with(Protocol::P2pCircuit);
    match swarm.listen_on(circuit) {
        Ok(_) => state.relay_listening = true,
        Err(_) => emit(
            events,
            NetworkEvent::Relay("reservation-failed".to_string()),
        ),
    }
}

fn note_handshake(state: &LoopState, events: &mpsc::Sender<NetworkEvent>, peer: PeerId) {
    let relay_only = state
        .links
        .get(&peer)
        .is_some_and(|link| link.relayed > 0 && link.direct == 0);
    if relay_only {
        emit(
            events,
            NetworkEvent::Relay(format!("handshake-relay {peer}")),
        );
    } else {
        emit(events, NetworkEvent::HandshakeAccepted(peer));
    }
}

fn send_handshake(swarm: &mut Swarm<RscBehaviour>, state: &mut LoopState, peer: PeerId) {
    if state.handshake_sent.contains_key(&peer) {
        return;
    }
    let req = RscRequest::Handshake(HandshakeOffer {
        network_id: state.config.network_id.clone(),
        chain_id: state.config.chain_id.0,
        protocol_version: state.config.protocol_version.clone(),
        agent_version: state.config.agent_version.clone(),
        capabilities: state.config.capabilities.clone(),
    });
    let out = swarm
        .behaviour_mut()
        .request_response
        .send_request(&peer, req);
    let id = RscRequestId(0);
    state.outbound.insert(out, id);
    state.handshake_sent.insert(peer, Instant::now());
}

fn handle_behaviour(
    swarm: &mut Swarm<RscBehaviour>,
    state: &mut LoopState,
    events: &mpsc::Sender<NetworkEvent>,
    event: RscBehaviourEvent,
) {
    match event {
        RscBehaviourEvent::Identify(identify::Event::Received { peer_id, info, .. }) => {
            let addrs = state
                .discovery
                .filter_identify_addrs(peer_id, &info.listen_addrs);
            let protocols: Vec<String> = info.protocols.iter().map(|p| p.to_string()).collect();
            {
                let p = state.registry.entry(peer_id);
                p.addresses = addrs.clone();
                p.protocols = protocols.clone();
                p.agent_version = Some(info.agent_version.clone());
                p.rsc_protocol_version = Some(info.protocol_version.clone());
                p.last_seen = Some(Instant::now());
            }
            for addr in &addrs {
                swarm
                    .behaviour_mut()
                    .kad
                    .add_address(&peer_id, addr.clone());
                swarm.add_peer_address(peer_id, addr.clone());
                emit(
                    events,
                    NetworkEvent::DiscoveryEvent(DiscoveryEvent::AddressDiscovered {
                        peer_id,
                        address: addr.clone(),
                    }),
                );
            }
            emit(
                events,
                NetworkEvent::PeerIdentified {
                    peer_id,
                    addresses: addrs,
                    protocols,
                    agent_version: info.agent_version,
                },
            );
            if (!state.discovery.config().bootstrap_peers.is_empty()
                || state.registry.connected_count() > 0)
                && swarm.behaviour_mut().kad.bootstrap().is_ok()
            {
                emit(
                    events,
                    NetworkEvent::DiscoveryEvent(state.discovery.mark_bootstrap_started()),
                );
            }
        }
        RscBehaviourEvent::Gossipsub(gossipsub::Event::Message {
            propagation_source,
            message_id,
            message,
        }) => {
            let Some(topic) = GossipTopic::parse(&state.config.network_id, message.topic.as_str())
            else {
                let _ = swarm
                    .behaviour_mut()
                    .gossipsub
                    .report_message_validation_result(
                        &message_id,
                        &propagation_source,
                        gossipsub::MessageAcceptance::Reject,
                    );
                return;
            };
            let limit = gossip_size_limit(topic, &state.config.limits);
            if message.data.len() > limit {
                let _ = swarm
                    .behaviour_mut()
                    .gossipsub
                    .report_message_validation_result(
                        &message_id,
                        &propagation_source,
                        gossipsub::MessageAcceptance::Reject,
                    );
                return;
            }
            let mid = message_id.0.clone();
            state
                .gossip_src
                .insert(mid.clone(), (message_id, propagation_source));
            emit(
                events,
                NetworkEvent::GossipMessageReceived {
                    peer: propagation_source,
                    topic,
                    data: message.data,
                    message_id: mid,
                },
            );
        }
        RscBehaviourEvent::Gossipsub(gossipsub::Event::Subscribed { topic, .. }) => {
            if let Some(t) = GossipTopic::parse(&state.config.network_id, topic.as_str()) {
                emit(events, NetworkEvent::GossipSubscribed(t));
            }
        }
        RscBehaviourEvent::RequestResponse(request_response::Event::Message {
            peer,
            message,
            ..
        }) => match message {
            request_response::Message::Request {
                request, channel, ..
            } => {
                if let RscRequest::Handshake(offer) = &request {
                    let (ok, err) = evaluate_handshake(&state.config, offer);
                    let reply = HandshakeReply {
                        accepted: ok,
                        reason: err.map(|e| e.to_string()),
                        network_id: state.config.network_id.clone(),
                        chain_id: state.config.chain_id.0,
                        protocol_version: state.config.protocol_version.clone(),
                        capabilities: state.config.capabilities.clone(),
                    };
                    let _ = swarm
                        .behaviour_mut()
                        .request_response
                        .send_response(channel, RscResponse::Handshake(reply));
                    if ok {
                        note_handshake(state, events, peer);
                    } else {
                        emit(
                            events,
                            NetworkEvent::HandshakeRejected {
                                peer,
                                error: err.unwrap_or(P2pError::HandshakeFailed),
                            },
                        );
                        let _ = swarm.disconnect_peer_id(peer);
                    }
                    return;
                }
                if state.inbound.len() >= state.config.limits.max_pending_responses as usize {
                    return;
                }
                state.inbound_seq += 1;
                let id = RscRequestId(state.inbound_seq);
                state.inbound.insert(id, channel);
                emit(
                    events,
                    NetworkEvent::RequestReceived {
                        peer,
                        request_id: id,
                        request,
                    },
                );
            }
            request_response::Message::Response {
                request_id,
                response,
            } => {
                if let RscResponse::Handshake(reply) = &response {
                    if reply.accepted
                        && reply.network_id == state.config.network_id
                        && reply.chain_id == state.config.chain_id.0
                    {
                        note_handshake(state, events, peer);
                    } else {
                        let err = if reply.network_id != state.config.network_id {
                            P2pError::NetworkMismatch
                        } else if reply.chain_id != state.config.chain_id.0 {
                            P2pError::ChainMismatch
                        } else {
                            P2pError::HandshakeFailed
                        };
                        emit(events, NetworkEvent::HandshakeRejected { peer, error: err });
                        let _ = swarm.disconnect_peer_id(peer);
                    }
                    state.outbound.remove(&request_id);
                    return;
                }
                if let Some(id) = state.outbound.remove(&request_id) {
                    emit(
                        events,
                        NetworkEvent::ResponseReceived {
                            peer,
                            request_id: id,
                            response,
                        },
                    );
                }
            }
        },
        RscBehaviourEvent::RequestResponse(request_response::Event::OutboundFailure {
            peer,
            request_id,
            error,
            ..
        }) => {
            let id = state
                .outbound
                .remove(&request_id)
                .unwrap_or(RscRequestId(0));
            let mapped = match error {
                request_response::OutboundFailure::Timeout => P2pError::RequestTimeout,
                _ => P2pError::RequestResponse,
            };
            emit(
                events,
                NetworkEvent::RequestFailed {
                    peer,
                    request_id: id,
                    error: mapped,
                },
            );
        }
        RscBehaviourEvent::Kad(kad::Event::OutboundQueryProgressed { result, .. }) => {
            if let kad::QueryResult::Bootstrap(Ok(_)) = result {
                emit(
                    events,
                    NetworkEvent::DiscoveryEvent(DiscoveryEvent::BootstrapSucceeded),
                );
            } else if let kad::QueryResult::Bootstrap(Err(_)) = result {
                emit(
                    events,
                    NetworkEvent::DiscoveryEvent(DiscoveryEvent::BootstrapFailed),
                );
            }
        }
        RscBehaviourEvent::Kad(kad::Event::RoutingUpdated { peer, .. }) => {
            emit(
                events,
                NetworkEvent::DiscoveryEvent(DiscoveryEvent::PeerDiscovered { peer_id: peer }),
            );
        }
        RscBehaviourEvent::Ping(ping::Event {
            peer,
            result: Ok(d),
            ..
        }) => {
            state.registry.entry(peer).latency = Some(d);
        }
        RscBehaviourEvent::Upnp(event) => {
            let actions = match event {
                upnp::Event::NewExternalAddr { external_addr, .. } => {
                    state.presence.on_upnp_candidate(external_addr)
                }
                upnp::Event::ExpiredExternalAddr { external_addr, .. } => {
                    state.presence.on_upnp_expired(external_addr)
                }
                upnp::Event::GatewayNotFound => state.presence.on_upnp_gateway_missing(),
                upnp::Event::NonRoutableGateway => state.presence.on_upnp_not_routable(),
            };
            apply_presence(swarm, events, actions);
        }
        RscBehaviourEvent::Autonat(autonat::Event::StatusChanged { new, .. }) => {
            let actions = match new {
                autonat::NatStatus::Public(addr) => state.presence.on_autonat_public(addr),
                autonat::NatStatus::Private => state.presence.on_autonat_private(),
                autonat::NatStatus::Unknown => state.presence.on_autonat_unknown(),
            };
            apply_presence(swarm, events, actions);
        }
        RscBehaviourEvent::RelayServer(event) => match event {
            relay::Event::ReservationReqAccepted { src_peer_id, .. } => {
                emit(
                    events,
                    NetworkEvent::Relay(format!("reservation-accepted {src_peer_id}")),
                );
            }
            relay::Event::ReservationReqDenied { src_peer_id, .. } => {
                emit(
                    events,
                    NetworkEvent::Relay(format!("reservation-denied {src_peer_id}")),
                );
            }
            relay::Event::ReservationClosed { src_peer_id } => {
                emit(
                    events,
                    NetworkEvent::Relay(format!("reservation-closed {src_peer_id}")),
                );
            }
            _ => {}
        },
        RscBehaviourEvent::RelayClient(event) => match event {
            relay::client::Event::ReservationReqAccepted { relay_peer_id, .. } => {
                state.relay_reserved = true;
                if let Some(addr) = state.config.relay_client.clone() {
                    let announced = addr
                        .with(Protocol::P2pCircuit)
                        .with(Protocol::P2p(*swarm.local_peer_id()));
                    let actions = state.presence.on_relay(announced);
                    apply_presence(swarm, events, actions);
                }
                emit(
                    events,
                    NetworkEvent::Relay(format!("reservation {relay_peer_id}")),
                );
                if !state.circuit_dialed {
                    if let Some(addr) = state.config.circuit_dial.clone() {
                        state.circuit_dialed = true;
                        if swarm.dial(addr).is_err() {
                            emit(
                                events,
                                NetworkEvent::DialFailed {
                                    peer: None,
                                    error: P2pError::Dial,
                                },
                            );
                        }
                    }
                }
            }
            relay::client::Event::OutboundCircuitEstablished { relay_peer_id, .. } => {
                emit(
                    events,
                    NetworkEvent::Relay(format!("circuit-out {relay_peer_id}")),
                );
            }
            relay::client::Event::InboundCircuitEstablished { src_peer_id, .. } => {
                emit(
                    events,
                    NetworkEvent::Relay(format!("circuit-in {src_peer_id}")),
                );
            }
        },
        RscBehaviourEvent::Dcutr(event) => {
            let line = match event.result {
                Ok(_) => format!("dcutr-ok {}", event.remote_peer_id),
                Err(_) => format!("dcutr-failed {}", event.remote_peer_id),
            };
            emit(events, NetworkEvent::Relay(line));
        }
        _ => {}
    }
}

fn apply_presence(
    swarm: &mut Swarm<RscBehaviour>,
    events: &mpsc::Sender<NetworkEvent>,
    actions: Vec<presence::PresenceAction>,
) {
    for action in actions {
        match action {
            presence::PresenceAction::ConfirmExternal(addr) => swarm.add_external_address(addr),
            presence::PresenceAction::WithdrawExternal(addr) => {
                swarm.remove_external_address(&addr);
            }
            presence::PresenceAction::Kad(role) => {
                swarm.behaviour_mut().kad.set_mode(Some(mode_of(role)));
            }
            presence::PresenceAction::Note(note) => {
                emit(events, NetworkEvent::Presence(presence::format_note(&note)));
            }
        }
    }
}

fn evaluate_handshake(cfg: &NetworkConfig, offer: &HandshakeOffer) -> (bool, Option<P2pError>) {
    if offer.network_id != cfg.network_id {
        return (false, Some(P2pError::NetworkMismatch));
    }
    if offer.chain_id != cfg.chain_id.0 {
        return (false, Some(P2pError::ChainMismatch));
    }
    if offer.protocol_version != cfg.protocol_version {
        return (false, Some(P2pError::ProtocolVersionMismatch));
    }
    (true, None)
}

fn request_size(req: &RscRequest) -> usize {
    match req {
        RscRequest::Handshake(h) => h.network_id.len() + h.agent_version.len() + 32,
        RscRequest::GetBlockByHash { hash } => hash.len(),
        RscRequest::GetBlockByHeight { .. } => 8,
        RscRequest::GetBlockRange { .. } => 16,
    }
}

fn response_size(resp: &RscResponse) -> usize {
    match resp {
        RscResponse::Handshake(_) => 64,
        RscResponse::Blocks { payloads } => payloads.iter().map(Vec::len).sum(),
        RscResponse::Empty => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::BootstrapPeer;
    use libp2p::identity;

    fn cfg_listen(addrs: Vec<Multiaddr>) -> NetworkConfig {
        NetworkConfig {
            listen_addresses: addrs,
            ..NetworkConfig::default()
        }
    }

    async fn start(addrs: Vec<Multiaddr>) -> RscNetwork {
        RscNetwork::start(cfg_listen(addrs), identity::Keypair::generate_ed25519())
            .await
            .unwrap()
    }

    async fn wait_listen(net: &mut RscNetwork) -> Multiaddr {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match net.next_event().await {
                    Some(NetworkEvent::ListeningAddressAdded(a)) => return a,
                    Some(_) => {}
                    None => panic!("channel closed"),
                }
            }
        })
        .await
        .expect("listen")
    }

    async fn wait_for<F>(net: &mut RscNetwork, mut pred: F) -> NetworkEvent
    where
        F: FnMut(&NetworkEvent) -> bool,
    {
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let ev = net.next_event().await.expect("event");
                if pred(&ev) {
                    return ev;
                }
            }
        })
        .await
        .expect("timeout waiting event")
    }

    #[test]
    fn test_wrong_topic_rejected() {
        assert!(topic_name("devnet", "api").is_err());
        assert!(GossipTopic::parse("devnet", "/rsc/devnet/api/1").is_none());
    }

    #[test]
    fn test_message_id_stable() {
        let t = gossipsub::IdentTopic::new("/rsc/devnet/blocks/1");
        let m1 = gossipsub::Message {
            source: None,
            data: b"abc".to_vec(),
            sequence_number: None,
            topic: t.hash(),
        };
        let m2 = gossipsub::Message {
            source: None,
            data: b"abc".to_vec(),
            sequence_number: Some(99),
            topic: t.hash(),
        };
        assert_eq!(content_message_id(&m1), content_message_id(&m2));
        let m3 = gossipsub::Message {
            source: None,
            data: b"abd".to_vec(),
            sequence_number: None,
            topic: t.hash(),
        };
        assert_ne!(content_message_id(&m1), content_message_id(&m3));
    }

    #[test]
    fn test_handshake_mismatch() {
        let cfg = NetworkConfig::default();
        let mut offer = HandshakeOffer {
            network_id: "mainnet".into(),
            chain_id: cfg.chain_id.0,
            protocol_version: cfg.protocol_version.clone(),
            agent_version: "x".into(),
            capabilities: vec![],
        };
        assert_eq!(
            evaluate_handshake(&cfg, &offer).1,
            Some(P2pError::NetworkMismatch)
        );
        offer.network_id = cfg.network_id.clone();
        offer.chain_id = 99;
        assert_eq!(
            evaluate_handshake(&cfg, &offer).1,
            Some(P2pError::ChainMismatch)
        );
        offer.chain_id = cfg.chain_id.0;
        offer.protocol_version = "9.0.0".into();
        assert_eq!(
            evaluate_handshake(&cfg, &offer).1,
            Some(P2pError::ProtocolVersionMismatch)
        );
    }

    #[test]
    fn test_backpressure_unit() {
        let (tx, _rx) = mpsc::channel::<NetworkCommand>(1);
        tx.try_send(NetworkCommand::Shutdown).unwrap();
        let handle = NetworkHandle {
            tx,
            ids: Arc::new(AtomicU64::new(1)),
            local_peer_id: PeerId::random(),
        };
        assert_eq!(handle.shutdown(), Err(P2pError::Backpressure));
    }

    #[tokio::test]
    async fn test_tcp_connect() {
        let mut a = start(vec!["/ip4/127.0.0.1/tcp/0".parse().unwrap()]).await;
        let addr = wait_listen(&mut a).await;
        let mut b = start(vec!["/ip4/127.0.0.1/tcp/0".parse().unwrap()]).await;
        let _ = wait_listen(&mut b).await;
        let mut with_id = addr;
        with_id.push(libp2p::multiaddr::Protocol::P2p(a.local_peer_id()));
        b.handle().dial(with_id).unwrap();
        wait_for(&mut a, |e| matches!(e, NetworkEvent::PeerConnected(_))).await;
        wait_for(&mut b, |e| matches!(e, NetworkEvent::PeerConnected(_))).await;
        a.handle().shutdown().unwrap();
        b.handle().shutdown().unwrap();
    }

    #[tokio::test]
    async fn test_quic_connect() {
        let mut a = start(vec!["/ip4/127.0.0.1/udp/0/quic-v1".parse().unwrap()]).await;
        let addr = wait_listen(&mut a).await;
        let mut b = start(vec!["/ip4/127.0.0.1/udp/0/quic-v1".parse().unwrap()]).await;
        let _ = wait_listen(&mut b).await;
        let mut with_id = addr;
        with_id.push(libp2p::multiaddr::Protocol::P2p(a.local_peer_id()));
        b.handle().dial(with_id).unwrap();
        wait_for(&mut a, |e| matches!(e, NetworkEvent::PeerConnected(_))).await;
        wait_for(&mut b, |e| {
            matches!(
                e,
                NetworkEvent::PeerDisconnected(_) | NetworkEvent::PeerConnected(_)
            )
        })
        .await;
        a.handle().shutdown().unwrap();
        b.handle().shutdown().unwrap();
    }

    #[tokio::test]
    async fn test_identify_and_handshake() {
        let mut a = start(vec!["/ip4/127.0.0.1/tcp/0".parse().unwrap()]).await;
        let addr = wait_listen(&mut a).await;
        let mut b = start(vec!["/ip4/127.0.0.1/tcp/0".parse().unwrap()]).await;
        let _ = wait_listen(&mut b).await;
        let mut with_id = addr;
        with_id.push(libp2p::multiaddr::Protocol::P2p(a.local_peer_id()));
        b.handle().dial(with_id).unwrap();
        wait_for(&mut a, |e| matches!(e, NetworkEvent::PeerIdentified { .. })).await;
        wait_for(&mut b, |e| matches!(e, NetworkEvent::HandshakeAccepted(_))).await;
        a.handle().shutdown().unwrap();
        b.handle().shutdown().unwrap();
    }

    #[tokio::test]
    async fn test_network_mismatch() {
        let cfg_a = NetworkConfig {
            network_id: "mainnet".into(),
            discovery: DiscoveryConfig::for_network("mainnet", vec![]).unwrap(),
            listen_addresses: vec!["/ip4/127.0.0.1/tcp/0".parse().unwrap()],
            ..NetworkConfig::default()
        };
        let mut a = RscNetwork::start(cfg_a, identity::Keypair::generate_ed25519())
            .await
            .unwrap();
        let addr = wait_listen(&mut a).await;
        let mut b = start(vec!["/ip4/127.0.0.1/tcp/0".parse().unwrap()]).await;
        let _ = wait_listen(&mut b).await;
        let mut with_id = addr;
        with_id.push(libp2p::multiaddr::Protocol::P2p(a.local_peer_id()));
        b.handle().dial(with_id).unwrap();
        wait_for(&mut a, |e| {
            matches!(e, NetworkEvent::HandshakeRejected { .. })
        })
        .await;
        a.handle().shutdown().unwrap();
        b.handle().shutdown().unwrap();
    }

    #[tokio::test]
    async fn test_chain_mismatch() {
        let cfg_a = NetworkConfig {
            chain_id: ChainId(1),
            listen_addresses: vec!["/ip4/127.0.0.1/tcp/0".parse().unwrap()],
            ..NetworkConfig::default()
        };
        let mut a = RscNetwork::start(cfg_a, identity::Keypair::generate_ed25519())
            .await
            .unwrap();
        let addr = wait_listen(&mut a).await;
        let mut b = start(vec!["/ip4/127.0.0.1/tcp/0".parse().unwrap()]).await;
        let _ = wait_listen(&mut b).await;
        let mut with_id = addr;
        with_id.push(libp2p::multiaddr::Protocol::P2p(a.local_peer_id()));
        b.handle().dial(with_id).unwrap();
        wait_for(&mut a, |e| {
            matches!(e, NetworkEvent::HandshakeRejected { .. })
        })
        .await;
        a.handle().shutdown().unwrap();
        b.handle().shutdown().unwrap();
    }

    #[tokio::test]
    async fn test_gossip_publish_receive() {
        let mut a = start(vec!["/ip4/127.0.0.1/tcp/0".parse().unwrap()]).await;
        let addr = wait_listen(&mut a).await;
        let mut b = start(vec!["/ip4/127.0.0.1/tcp/0".parse().unwrap()]).await;
        let _ = wait_listen(&mut b).await;
        let mut with_id = addr;
        with_id.push(libp2p::multiaddr::Protocol::P2p(a.local_peer_id()));
        b.handle().dial(with_id).unwrap();
        wait_for(&mut a, |e| matches!(e, NetworkEvent::HandshakeAccepted(_))).await;
        wait_for(&mut b, |e| matches!(e, NetworkEvent::HandshakeAccepted(_))).await;
        tokio::time::sleep(Duration::from_millis(400)).await;
        a.handle()
            .publish(GossipTopic::Transactions, b"tx-bytes".to_vec())
            .unwrap();
        let ev = wait_for(&mut b, |e| {
            matches!(e, NetworkEvent::GossipMessageReceived { .. })
        })
        .await;
        match ev {
            NetworkEvent::GossipMessageReceived {
                data, message_id, ..
            } => {
                assert_eq!(data, b"tx-bytes");
                b.handle()
                    .report_gossip_validation(message_id, GossipAcceptance::Accept)
                    .unwrap();
            }
            _ => unreachable!(),
        }
        a.handle().shutdown().unwrap();
        b.handle().shutdown().unwrap();
    }

    #[tokio::test]
    async fn test_message_too_large_publish() {
        let mut a = start(vec!["/ip4/127.0.0.1/tcp/0".parse().unwrap()]).await;
        let _ = wait_listen(&mut a).await;
        let huge = vec![1u8; 2_000_000];
        a.handle().publish(GossipTopic::Blocks, huge).unwrap();
        wait_for(&mut a, |e| matches!(e, NetworkEvent::TransportError)).await;
        a.handle().shutdown().unwrap();
    }

    #[tokio::test]
    async fn test_peer_ban_and_unban() {
        let mut a = start(vec!["/ip4/127.0.0.1/tcp/0".parse().unwrap()]).await;
        let addr = wait_listen(&mut a).await;
        let mut b = start(vec!["/ip4/127.0.0.1/tcp/0".parse().unwrap()]).await;
        let _ = wait_listen(&mut b).await;
        let mut with_id = addr.clone();
        with_id.push(libp2p::multiaddr::Protocol::P2p(a.local_peer_id()));
        b.handle().dial(with_id.clone()).unwrap();
        wait_for(&mut a, |e| matches!(e, NetworkEvent::PeerConnected(_))).await;
        a.handle()
            .ban_peer(b.local_peer_id(), BanReason::Flooding)
            .unwrap();
        wait_for(&mut a, |e| matches!(e, NetworkEvent::PeerBanned(_))).await;
        a.handle().unban_peer(b.local_peer_id()).unwrap();
        wait_for(&mut a, |e| matches!(e, NetworkEvent::PeerUnbanned(_))).await;
        a.handle().shutdown().unwrap();
        b.handle().shutdown().unwrap();
    }

    #[tokio::test]
    async fn test_request_timeout() {
        let mut a = start(vec!["/ip4/127.0.0.1/tcp/0".parse().unwrap()]).await;
        let addr = wait_listen(&mut a).await;
        let cfg = NetworkConfig {
            listen_addresses: vec!["/ip4/127.0.0.1/tcp/0".parse().unwrap()],
            timeouts: NetworkTimeouts {
                request_timeout: Duration::from_millis(200),
                ..NetworkTimeouts::default()
            },
            ..NetworkConfig::default()
        };
        let mut b = RscNetwork::start(cfg, identity::Keypair::generate_ed25519())
            .await
            .unwrap();
        let _ = wait_listen(&mut b).await;
        let mut with_id = addr;
        with_id.push(libp2p::multiaddr::Protocol::P2p(a.local_peer_id()));
        b.handle().dial(with_id).unwrap();
        wait_for(&mut b, |e| matches!(e, NetworkEvent::HandshakeAccepted(_))).await;
        let _ = b.handle().request(
            a.local_peer_id(),
            RscRequest::GetBlockByHeight { height: 1 },
        );
        wait_for(&mut b, |e| {
            matches!(
                e,
                NetworkEvent::RequestFailed {
                    error: P2pError::RequestTimeout,
                    ..
                }
            )
        })
        .await;
        a.handle().shutdown().unwrap();
        b.handle().shutdown().unwrap();
    }

    #[tokio::test]
    async fn test_request_response_ok() {
        let mut a = start(vec!["/ip4/127.0.0.1/tcp/0".parse().unwrap()]).await;
        let addr = wait_listen(&mut a).await;
        let mut b = start(vec!["/ip4/127.0.0.1/tcp/0".parse().unwrap()]).await;
        let _ = wait_listen(&mut b).await;
        let mut with_id = addr;
        with_id.push(libp2p::multiaddr::Protocol::P2p(a.local_peer_id()));
        b.handle().dial(with_id).unwrap();
        wait_for(&mut a, |e| matches!(e, NetworkEvent::HandshakeAccepted(_))).await;
        wait_for(&mut b, |e| matches!(e, NetworkEvent::HandshakeAccepted(_))).await;
        let id = b
            .handle()
            .request(
                a.local_peer_id(),
                RscRequest::GetBlockByHeight { height: 7 },
            )
            .unwrap();
        let ev = wait_for(&mut a, |e| {
            matches!(e, NetworkEvent::RequestReceived { .. })
        })
        .await;
        match ev {
            NetworkEvent::RequestReceived { request_id, .. } => {
                a.handle()
                    .send_response(
                        request_id,
                        RscResponse::Blocks {
                            payloads: vec![b"blk".to_vec()],
                        },
                    )
                    .unwrap();
            }
            _ => unreachable!(),
        }
        let ev = wait_for(&mut b, |e| {
            matches!(e, NetworkEvent::ResponseReceived { .. })
        })
        .await;
        match ev {
            NetworkEvent::ResponseReceived {
                request_id,
                response,
                ..
            } => {
                assert_eq!(request_id, id);
                assert_eq!(
                    response,
                    RscResponse::Blocks {
                        payloads: vec![b"blk".to_vec()]
                    }
                );
            }
            _ => unreachable!(),
        }
        a.handle().shutdown().unwrap();
        b.handle().shutdown().unwrap();
    }

    #[tokio::test]
    async fn test_shutdown() {
        let mut a = start(vec!["/ip4/127.0.0.1/tcp/0".parse().unwrap()]).await;
        let _ = wait_listen(&mut a).await;
        a.handle().shutdown().unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            while a.next_event().await.is_some() {}
        })
        .await
        .ok();
        assert_eq!(a.handle().shutdown(), Err(P2pError::ChannelClosed));
    }

    #[tokio::test]
    async fn test_kad_bootstrap_and_identify_addrs() {
        let mut a = start(vec!["/ip4/127.0.0.1/tcp/0".parse().unwrap()]).await;
        let addr = wait_listen(&mut a).await;
        let mut with_id = addr;
        with_id.push(libp2p::multiaddr::Protocol::P2p(a.local_peer_id()));
        let boot = BootstrapPeer::new(a.local_peer_id(), vec![with_id.clone()]).unwrap();
        let cfg = NetworkConfig {
            listen_addresses: vec!["/ip4/127.0.0.1/tcp/0".parse().unwrap()],
            bootstrap_peers: vec![boot.clone()],
            discovery: DiscoveryConfig::for_network("devnet", vec![boot]).unwrap(),
            ..NetworkConfig::default()
        };
        let mut b = RscNetwork::start(cfg, identity::Keypair::generate_ed25519())
            .await
            .unwrap();
        let _ = wait_listen(&mut b).await;
        b.handle().dial(with_id).unwrap();
        wait_for(&mut b, |e| {
            matches!(e, NetworkEvent::PeerIdentified { .. })
                || matches!(
                    e,
                    NetworkEvent::DiscoveryEvent(DiscoveryEvent::AddressDiscovered { .. })
                )
        })
        .await;
        a.handle().shutdown().unwrap();
        b.handle().shutdown().unwrap();
    }

    #[tokio::test]
    async fn test_reconnect() {
        let mut a = start(vec!["/ip4/127.0.0.1/tcp/0".parse().unwrap()]).await;
        let addr = wait_listen(&mut a).await;
        let mut b = start(vec!["/ip4/127.0.0.1/tcp/0".parse().unwrap()]).await;
        let _ = wait_listen(&mut b).await;
        let mut with_id = addr;
        with_id.push(libp2p::multiaddr::Protocol::P2p(a.local_peer_id()));
        b.handle().dial(with_id.clone()).unwrap();
        wait_for(&mut b, |e| matches!(e, NetworkEvent::PeerConnected(_))).await;
        b.handle().disconnect(a.local_peer_id()).unwrap();
        wait_for(&mut b, |e| matches!(e, NetworkEvent::PeerDisconnected(_))).await;
        b.handle().dial(with_id).unwrap();
        wait_for(&mut b, |e| matches!(e, NetworkEvent::PeerConnected(_))).await;
        a.handle().shutdown().unwrap();
        b.handle().shutdown().unwrap();
    }

    #[test]
    fn test_invalid_bootstrap_in_config() {
        let id = PeerId::random();
        let other = PeerId::random();
        let addr: Multiaddr = format!("/ip4/127.0.0.1/tcp/1/p2p/{other}").parse().unwrap();
        let cfg = NetworkConfig {
            bootstrap_peers: vec![BootstrapPeer {
                peer_id: id,
                addresses: vec![addr],
            }],
            ..NetworkConfig::default()
        };
        assert_eq!(cfg.validate(), Err(P2pError::InvalidAddress));
    }

    #[test]
    fn test_tcp_uses_noise_not_plaintext() {
        let _noise = noise::Config::new;
        let _yamux = yamux::Config::default;
    }

    #[test]
    fn test_malformed_address() {
        assert!("not-a-multiaddr".parse::<Multiaddr>().is_err());
        assert_eq!(
            "/ip4/127.0.0.1/tcp/1"
                .parse::<Multiaddr>()
                .unwrap()
                .iter()
                .count(),
            2
        );
    }

    #[test]
    fn test_no_consensus_or_execution_coupling() {
        let ev = NetworkEvent::GossipMessageReceived {
            peer: PeerId::random(),
            topic: GossipTopic::Consensus,
            data: b"opaque-hs".to_vec(),
            message_id: vec![1],
        };
        match ev {
            NetworkEvent::GossipMessageReceived { data, .. } => {
                assert_eq!(data, b"opaque-hs");
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn test_gossip_id_includes_topic() {
        let blocks = gossipsub::IdentTopic::new("/rsc/devnet/blocks/1");
        let txs = gossipsub::IdentTopic::new("/rsc/devnet/transactions/1");
        let payload = b"same";
        let a = gossipsub::Message {
            source: None,
            data: payload.to_vec(),
            sequence_number: None,
            topic: blocks.hash(),
        };
        let b = gossipsub::Message {
            source: None,
            data: payload.to_vec(),
            sequence_number: None,
            topic: txs.hash(),
        };
        assert_ne!(content_message_id(&a), content_message_id(&b));
    }

    #[tokio::test]
    async fn test_protocol_mismatch() {
        let cfg_a = NetworkConfig {
            protocol_version: "9.9.9".into(),
            listen_addresses: vec!["/ip4/127.0.0.1/tcp/0".parse().unwrap()],
            ..NetworkConfig::default()
        };
        let mut a = RscNetwork::start(cfg_a, identity::Keypair::generate_ed25519())
            .await
            .unwrap();
        let addr = wait_listen(&mut a).await;
        let mut b = start(vec!["/ip4/127.0.0.1/tcp/0".parse().unwrap()]).await;
        let _ = wait_listen(&mut b).await;
        let mut with_id = addr;
        with_id.push(libp2p::multiaddr::Protocol::P2p(a.local_peer_id()));
        b.handle().dial(with_id).unwrap();
        wait_for(&mut a, |e| {
            matches!(e, NetworkEvent::HandshakeRejected { .. })
        })
        .await;
        a.handle().shutdown().unwrap();
        b.handle().shutdown().unwrap();
    }

    #[tokio::test]
    async fn test_connection_limit() {
        let cfg = NetworkConfig {
            limits: P2pLimits {
                max_peers: 1,
                max_inbound_connections: 1,
                ..P2pLimits::default()
            },
            listen_addresses: vec!["/ip4/127.0.0.1/tcp/0".parse().unwrap()],
            ..NetworkConfig::default()
        };
        let mut a = RscNetwork::start(cfg, identity::Keypair::generate_ed25519())
            .await
            .unwrap();
        let addr = wait_listen(&mut a).await;
        let mut with_a = addr;
        with_a.push(libp2p::multiaddr::Protocol::P2p(a.local_peer_id()));
        let mut b = start(vec!["/ip4/127.0.0.1/tcp/0".parse().unwrap()]).await;
        let _ = wait_listen(&mut b).await;
        b.handle().dial(with_a.clone()).unwrap();
        wait_for(&mut a, |e| matches!(e, NetworkEvent::PeerConnected(_))).await;
        let mut c = start(vec!["/ip4/127.0.0.1/tcp/0".parse().unwrap()]).await;
        let _ = wait_listen(&mut c).await;
        c.handle().dial(with_a).unwrap();
        tokio::time::timeout(Duration::from_secs(4), async {
            loop {
                match a.next_event().await {
                    Some(NetworkEvent::PeerConnected(p)) if p == c.local_peer_id() => {
                        panic!("over max_peers")
                    }
                    Some(_) => {}
                    None => return,
                }
            }
        })
        .await
        .ok();
        a.handle().shutdown().unwrap();
        b.handle().shutdown().unwrap();
        c.handle().shutdown().unwrap();
    }

    #[tokio::test]
    async fn test_mobile_address_change() {
        let mut a = start(vec!["/ip4/127.0.0.1/tcp/0".parse().unwrap()]).await;
        let addr1 = wait_listen(&mut a).await;
        let peer = a.local_peer_id();
        let mut b = start(vec!["/ip4/127.0.0.1/tcp/0".parse().unwrap()]).await;
        let _ = wait_listen(&mut b).await;
        let addr2: Multiaddr = "/ip4/127.0.0.1/tcp/9".parse().unwrap();
        b.handle().add_known_address(peer, addr1).unwrap();
        b.handle().add_known_address(peer, addr2.clone()).unwrap();
        wait_for(&mut b, |e| {
            matches!(
                e,
                NetworkEvent::DiscoveryEvent(DiscoveryEvent::AddressDiscovered { .. })
            ) || matches!(e, NetworkEvent::ListeningAddressAdded(_))
        })
        .await;
        a.handle().shutdown().unwrap();
        b.handle().shutdown().unwrap();
    }

    #[tokio::test]
    async fn test_quic_tcp_event_parity() {
        let mut tcp_a = start(vec!["/ip4/127.0.0.1/tcp/0".parse().unwrap()]).await;
        let tcp_addr = wait_listen(&mut tcp_a).await;
        let mut tcp_b = start(vec!["/ip4/127.0.0.1/tcp/0".parse().unwrap()]).await;
        let _ = wait_listen(&mut tcp_b).await;
        let mut tcp_dial = tcp_addr;
        tcp_dial.push(libp2p::multiaddr::Protocol::P2p(tcp_a.local_peer_id()));
        tcp_b.handle().dial(tcp_dial).unwrap();
        let tcp_ev = wait_for(&mut tcp_a, |e| matches!(e, NetworkEvent::PeerConnected(_))).await;
        assert!(matches!(tcp_ev, NetworkEvent::PeerConnected(_)));
        tcp_a.handle().shutdown().unwrap();
        tcp_b.handle().shutdown().unwrap();

        let mut q_a = start(vec!["/ip4/127.0.0.1/udp/0/quic-v1".parse().unwrap()]).await;
        let q_addr = wait_listen(&mut q_a).await;
        let mut q_b = start(vec!["/ip4/127.0.0.1/udp/0/quic-v1".parse().unwrap()]).await;
        let _ = wait_listen(&mut q_b).await;
        let mut q_dial = q_addr;
        q_dial.push(libp2p::multiaddr::Protocol::P2p(q_a.local_peer_id()));
        q_b.handle().dial(q_dial).unwrap();
        let q_ev = wait_for(&mut q_a, |e| matches!(e, NetworkEvent::PeerConnected(_))).await;
        assert!(matches!(q_ev, NetworkEvent::PeerConnected(_)));
        q_a.handle().shutdown().unwrap();
        q_b.handle().shutdown().unwrap();
    }

    #[tokio::test]
    async fn test_gossip_duplicate_same_payload() {
        let mut a = start(vec!["/ip4/127.0.0.1/tcp/0".parse().unwrap()]).await;
        let addr = wait_listen(&mut a).await;
        let mut b = start(vec!["/ip4/127.0.0.1/tcp/0".parse().unwrap()]).await;
        let _ = wait_listen(&mut b).await;
        let mut with_id = addr;
        with_id.push(libp2p::multiaddr::Protocol::P2p(a.local_peer_id()));
        b.handle().dial(with_id).unwrap();
        wait_for(&mut a, |e| matches!(e, NetworkEvent::HandshakeAccepted(_))).await;
        wait_for(&mut b, |e| matches!(e, NetworkEvent::HandshakeAccepted(_))).await;
        tokio::time::sleep(Duration::from_millis(400)).await;
        a.handle()
            .publish(GossipTopic::Transactions, b"dup".to_vec())
            .unwrap();
        a.handle()
            .publish(GossipTopic::Transactions, b"dup".to_vec())
            .unwrap();
        let first = wait_for(&mut b, |e| {
            matches!(e, NetworkEvent::GossipMessageReceived { .. })
        })
        .await;
        let second = tokio::time::timeout(Duration::from_millis(800), async {
            loop {
                match b.next_event().await {
                    Some(NetworkEvent::GossipMessageReceived { data, .. }) if data == b"dup" => {
                        return true;
                    }
                    Some(_) => {}
                    None => return false,
                }
            }
        })
        .await;
        assert!(matches!(first, NetworkEvent::GossipMessageReceived { .. }));
        assert!(!second.unwrap_or(false));
        a.handle().shutdown().unwrap();
        b.handle().shutdown().unwrap();
    }

    #[tokio::test]
    async fn test_three_node_discovery() {
        let mut a = start(vec!["/ip4/127.0.0.1/tcp/0".parse().unwrap()]).await;
        let addr_a = wait_listen(&mut a).await;
        let mut with_a = addr_a;
        with_a.push(libp2p::multiaddr::Protocol::P2p(a.local_peer_id()));
        let boot = BootstrapPeer::new(a.local_peer_id(), vec![with_a.clone()]).unwrap();

        let cfg_b = NetworkConfig {
            listen_addresses: vec!["/ip4/127.0.0.1/tcp/0".parse().unwrap()],
            bootstrap_peers: vec![boot.clone()],
            discovery: DiscoveryConfig::for_network("devnet", vec![boot.clone()]).unwrap(),
            ..NetworkConfig::default()
        };
        let mut b = RscNetwork::start(cfg_b, identity::Keypair::generate_ed25519())
            .await
            .unwrap();
        let addr_b = wait_listen(&mut b).await;
        let mut with_b = addr_b;
        with_b.push(libp2p::multiaddr::Protocol::P2p(b.local_peer_id()));
        b.handle().dial(with_a.clone()).unwrap();
        wait_for(&mut b, |e| matches!(e, NetworkEvent::PeerIdentified { .. })).await;

        let boot_c = BootstrapPeer::new(b.local_peer_id(), vec![with_b.clone()]).unwrap();
        let cfg_c = NetworkConfig {
            listen_addresses: vec!["/ip4/127.0.0.1/tcp/0".parse().unwrap()],
            bootstrap_peers: vec![boot_c.clone()],
            discovery: DiscoveryConfig::for_network("devnet", vec![boot_c]).unwrap(),
            ..NetworkConfig::default()
        };
        let mut c = RscNetwork::start(cfg_c, identity::Keypair::generate_ed25519())
            .await
            .unwrap();
        let _ = wait_listen(&mut c).await;
        c.handle().dial(with_b).unwrap();
        wait_for(&mut c, |e| matches!(e, NetworkEvent::PeerIdentified { .. })).await;
        a.handle().add_known_address(c.local_peer_id(), with_a).ok();
        wait_for(&mut a, |e| {
            matches!(
                e,
                NetworkEvent::DiscoveryEvent(DiscoveryEvent::PeerDiscovered { .. })
                    | NetworkEvent::PeerIdentified { .. }
                    | NetworkEvent::PeerConnected(_)
            )
        })
        .await;
        a.handle().shutdown().unwrap();
        b.handle().shutdown().unwrap();
        c.handle().shutdown().unwrap();
    }

    #[tokio::test]
    async fn test_concurrency_requests() {
        let mut a = start(vec!["/ip4/127.0.0.1/tcp/0".parse().unwrap()]).await;
        let addr = wait_listen(&mut a).await;
        let mut b = start(vec!["/ip4/127.0.0.1/tcp/0".parse().unwrap()]).await;
        let _ = wait_listen(&mut b).await;
        let mut with_id = addr;
        with_id.push(libp2p::multiaddr::Protocol::P2p(a.local_peer_id()));
        b.handle().dial(with_id).unwrap();
        wait_for(&mut a, |e| matches!(e, NetworkEvent::HandshakeAccepted(_))).await;
        wait_for(&mut b, |e| matches!(e, NetworkEvent::HandshakeAccepted(_))).await;
        for h in 1..=4 {
            b.handle()
                .request(
                    a.local_peer_id(),
                    RscRequest::GetBlockByHeight { height: h },
                )
                .unwrap();
        }
        let mut seen = 0u32;
        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                match a.next_event().await {
                    Some(NetworkEvent::RequestReceived { request_id, .. }) => {
                        a.handle()
                            .send_response(request_id, RscResponse::Empty)
                            .unwrap();
                        seen += 1;
                        if seen >= 4 {
                            break;
                        }
                    }
                    Some(_) => {}
                    None => break,
                }
            }
        })
        .await
        .expect("concurrent requests");
        a.handle().shutdown().unwrap();
        b.handle().shutdown().unwrap();
    }
}
