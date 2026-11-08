//! Web Bridge. Recibe una ficha firmada, la valida y la sirve.
//! No abre la cadena, no firma por un nodo y no guarda claves privadas.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use rsc_p2p::presence::{PresenceError, SignedPresence};
use rsc_p2p::validate_network_id;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;

const BODY_LIMIT: usize = 64 * 1024;

struct Stored {
    wire: Vec<u8>,
    sequence: u64,
    expires_at: u64,
}

/// Directorio de fichas públicas. Una ficha por PeerId.
pub struct Bridge {
    network_id: String,
    chain_id: u32,
    dir: PathBuf,
    cards: Mutex<BTreeMap<String, Stored>>,
}

/// Rechazo de `POST /network/presence`. El texto es el de la ficha o `duplicate sequence`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BridgeReject {
    Presence(PresenceError),
    Duplicate,
}

impl std::fmt::Display for BridgeReject {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BridgeReject::Presence(err) => write!(formatter, "{err}"),
            BridgeReject::Duplicate => write!(formatter, "duplicate sequence"),
        }
    }
}

impl Bridge {
    /// Carga las fichas ya guardadas. Una ficha ilegible impide arrancar.
    pub fn open(dir: impl Into<PathBuf>, network_id: &str, chain_id: u32) -> Result<Self, PresenceError> {
        validate_network_id(network_id).map_err(|_| PresenceError::Invalid)?;
        let dir = dir.into();
        std::fs::create_dir_all(&dir).map_err(|_| PresenceError::Invalid)?;
        let mut cards = BTreeMap::new();
        for entry in std::fs::read_dir(&dir).map_err(|_| PresenceError::Invalid)? {
            let entry = entry.map_err(|_| PresenceError::Invalid)?;
            let kind = entry.file_type().map_err(|_| PresenceError::Invalid)?;
            if !kind.is_file() {
                return Err(PresenceError::Invalid);
            }
            let name = entry.file_name();
            let Some(peer) = name.to_str() else {
                return Err(PresenceError::Invalid);
            };
            if peer.starts_with('.') {
                continue;
            }
            let bytes = std::fs::read(entry.path()).map_err(|_| PresenceError::Invalid)?;
            let signed = load_signed(&bytes, network_id, chain_id)?;
            if signed.record.peer_id.to_string() != peer {
                return Err(PresenceError::Invalid);
            }
            cards.insert(
                peer.to_string(),
                Stored {
                    sequence: signed.record.sequence,
                    expires_at: signed.record.expires_at,
                    wire: bytes,
                },
            );
        }
        Ok(Self {
            network_id: network_id.to_string(),
            chain_id,
            dir,
            cards: Mutex::new(cards),
        })
    }

    /// Guarda `body` si la ficha es válida y su secuencia no es menor.
    /// La misma secuencia con los mismos bytes se acepta y no se reescribe.
    pub fn submit(&self, now: u64, body: &[u8]) -> Result<(), BridgeReject> {
        let signed = SignedPresence::from_wire(body).map_err(BridgeReject::Presence)?;
        let peer = signed.record.peer_id.to_string();
        let mut cards = self.cards.lock().unwrap_or_else(|err| err.into_inner());
        let latest = cards.get(&peer).map(|card| card.sequence).unwrap_or(0);
        signed
            .accept(now, &self.network_id, self.chain_id, latest)
            .map_err(BridgeReject::Presence)?;
        if let Some(existing) = cards.get(&peer) {
            if signed.record.sequence == existing.sequence {
                if existing.wire.as_slice() == body {
                    return Ok(());
                }
                return Err(BridgeReject::Duplicate);
            }
        }
        write_card(&self.dir, &peer, body)?;
        cards.insert(
            peer,
            Stored {
                wire: body.to_vec(),
                sequence: signed.record.sequence,
                expires_at: signed.record.expires_at,
            },
        );
        Ok(())
    }

    /// Fichas con `expires_at > now`, en el orden de PeerId. Los bytes no se reescriben.
    pub fn current(&self, now: u64) -> Vec<u8> {
        let cards = self.cards.lock().unwrap_or_else(|err| err.into_inner());
        let mut out = Vec::new();
        for card in cards.values() {
            if card.expires_at > now {
                out.extend_from_slice(&card.wire);
            }
        }
        out
    }
}

fn load_signed(bytes: &[u8], network_id: &str, chain_id: u32) -> Result<SignedPresence, PresenceError> {
    let signed = SignedPresence::from_wire(bytes)?;
    if signed.record.expires_at == 0 {
        return Err(PresenceError::Invalid);
    }
    signed.accept(
        signed.record.expires_at - 1,
        network_id,
        chain_id,
        0,
    )?;
    Ok(signed)
}

fn write_card(dir: &Path, peer: &str, body: &[u8]) -> Result<(), BridgeReject> {
    use std::io::Write;
    if peer.is_empty() || peer.contains('/') || peer.contains('\\') || peer.contains("..") {
        return Err(BridgeReject::Presence(PresenceError::Invalid));
    }
    let path = dir.join(peer);
    let tmp = dir.join(format!(".{peer}.tmp"));
    let mut file = std::fs::File::create(&tmp)
        .map_err(|_| BridgeReject::Presence(PresenceError::Invalid))?;
    file.write_all(body)
        .map_err(|_| BridgeReject::Presence(PresenceError::Invalid))?;
    file.sync_all()
        .map_err(|_| BridgeReject::Presence(PresenceError::Invalid))?;
    drop(file);
    std::fs::rename(&tmp, &path).map_err(|_| BridgeReject::Presence(PresenceError::Invalid))?;
    Ok(())
}

pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

pub fn router(bridge: Arc<Bridge>) -> Router {
    Router::new()
        .route("/network/presence", post(post_presence))
        .route("/network", get(get_network))
        .layer(RequestBodyLimitLayer::new(BODY_LIMIT))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            std::time::Duration::from_secs(5),
        ))
        .with_state(bridge)
}

async fn post_presence(State(bridge): State<Arc<Bridge>>, body: Bytes) -> Response {
    match bridge.submit(unix_now(), &body) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(BridgeReject::Duplicate) => (StatusCode::CONFLICT, "duplicate sequence").into_response(),
        Err(BridgeReject::Presence(err)) => (StatusCode::BAD_REQUEST, err.to_string()).into_response(),
    }
}

async fn get_network(State(bridge): State<Arc<Bridge>>) -> Response {
    let now = unix_now();
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    let stamp = HeaderValue::from_str(&now.to_string()).unwrap_or(HeaderValue::from_static("0"));
    headers.insert(HeaderName::from_static("x-rsc-bridge-time"), stamp);
    (StatusCode::OK, headers, bridge.current(now)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use libp2p::{identity, Multiaddr, PeerId};
    use rsc_p2p::presence::{PresenceRecord, Reach};
    use rsc_p2p::{
        BLOCK_SYNC_PROTOCOL, GOSSIPSUB_PROTOCOL, HANDSHAKE_PROTOCOL, RSC_P2P_PROTOCOL_VERSION,
    };

    static TEMPS: AtomicU64 = AtomicU64::new(0);

    struct Tmp {
        path: PathBuf,
    }

    impl Tmp {
        fn new() -> Self {
            let n = TEMPS.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "rsc-bridge-{}-{n}-{nanos}",
                std::process::id()
            ));
            std::fs::create_dir(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn protocols() -> [String; 4] {
        [
            RSC_P2P_PROTOCOL_VERSION.to_string(),
            HANDSHAKE_PROTOCOL.to_string(),
            BLOCK_SYNC_PROTOCOL.to_string(),
            GOSSIPSUB_PROTOCOL.to_string(),
        ]
    }

    fn addr(text: &str) -> Multiaddr {
        text.parse().unwrap()
    }

    struct Spec<'a> {
        peer: PeerId,
        sequence: u64,
        expires_at: u64,
        reach: Reach,
        address: &'a str,
        network_id: &'a str,
        chain_id: u32,
    }

    fn sign(key: &identity::Keypair, spec: Spec<'_>) -> Vec<u8> {
        let record = PresenceRecord {
            network_id: spec.network_id.to_string(),
            chain_id: spec.chain_id,
            peer_id: spec.peer,
            public_key: key.public().encode_protobuf(),
            protocols: protocols(),
            chain: None,
            reach: spec.reach,
            addresses: vec![addr(spec.address)],
            issued_at: 1,
            expires_at: spec.expires_at,
            sequence: spec.sequence,
        };
        let canon = record.canonical_bytes().unwrap();
        let raw = key.sign(&canon).unwrap();
        let mut wire = canon;
        wire.extend_from_slice(&raw);
        wire
    }

    fn valid(key: &identity::Keypair, sequence: u64, address: &str) -> Vec<u8> {
        sign(
            key,
            Spec {
                peer: PeerId::from(&key.public()),
                sequence,
                expires_at: 5_000,
                reach: Reach::PublicDirect,
                address,
                network_id: "devnet",
                chain_id: 3,
            },
        )
    }

    fn open_bridge(dir: &Path) -> Bridge {
        Bridge::open(dir, "devnet", 3).unwrap()
    }

    #[test]
    fn keeps_exact_bytes_and_replaces_only_a_higher_sequence() {
        let dir = Tmp::new();
        let bridge = open_bridge(&dir.path);
        let key = identity::Keypair::generate_ed25519();
        let first = valid(&key, 2, "/ip4/203.0.113.10/tcp/4001");
        bridge.submit(1_000, &first).unwrap();
        assert_eq!(bridge.current(1_000), first);
        bridge.submit(1_000, &first).unwrap();
        assert_eq!(bridge.current(1_000), first);
        let second = valid(&key, 3, "/ip4/203.0.113.11/tcp/4001");
        bridge.submit(1_000, &second).unwrap();
        assert_eq!(bridge.current(1_000), second);
        let peer = PeerId::from(&key.public()).to_string();
        assert_eq!(std::fs::read(dir.path.join(&peer)).unwrap(), second);
        drop(bridge);
        let reopened = open_bridge(&dir.path);
        assert_eq!(reopened.current(1_000), second);
        assert!(reopened.current(5_000).is_empty());
        assert!(dir.path.join(&peer).is_file());
    }

    #[test]
    fn rejected_cards_stay_out_of_the_listing() {
        let dir = Tmp::new();
        let bridge = open_bridge(&dir.path);
        let key = identity::Keypair::generate_ed25519();
        let peer = PeerId::from(&key.public());
        let good = valid(&key, 4, "/ip4/203.0.113.10/tcp/4001");
        bridge.submit(100, &good).unwrap();

        let public = "/ip4/203.0.113.10/tcp/4001";
        let mismatch = sign(&key, Spec {
            peer: PeerId::random(),
            sequence: 5,
            expires_at: 5_000,
            reach: Reach::PublicDirect,
            address: public,
            network_id: "devnet",
            chain_id: 3,
        });
        let mut bad_sig = good.clone();
        let last = bad_sig.len() - 1;
        bad_sig[last] ^= 0x01;
        let network = sign(&key, Spec {
            peer,
            sequence: 5,
            expires_at: 5_000,
            reach: Reach::PublicDirect,
            address: public,
            network_id: "other",
            chain_id: 3,
        });
        let chain = sign(&key, Spec {
            peer,
            sequence: 5,
            expires_at: 5_000,
            reach: Reach::PublicDirect,
            address: public,
            network_id: "devnet",
            chain_id: 9,
        });
        let expired = sign(&key, Spec {
            peer,
            sequence: 5,
            expires_at: 100,
            reach: Reach::PublicDirect,
            address: public,
            network_id: "devnet",
            chain_id: 3,
        });
        let stale = sign(&key, Spec {
            peer,
            sequence: 3,
            expires_at: 5_000,
            reach: Reach::PublicDirect,
            address: "/ip4/203.0.113.12/tcp/4001",
            network_id: "devnet",
            chain_id: 3,
        });
        let duplicate = sign(&key, Spec {
            peer,
            sequence: 4,
            expires_at: 5_000,
            reach: Reach::PublicDirect,
            address: "/ip4/203.0.113.12/tcp/4001",
            network_id: "devnet",
            chain_id: 3,
        });
        let private_addr = sign(&key, Spec {
            peer,
            sequence: 5,
            expires_at: 5_000,
            reach: Reach::PublicDirect,
            address: "/ip4/10.0.0.1/tcp/4001",
            network_id: "devnet",
            chain_id: 3,
        });
        let relay = sign(&key, Spec {
            peer,
            sequence: 5,
            expires_at: 5_000,
            reach: Reach::Relay,
            address: public,
            network_id: "devnet",
            chain_id: 3,
        });
        let cases = [
            (mismatch, BridgeReject::Presence(PresenceError::IdentityMismatch)),
            (bad_sig, BridgeReject::Presence(PresenceError::InvalidSignature)),
            (network, BridgeReject::Presence(PresenceError::NetworkMismatch)),
            (chain, BridgeReject::Presence(PresenceError::ChainMismatch)),
            (expired, BridgeReject::Presence(PresenceError::Expired)),
            (stale, BridgeReject::Presence(PresenceError::StaleSequence)),
            (duplicate, BridgeReject::Duplicate),
            (private_addr, BridgeReject::Presence(PresenceError::NotPublishable)),
            (relay, BridgeReject::Presence(PresenceError::NotRelayAddress)),
            (
                b"not-a-card".to_vec(),
                BridgeReject::Presence(PresenceError::Invalid),
            ),
        ];
        for (body, expected) in cases {
            assert_eq!(bridge.submit(100, &body), Err(expected));
            assert_eq!(bridge.current(100), good);
        }
        assert_eq!(SignedPresence::read_list(&bridge.current(100)).unwrap().len(), 1);
    }

    #[test]
    fn two_bridges_store_the_same_card_without_signing_it() {
        let left_dir = Tmp::new();
        let right_dir = Tmp::new();
        let left = open_bridge(&left_dir.path);
        let right = open_bridge(&right_dir.path);
        let key = identity::Keypair::generate_ed25519();
        let wire = valid(&key, 1, "/ip4/203.0.113.10/tcp/9");
        left.submit(10, &wire).unwrap();
        right.submit(10, &wire).unwrap();
        assert_eq!(left.current(10), right.current(10));
        let card = SignedPresence::from_wire(&left.current(10)).unwrap();
        assert!(card.verify().is_ok());
        assert_eq!(
            card.signature,
            SignedPresence::from_wire(&wire).unwrap().signature
        );
    }
}
