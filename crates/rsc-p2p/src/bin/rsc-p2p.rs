//! Proceso de escucha, bootstrap, `dial`, transacciones, propuestas y producción.
//! La base se abre solo en el modo `produce`.

use std::io::Write;
use std::net::SocketAddr;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rsc_chain::{
    decode_block, encode_block, validate_block, Block, BlockHeader, ChainStore,
    ChainValidationContext, RawTransaction, RocksDbChainStore,
};
use rsc_consensus::finality::{
    aggregate_qc, has_quorum, sign_vote, verify_commit_chain, verify_qc, verify_validator_vote,
    vote_bytes,
};
use rsc_consensus::{
    consensus_beacon, validate_validator_record, Committee, ConsensusConfig, ConsensusEngine,
    ConsensusEvent, ConsensusPhase, NetworkMode, ParticipationScore, QuorumCertificate,
    ValidatorRecord, ValidatorSet, Vote, VotePhase,
};
use rsc_core::Signature;
use rsc_genesis::{
    Genesis, GenesisAccount, GenesisValidator, NetworkParameters, GENESIS_VERSION_V1,
};
use rsc_core::{AccountId, BlockHash, BlockHeight, ChainId, Hash, Nonce, Timestamp};
use rsc_crypto::{PrivateKey, PublicKey};
use rsc_ledger::transaction::TRANSACTION_VERSION_V1;
use rsc_ledger::{Ledger, Transaction, TransactionError, TransactionPayload};
use rsc_mempool::{
    Mempool, MempoolConfig, MempoolError, MempoolTransaction, MempoolValidationError,
    MempoolValidator, NonceLookupError, PendingNonceProvider,
};
use rsc_state::{GenesisAccount as StateGenesisAccount, State, StateOperation, StateTransition};
use rsc_storage::{DatabaseConfig, RscDatabase, StorageBatch};
use rsc_p2p::{
    BootstrapPeer, DiscoveryConfig, DiscoveryEvent, GossipAcceptance, GossipTopic,
    Multiaddr, NetworkConfig, NetworkEvent, NetworkHandle, P2pError, PeerId,
    RscNetwork, RscRequest, RscRequestId, RscResponse, BLOCK_SYNC_PROTOCOL,
};
use rsc_rpc::{
    encode_hex, ChainService, ConsensusService, NodeService, RpcAccount, RpcBalance, RpcBlock,
    RpcConfig, RpcContext, RpcFinality, RpcNodeInfo, RpcServer, RpcStatus, RpcSubmitResult,
    RpcTransaction, RpcValidator, ServiceError, ServiceFut, StateService, TransactionService,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum ConsensusRole {
    Off,
    Propose,
    Observe,
    Vote(u8),
    Foreign,
    Produce {
        index: u8,
        casts: bool,
        goal: u8,
        pull: bool,
        fault: bool,
        audit: bool,
    },
}

struct Config {
    data_dir: String,
    network_id: String,
    chain_id: u32,
    dial: Option<String>,
    bootstrap: Option<String>,
    rpc: Option<String>,
    listen: Option<String>,
    signer_key: Option<String>,
    persist_peer: Option<String>,
    presence: Option<String>,
    relay: Option<String>,
    relay_client: Option<String>,
    circuit_dial: Option<String>,
    publish_presence: Option<String>,
    presence_reach: Option<String>,
    presence_addr: Option<String>,
    presence_ttl: Option<String>,
    consensus: ConsensusRole,
}

struct Duty {
    engine: Mutex<ConsensusEngine>,
    ctx: ChainValidationContext,
    key: Option<PrivateKey>,
    index: Option<u8>,
    validators: ValidatorSet,
    committee: Committee,
    pending: Mutex<Vec<Vec<u8>>>,
    ready: AtomicBool,
    publishes_block: AtomicBool,
    precommit_open: AtomicBool,
    commit_open: AtomicBool,
    seen: Mutex<Vec<Vote>>,
    once: Mutex<[bool; 3]>,
    casts: bool,
    goal: u8,
    pull: bool,
    fault: bool,
    audit: bool,
    sync_started: AtomicBool,
    sync_next: AtomicU64,
    parent: Mutex<Option<BlockHeader>>,
    proposal: Mutex<Option<Block>>,
    chain: Option<Arc<LiveChain>>,
}

struct LiveChain {
    state: Arc<State>,
    store: RocksDbChainStore,
    keys: Vec<(AccountId, PublicKey)>,
    pool: Mutex<Option<Arc<Mempool>>>,
    committed: AtomicBool,
}

fn main() -> ExitCode {
    let config = match parse_args(std::env::args().skip(1)) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(1);
        }
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(1);
        }
    };
    if let Err(err) = runtime.block_on(run(config)) {
        eprintln!("{err}");
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Config, String> {
    let mut data_dir = None;
    let mut network_id = None;
    let mut chain_id = None;
    let mut dial = None;
    let mut bootstrap = None;
    let mut rpc = None;
    let mut listen = None;
    let mut signer_key = None;
    let mut persist_peer = None;
    let mut presence = None;
    let mut relay = None;
    let mut relay_client = None;
    let mut circuit_dial = None;
    let mut publish_presence = None;
    let mut presence_reach = None;
    let mut presence_addr = None;
    let mut presence_ttl = None;
    let mut consensus = None;
    let mut args = args.into_iter();
    while let Some(flag) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| format!("falta el valor de {flag}"))?;
        match flag.as_str() {
            "--data-dir" => set(&mut data_dir, value, "data-dir")?,
            "--network-id" => set(&mut network_id, value, "network-id")?,
            "--dial" => set(&mut dial, value, "dial")?,
            "--bootstrap" => set(&mut bootstrap, value, "bootstrap")?,
            "--rpc" => set(&mut rpc, value, "rpc")?,
            "--listen" => set(&mut listen, value, "listen")?,
            "--signer-key" => set(&mut signer_key, value, "signer-key")?,
            "--persist-peer" => set(&mut persist_peer, value, "persist-peer")?,
            "--presence" => set(&mut presence, value, "presence")?,
            "--relay" => {
                set(&mut relay, value, "relay")?;
                if relay.as_deref() != Some("server") {
                    return Err("relay".into());
                }
            }
            "--relay-client" => set(&mut relay_client, value, "relay-client")?,
            "--circuit-dial" => set(&mut circuit_dial, value, "circuit-dial")?,
            "--publish-presence" => set(&mut publish_presence, value, "publish-presence")?,
            "--presence-reach" => set(&mut presence_reach, value, "presence-reach")?,
            "--presence-addr" => set(&mut presence_addr, value, "presence-addr")?,
            "--presence-ttl" => set(&mut presence_ttl, value, "presence-ttl")?,
            "--consensus" => set(&mut consensus, value, "consensus")?,
            "--chain-id" => {
                if chain_id.is_some() {
                    return Err("chain-id repetido".into());
                }
                if value.is_empty() {
                    return Err("chain-id vacío".into());
                }
                chain_id = Some(
                    value
                        .parse::<u32>()
                        .map_err(|_| "chain-id no es u32".to_string())?,
                );
            }
            other => return Err(format!("argumento desconocido {other}")),
        }
    }
    Ok(Config {
        data_dir: data_dir.ok_or("falta data-dir")?,
        network_id: network_id.ok_or("falta network-id")?,
        chain_id: chain_id.ok_or("falta chain-id")?,
        dial,
        bootstrap,
        rpc,
        listen,
        signer_key,
        persist_peer,
        presence,
        relay,
        relay_client,
        circuit_dial,
        publish_presence,
        presence_reach,
        presence_addr,
        presence_ttl,
        consensus: match consensus.as_deref() {
            None => ConsensusRole::Off,
            Some("propose") => ConsensusRole::Propose,
            Some("observe") => ConsensusRole::Observe,
            Some(role) => named_role(role)?,
        },
    })
}

fn set(slot: &mut Option<String>, value: String, name: &str) -> Result<(), String> {
    if slot.is_some() {
        return Err(format!("{name} repetido"));
    }
    if value.is_empty() {
        return Err(format!("{name} vacío"));
    }
    *slot = Some(value);
    Ok(())
}

async fn run(config: Config) -> Result<(), String> {
    if config.publish_presence.is_some() {
        return publish_presence(&config);
    }
    if let ConsensusRole::Foreign = config.consensus {
        return reject_swapped_key(config.chain_id);
    }
    std::fs::create_dir_all(&config.data_dir).map_err(|err| err.to_string())?;
    let produced = match config.consensus {
        ConsensusRole::Produce {
            index,
            casts,
            goal,
            pull,
            fault,
            audit,
        } => Some(open_produce(
            &config,
            ProduceOpts {
                index,
                casts,
                goal,
                pull,
                fault,
                audit,
            },
        )?),
        _ => None,
    };
    let pool = if let Some((live, _)) = &produced {
        Some(open_chain_pool(live)?)
    } else {
        open_pool(config.signer_key.as_deref())?
    };
    if pool.is_some() && config.rpc.is_none() {
        return Err("falta rpc".into());
    }
    if pool.is_none() && config.rpc.is_some() {
        return Err("falta signer-key".into());
    }
    let dial = config
        .dial
        .map(|value| value.parse::<Multiaddr>())
        .transpose()
        .map_err(|_| P2pError::InvalidAddress.to_string())?;
    let bootstrap = bootstrap_peers(config.bootstrap.as_deref())?;
    let listen = listen_addresses(config.listen.as_deref(), config.presence.as_deref())?;
    let relay_client = optional_peer_addr(config.relay_client.as_deref())?;
    let circuit_dial = optional_peer_addr(config.circuit_dial.as_deref())?;
    let network = NetworkConfig {
        network_id: config.network_id.clone(),
        chain_id: ChainId(config.chain_id),
        listen_addresses: listen,
        bootstrap_peers: bootstrap.clone(),
        discovery: DiscoveryConfig::for_network(&config.network_id, bootstrap)
            .map_err(|err| err.to_string())?,
        relay_server: config.relay.as_deref() == Some("server"),
        relay_client,
        circuit_dial,
        ..NetworkConfig::default()
    };
    network.validate().map_err(|err| err.to_string())?;
    let keypair = open_network_key(config.persist_peer.as_deref())?;
    let mut network = RscNetwork::start(network, keypair)
        .await
        .map_err(|err| err.to_string())?;
    say(&format!("peer {}", network.local_peer_id()));
    let listen = wait_listen(&mut network).await?;
    let mut announced = listen;
    announced.push(libp2p::multiaddr::Protocol::P2p(network.local_peer_id()));
    say(&format!("listen {announced}"));
    if let Some(address) = dial {
        network.handle().dial(address).map_err(|err| err.to_string())?;
    }
    let live = produced.as_ref().map(|(live, _)| Arc::clone(live));
    if let (Some(pool), Some(rpc)) = (pool.clone(), config.rpc.as_deref()) {
        serve_rpc(
            pool,
            network.handle().clone(),
            &config.network_id,
            config.chain_id,
            network.local_peer_id().to_string(),
            rpc,
            live,
        )?;
    }
    let duty = if let Some((_, duty)) = produced {
        Some(duty)
    } else {
        open_duty(config.consensus, config.chain_id)?
    };
    let handshakes = Arc::new(AtomicUsize::new(0));
    match (config.consensus, duty.clone()) {
        (ConsensusRole::Propose, Some(duty)) => {
            let handle = network.handle().clone();
            let pending = Arc::clone(&handshakes);
            tokio::spawn(async move {
                publish_proposals(duty, handle, pending).await;
            });
        }
        (ConsensusRole::Vote(_), Some(duty)) => {
            let handle = network.handle().clone();
            let pending = Arc::clone(&handshakes);
            tokio::spawn(async move {
                publish_vote_block(duty, handle, pending).await;
            });
        }
        (ConsensusRole::Produce { .. }, Some(duty)) => {
            let handle = network.handle().clone();
            let pending = Arc::clone(&handshakes);
            tokio::spawn(async move {
                watch_ready(duty, handle, pending).await;
            });
        }
        _ => {}
    }
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .map_err(|err| err.to_string())?;
    loop {
        tokio::select! {
            _ = terminate.recv() => {
                network.handle().shutdown().map_err(|err| err.to_string())?;
                while network.next_event().await.is_some() {}
                return Ok(());
            }
            event = network.next_event() => {
                match event {
                    Some(event) => {
                        dispatch(pool.as_ref(), duty.as_ref(), &handshakes, network.handle(), event)
                    }
                    None => return Ok(()),
                }
            }
        }
    }
}

fn publish_presence(config: &Config) -> Result<(), String> {
    if !matches!(config.consensus, ConsensusRole::Off) {
        return Err("presence publish".into());
    }
    let url = config
        .publish_presence
        .as_deref()
        .ok_or("falta publish-presence")?;
    let key_path = config.persist_peer.as_deref().ok_or("falta persist-peer")?;
    let keypair = open_network_key(Some(key_path))?;
    let (reach, nat) = match config.presence_reach.as_deref() {
        Some("public") => (
            rsc_p2p::presence::Reach::PublicDirect,
            rsc_p2p::presence::NatView::Public,
        ),
        Some("relay") => (
            rsc_p2p::presence::Reach::Relay,
            rsc_p2p::presence::NatView::Private,
        ),
        Some("local") => (
            rsc_p2p::presence::Reach::Local,
            rsc_p2p::presence::NatView::Private,
        ),
        Some("unknown") => (
            rsc_p2p::presence::Reach::Unknown,
            rsc_p2p::presence::NatView::Unknown,
        ),
        _ => return Err("presence-reach".into()),
    };
    let addresses = match config.presence_addr.as_deref() {
        Some(value) => vec![value.parse::<Multiaddr>().map_err(|_| "invalid address".to_string())?],
        None => Vec::new(),
    };
    let ttl = match config.presence_ttl.as_deref() {
        None => 300_u64,
        Some(value) => value.parse::<u64>().map_err(|_| "presence-ttl".to_string())?,
    };
    if ttl == 0 {
        return Err("presence-ttl".into());
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| "presence clock".to_string())?
        .as_secs();
    let path = std::path::Path::new(key_path);
    let sequence = rsc_p2p::presence::read_sequence(path)
        .map_err(|_| "presence sequence".to_string())?
        .checked_add(1)
        .ok_or("presence sequence")?;
    let signed = rsc_p2p::presence::PresenceDraft {
        network_id: config.network_id.clone(),
        chain_id: config.chain_id,
        chain: None,
        reach,
        nat,
        addresses,
        issued_at: now,
        expires_at: now.checked_add(ttl).ok_or("presence-ttl")?,
        sequence,
    }
    .sign(&keypair)
    .map_err(|err| err.to_string())?;
    let wire = signed.to_wire().map_err(|err| err.to_string())?;
    rsc_p2p::presence::write_sequence(path, sequence).map_err(|_| "presence sequence".to_string())?;
    post_presence(url, &wire)?;
    println!("published {} {sequence}", signed.record.peer_id);
    Ok(())
}

fn post_presence(url: &str, body: &[u8]) -> Result<(), String> {
    use std::io::{Read, Write};
    let rest = url.strip_prefix("http://").ok_or("presence url")?;
    let (host, path) = rest.split_once('/').ok_or("presence url")?;
    let address: SocketAddr = host.parse().map_err(|_| "presence url".to_string())?;
    let mut stream = std::net::TcpStream::connect_timeout(&address, Duration::from_secs(5))
        .map_err(|_| "presence post".to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|_| "presence post".to_string())?;
    let header = format!(
        "POST /{path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(header.as_bytes())
        .map_err(|_| "presence post".to_string())?;
    stream.write_all(body).map_err(|_| "presence post".to_string())?;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .map_err(|_| "presence post".to_string())?;
    let text = String::from_utf8_lossy(&response);
    let status = text.split("\r\n").next().unwrap_or("");
    if status.starts_with("HTTP/1.1 204") {
        Ok(())
    } else {
        Err(format!("presence rejected {status}"))
    }
}

fn open_network_key(path: Option<&str>) -> Result<libp2p::identity::Keypair, String> {
    let Some(path) = path else {
        return Ok(libp2p::identity::Keypair::generate_ed25519());
    };
    let path = std::path::Path::new(path);
    if path.is_file() {
        let bytes = std::fs::read(path).map_err(|_| "network identity".to_string())?;
        let keypair = libp2p::identity::Keypair::from_protobuf_encoding(&bytes)
            .map_err(|_| "network identity".to_string())?;
        if keypair.key_type() != libp2p::identity::KeyType::Ed25519 {
            return Err("network identity".into());
        }
        return Ok(keypair);
    }
    if path.exists() {
        return Err("network identity".into());
    }
    match path.parent() {
        Some(dir) if dir.is_dir() => {}
        _ => return Err("network identity".into()),
    }
    let keypair = libp2p::identity::Keypair::generate_ed25519();
    let bytes = keypair
        .to_protobuf_encoding()
        .map_err(|_| "network identity".to_string())?;
    write_network_key(path, &bytes)?;
    Ok(keypair)
}

fn write_network_key(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("network identity")?;
    let tmp = path.with_file_name(format!(".{name}.tmp"));
    if tmp.exists() && !path.exists() {
        std::fs::remove_file(&tmp).map_err(|_| "network identity".to_string())?;
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(|_| "network identity".to_string())?;
    file.write_all(bytes)
        .map_err(|_| "network identity".to_string())?;
    file.sync_all()
        .map_err(|_| "network identity".to_string())?;
    drop(file);
    std::fs::rename(&tmp, path).map_err(|_| "network identity".to_string())?;
    Ok(())
}

fn optional_peer_addr(value: Option<&str>) -> Result<Option<Multiaddr>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let addr = value
        .parse::<Multiaddr>()
        .map_err(|_| P2pError::InvalidAddress.to_string())?;
    if rsc_p2p::discovery::peer_id_from_multiaddr(&addr).is_none() {
        return Err(P2pError::InvalidAddress.to_string());
    }
    Ok(Some(addr))
}

fn listen_addresses(listen: Option<&str>, presence: Option<&str>) -> Result<Vec<Multiaddr>, String> {
    if presence.is_some() && presence != Some("auto") {
        return Err("presence".into());
    }
    if let Some(value) = listen {
        let addr = value.parse::<Multiaddr>().map_err(|_| "listen".to_string())?;
        ensure_listen_free(&addr)?;
        return Ok(vec![addr]);
    }
    if presence == Some("auto") {
        let addrs = rsc_p2p::presence::autonomous_listen();
        for addr in &addrs {
            ensure_listen_free(addr)?;
        }
        return Ok(addrs);
    }
    let addr = "/ip4/127.0.0.1/tcp/0"
        .parse::<Multiaddr>()
        .map_err(|_| "listen".to_string())?;
    Ok(vec![addr])
}

fn bootstrap_peers(value: Option<&str>) -> Result<Vec<BootstrapPeer>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let address = value
        .parse::<Multiaddr>()
        .map_err(|_| P2pError::InvalidAddress.to_string())?;
    let peer_id = rsc_p2p::discovery::peer_id_from_multiaddr(&address)
        .ok_or_else(|| P2pError::InvalidAddress.to_string())?;
    let peer = BootstrapPeer::new(peer_id, vec![address]).map_err(|err| err.to_string())?;
    Ok(vec![peer])
}

async fn wait_listen(network: &mut RscNetwork) -> Result<Multiaddr, String> {
    loop {
        match network.next_event().await {
            Some(NetworkEvent::ListeningAddressAdded(address)) => return Ok(address),
            Some(event) => report(event),
            None => return Err("p2p cerrado antes de escuchar".into()),
        }
    }
}

fn report(event: NetworkEvent) {
    match event {
        NetworkEvent::ListeningAddressAdded(address) => say(&format!("listen {address}")),
        NetworkEvent::Presence(line) => say(&line),
        NetworkEvent::Relay(line) => say(&line),
        NetworkEvent::PeerConnected(peer) => say(&format!("connected {peer}")),
        NetworkEvent::HandshakeAccepted(peer) => say(&format!("handshake {peer}")),
        NetworkEvent::HandshakeRejected { peer, error } => {
            say(&format!("rejected {peer} {error}"));
        }
        NetworkEvent::DialFailed { error, .. } => say(&format!("dial-failed {error}")),
        NetworkEvent::PeerIdentified { peer_id, .. } => say(&format!("identified {peer_id}")),
        NetworkEvent::DiscoveryEvent(event) => report_discovery(event),
        _ => {}
    }
}

fn report_discovery(event: DiscoveryEvent) {
    match event {
        DiscoveryEvent::BootstrapStarted => say("bootstrap-started"),
        DiscoveryEvent::BootstrapSucceeded => say("bootstrap-ok"),
        DiscoveryEvent::BootstrapFailed => say("bootstrap-failed"),
        DiscoveryEvent::PeerDiscovered { peer_id } => say(&format!("discovered {peer_id}")),
        DiscoveryEvent::AddressDiscovered { peer_id, address } => {
            say(&format!("address {peer_id} {address}"));
        }
        DiscoveryEvent::AddressRemoved { peer_id, address } => {
            say(&format!("address-removed {peer_id} {address}"));
        }
    }
}

fn say(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

fn open_pool(signer: Option<&str>) -> Result<Option<Arc<Mempool>>, String> {
    let Some(signer) = signer else {
        return Ok(None);
    };
    let bytes = rsc_rpc::decode_hex(signer).map_err(|_| "signer-key inválida".to_string())?;
    let key = PublicKey::from_bytes(&bytes).map_err(|_| "signer-key inválida".to_string())?;
    let admission = Arc::new(SignedAdmission { key });
    Ok(Some(Arc::new(Mempool::new(
        MempoolConfig::default(),
        admission,
        Arc::new(FreshAccounts),
    ))))
}

fn ensure_listen_free(addr: &Multiaddr) -> Result<(), String> {
    let port = addr.iter().find_map(|protocol| match protocol {
        libp2p::multiaddr::Protocol::Tcp(port) => Some(port),
        _ => None,
    });
    let Some(port) = port else {
        return Ok(());
    };
    if port == 0 {
        return Ok(());
    }
    let _probe =
        std::net::TcpListener::bind(("127.0.0.1", port)).map_err(|_| "listen failed".to_string())?;
    Ok(())
}

fn reject_swapped_key(chain_id: u32) -> Result<(), String> {
    let (genesis, keys) = vote_genesis(chain_id)?;
    let validators = genesis.validator_set().map_err(|err| err.to_string())?;
    let record = validators
        .validators
        .first()
        .ok_or("validador ausente")?;
    let key = keys.get(1).ok_or("clave ausente")?;
    if record.owner_public_key == key.public_key() && record.owner_key_id == key.key_id() {
        return Ok(());
    }
    Err("invalid signature".into())
}

fn serve_rpc(
    pool: Arc<Mempool>,
    p2p: NetworkHandle,
    network_id: &str,
    chain_id: u32,
    peer: String,
    rpc: &str,
    live: Option<Arc<LiveChain>>,
) -> Result<(), String> {
    let bind: SocketAddr = rpc.parse().map_err(|_| "rpc inválido".to_string())?;
    let probe = std::net::TcpListener::bind(bind).map_err(|err| format!("rpc {err}"))?;
    drop(probe);
    let service = Arc::new(Propagation {
        pool,
        p2p,
        network_id: network_id.to_string(),
        chain_id,
        peer,
        live,
    });
    let server = RpcServer::new(
        RpcContext::new(
            Arc::clone(&service) as Arc<dyn NodeService>,
            Arc::clone(&service) as Arc<dyn ChainService>,
            Arc::clone(&service) as Arc<dyn TransactionService>,
            Arc::clone(&service) as Arc<dyn StateService>,
            service as Arc<dyn ConsensusService>,
        ),
        RpcConfig {
            bind_address: bind,
            ..RpcConfig::default()
        },
    );
    tokio::spawn(async move {
        if let Err(err) = server.run().await {
            eprintln!("rpc {err}");
        }
    });
    say(&format!("rpc {bind}"));
    Ok(())
}

fn dispatch(
    pool: Option<&Arc<Mempool>>,
    duty: Option<&Arc<Duty>>,
    handshakes: &AtomicUsize,
    handle: &NetworkHandle,
    event: NetworkEvent,
) {
    if matches!(event, NetworkEvent::HandshakeAccepted(_)) {
        handshakes.fetch_add(1, Ordering::Relaxed);
    }
    match event {
        NetworkEvent::GossipMessageReceived {
            topic,
            data,
            message_id,
            ..
        } => match topic {
            GossipTopic::Transactions => {
                if let Some(pool) = pool {
                    admit_gossip(pool, handle, &data, message_id);
                }
            }
            GossipTopic::Blocks => {
                if let Some(duty) = duty {
                    if !duty.pull {
                        accept_proposal(duty, handle, &data, message_id);
                    }
                }
            }
            GossipTopic::Consensus => {
                if let Some(duty) = duty {
                    if duty.index.is_some() && !duty.pull && !duty.fault {
                        handle_consensus(duty, handle, data, message_id);
                    }
                }
            }
            _ => {}
        },
        NetworkEvent::HandshakeAccepted(peer) => {
            report(NetworkEvent::HandshakeAccepted(peer));
            if let Some(duty) = duty {
                if duty.pull && !duty.sync_started.swap(true, Ordering::Relaxed) {
                    request_height(duty, handle, peer, sync_start(duty));
                }
            }
        }
        NetworkEvent::RequestReceived {
            request_id,
            request,
            ..
        } => {
            if let Some(duty) = duty {
                serve_sync(duty, handle, request_id, request);
            }
        }
        NetworkEvent::ResponseReceived { peer, response, .. } => {
            if let Some(duty) = duty {
                take_sync(duty, handle, peer, response);
            }
        }
        NetworkEvent::RequestFailed { error, .. } => {
            if duty.is_some_and(|duty| duty.pull) {
                say(&format!("sync-failed {error}"));
            }
        }
        other => report(other),
    }
}

fn named_role(role: &str) -> Result<ConsensusRole, String> {
    if let Some(index) = role.strip_prefix("vote-") {
        return parse_index(index).map(ConsensusRole::Vote);
    }
    if let Some(role) = role.strip_prefix("produce-") {
        return parse_index(role).map(|index| produce_role(index, true, 1, false, false, false));
    }
    if let Some(role) = role.strip_prefix("shadow-") {
        return parse_index(role).map(|index| produce_role(index, false, 1, false, false, false));
    }
    if let Some(role) = role.strip_prefix("sync-") {
        return parse_index(role).map(|index| produce_role(index, true, 2, false, false, false));
    }
    if let Some(role) = role.strip_prefix("catch-") {
        return parse_index(role).map(|index| produce_role(index, true, 3, false, false, false));
    }
    if let Some(role) = role.strip_prefix("devnet-") {
        return parse_index(role).map(|index| produce_role(index, true, 4, false, false, false));
    }
    if let Some(role) = role.strip_prefix("stress-") {
        return parse_index(role).map(|index| produce_role(index, true, 48, false, false, false));
    }
    if role == "foreign" {
        return Ok(ConsensusRole::Foreign);
    }
    if role == "behind" {
        return Ok(produce_role(3, false, 0, true, false, false));
    }
    if role == "probe" {
        return Ok(produce_role(1, false, 0, true, false, true));
    }
    if role == "gap" {
        return Ok(produce_role(2, false, 0, false, true, false));
    }
    if role == "audit" {
        return Ok(produce_role(0, false, 0, true, false, true));
    }
    if role == "fault" {
        return Ok(produce_role(0, false, 0, false, true, false));
    }
    Err("consensus inválido".into())
}

fn produce_role(
    index: u8,
    casts: bool,
    goal: u8,
    pull: bool,
    fault: bool,
    audit: bool,
) -> ConsensusRole {
    ConsensusRole::Produce {
        index,
        casts,
        goal,
        pull,
        fault,
        audit,
    }
}

fn parse_index(index: &str) -> Result<u8, String> {
    let index = index
        .parse::<u8>()
        .map_err(|_| "consensus inválido".to_string())?;
    if index > 3 {
        return Err("consensus inválido".into());
    }
    Ok(index)
}

fn open_duty(role: ConsensusRole, chain_id: u32) -> Result<Option<Arc<Duty>>, String> {
    if matches!(role, ConsensusRole::Off | ConsensusRole::Foreign) {
        return Ok(None);
    }
    if let ConsensusRole::Vote(index) = role {
        return open_vote_duty(index, chain_id).map(Some);
    }
    let (validators, proposer) = protocol_validator()?;
    let local = if role == ConsensusRole::Propose {
        proposer
    } else {
        account_of_seed(0x22)?
    };
    let engine = ConsensusEngine::new(
        consensus_config(),
        validators.clone(),
        consensus_beacon(&Hash::ZERO, 0, 0, &Hash::ZERO),
        local,
        0,
    )
    .map_err(|err| err.to_string())?;
    Ok(Some(pack_duty(
        engine,
        chain_context(chain_id),
        None,
        None,
        validators,
    )))
}

fn open_vote_duty(index: u8, chain_id: u32) -> Result<Arc<Duty>, String> {
    let (genesis, mut keys) = vote_genesis(chain_id)?;
    let validators = genesis.validator_set().map_err(|err| err.to_string())?;
    let config = genesis
        .parameters
        .consensus_config()
        .map_err(|err| err.to_string())?;
    let local = validators.validators[usize::from(index)].id;
    let key = keys.remove(usize::from(index));
    let engine = ConsensusEngine::new(
        config,
        validators.clone(),
        genesis.parameters.consensus_randomness_seed,
        local,
        0,
    )
    .map_err(|err| err.to_string())?;
    let proposer = engine.proposer().map_err(|err| err.to_string())?;
    let power = validators.validators[usize::from(index)].voting_power;
    say(&format!(
        "validator {index} {} {power}",
        encode_hex(&local.0)
    ));
    say(&format!("proposer {}", encode_hex(&proposer.0)));
    Ok(pack_duty(
        engine,
        chain_context(chain_id),
        Some(key),
        Some(index),
        validators,
    ))
}

fn pack_duty(
    engine: ConsensusEngine,
    ctx: ChainValidationContext,
    key: Option<PrivateKey>,
    index: Option<u8>,
    validators: ValidatorSet,
) -> Arc<Duty> {
    let committee = engine.state().committee.clone();
    Arc::new(Duty {
        engine: Mutex::new(engine),
        ctx,
        key,
        index,
        validators,
        committee,
        pending: Mutex::new(Vec::new()),
        ready: AtomicBool::new(false),
        publishes_block: AtomicBool::new(false),
        precommit_open: AtomicBool::new(false),
        commit_open: AtomicBool::new(false),
        seen: Mutex::new(Vec::new()),
        once: Mutex::new([false; 3]),
        casts: true,
        goal: 0,
        pull: false,
        fault: false,
        audit: false,
        sync_started: AtomicBool::new(false),
        sync_next: AtomicU64::new(1),
        parent: Mutex::new(None),
        proposal: Mutex::new(None),
        chain: None,
    })
}

fn context(duty: &Duty) -> ChainValidationContext {
    let mut ctx = duty.ctx.clone();
    if let Ok(parent) = duty.parent.lock() {
        if let Some(header) = parent.as_ref() {
            ctx.parent = Some(header.clone());
        }
    }
    ctx
}

fn chain_context(chain_id: u32) -> ChainValidationContext {
    ChainValidationContext {
        expected_chain_id: ChainId(chain_id),
        expected_protocol_version: 1,
        max_block_size: 1_048_576,
        max_transactions_per_block: 1_000,
        parent: None,
    }
}

fn vote_genesis(chain_id: u32) -> Result<(Genesis, Vec<PrivateKey>), String> {
    let seeds = [0x11u8, 0x22, 0x33, 0x44];
    let powers = [2u128, 2, 1, 1];
    let mut keys = Vec::new();
    let mut validators = Vec::new();
    let mut accounts = Vec::new();
    for (seed, power) in seeds.into_iter().zip(powers) {
        let key = PrivateKey::from_bytes(&[seed; 32]).map_err(|_| "clave".to_string())?;
        let id = AccountId::from(rsc_state::derive_account_id(&key.key_id()));
        accounts.push(GenesisAccount {
            account_id: id,
            owner_key_id: key.key_id(),
            native_balance: 1,
            allocation: rsc_genesis::GenesisAllocationCategory::Community,
        });
        validators.push(GenesisValidator {
            account_id: id,
            owner_key_id: key.key_id(),
            owner_public_key: key.public_key(),
            voting_power: power,
            eligible: true,
        });
        keys.push(key);
    }
    let genesis = Genesis::new(
        GENESIS_VERSION_V1,
        ChainId(chain_id),
        1,
        NetworkParameters {
            amount_scale: 1_000_000,
            max_block_size: 1_048_576,
            max_transactions_per_block: 1_000,
            max_transaction_bytes: 128 * 1024,
            minimum_fee: 0,
            committee_size: 4,
            epoch_length: 10,
            min_validator_weight: 1,
            max_validator_weight: 20,
            min_validators_for_distributed: 5,
            consensus_randomness_seed: Hash([0x5A; 32]),
        },
        accounts,
        validators,
    );
    genesis.validate().map_err(|err| err.to_string())?;
    Ok((genesis, keys))
}

fn consensus_config() -> ConsensusConfig {
    ConsensusConfig {
        committee_size: 1,
        epoch_length: 10,
        min_validator_weight: 1,
        max_validator_weight: 20,
        min_validators_for_distributed: 4,
    }
}

fn protocol_validator() -> Result<(ValidatorSet, AccountId), String> {
    let key = PrivateKey::from_bytes(&[0x11; 32]).map_err(|_| "clave".to_string())?;
    let id = AccountId::from(rsc_state::derive_account_id(&key.key_id()));
    let record = ValidatorRecord {
        id,
        owner_key_id: key.key_id(),
        owner_public_key: key.public_key(),
        voting_power: 1,
        participation_score: ParticipationScore(0),
        eligible: true,
    };
    validate_validator_record(&record).map_err(|err| err.to_string())?;
    Ok((
        ValidatorSet {
            validators: vec![record],
            mode: NetworkMode::Bootstrap,
        },
        id,
    ))
}

fn account_of_seed(seed: u8) -> Result<AccountId, String> {
    let key = PrivateKey::from_bytes(&[seed; 32]).map_err(|_| "clave".to_string())?;
    Ok(AccountId::from(rsc_state::derive_account_id(&key.key_id())))
}

async fn publish_proposals(duty: Arc<Duty>, handle: NetworkHandle, handshakes: Arc<AtomicUsize>) {
    while handshakes.load(Ordering::Relaxed) < 2 {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    let proposer = match duty.engine.lock() {
        Ok(engine) => match engine.proposer() {
            Ok(id) => id,
            Err(err) => {
                say(&format!("publish-failed {err}"));
                return;
            }
        },
        Err(_) => return,
    };
    say(&format!("proposer {}", encode_hex(&proposer.0)));
    let chain_id = duty.ctx.expected_chain_id;
    let good = proposal_block(proposer, 0, true, chain_id);
    let hash = match good.block_hash() {
        Ok(hash) => encode_hex(&hash.0 .0),
        Err(err) => {
            say(&format!("publish-failed {err}"));
            return;
        }
    };
    let bytes = match encode_block(&good) {
        Ok(bytes) => bytes,
        Err(err) => {
            say(&format!("publish-failed {err}"));
            return;
        }
    };
    publish_bytes(
        &handle,
        GossipTopic::Blocks,
        bytes.clone(),
        &format!("published {hash}"),
    );
    publish_bytes(
        &handle,
        GossipTopic::Blocks,
        bytes,
        &format!("published {hash}"),
    );
    tokio::time::sleep(Duration::from_secs(1)).await;
    let Ok(other) = account_of_seed(0x22) else {
        return;
    };
    if let Ok(bytes) = encode_block(&proposal_block(other, 0, true, chain_id)) {
        publish_bytes(
            &handle,
            GossipTopic::Blocks,
            bytes,
            "published-invalid invalid proposer",
        );
    }
    if let Ok(bytes) = encode_block(&proposal_block(proposer, 1, true, chain_id)) {
        publish_bytes(
            &handle,
            GossipTopic::Blocks,
            bytes,
            "published-invalid wrong view",
        );
    }
    if let Ok(bytes) = encode_block(&proposal_block(proposer, 0, false, chain_id)) {
        publish_bytes(
            &handle,
            GossipTopic::Blocks,
            bytes,
            "published-invalid invalid block",
        );
    }
    publish_bytes(
        &handle,
        GossipTopic::Blocks,
        vec![0xff],
        "published-invalid invalid encoding",
    );
}

fn proposal_block(proposer: AccountId, height: u64, honest_root: bool, chain_id: ChainId) -> Block {
    let mut block = Block {
        header: BlockHeader {
            version: 1,
            chain_id,
            height: BlockHeight(height),
            previous_block_hash: BlockHash::ZERO,
            timestamp: Timestamp::from_secs(1),
            transactions_root: Hash::ZERO,
            state_root: Hash::ZERO,
            proposer,
        },
        transactions: Vec::new(),
    };
    block.header.transactions_root = if honest_root {
        block.compute_transaction_root()
    } else {
        Hash([1; 32])
    };
    block
}

fn publish_bytes(handle: &NetworkHandle, topic: GossipTopic, bytes: Vec<u8>, label: &str) {
    match handle.publish(topic, bytes) {
        Ok(()) => say(label),
        Err(err) => say(&format!("publish-failed {err}")),
    }
}

fn accept_proposal(duty: &Arc<Duty>, handle: &NetworkHandle, data: &[u8], message_id: Vec<u8>) {
    let block = match decode_block(data) {
        Ok(block) => block,
        Err(err) => {
            say(&format!("proposal-rejected {err}"));
            let _ = handle.report_gossip_validation(message_id, GossipAcceptance::Reject);
            return;
        }
    };
    let mut engine = match duty.engine.lock() {
        Ok(engine) => engine,
        Err(_) => return,
    };
    let ctx = context(duty);
    let accepted = match engine.receive_proposal(&block, Some(&ctx)) {
        Ok(ConsensusEvent::ProposalAccepted) => {
            let hash = block
                .block_hash()
                .map(|hash| encode_hex(&hash.0 .0))
                .unwrap_or_default();
            if let Ok(mut kept) = duty.proposal.lock() {
                *kept = Some(block);
            }
            say(&format!("accepted {hash}"));
            say_state(&engine);
            let _ = handle.report_gossip_validation(message_id, GossipAcceptance::Accept);
            true
        }
        Ok(_) => false,
        Err(err) => {
            say(&format!("proposal-rejected {err}"));
            say_state(&engine);
            let _ = handle.report_gossip_validation(message_id, GossipAcceptance::Reject);
            false
        }
    };
    drop(engine);
    if accepted {
        note_proposal(duty, handle);
    }
}

fn note_proposal(duty: &Arc<Duty>, handle: &NetworkHandle) {
    if duty.index.is_none() {
        return;
    }
    duty.ready.store(true, Ordering::Relaxed);
    flush_consensus(duty, handle);
    if duty.casts && duty.index.unwrap_or(9) < 3 && !duty.publishes_block.load(Ordering::Relaxed) {
        cast_vote(duty, handle, VotePhase::Prepare);
    }
    if duty.chain.is_some() && duty.index == Some(3) {
        let duty = Arc::clone(duty);
        let handle = handle.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            publish_block_negatives(&duty, &handle);
        });
    }
}

async fn publish_vote_block(duty: Arc<Duty>, handle: NetworkHandle, handshakes: Arc<AtomicUsize>) {
    let (proposer, local) = {
        let Ok(engine) = duty.engine.lock() else {
            return;
        };
        let Ok(proposer) = engine.proposer() else {
            return;
        };
        let Some(key) = duty.key.as_ref() else {
            return;
        };
        let local = AccountId::from(rsc_state::derive_account_id(&key.key_id()));
        (proposer, local)
    };
    if proposer != local {
        return;
    }
    duty.publishes_block.store(true, Ordering::Relaxed);
    let started = std::time::Instant::now();
    while handshakes.load(Ordering::Relaxed) < 6 {
        let seen = handshakes.load(Ordering::Relaxed);
        let elapsed = started.elapsed();
        if seen >= 2 && elapsed >= Duration::from_secs(8) {
            break;
        }
        if elapsed >= Duration::from_secs(15) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    let block = proposal_block(proposer, 0, true, duty.ctx.expected_chain_id);
    let hash = match block.block_hash() {
        Ok(hash) => encode_hex(&hash.0 .0),
        Err(err) => {
            say(&format!("publish-failed {err}"));
            return;
        }
    };
    let bytes = match encode_block(&block) {
        Ok(bytes) => bytes,
        Err(err) => {
            say(&format!("publish-failed {err}"));
            return;
        }
    };
    publish_bytes(
        &handle,
        GossipTopic::Blocks,
        bytes,
        &format!("published {hash}"),
    );
    {
        let Ok(mut engine) = duty.engine.lock() else {
            return;
        };
        if engine
            .receive_proposal(&block, Some(&context(&duty)))
            .is_ok()
        {
            if let Ok(mut kept) = duty.proposal.lock() {
                *kept = Some(block);
            }
            say(&format!("accepted {hash}"));
            say_state(&engine);
        }
    }
    note_proposal(&duty, &handle);
    tokio::time::sleep(Duration::from_secs(1)).await;
    if duty.index.unwrap_or(9) < 3 {
        cast_vote(&duty, &handle, VotePhase::Prepare);
    }
}

fn cast_vote(duty: &Arc<Duty>, handle: &NetworkHandle, phase: VotePhase) {
    let Some(key) = duty.key.as_ref() else {
        return;
    };
    let vote = {
        let Ok(mut engine) = duty.engine.lock() else {
            return;
        };
        match engine.create_vote(phase, key) {
            Ok(vote) => vote,
            Err(err) => {
                say(&format!("publish-failed {err}"));
                return;
            }
        }
    };
    let Some(record) = duty
        .validators
        .validators
        .iter()
        .find(|record| record.id == vote.validator)
    else {
        return;
    };
    if let Err(err) = verify_validator_vote(record, &vote) {
        say(&format!("publish-failed {err}"));
        return;
    }
    let id = encode_hex(&vote.validator.0);
    let bytes = encode_vote_frame(&vote);
    apply_vote(duty, handle, vote);
    publish_bytes(
        handle,
        GossipTopic::Consensus,
        bytes,
        &format!("vote {} {id}", vote_phase_name(phase)),
    );
}

fn handle_consensus(duty: &Arc<Duty>, handle: &NetworkHandle, data: Vec<u8>, message_id: Vec<u8>) {
    if !duty.ready.load(Ordering::Relaxed) {
        if let Ok(mut pending) = duty.pending.lock() {
            pending.push(data);
        }
        let _ = handle.report_gossip_validation(message_id, GossipAcceptance::Accept);
        return;
    }
    apply_consensus(duty, handle, &data, Some(message_id));
}

fn flush_consensus(duty: &Arc<Duty>, handle: &NetworkHandle) {
    let batch = match duty.pending.lock() {
        Ok(mut pending) => std::mem::take(&mut *pending),
        Err(_) => return,
    };
    for data in batch {
        apply_consensus(duty, handle, &data, None);
    }
}

fn apply_consensus(
    duty: &Arc<Duty>,
    handle: &NetworkHandle,
    data: &[u8],
    message_id: Option<Vec<u8>>,
) {
    if let Ok(vote) = decode_vote_frame(data) {
        if vote_parked(duty, &vote) {
            if let Ok(mut pending) = duty.pending.lock() {
                pending.push(data.to_vec());
            }
            report_optional(handle, message_id, GossipAcceptance::Accept);
            return;
        }
        let acceptance = apply_vote(duty, handle, vote);
        report_optional(handle, message_id, acceptance);
        return;
    }
    if let Ok(qc) = decode_qc_frame(data) {
        let acceptance = apply_qc(&qc, &duty.committee);
        report_optional(handle, message_id, acceptance);
        return;
    }
    say("consensus-rejected invalid encoding");
    report_optional(handle, message_id, GossipAcceptance::Reject);
}

fn vote_parked(duty: &Duty, vote: &Vote) -> bool {
    match vote.phase {
        VotePhase::Prepare => false,
        VotePhase::PreCommit => !duty.precommit_open.load(Ordering::Relaxed),
        VotePhase::Commit => !duty.commit_open.load(Ordering::Relaxed),
    }
}

fn apply_vote(duty: &Arc<Duty>, handle: &NetworkHandle, vote: Vote) -> GossipAcceptance {
    if let Some(record) = duty
        .validators
        .validators
        .iter()
        .find(|record| record.id == vote.validator)
    {
        let _ = verify_validator_vote(record, &vote);
    }
    let event = {
        let Ok(mut engine) = duty.engine.lock() else {
            return GossipAcceptance::Reject;
        };
        engine.receive_vote(vote.clone())
    };
    match event {
        Ok(ConsensusEvent::ProposalAccepted) => {
            say(&format!(
                "vote-duplicate {} {}",
                vote_phase_name(vote.phase),
                encode_hex(&vote.validator.0)
            ));
            GossipAcceptance::Accept
        }
        Ok(ConsensusEvent::VoteCreated(_)) => {
            say(&format!(
                "vote-ok {} {}",
                vote_phase_name(vote.phase),
                encode_hex(&vote.validator.0)
            ));
            let phase = vote.phase;
            remember(duty, vote);
            report_short(duty, phase);
            GossipAcceptance::Accept
        }
        Ok(ConsensusEvent::QuorumReached(phase)) => {
            say(&format!(
                "vote-ok {} {}",
                vote_phase_name(vote.phase),
                encode_hex(&vote.validator.0)
            ));
            remember(duty, vote);
            announce_quorum(duty, handle, phase);
            GossipAcceptance::Accept
        }
        Ok(ConsensusEvent::BlockFinalized(proof)) => {
            say(&format!(
                "vote-ok {} {}",
                vote_phase_name(vote.phase),
                encode_hex(&vote.validator.0)
            ));
            remember(duty, vote);
            announce_quorum(duty, handle, VotePhase::Commit);
            if verify_commit_chain(&proof, &duty.committee).is_ok() {
                let hash = encode_hex(&proof.block_hash.0 .0);
                say(&format!("finalized {hash}"));
                say(&format!(
                    "proof {hash} {} {} {} {} {} {}",
                    proof.prepare.epoch,
                    proof.height,
                    proof.view,
                    proof.prepare.voting_power,
                    proof.precommit.voting_power,
                    proof.commit.voting_power
                ));
                for (name, qc) in [
                    ("Prepare", &proof.prepare),
                    ("PreCommit", &proof.precommit),
                    ("Commit", &proof.commit),
                ] {
                    let voters = qc
                        .voters
                        .iter()
                        .map(|(id, _)| encode_hex(&id.0))
                        .collect::<Vec<_>>()
                        .join(" ");
                    say(&format!("proof-voters {name} {voters} {}", qc.voting_power));
                }
                commit_finalized(duty, handle);
            }
            if let Ok(engine) = duty.engine.lock() {
                say_state(&engine);
            }
            GossipAcceptance::Accept
        }
        Ok(ConsensusEvent::Equivocation(_)) => {
            say("vote-rejected equivocation");
            GossipAcceptance::Reject
        }
        Ok(_) => GossipAcceptance::Accept,
        Err(err) => {
            say(&format!("vote-rejected {err}"));
            GossipAcceptance::Reject
        }
    }
}

fn remember(duty: &Duty, vote: Vote) {
    if let Ok(mut seen) = duty.seen.lock() {
        if !seen
            .iter()
            .any(|kept| kept.validator == vote.validator && kept.phase == vote.phase)
        {
            seen.push(vote);
        }
    }
}

fn announce_quorum(duty: &Arc<Duty>, handle: &NetworkHandle, phase: VotePhase) {
    let seen = match duty.seen.lock() {
        Ok(seen) => seen.clone(),
        Err(_) => return,
    };
    let Ok((Some(qc), _)) = aggregate_qc(&seen, &duty.committee, phase) else {
        return;
    };
    if verify_qc(&qc, &duty.committee).is_err() {
        return;
    }
    let power = qc.voting_power;
    let total = duty.committee.total_voting_power;
    if has_quorum(power, total).ok() != Some(true) {
        return;
    }
    say(&format!(
        "quorum {} {power} {total} {} {}",
        vote_phase_name(phase),
        power.saturating_mul(3),
        total.saturating_mul(2)
    ));
    let voters = qc
        .voters
        .iter()
        .map(|(id, _)| encode_hex(&id.0))
        .collect::<Vec<_>>()
        .join(" ");
    say(&format!("voters {} {voters}", vote_phase_name(phase)));
    say(&format!("qc-ok {} {power}", vote_phase_name(phase)));
    if !mark_once(duty, phase) {
        return;
    }
    if duty.index == Some(0) {
        let bytes = encode_qc_frame(&qc);
        apply_qc(&qc, &duty.committee);
        publish_bytes(
            handle,
            GossipTopic::Consensus,
            bytes,
            &format!("qc {}", vote_phase_name(phase)),
        );
    }
    schedule_next(duty, handle, phase);
}

fn schedule_next(duty: &Arc<Duty>, handle: &NetworkHandle, phase: VotePhase) {
    if phase == VotePhase::Prepare && duty.index == Some(3) {
        let duty = Arc::clone(duty);
        let handle = handle.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            publish_negatives(&duty, &handle);
        });
    }
    if phase == VotePhase::Prepare && duty.index == Some(0) {
        let duty = Arc::clone(duty);
        let handle = handle.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            publish_equivocation(&duty, &handle);
        });
    }
    let wait = match phase {
        VotePhase::Prepare => Duration::from_secs(3),
        VotePhase::PreCommit => Duration::from_secs(1),
        VotePhase::Commit => return,
    };
    let next = match phase {
        VotePhase::Prepare => VotePhase::PreCommit,
        VotePhase::PreCommit => VotePhase::Commit,
        VotePhase::Commit => return,
    };
    let duty = Arc::clone(duty);
    let handle = handle.clone();
    tokio::spawn(async move {
        tokio::time::sleep(wait).await;
        match next {
            VotePhase::PreCommit => duty.precommit_open.store(true, Ordering::Relaxed),
            VotePhase::Commit => duty.commit_open.store(true, Ordering::Relaxed),
            VotePhase::Prepare => {}
        }
        flush_consensus(&duty, &handle);
        if duty.casts && duty.index.unwrap_or(9) < 3 {
            cast_vote(&duty, &handle, next);
        }
    });
}

fn publish_negatives(duty: &Arc<Duty>, handle: &NetworkHandle) {
    let seen = match duty.seen.lock() {
        Ok(seen) => seen.clone(),
        Err(_) => return,
    };
    let Some(base) = seen
        .iter()
        .find(|vote| vote.phase == VotePhase::Prepare)
        .cloned()
    else {
        return;
    };
    let mut bad_sig = base.clone();
    bad_sig.signature.0[0] ^= 0xff;
    publish_frame(duty, handle, &encode_vote_frame(&bad_sig));
    if let Ok(key) = PrivateKey::from_bytes(&[0x55; 32]) {
        let mut outsider = base.clone();
        outsider.validator = AccountId::from(rsc_state::derive_account_id(&key.key_id()));
        outsider.signature = Signature([0; 64]);
        if sign_vote(&mut outsider, &key).is_ok() {
            publish_frame(duty, handle, &encode_vote_frame(&outsider));
        }
    }
    for (epoch, height, view) in [
        (base.epoch.wrapping_add(1), base.height, base.view),
        (base.epoch, base.height.wrapping_add(1), base.view),
        (base.epoch, base.height, base.view.wrapping_add(1)),
    ] {
        let mut vote = base.clone();
        vote.epoch = epoch;
        vote.height = height;
        vote.view = view;
        publish_frame(duty, handle, &encode_vote_frame(&vote));
    }
    let mut bad_hash = base.clone();
    bad_hash.block_hash.0 .0[0] ^= 0xff;
    publish_frame(duty, handle, &encode_vote_frame(&bad_hash));
    if let Some(author) = duty
        .validators
        .validators
        .iter()
        .find(|record| record.id != base.validator)
    {
        let mut other_author = base.clone();
        other_author.validator = author.id;
        publish_frame(duty, handle, &encode_vote_frame(&other_author));
    }
    publish_frame(duty, handle, &encode_vote_frame(&base));
    publish_frame(duty, handle, &[0xab; 7]);
    if let Ok((Some(qc), _)) = aggregate_qc(&seen, &duty.committee, VotePhase::Prepare) {
        let mut corrupt = encode_qc_frame(&qc);
        if let Some(last) = corrupt.last_mut() {
            *last ^= 0xff;
        }
        publish_frame(duty, handle, &corrupt);
        let mut other_hash = encode_qc_frame(&qc);
        if other_hash.len() > 4 {
            other_hash[4] ^= 0xff;
        }
        publish_frame(duty, handle, &other_hash);
    }
}

fn report_short(duty: &Duty, phase: VotePhase) {
    let seen = match duty.seen.lock() {
        Ok(seen) => seen.clone(),
        Err(_) => return,
    };
    let Ok((None, _)) = aggregate_qc(&seen, &duty.committee, phase) else {
        return;
    };
    let Some(sample) = seen.iter().find(|vote| vote.phase == phase) else {
        return;
    };
    let mut power = 0u128;
    for vote in seen.iter().filter(|vote| {
        vote.phase == phase
            && vote.block_hash == sample.block_hash
            && vote.epoch == sample.epoch
            && vote.height == sample.height
            && vote.view == sample.view
    }) {
        if let Some(record) = duty
            .validators
            .validators
            .iter()
            .find(|record| record.id == vote.validator)
        {
            power = power.saturating_add(record.voting_power);
        }
    }
    let total = duty.committee.total_voting_power;
    if has_quorum(power, total).ok() == Some(false) && power > 0 {
        say(&format!(
            "no-quorum {} {power} {total} {} {}",
            vote_phase_name(phase),
            power.saturating_mul(3),
            total.saturating_mul(2)
        ));
    }
}

fn publish_equivocation(duty: &Arc<Duty>, handle: &NetworkHandle) {
    let Some(key) = duty.key.as_ref() else {
        return;
    };
    let seen = match duty.seen.lock() {
        Ok(seen) => seen.clone(),
        Err(_) => return,
    };
    let local = AccountId::from(rsc_state::derive_account_id(&key.key_id()));
    let Some(mut other) = seen
        .iter()
        .find(|vote| vote.phase == VotePhase::Prepare && vote.validator == local)
        .cloned()
    else {
        return;
    };
    other.block_hash.0 .0[0] ^= 0xff;
    other.signature = Signature([0; 64]);
    if sign_vote(&mut other, key).is_err() {
        return;
    }
    let bytes = encode_vote_frame(&other);
    apply_vote(duty, handle, other);
    publish_bytes(handle, GossipTopic::Consensus, bytes, "published-equivocation");
}

fn publish_frame(duty: &Arc<Duty>, handle: &NetworkHandle, bytes: &[u8]) {
    apply_consensus(duty, handle, bytes, None);
    publish_bytes(
        handle,
        GossipTopic::Consensus,
        bytes.to_vec(),
        "published-invalid consensus",
    );
}

fn apply_qc(qc: &QuorumCertificate, committee: &Committee) -> GossipAcceptance {
    match verify_qc(qc, committee) {
        Ok(()) => {
            say(&format!(
                "qc-ok {} {}",
                vote_phase_name(qc.phase),
                qc.voting_power
            ));
            GossipAcceptance::Accept
        }
        Err(err) => {
            say(&format!("qc-rejected {err}"));
            GossipAcceptance::Reject
        }
    }
}

fn mark_once(duty: &Duty, phase: VotePhase) -> bool {
    let Ok(mut once) = duty.once.lock() else {
        return false;
    };
    let index = match phase {
        VotePhase::Prepare => 0,
        VotePhase::PreCommit => 1,
        VotePhase::Commit => 2,
    };
    if once[index] {
        return false;
    }
    once[index] = true;
    true
}

fn report_optional(handle: &NetworkHandle, message_id: Option<Vec<u8>>, acceptance: GossipAcceptance) {
    if let Some(message_id) = message_id {
        let _ = handle.report_gossip_validation(message_id, acceptance);
    }
}

fn encode_vote_frame(vote: &Vote) -> Vec<u8> {
    let mut out = Vec::with_capacity(32 + 64 + 57);
    out.extend_from_slice(&vote.validator.0);
    out.extend_from_slice(&vote.signature.0);
    out.extend_from_slice(&vote_bytes(vote));
    out
}

fn decode_vote_frame(bytes: &[u8]) -> Result<Vote, ()> {
    if bytes.len() != 32 + 64 + 57 {
        return Err(());
    }
    let validator = AccountId(bytes[0..32].try_into().map_err(|_| ())?);
    let signature = Signature(bytes[32..96].try_into().map_err(|_| ())?);
    let message = &bytes[96..];
    let phase = match message[0] {
        0 => VotePhase::Prepare,
        1 => VotePhase::PreCommit,
        2 => VotePhase::Commit,
        _ => return Err(()),
    };
    let epoch = u64::from_le_bytes(message[1..9].try_into().map_err(|_| ())?);
    let height = u64::from_le_bytes(message[9..17].try_into().map_err(|_| ())?);
    let view = u64::from_le_bytes(message[17..25].try_into().map_err(|_| ())?);
    let block_hash = rsc_core::BlockHash(Hash(message[25..57].try_into().map_err(|_| ())?));
    let vote = Vote {
        validator,
        epoch,
        height,
        view,
        block_hash,
        phase,
        signature,
    };
    if vote_bytes(&vote) != message {
        return Err(());
    }
    Ok(vote)
}

fn encode_qc_frame(qc: &QuorumCertificate) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&qc.block_hash.0 .0);
    body.extend_from_slice(&qc.height.to_le_bytes());
    body.extend_from_slice(&qc.view.to_le_bytes());
    body.extend_from_slice(&qc.epoch.to_le_bytes());
    body.push(match qc.phase {
        VotePhase::Prepare => 0,
        VotePhase::PreCommit => 1,
        VotePhase::Commit => 2,
    });
    body.extend_from_slice(&qc.voting_power.to_le_bytes());
    let Ok(voters) = u32::try_from(qc.voters.len()) else {
        return Vec::new();
    };
    body.extend_from_slice(&voters.to_le_bytes());
    for (id, power) in &qc.voters {
        body.extend_from_slice(&id.0);
        body.extend_from_slice(&power.to_le_bytes());
    }
    let signatures = qc.signatures.to_bytes();
    let Ok(sig_len) = u32::try_from(signatures.len()) else {
        return Vec::new();
    };
    body.extend_from_slice(&sig_len.to_le_bytes());
    body.extend_from_slice(&signatures);
    let Ok(len) = u32::try_from(body.len()) else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&body);
    out
}

fn decode_qc_frame(bytes: &[u8]) -> Result<QuorumCertificate, ()> {
    if bytes.len() < 4 {
        return Err(());
    }
    let len = u32::from_le_bytes(bytes[0..4].try_into().map_err(|_| ())?) as usize;
    if bytes.len() != 4 + len {
        return Err(());
    }
    let mut body = &bytes[4..];
    let block_hash = rsc_core::BlockHash(Hash(read_array(&mut body)?));
    let height = u64::from_le_bytes(read_array(&mut body)?);
    let view = u64::from_le_bytes(read_array(&mut body)?);
    let epoch = u64::from_le_bytes(read_array(&mut body)?);
    let phase = match read_array::<1>(&mut body)?[0] {
        0 => VotePhase::Prepare,
        1 => VotePhase::PreCommit,
        2 => VotePhase::Commit,
        _ => return Err(()),
    };
    let voting_power = u128::from_le_bytes(read_array(&mut body)?);
    let voters_n = u32::from_le_bytes(read_array(&mut body)?) as usize;
    let mut voters = Vec::with_capacity(voters_n);
    for _ in 0..voters_n {
        let id = AccountId(read_array(&mut body)?);
        let power = u128::from_le_bytes(read_array(&mut body)?);
        voters.push((id, power));
    }
    let sig_len = u32::from_le_bytes(read_array(&mut body)?) as usize;
    if body.len() != sig_len {
        return Err(());
    }
    let signatures = rsc_crypto::SignatureBundle::from_bytes(body).map_err(|_| ())?;
    Ok(QuorumCertificate {
        block_hash,
        height,
        view,
        phase,
        epoch,
        voters,
        voting_power,
        signatures,
    })
}

fn read_array<const N: usize>(bytes: &mut &[u8]) -> Result<[u8; N], ()> {
    if bytes.len() < N {
        return Err(());
    }
    let (head, tail) = bytes.split_at(N);
    *bytes = tail;
    head.try_into().map_err(|_| ())
}

fn vote_phase_name(phase: VotePhase) -> &'static str {
    match phase {
        VotePhase::Prepare => "Prepare",
        VotePhase::PreCommit => "PreCommit",
        VotePhase::Commit => "Commit",
    }
}

fn say_state(engine: &ConsensusEngine) {
    let state = engine.state();
    let proposal = state
        .proposal
        .map(|hash| encode_hex(&hash.0 .0))
        .unwrap_or_else(|| "none".into());
    say(&format!(
        "state height {} epoch {} view {} phase {} proposal {proposal}",
        state.height,
        state.epoch,
        state.view,
        phase_name(state.phase)
    ));
}

fn phase_name(phase: ConsensusPhase) -> &'static str {
    match phase {
        ConsensusPhase::NewHeight => "NewHeight",
        ConsensusPhase::NewView => "NewView",
        ConsensusPhase::Propose => "Propose",
        ConsensusPhase::Prepare => "Prepare",
        ConsensusPhase::PreCommit => "PreCommit",
        ConsensusPhase::Commit => "Commit",
        ConsensusPhase::Decide => "Decide",
        ConsensusPhase::Timeout => "Timeout",
    }
}

fn chain_tip(live: Option<&LiveChain>) -> (u64, String) {
    let Some(live) = live else {
        return (0, encode_hex(&[0u8; 32]));
    };
    let Ok(Some(hash)) = live.store.get_canonical_tip() else {
        return (0, encode_hex(&[0u8; 32]));
    };
    let Ok(Some(block)) = live.store.get_block(&hash) else {
        return (0, encode_hex(&hash.0 .0));
    };
    (block.header.height.0, encode_hex(&hash.0 .0))
}

struct ProduceOpts {
    index: u8,
    casts: bool,
    goal: u8,
    pull: bool,
    fault: bool,
    audit: bool,
}

fn open_produce(config: &Config, opts: ProduceOpts) -> Result<(Arc<LiveChain>, Arc<Duty>), String> {
    let (genesis, mut keys) = vote_genesis(config.chain_id)?;
    let database = Arc::new(
        RscDatabase::open(
            &config.data_dir,
            DatabaseConfig {
                max_value_size: 8 * 1024 * 1024,
                network_id: config.network_id.clone(),
                chain_id: config.chain_id.to_string(),
            },
        )
        .map_err(|err| err.to_string())?,
    );
    let state = Arc::new(State::open(Arc::clone(&database)).map_err(|err| err.to_string())?);
    let store = RocksDbChainStore::from_shared(database);
    let parent = install_genesis(&state, &store, &genesis)?;
    let tip = store
        .get_canonical_tip()
        .map_err(|err| err.to_string())?
        .ok_or("falta tip")?;
    say(&format!(
        "tip {} {}",
        encode_hex(&tip.0 .0),
        parent.height.0
    ));
    let validators = genesis.validator_set().map_err(|err| err.to_string())?;
    let consensus = genesis
        .parameters
        .consensus_config()
        .map_err(|err| err.to_string())?;
    let index = opts.index;
    let local = validators.validators[usize::from(index)].id;
    let key = keys.remove(usize::from(index));
    let seed = genesis.parameters.consensus_randomness_seed;
    let resumed = parent.height.0;
    let (engine_height, randomness) = if opts.goal == 0 || resumed == 0 {
        (if opts.goal == 0 { 0 } else { 1 }, seed)
    } else {
        (resumed.saturating_add(1), Hash::ZERO)
    };
    let engine = ConsensusEngine::new(
        consensus,
        validators.clone(),
        randomness,
        local,
        engine_height,
    )
    .map_err(|err| err.to_string())?;
    let committee = engine.state().committee.clone();
    let proposer = engine.proposer().map_err(|err| err.to_string())?;
    let power = validators.validators[usize::from(index)].voting_power;
    say(&format!(
        "validator {index} {} {power}",
        encode_hex(&local.0)
    ));
    say(&format!("proposer {}", encode_hex(&proposer.0)));
    if opts.pull {
        say(&format!("sync-protocol {BLOCK_SYNC_PROTOCOL}"));
    }
    let chain = Arc::new(LiveChain {
        state,
        store,
        keys: genesis
            .validators
            .iter()
            .map(|validator| (validator.account_id, validator.owner_public_key))
            .collect(),
        pool: Mutex::new(None),
        committed: AtomicBool::new(false),
    });
    let duty = Duty {
        engine: Mutex::new(engine),
        ctx: ChainValidationContext {
            expected_chain_id: genesis.chain_id,
            expected_protocol_version: genesis.version,
            max_block_size: genesis.parameters.max_block_size as usize,
            max_transactions_per_block: genesis.parameters.max_transactions_per_block as usize,
            parent: Some(parent),
        },
        key: Some(key),
        index: Some(index),
        validators,
        committee,
        pending: Mutex::new(Vec::new()),
        ready: AtomicBool::new(false),
        publishes_block: AtomicBool::new(false),
        precommit_open: AtomicBool::new(false),
        commit_open: AtomicBool::new(false),
        seen: Mutex::new(Vec::new()),
        once: Mutex::new([false; 3]),
        casts: opts.casts,
        goal: opts.goal,
        pull: opts.pull,
        fault: opts.fault,
        audit: opts.audit,
        sync_started: AtomicBool::new(false),
        sync_next: AtomicU64::new(1),
        parent: Mutex::new(None),
        proposal: Mutex::new(None),
        chain: Some(Arc::clone(&chain)),
    };
    Ok((chain, Arc::new(duty)))
}

fn install_genesis(
    state: &State,
    store: &RocksDbChainStore,
    genesis: &Genesis,
) -> Result<BlockHeader, String> {
    if store
        .get_canonical_tip()
        .map_err(|err| err.to_string())?
        .is_some()
    {
        let hash = store
            .get_canonical_tip()
            .map_err(|err| err.to_string())?
            .ok_or("falta tip")?;
        let block = store
            .get_block(&hash)
            .map_err(|err| err.to_string())?
            .ok_or("bloque ausente")?;
        let encoded = encode_block(&block).map_err(|err| err.to_string())?;
        say(&format!(
            "recovered {} {}",
            block.header.height.0,
            encode_hex(&hash.0 .0)
        ));
        say(&format!(
            "stored {} {}",
            block.header.height.0,
            encode_hex(&encoded)
        ));
        let root = state.state_root().map_err(|err| err.to_string())?;
        let ledger = ledger_of(store)?;
        say(&format!(
            "roots {} {}",
            encode_hex(root.as_bytes()),
            encode_hex(ledger.root().0.as_bytes())
        ));
        return Ok(block.header);
    }
    let mut accounts = Vec::with_capacity(genesis.initial_accounts.len());
    for account in &genesis.initial_accounts {
        let staged = StateGenesisAccount::new(account.owner_key_id, account.native_balance);
        let derived = rsc_core::AccountId::from(staged.account_id());
        if derived != account.account_id {
            return Err("genesis account id distinto".into());
        }
        accounts.push(staged);
    }
    state
        .initialize_genesis(&accounts, u64::from(genesis.version))
        .map_err(|err| err.to_string())?;
    let root = state.state_root().map_err(|err| err.to_string())?;
    let mut block = Block {
        header: BlockHeader {
            version: genesis.version,
            chain_id: genesis.chain_id,
            height: BlockHeight::GENESIS,
            previous_block_hash: BlockHash::ZERO,
            timestamp: Timestamp::from_secs(genesis.timestamp),
            transactions_root: Hash::ZERO,
            state_root: Hash(*root.as_bytes()),
            proposer: AccountId::ZERO,
        },
        transactions: Vec::new(),
    };
    block.header.transactions_root = block.compute_transaction_root();
    let limits = genesis.parameters;
    validate_block(
        &block,
        &ChainValidationContext {
            expected_chain_id: genesis.chain_id,
            expected_protocol_version: genesis.version,
            max_block_size: limits.max_block_size as usize,
            max_transactions_per_block: limits.max_transactions_per_block as usize,
            parent: None,
        },
    )
    .map_err(|err| err.to_string())?;
    store.put_block(&block).map_err(|err| err.to_string())?;
    let hash = block.block_hash().map_err(|err| err.to_string())?;
    store
        .set_canonical_tip(&hash)
        .map_err(|err| err.to_string())?;
    say(&format!("installed-genesis {}", encode_hex(&hash.0 .0)));
    Ok(block.header)
}

struct ChainAdmission {
    state: Arc<State>,
    keys: Vec<(AccountId, PublicKey)>,
}

fn open_chain_pool(live: &LiveChain) -> Result<Arc<Mempool>, String> {
    let admission = Arc::new(ChainAdmission {
        state: Arc::clone(&live.state),
        keys: live.keys.clone(),
    });
    let pool = Arc::new(Mempool::new(
        MempoolConfig::default(),
        Arc::clone(&admission) as Arc<dyn MempoolValidator>,
        admission,
    ));
    *live.pool.lock().map_err(|_| "pool".to_string())? = Some(Arc::clone(&pool));
    Ok(pool)
}

impl MempoolValidator for ChainAdmission {
    fn validate(
        &self,
        raw_transaction: &[u8],
    ) -> Result<MempoolTransaction, MempoolValidationError> {
        let transaction = Transaction::from_canonical_bytes(raw_transaction).map_err(map_tx)?;
        if transaction.version != rsc_ledger::transaction::TRANSACTION_VERSION_V1 {
            return Err(MempoolValidationError::ValidationFailed);
        }
        let TransactionPayload::Transfer { amount, .. } = transaction.payload else {
            return Err(MempoolValidationError::ValidationFailed);
        };
        if amount == 0 {
            return Err(MempoolValidationError::ValidationFailed);
        }
        let Some((_, key)) = self
            .keys
            .iter()
            .find(|(id, _)| *id == transaction.sender)
        else {
            return Err(MempoolValidationError::ValidationFailed);
        };
        transaction.validate(key).map_err(map_tx)?;
        match self
            .state
            .get_account(&rsc_state::AccountId::from(transaction.sender))
        {
            Ok(Some(_)) => {}
            Ok(None) => return Err(MempoolValidationError::ValidationFailed),
            Err(_) => return Err(MempoolValidationError::ValidationUnavailable),
        }
        let id = transaction.id();
        Ok(MempoolTransaction {
            hash: rsc_core::TransactionId(Hash(*id.0.as_bytes())),
            sender: transaction.sender,
            nonce: Nonce(transaction.nonce),
            raw_transaction: raw_transaction.to_vec(),
            received_at: std::time::Instant::now(),
        })
    }
}

impl PendingNonceProvider for ChainAdmission {
    fn current_nonce(&self, sender: &AccountId) -> Result<Nonce, NonceLookupError> {
        match self
            .state
            .get_account(&rsc_state::AccountId::from(*sender))
        {
            Ok(Some(account)) => Ok(Nonce(account.nonce())),
            Ok(None) | Err(_) => Err(NonceLookupError::Unavailable),
        }
    }
}

async fn watch_ready(duty: Arc<Duty>, handle: NetworkHandle, handshakes: Arc<AtomicUsize>) {
    if duty.goal == 0 || duty.pull || duty.fault {
        return;
    }
    let (proposer, local) = {
        let Ok(engine) = duty.engine.lock() else {
            return;
        };
        let Ok(proposer) = engine.proposer() else {
            return;
        };
        let Some(key) = duty.key.as_ref() else {
            return;
        };
        let local = AccountId::from(rsc_state::derive_account_id(&key.key_id()));
        (proposer, local)
    };
    if proposer != local {
        return;
    }
    duty.publishes_block.store(true, Ordering::Relaxed);
    let started = std::time::Instant::now();
    while handshakes.load(Ordering::Relaxed) < 6 {
        let seen = handshakes.load(Ordering::Relaxed);
        let elapsed = started.elapsed();
        if seen >= 2 && elapsed >= Duration::from_secs(8) {
            break;
        }
        if elapsed >= Duration::from_secs(15) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    let block = loop {
        if duty
            .chain
            .as_ref()
            .is_some_and(|chain| chain.committed.load(Ordering::Relaxed))
        {
            return;
        }
        match block_from_ready(&duty) {
            Ok(Some(block)) => break block,
            Ok(None) => tokio::time::sleep(Duration::from_millis(50)).await,
            Err(err) => {
                say(&format!("publish-failed {err}"));
                return;
            }
        }
    };
    let hash = match block.block_hash() {
        Ok(hash) => encode_hex(&hash.0 .0),
        Err(err) => {
            say(&format!("publish-failed {err}"));
            return;
        }
    };
    let bytes = match encode_block(&block) {
        Ok(bytes) => bytes,
        Err(err) => {
            say(&format!("publish-failed {err}"));
            return;
        }
    };
    publish_bytes(
        &handle,
        GossipTopic::Blocks,
        bytes,
        &format!(
            "block {hash} {} {} {} {}",
            encode_hex(&block.header.previous_block_hash.0 .0),
            block.header.height.0,
            encode_hex(&block.header.transactions_root.0),
            encode_hex(&block.header.state_root.0)
        ),
    );
    {
        let Ok(mut engine) = duty.engine.lock() else {
            return;
        };
        if engine
            .receive_proposal(&block, Some(&context(&duty)))
            .is_ok()
        {
            if let Ok(mut kept) = duty.proposal.lock() {
                *kept = Some(block);
            }
            say(&format!("accepted {hash}"));
            say_state(&engine);
        }
    }
    note_proposal(&duty, &handle);
    tokio::time::sleep(Duration::from_secs(1)).await;
    if duty.casts && duty.index.unwrap_or(9) < 3 {
        cast_vote(&duty, &handle, VotePhase::Prepare);
    }
}

fn block_from_ready(duty: &Duty) -> Result<Option<Block>, String> {
    let Some(chain) = duty.chain.as_ref() else {
        return Ok(None);
    };
    let pool = chain
        .pool
        .lock()
        .map_err(|_| "pool".to_string())?
        .clone()
        .ok_or("falta mempool")?;
    let Some(pending) = pool.ready_transactions(1).into_iter().next() else {
        return Ok(None);
    };
    let transaction = Transaction::from_canonical_bytes(&pending.raw_transaction)
        .map_err(|err| err.to_string())?;
    let id = transaction.id();
    if pending.hash != rsc_core::TransactionId(Hash(*id.0.as_bytes())) {
        return Err("hash distinto".into());
    }
    let TransactionPayload::Transfer { recipient, amount } = transaction.payload else {
        return Err("payload distinto".into());
    };
    let root = chain.state.state_root().map_err(|err| err.to_string())?;
    let version = chain.state.version().map_err(|err| err.to_string())?;
    let mut transition = StateTransition::new(root, version);
    transition.push(StateOperation::Transfer {
        from: transaction.sender.into(),
        to: recipient.into(),
        amount,
        expected_nonce: transaction.nonce,
    });
    let next = version.checked_add(1).ok_or("versión de estado agotada")?;
    let mut batch = StorageBatch::new();
    let commitment = chain
        .state
        .stage_transition(&mut batch, &transition, next)
        .map_err(|err| err.to_string())?;
    drop(batch);
    let tip = chain
        .store
        .get_canonical_tip()
        .map_err(|err| err.to_string())?
        .ok_or("falta tip")?;
    let parent = chain
        .store
        .get_block(&tip)
        .map_err(|err| err.to_string())?
        .ok_or("bloque ausente")?;
    let proposer = duty
        .engine
        .lock()
        .map_err(|_| "consenso".to_string())?
        .proposer()
        .map_err(|err| err.to_string())?;
    let mut block = Block {
        header: BlockHeader {
            version: parent.header.version,
            chain_id: parent.header.chain_id,
            height: parent
                .header
                .height
                .checked_next()
                .map_err(|err| err.to_string())?,
            previous_block_hash: tip,
            timestamp: parent.header.timestamp,
            transactions_root: Hash::ZERO,
            state_root: Hash(*commitment.new_root().as_bytes()),
            proposer,
        },
        transactions: vec![RawTransaction(pending.raw_transaction)],
    };
    block.header.transactions_root = block.compute_transaction_root();
    let encoded = encode_block(&block).map_err(|err| err.to_string())?;
    validate_block(
        &block,
        &ChainValidationContext {
            expected_chain_id: parent.header.chain_id,
            expected_protocol_version: parent.header.version,
            max_block_size: encoded.len(),
            max_transactions_per_block: block.transaction_count(),
            parent: Some(parent.header),
        },
    )
    .map_err(|err| err.to_string())?;
    Ok(Some(block))
}

fn commit_finalized(duty: &Arc<Duty>, handle: &NetworkHandle) {
    let Some(chain) = duty.chain.as_ref() else {
        return;
    };
    if chain.committed.swap(true, Ordering::Relaxed) {
        return;
    }
    let block = match duty.proposal.lock() {
        Ok(kept) => kept.clone(),
        Err(_) => None,
    };
    let Some(block) = block else {
        chain.committed.store(false, Ordering::Relaxed);
        say("commit-failed bloque ausente");
        return;
    };
    if let Err(err) = write_committed(chain, &block) {
        chain.committed.store(false, Ordering::Relaxed);
        say(&format!("commit-failed {err}"));
        return;
    }
    if block.header.height.0 < u64::from(duty.goal) {
        advance_round(duty, handle, &block);
    }
}

fn advance_round(duty: &Arc<Duty>, handle: &NetworkHandle, block: &Block) {
    {
        let Ok(mut engine) = duty.engine.lock() else {
            return;
        };
        if let Err(err) = engine.new_height(None, Hash::ZERO) {
            say(&format!("advance-failed {err}"));
            return;
        }
        if let Ok(proposer) = engine.proposer() {
            say(&format!("proposer {}", encode_hex(&proposer.0)));
        }
        say_state(&engine);
    }
    if let Ok(mut seen) = duty.seen.lock() {
        seen.clear();
    }
    if let Ok(mut once) = duty.once.lock() {
        *once = [false; 3];
    }
    duty.precommit_open.store(false, Ordering::Relaxed);
    duty.commit_open.store(false, Ordering::Relaxed);
    duty.ready.store(false, Ordering::Relaxed);
    duty.publishes_block.store(false, Ordering::Relaxed);
    if let Some(chain) = duty.chain.as_ref() {
        chain.committed.store(false, Ordering::Relaxed);
    }
    if let Ok(mut kept) = duty.proposal.lock() {
        *kept = None;
    }
    if let Ok(mut parent) = duty.parent.lock() {
        *parent = Some(block.header.clone());
    }
    let duty = Arc::clone(duty);
    let handle = handle.clone();
    tokio::spawn(async move {
        watch_ready(duty, handle, Arc::new(AtomicUsize::new(6))).await;
    });
}

fn sync_start(duty: &Duty) -> u64 {
    let height = canonical_header(duty)
        .ok()
        .map(|(_, header)| header.height.0)
        .unwrap_or(0);
    if duty.audit && height == 0 {
        1
    } else {
        height.saturating_add(1)
    }
}

fn request_height(duty: &Duty, handle: &NetworkHandle, peer: PeerId, height: u64) {
    duty.sync_next.store(height, Ordering::Relaxed);
    say(&format!("sync-request {height}"));
    if handle
        .request(peer, RscRequest::GetBlockByHeight { height })
        .is_err()
    {
        say("sync-failed");
    }
}

fn audit_next(duty: &Duty, handle: &NetworkHandle, peer: PeerId) {
    let next = match duty.sync_next.load(Ordering::Relaxed) {
        1 => 2,
        2 => 3,
        3 => 4,
        4 => 5,
        5 => 9,
        _ => return,
    };
    request_height(duty, handle, peer, next);
}

fn serve_sync(duty: &Duty, handle: &NetworkHandle, id: RscRequestId, request: RscRequest) {
    if matches!(request, RscRequest::Handshake(_)) {
        return;
    }
    let Some(chain) = duty.chain.as_ref() else {
        return;
    };
    let response = if duty.fault && duty.index == Some(2) {
        gap_response(chain, &request)
    } else if duty.fault {
        fault_response(chain, &request)
    } else {
        honest_response(chain, &request)
    };
    let _ = handle.send_response(id, response);
}

fn honest_response(chain: &LiveChain, request: &RscRequest) -> RscResponse {
    match request {
        RscRequest::GetBlockByHeight { height } => match encoded_height(&chain.store, *height) {
            Ok(Some((hash, bytes))) => {
                say(&format!("sync-serve {height} {}", encode_hex(&hash.0 .0)));
                RscResponse::Blocks {
                    payloads: vec![bytes],
                }
            }
            _ => {
                say(&format!("sync-serve-empty {height}"));
                RscResponse::Empty
            }
        },
        RscRequest::GetBlockByHash { hash } => {
            let Ok(hash) = <[u8; 32]>::try_from(hash.as_slice()) else {
                return RscResponse::Empty;
            };
            let hash = BlockHash(Hash(hash));
            match chain.store.get_block(&hash) {
                Ok(Some(block)) => match encode_block(&block) {
                    Ok(bytes) => {
                        say(&format!(
                            "sync-serve {} {}",
                            block.header.height.0,
                            encode_hex(&hash.0 .0)
                        ));
                        RscResponse::Blocks {
                            payloads: vec![bytes],
                        }
                    }
                    Err(_) => RscResponse::Empty,
                },
                _ => RscResponse::Empty,
            }
        }
        RscRequest::GetBlockRange { start, end } => {
            if end < start {
                return RscResponse::Empty;
            }
            let last = (*end).min(start.saturating_add(8));
            let mut payloads = Vec::new();
            for height in *start..=last {
                match encoded_height(&chain.store, height) {
                    Ok(Some((_, bytes))) => payloads.push(bytes),
                    _ => break,
                }
            }
            if payloads.is_empty() {
                say(&format!("sync-serve-empty {start}"));
                RscResponse::Empty
            } else {
                say(&format!("sync-serve {start}"));
                RscResponse::Blocks { payloads }
            }
        }
        RscRequest::Handshake(_) => RscResponse::Empty,
    }
}

fn encoded_height(store: &RocksDbChainStore, height: u64) -> Result<Option<(BlockHash, Vec<u8>)>, String> {
    let Some(hash) = store
        .get_block_by_height(BlockHeight(height))
        .map_err(|err| err.to_string())?
    else {
        return Ok(None);
    };
    let Some(block) = store.get_block(&hash).map_err(|err| err.to_string())? else {
        return Ok(None);
    };
    if block.header.height.0 != height {
        return Ok(None);
    }
    let bytes = encode_block(&block).map_err(|err| err.to_string())?;
    Ok(Some((hash, bytes)))
}

fn fault_response(chain: &LiveChain, request: &RscRequest) -> RscResponse {
    let RscRequest::GetBlockByHeight { height } = request else {
        return RscResponse::Empty;
    };
    let height = *height;
    say(&format!("sync-serve-fault {height}"));
    let Ok(Some(parent_hash)) = chain.store.get_canonical_tip() else {
        return RscResponse::Empty;
    };
    let Ok(Some(parent)) = chain.store.get_block(&parent_hash) else {
        return RscResponse::Empty;
    };
    let payload = match height {
        1 => Some(vec![0xab; 7]),
        2 => fault_block(&parent, 1, BlockHash::ZERO, parent.header.state_root, false).ok(),
        3 => fault_block(&parent, 9, parent_hash, parent.header.state_root, false).ok(),
        4 => fault_block(&parent, 1, parent_hash, parent.header.state_root, true).ok(),
        5 => fault_block(&parent, 1, parent_hash, Hash([9; 32]), false).ok(),
        _ => None,
    };
    match payload {
        Some(bytes) => RscResponse::Blocks {
            payloads: vec![bytes],
        },
        None => RscResponse::Empty,
    }
}

fn gap_response(chain: &LiveChain, request: &RscRequest) -> RscResponse {
    let RscRequest::GetBlockByHeight { height } = request else {
        return RscResponse::Empty;
    };
    let height = *height;
    say(&format!("sync-serve-fault {height}"));
    let Ok(Some(parent_hash)) = chain.store.get_canonical_tip() else {
        return RscResponse::Empty;
    };
    let Ok(Some(parent)) = chain.store.get_block(&parent_hash) else {
        return RscResponse::Empty;
    };
    let next = parent.header.height.0.saturating_add(1);
    let payload = match height {
        2 => fault_block(&parent, next, parent_hash, parent.header.state_root, true).ok(),
        3 => fault_block(&parent, next, BlockHash::ZERO, parent.header.state_root, false).ok(),
        4 => fault_block(
            &parent,
            next.saturating_add(1),
            parent_hash,
            parent.header.state_root,
            false,
        )
        .ok(),
        _ => None,
    };
    match payload {
        Some(bytes) => RscResponse::Blocks {
            payloads: vec![bytes],
        },
        None => RscResponse::Empty,
    }
}

fn fault_block(
    parent: &Block,
    height: u64,
    previous: BlockHash,
    state_root: Hash,
    flip_root: bool,
) -> Result<Vec<u8>, String> {
    let mut block = Block {
        header: BlockHeader {
            version: parent.header.version,
            chain_id: parent.header.chain_id,
            height: BlockHeight(height),
            previous_block_hash: previous,
            timestamp: parent.header.timestamp,
            transactions_root: Hash::ZERO,
            state_root,
            proposer: parent.header.proposer,
        },
        transactions: vec![RawTransaction(transfer_raw()?)],
    };
    block.header.transactions_root = block.compute_transaction_root();
    if flip_root {
        block.header.transactions_root.0[0] ^= 0xff;
    }
    encode_block(&block).map_err(|err| err.to_string())
}

fn transfer_raw() -> Result<Vec<u8>, String> {
    let key = PrivateKey::from_bytes(&[0x11; 32]).map_err(|err| err.to_string())?;
    let recipient = PrivateKey::from_bytes(&[0x22; 32]).map_err(|err| err.to_string())?;
    let mut transaction = Transaction {
        version: TRANSACTION_VERSION_V1,
        sender: AccountId::from(rsc_state::derive_account_id(&key.key_id())),
        nonce: 0,
        payload: TransactionPayload::Transfer {
            recipient: AccountId::from(rsc_state::derive_account_id(&recipient.key_id())),
            amount: 1,
        },
        signature: rsc_crypto::Signature::from_bytes(&[0; 64]).map_err(|err| err.to_string())?,
    };
    transaction.sign(&key).map_err(|err| err.to_string())?;
    Ok(transaction.canonical_bytes())
}

fn canonical_header(duty: &Duty) -> Result<(BlockHash, BlockHeader), String> {
    let chain = duty.chain.as_ref().ok_or("falta cadena")?;
    let hash = chain
        .store
        .get_canonical_tip()
        .map_err(|err| err.to_string())?
        .ok_or("falta tip")?;
    let block = chain
        .store
        .get_block(&hash)
        .map_err(|err| err.to_string())?
        .ok_or("bloque ausente")?;
    Ok((hash, block.header))
}

fn classify(duty: &Duty, bytes: &[u8]) -> Result<Block, String> {
    let block = decode_block(bytes).map_err(|_| "invalid encoding".to_string())?;
    let (tip, parent) = canonical_header(duty)?;
    let expected = parent
        .height
        .0
        .checked_add(1)
        .ok_or("invalid height".to_string())?;
    if block.header.height.0 != expected {
        return Err("invalid height".into());
    }
    if block.header.previous_block_hash != tip {
        return Err("invalid parent".into());
    }
    let mut ctx = duty.ctx.clone();
    ctx.parent = Some(parent);
    validate_block(&block, &ctx).map_err(|_| "invalid block".to_string())?;
    Ok(block)
}

fn take_sync(duty: &Arc<Duty>, handle: &NetworkHandle, peer: PeerId, response: RscResponse) {
    if !duty.pull {
        return;
    }
    let asked = duty.sync_next.load(Ordering::Relaxed);
    match response {
        RscResponse::Empty => {
            say(&format!("sync-empty {asked}"));
            if duty.audit {
                say("sync-failed");
                audit_next(duty, handle, peer);
                return;
            }
            match canonical_header(duty) {
                Ok((_, header)) if header.height.0 >= 2 => say("sync-complete"),
                _ => say("sync-failed"),
            }
        }
        RscResponse::Blocks { payloads } => {
            let Some(bytes) = payloads.first() else {
                say("sync-rejected invalid encoding");
                if duty.audit {
                    audit_next(duty, handle, peer);
                }
                return;
            };
            if payloads.len() != 1 {
                say("sync-rejected invalid encoding");
                if duty.audit {
                    audit_next(duty, handle, peer);
                }
                return;
            }
            let block = match classify(duty, bytes) {
                Ok(block) => block,
                Err(err) => {
                    say(&format!("sync-rejected {err}"));
                    if duty.audit {
                        audit_next(duty, handle, peer);
                    }
                    return;
                }
            };
            let Some(chain) = duty.chain.as_ref() else {
                return;
            };
            if let Err(err) = write_committed(chain, &block) {
                say(&format!("sync-rejected {err}"));
                if duty.audit {
                    audit_next(duty, handle, peer);
                }
                return;
            }
            if duty.audit {
                say("sync-rejected invalid block");
                return;
            }
            let Ok(hash) = block.block_hash() else {
                say("sync-rejected invalid block");
                return;
            };
            say(&format!(
                "synced {} {} {}",
                block.header.height.0,
                encode_hex(&hash.0 .0),
                encode_hex(&block.header.previous_block_hash.0 .0)
            ));
            if let Some(next) = block.header.height.0.checked_add(1) {
                request_height(duty, handle, peer, next);
            }
        }
        RscResponse::Handshake(_) => say("sync-rejected invalid encoding"),
    }
}

fn write_committed(chain: &LiveChain, block: &Block) -> Result<(), String> {
    let raw = block.transactions.first().ok_or("bloque vacío")?;
    let transaction =
        Transaction::from_canonical_bytes(&raw.0).map_err(|err| err.to_string())?;
    let id = transaction.id();
    let TransactionPayload::Transfer { recipient, amount } = transaction.payload else {
        return Err("payload distinto".into());
    };
    let root = chain.state.state_root().map_err(|err| err.to_string())?;
    let version = chain.state.version().map_err(|err| err.to_string())?;
    let mut transition = StateTransition::new(root, version);
    transition.push(StateOperation::Transfer {
        from: transaction.sender.into(),
        to: recipient.into(),
        amount,
        expected_nonce: transaction.nonce,
    });
    let next = version.checked_add(1).ok_or("versión de estado agotada")?;
    let mut batch = StorageBatch::new();
    let commitment = chain
        .state
        .stage_transition(&mut batch, &transition, next)
        .map_err(|err| err.to_string())?;
    if commitment.previous_root() != root
        || commitment.previous_version() != version
        || commitment.new_version() != next
        || Hash(*commitment.new_root().as_bytes()) != block.header.state_root
    {
        return Err("state root distinto".into());
    }
    if block.compute_transaction_root() != block.header.transactions_root {
        return Err("transactions root distinto".into());
    }
    let mut ledger = ledger_of(&chain.store)?;
    ledger.append(id).map_err(|err| err.to_string())?;
    let ledger_root = ledger.root();
    let hash = chain
        .store
        .stage_canonical(&mut batch, block)
        .map_err(|err| err.to_string())?;
    chain
        .store
        .database()
        .write(batch)
        .map_err(|err| err.to_string())?;
    if chain.store.get_canonical_tip().map_err(|err| err.to_string())? != Some(hash) {
        return Err("tip distinto".into());
    }
    if chain.state.state_root().map_err(|err| err.to_string())? != commitment.new_root() {
        return Err("state root distinto".into());
    }
    if ledger_of(&chain.store)?.root() != ledger_root {
        return Err("ledger distinto".into());
    }
    let encoded = encode_block(block).map_err(|err| err.to_string())?;
    say(&format!(
        "bytes {} {}",
        block.header.height.0,
        encode_hex(&encoded)
    ));
    let mempool_hash = rsc_core::TransactionId(Hash(*id.0.as_bytes()));
    if let Ok(guard) = chain.pool.lock() {
        if let Some(pool) = guard.as_ref() {
            pool.remove_committed(&[mempool_hash]);
            say(&format!("pool {}", pool.len()));
        }
    }
    let children = chain
        .store
        .get_children(&block.header.previous_block_hash)
        .map_err(|err| err.to_string())?;
    say(&format!("children {}", children.len()));
    say(&format!(
        "committed {} {} {} {} {} {} {}",
        encode_hex(&hash.0 .0),
        block.header.height.0,
        encode_hex(&block.header.previous_block_hash.0 .0),
        encode_hex(&block.header.transactions_root.0),
        encode_hex(&block.header.state_root.0),
        encode_hex(ledger_root.0.as_bytes()),
        encode_hex(id.0.as_bytes())
    ));
    say(&format!(
        "tip {} {}",
        encode_hex(&hash.0 .0),
        block.header.height.0
    ));
    say_account(&chain.state, transaction.sender)?;
    say_account(&chain.state, recipient)?;
    Ok(())
}

fn say_account(state: &State, id: AccountId) -> Result<(), String> {
    let Some(account) = state
        .get_account(&rsc_state::AccountId::from(id))
        .map_err(|err| err.to_string())?
    else {
        return Err("cuenta ausente".into());
    };
    say(&format!(
        "account {} {} {}",
        encode_hex(&id.0),
        account.native_balance(),
        account.nonce()
    ));
    Ok(())
}

fn ledger_of(store: &RocksDbChainStore) -> Result<Ledger, String> {
    let Some(mut current) = store.get_canonical_tip().map_err(|err| err.to_string())? else {
        return Ok(Ledger::new());
    };
    let mut toward_genesis = Vec::new();
    loop {
        let block = store
            .get_block(&current)
            .map_err(|err| err.to_string())?
            .ok_or("bloque ausente")?;
        let parent = block.header.previous_block_hash;
        let genesis = block.header.height == BlockHeight::GENESIS;
        toward_genesis.push(block);
        if genesis {
            break;
        }
        current = parent;
    }
    toward_genesis.reverse();
    let mut ledger = Ledger::new();
    for block in &toward_genesis {
        for raw in &block.transactions {
            let transaction =
                Transaction::from_canonical_bytes(&raw.0).map_err(|err| err.to_string())?;
            ledger.append(transaction.id()).map_err(|err| err.to_string())?;
        }
    }
    Ok(ledger)
}

fn publish_block_negatives(duty: &Arc<Duty>, handle: &NetworkHandle) {
    let Some(block) = duty.proposal.lock().ok().and_then(|kept| kept.clone()) else {
        return;
    };
    if let Some(other) = duty
        .validators
        .validators
        .iter()
        .find(|record| record.id != block.header.proposer)
    {
        let mut wrong = block.clone();
        wrong.header.proposer = other.id;
        if let Ok(bytes) = encode_block(&wrong) {
            publish_bytes(
                handle,
                GossipTopic::Blocks,
                bytes,
                "published-invalid invalid proposer",
            );
        }
    }
    let mut bad_root = block.clone();
    bad_root.header.transactions_root.0[0] ^= 0xff;
    if let Ok(bytes) = encode_block(&bad_root) {
        publish_bytes(
            handle,
            GossipTopic::Blocks,
            bytes,
            "published-invalid invalid block",
        );
    }
    if let Ok(bytes) = encode_block(&block) {
        let mut engine = match duty.engine.lock() {
            Ok(engine) => engine,
            Err(_) => return,
        };
        if engine.receive_proposal(&block, Some(&context(duty))).is_ok() {
            drop(engine);
            say(&format!(
                "accepted {}",
                encode_hex(&block.block_hash().map(|hash| hash.0 .0).unwrap_or([0; 32]))
            ));
        }
        let _ = bytes;
    }
}

fn admit_gossip(pool: &Mempool, handle: &NetworkHandle, data: &[u8], message_id: Vec<u8>) {
    match pool.insert(data.to_vec()) {
        Ok(hash) => {
            say(&format!("admitted {}", tx_hex(&hash)));
            if pool.queue_state(&hash) == Some(rsc_mempool::TransactionQueueState::Ready) {
                say(&format!("ready {}", tx_hex(&hash)));
            }
            let _ = handle.report_gossip_validation(message_id, GossipAcceptance::Accept);
        }
        Err(MempoolError::AlreadyKnown) => {
            if let Some(hex) = id_of(data) {
                say(&format!("known {hex}"));
            }
            let _ = handle.report_gossip_validation(message_id, GossipAcceptance::Accept);
        }
        Err(err) => {
            say(&format!("gossip-rejected {err}"));
            let _ = handle.report_gossip_validation(message_id, GossipAcceptance::Reject);
        }
    }
}

fn tx_hex(hash: &rsc_core::TransactionId) -> String {
    encode_hex(&hash.0 .0)
}

fn id_of(raw: &[u8]) -> Option<String> {
    Transaction::from_canonical_bytes(raw)
        .ok()
        .map(|tx| encode_hex(tx.id().0.as_bytes()))
}

struct SignedAdmission {
    key: PublicKey,
}

impl MempoolValidator for SignedAdmission {
    fn validate(
        &self,
        raw_transaction: &[u8],
    ) -> Result<MempoolTransaction, MempoolValidationError> {
        let transaction = Transaction::from_canonical_bytes(raw_transaction).map_err(map_tx)?;
        transaction.validate(&self.key).map_err(map_tx)?;
        let id = transaction.id();
        Ok(MempoolTransaction {
            hash: rsc_core::TransactionId(Hash(*id.0.as_bytes())),
            sender: transaction.sender,
            nonce: Nonce(transaction.nonce),
            raw_transaction: raw_transaction.to_vec(),
            received_at: std::time::Instant::now(),
        })
    }
}

fn map_tx(err: TransactionError) -> MempoolValidationError {
    match err {
        TransactionError::InvalidEncoding
        | TransactionError::InvalidField
        | TransactionError::InvalidPayload => MempoolValidationError::InvalidTransaction,
        _ => MempoolValidationError::ValidationFailed,
    }
}

struct FreshAccounts;

impl PendingNonceProvider for FreshAccounts {
    fn current_nonce(&self, _sender: &AccountId) -> Result<Nonce, NonceLookupError> {
        Ok(Nonce(0))
    }
}

struct Propagation {
    pool: Arc<Mempool>,
    p2p: NetworkHandle,
    network_id: String,
    chain_id: u32,
    peer: String,
    live: Option<Arc<LiveChain>>,
}

fn ready<T>(value: T) -> ServiceFut<'static, T>
where
    T: Send + 'static,
{
    Box::pin(async move { Ok(value) })
}

fn failed<T>(err: ServiceError) -> ServiceFut<'static, T>
where
    T: Send + 'static,
{
    Box::pin(async move { Err(err) })
}

impl Propagation {
    fn node_info(&self) -> RpcNodeInfo {
        RpcNodeInfo {
            node_id: self.peer.clone(),
            version: rsc_p2p::RSC_P2P_PROTOCOL_VERSION.into(),
            network_id: self.network_id.clone(),
            chain_id: self.chain_id.to_string(),
            protocol_version: rsc_p2p::RSC_P2P_PROTOCOL_VERSION.into(),
            sync_status: "local".into(),
            latest_height: 0,
            latest_block_hash: encode_hex(&[0u8; 32]),
        }
    }
}

impl NodeService for Propagation {
    fn chain_id(&self) -> ServiceFut<'_, String> {
        ready(self.chain_id.to_string())
    }

    fn node_info(&self) -> ServiceFut<'_, RpcNodeInfo> {
        ready(self.node_info())
    }

    fn status(&self) -> ServiceFut<'_, RpcStatus> {
        let (height, hash) = chain_tip(self.live.as_deref());
        ready(RpcStatus {
            syncing: false,
            latest_height: height,
            latest_block_hash: hash,
            epoch: 0,
            finalized_height: height,
        })
    }
}

impl ChainService for Propagation {
    fn block_by_hash(&self, _block_hash: &str) -> ServiceFut<'_, RpcBlock> {
        failed(ServiceError::NotFound)
    }

    fn block_by_height(&self, height: u64) -> ServiceFut<'_, RpcBlock> {
        let Some(live) = &self.live else {
            return failed(ServiceError::NotFound);
        };
        let hash = match live.store.get_block_by_height(BlockHeight(height)) {
            Ok(Some(hash)) => hash,
            _ => return failed(ServiceError::NotFound),
        };
        let Some(block) = live.store.get_block(&hash).ok().flatten() else {
            return failed(ServiceError::NotFound);
        };
        let transactions = block
            .transactions
            .iter()
            .map(|raw| encode_hex(&raw.0))
            .collect();
        ready(RpcBlock {
            hash: encode_hex(&hash.0 .0),
            height: block.header.height.0,
            parent_hash: encode_hex(&block.header.previous_block_hash.0 .0),
            timestamp: block.header.timestamp.0,
            transaction_root: encode_hex(&block.header.transactions_root.0),
            transactions,
        })
    }

    fn transaction(&self, tx_hash: &str) -> ServiceFut<'_, RpcTransaction> {
        let Some(live) = &self.live else {
            return failed(ServiceError::NotFound);
        };
        match included_transaction(&live.store, tx_hash) {
            Ok(tx) => ready(tx),
            Err(err) => failed(err),
        }
    }
}

impl TransactionService for Propagation {
    fn submit_signed(&self, signed_tx: &[u8]) -> ServiceFut<'_, RpcSubmitResult> {
        match self.pool.insert(signed_tx.to_vec()) {
            Ok(hash) => {
                let hex = tx_hex(&hash);
                say(&format!("admitted {hex}"));
                if self.pool.queue_state(&hash) == Some(rsc_mempool::TransactionQueueState::Ready) {
                    say(&format!("ready {hex}"));
                }
                match self
                    .p2p
                    .publish(GossipTopic::Transactions, signed_tx.to_vec())
                {
                    Ok(()) => say(&format!("published {hex}")),
                    Err(err) => say(&format!("publish-failed {err}")),
                }
                ready(RpcSubmitResult {
                    transaction_hash: hex,
                })
            }
            Err(err) => {
                say(&format!("rejected {err}"));
                failed(ServiceError::InvalidTransaction)
            }
        }
    }
}

impl StateService for Propagation {
    fn account(&self, account: &str) -> ServiceFut<'_, RpcAccount> {
        let Some(live) = &self.live else {
            return failed(ServiceError::NotFound);
        };
        match live_account(live, account) {
            Ok((id, balance, nonce)) => ready(RpcAccount {
                account: id,
                balance,
                nonce,
            }),
            Err(err) => failed(err),
        }
    }

    fn balance(&self, account: &str) -> ServiceFut<'_, RpcBalance> {
        let Some(live) = &self.live else {
            return failed(ServiceError::NotFound);
        };
        match live_account(live, account) {
            Ok((id, balance, _)) => ready(RpcBalance {
                account: id,
                balance,
            }),
            Err(err) => failed(err),
        }
    }
}

fn live_account(live: &LiveChain, account: &str) -> Result<(String, String, u64), ServiceError> {
    let bytes = rsc_rpc::decode_hex(account).map_err(|_| ServiceError::NotFound)?;
    if bytes.len() != 32 {
        return Err(ServiceError::NotFound);
    }
    let mut id = [0u8; 32];
    id.copy_from_slice(&bytes);
    let Some(account) = live
        .state
        .get_account(&rsc_state::AccountId::from(rsc_core::AccountId(id)))
        .ok()
        .flatten()
    else {
        return Err(ServiceError::NotFound);
    };
    Ok((
        encode_hex(&id),
        account.native_balance().to_string(),
        account.nonce(),
    ))
}

fn included_transaction(
    store: &RocksDbChainStore,
    tx_hash: &str,
) -> Result<RpcTransaction, ServiceError> {
    let bytes = rsc_rpc::decode_hex(tx_hash).map_err(|_| ServiceError::InvalidState)?;
    if bytes.len() != 32 {
        return Err(ServiceError::InvalidState);
    }
    let Some(mut current) = store
        .get_canonical_tip()
        .map_err(|_| ServiceError::Internal)?
    else {
        return Err(ServiceError::NotFound);
    };
    let mut toward_genesis = Vec::new();
    for _ in 0..64 {
        let block = store
            .get_block(&current)
            .map_err(|_| ServiceError::Internal)?
            .ok_or(ServiceError::NotFound)?;
        let parent = block.header.previous_block_hash;
        let genesis = block.header.height == BlockHeight::GENESIS;
        toward_genesis.push(block);
        if genesis {
            break;
        }
        current = parent;
    }
    toward_genesis.reverse();
    let mut index = 0u64;
    for block in &toward_genesis {
        let hash = block.block_hash().map_err(|_| ServiceError::Internal)?;
        for raw in &block.transactions {
            let transaction = Transaction::from_canonical_bytes(&raw.0)
                .map_err(|_| ServiceError::Internal)?;
            let id = encode_hex(transaction.id().0.as_bytes());
            if id == tx_hash {
                return Ok(RpcTransaction {
                    hash: id,
                    raw: encode_hex(&raw.0),
                    status: "included".into(),
                    block_height: Some(block.header.height.0),
                    block_hash: Some(encode_hex(&hash.0 .0)),
                    index: Some(index),
                });
            }
            index = index.checked_add(1).ok_or(ServiceError::Internal)?;
        }
    }
    Err(ServiceError::NotFound)
}

impl ConsensusService for Propagation {
    fn validators(&self) -> ServiceFut<'_, Vec<RpcValidator>> {
        failed(ServiceError::ConsensusUnavailable)
    }

    fn finality(&self) -> ServiceFut<'_, RpcFinality> {
        failed(ServiceError::ConsensusUnavailable)
    }
}
