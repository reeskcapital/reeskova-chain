//! Anunciador. Firma con la clave persistente y renueva el heartbeat.
//!
//! No imprime la clave ni el token. Si el registro no responde, el nodo sigue
//! sirviendo y el siguiente ciclo reintenta.

use crate::sign::{dash, public_key_hex, sign_fields, SignedFields};
use crate::types::{NodeStatus, NodeType};
use crate::util::{sanitize_log, unix_now};
use crate::validate::{host_allowlist_from_env, parse_peer_id, screen_endpoints, validate_p2p_multiaddr};
use libp2p::identity::Keypair;
use libp2p::PeerId;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

const DEFAULT_REGISTRY: &str = "http://127.0.0.1:8550";

#[derive(Clone)]
pub struct AnnouncerConfig {
    pub identity_protobuf: Vec<u8>,
    pub peer_id: String,
    pub network_id: String,
    pub chain_id: u32,
    pub node_type: NodeType,
    pub registry_url: String,
    pub rpc_public_url: Option<String>,
    pub service_url: Option<String>,
    pub probe_health_url: Option<String>,
    pub software_version: String,
    pub sequence_path: PathBuf,
    pub heartbeat_secs: u64,
    pub ttl_secs: u64,
    pub host_allowlist: Vec<String>,
}

impl std::fmt::Debug for AnnouncerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnnouncerConfig")
            .field("peer_id", &self.peer_id)
            .field("network_id", &self.network_id)
            .field("chain_id", &self.chain_id)
            .field("node_type", &self.node_type)
            .field("registry_url", &sanitize_log(&self.registry_url))
            .finish()
    }
}

pub struct AnnouncerHandle {
    stop: Arc<AtomicBool>,
    p2p: Arc<Mutex<Option<String>>>,
    peer_id: String,
    join: Option<JoinHandle<()>>,
}

impl AnnouncerHandle {
    /// Guarda una multiaddr solo si es alcanzable y pertenece a este PeerId.
    pub fn set_p2p_addr(&self, addr: Option<String>) {
        let next = match addr {
            None => None,
            Some(text) => match parse_peer_id(&self.peer_id) {
                Ok(peer) if validate_p2p_multiaddr(&text, &peer, false).is_ok() => Some(text),
                _ => return,
            },
        };
        if let Ok(mut slot) = self.p2p.lock() {
            *slot = next;
        }
    }

    pub fn shutdown(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for AnnouncerHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

pub fn spawn_announcer(mut config: AnnouncerConfig) -> Result<AnnouncerHandle, String> {
    let keypair = Keypair::from_protobuf_encoding(&config.identity_protobuf)
        .map_err(|_| "identidad inválida".to_string())?;
    let derived = PeerId::from(keypair.public()).to_string();
    if derived != config.peer_id {
        return Err("identidad distinta".into());
    }
    if screen_endpoints(
        config.node_type,
        config.rpc_public_url.as_deref(),
        config.service_url.as_deref(),
        false,
        &config.host_allowlist,
    )
    .is_err()
    {
        eprintln!("anuncio: url pública no anunciable");
        config.rpc_public_url = None;
        config.service_url = None;
        screen_endpoints(
            config.node_type,
            None,
            None,
            false,
            &config.host_allowlist,
        )
        .map_err(|_| "endpoint no anunciable".to_string())?;
    }
    if let Some(parent) = config.sequence_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|_| "directorio de secuencia".to_string())?;
        }
    }
    let stop = Arc::new(AtomicBool::new(false));
    let p2p = Arc::new(Mutex::new(None));
    let stop_thread = Arc::clone(&stop);
    let p2p_thread = Arc::clone(&p2p);
    let peer_id = config.peer_id.clone();
    let join = std::thread::Builder::new()
        .name("rsc-announce".into())
        .spawn(move || run_loop(config, keypair, stop_thread, p2p_thread))
        .map_err(|_| "hilo de anuncio".to_string())?;
    Ok(AnnouncerHandle {
        stop,
        p2p,
        peer_id,
        join: Some(join),
    })
}

/// Arranque para mobile, community y validator. Lee la red desde el entorno.
/// Si faltan `RSC_NETWORK_ID` o `RSC_CHAIN_ID`, no anuncia y el proceso sigue.
pub fn spawn_from_env(
    kind: NodeType,
    key_path: &Path,
    probe_health_url: Option<String>,
) -> Option<AnnouncerHandle> {
    if std::env::var("RSC_REGISTRY_DISABLE").ok().as_deref() == Some("1") {
        eprintln!("anuncio desactivado");
        return None;
    }
    let network_id = match std::env::var("RSC_NETWORK_ID") {
        Ok(value) if !value.is_empty() => value,
        _ => {
            eprintln!("anuncio omitido: falta RSC_NETWORK_ID");
            return None;
        }
    };
    let chain_id = match std::env::var("RSC_CHAIN_ID") {
        Ok(value) => match value.parse::<u32>() {
            Ok(id) => id,
            Err(_) => {
                eprintln!("anuncio omitido: RSC_CHAIN_ID");
                return None;
            }
        },
        Err(_) => {
            eprintln!("anuncio omitido: falta RSC_CHAIN_ID");
            return None;
        }
    };
    let registry_url = std::env::var("RSC_REGISTRY_URL")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_REGISTRY.to_string());
    let keypair = match load_or_create_identity(key_path) {
        Ok(keypair) => keypair,
        Err(err) => {
            eprintln!("anuncio: {err}");
            return None;
        }
    };
    let identity_protobuf = match keypair.to_protobuf_encoding() {
        Ok(bytes) => bytes,
        Err(_) => {
            eprintln!("anuncio: identidad");
            return None;
        }
    };
    let rpc_public_url = if kind.allows_rpc_url() {
        std::env::var("RSC_RPC_PUBLIC_URL")
            .ok()
            .filter(|value| !value.is_empty())
    } else {
        None
    };
    let service_url = if matches!(kind, NodeType::Mobile | NodeType::Community) {
        std::env::var("RSC_SERVICE_URL")
            .ok()
            .filter(|value| !value.is_empty())
    } else {
        None
    };
    let sequence_path = {
        let mut name = key_path.as_os_str().to_owned();
        name.push(".seq");
        PathBuf::from(name)
    };
    let config = AnnouncerConfig {
        identity_protobuf,
        peer_id: PeerId::from(keypair.public()).to_string(),
        network_id,
        chain_id,
        node_type: kind,
        registry_url,
        rpc_public_url,
        service_url,
        probe_health_url,
        software_version: std::env::var("RSC_SOFTWARE_VERSION")
            .ok()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "0.1.0".into()),
        sequence_path,
        heartbeat_secs: 15,
        ttl_secs: 45,
        host_allowlist: host_allowlist_from_env(),
    };
    match spawn_announcer(config) {
        Ok(handle) => Some(handle),
        Err(err) => {
            eprintln!("anuncio: {err}");
            None
        }
    }
}

/// Carga o crea la clave y devuelve los bytes protobuf junto al PeerId.
/// No rota un archivo que ya existe y no es válido.
pub fn open_identity(path: &Path) -> Result<(Vec<u8>, String), String> {
    let keypair = load_or_create_identity(path)?;
    let peer_id = PeerId::from(keypair.public()).to_string();
    let bytes = keypair
        .to_protobuf_encoding()
        .map_err(|_| "identidad".to_string())?;
    Ok((bytes, peer_id))
}

/// Carga `p2p.key`. Si el archivo existe y no es Ed25519 válido, no lo rota.
pub fn load_or_create_identity(path: &Path) -> Result<Keypair, String> {
    if path.exists() {
        let bytes = std::fs::read(path).map_err(|_| "identidad ilegible".to_string())?;
        return Keypair::from_protobuf_encoding(&bytes)
            .map_err(|_| "identidad inválida".to_string());
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|_| "directorio de identidad".to_string())?;
        }
    }
    let keypair = Keypair::generate_ed25519();
    let bytes = keypair
        .to_protobuf_encoding()
        .map_err(|_| "identidad".to_string())?;
    let tmp = path.with_extension("tmp");
    {
        use std::io::Write;
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut file = opts.open(&tmp).map_err(|_| "identidad".to_string())?;
        file.write_all(&bytes).map_err(|_| "identidad".to_string())?;
        file.sync_all().map_err(|_| "identidad".to_string())?;
    }
    std::fs::rename(&tmp, path).map_err(|_| "identidad".to_string())?;
    let stored = std::fs::read(path).map_err(|_| "identidad".to_string())?;
    if stored != bytes {
        return Err("identidad".into());
    }
    Ok(keypair)
}

#[derive(Serialize)]
struct AnnounceBody {
    peer_id: String,
    public_key_hex: String,
    network_id: String,
    chain_id: u32,
    node_type: String,
    p2p_multiaddr: Option<String>,
    rpc_public_url: Option<String>,
    service_url: Option<String>,
    status: String,
    software_version: String,
    issued_at: u64,
    expires_at: u64,
    sequence: u64,
    ttl_secs: u64,
    signature_hex: String,
}

fn run_loop(
    config: AnnouncerConfig,
    keypair: Keypair,
    stop: Arc<AtomicBool>,
    p2p: Arc<Mutex<Option<String>>>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => {
            eprintln!("anuncio: runtime");
            return;
        }
    };
    runtime.block_on(async move {
        let client = match reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .build()
        {
            Ok(client) => client,
            Err(_) => {
                eprintln!("anuncio: cliente");
                return;
            }
        };
        let mut logged_down = false;
        let mut ever_online = false;
        loop {
            if stop.load(Ordering::SeqCst) {
                let _ = publish(&client, &config, &keypair, &p2p, NodeStatus::Offline, ever_online).await;
                break;
            }
            let probed = probe_ok(&client, config.probe_health_url.as_deref()).await;
            let status = if probed {
                ever_online = true;
                NodeStatus::Online
            } else if ever_online {
                NodeStatus::Degraded
            } else {
                NodeStatus::Starting
            };
            match publish(&client, &config, &keypair, &p2p, status, ever_online).await {
                Ok(()) => {
                    if !logged_down {
                        eprintln!("anuncio: {} {}", config.peer_id, status.as_str());
                    }
                    logged_down = false;
                }
                Err(()) => {
                    if !logged_down {
                        eprintln!("anuncio: registro no disponible");
                        logged_down = true;
                    }
                }
            }
            for _ in 0..config.heartbeat_secs.max(1) {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    });
}

async fn probe_ok(client: &reqwest::Client, url: Option<&str>) -> bool {
    let Some(url) = url else {
        return true;
    };
    match client.get(url).send().await {
        Ok(response) if response.status().is_success() => true,
        _ => false,
    }
}

async fn publish(
    client: &reqwest::Client,
    config: &AnnouncerConfig,
    keypair: &Keypair,
    p2p: &Mutex<Option<String>>,
    status: NodeStatus,
    known: bool,
) -> Result<(), ()> {
    let p2p_addr = p2p.lock().ok().and_then(|slot| slot.clone());
    let screened = screen_endpoints(
        config.node_type,
        config.rpc_public_url.as_deref(),
        config.service_url.as_deref(),
        false,
        &config.host_allowlist,
    )
    .map_err(|_| ())?;
    let sequence = next_sequence(&config.sequence_path).map_err(|_| ())?;
    let issued_at = unix_now();
    let ttl = config.ttl_secs.clamp(15, 120);
    let expires_at = issued_at.saturating_add(ttl);
    let fields = SignedFields {
        peer_id: &config.peer_id,
        network_id: &config.network_id,
        chain_id: config.chain_id,
        node_type: config.node_type.as_str(),
        p2p_multiaddr: &dash(&p2p_addr),
        rpc_public_url: &dash(&screened.rpc_public_url),
        service_url: &dash(&screened.service_url),
        status: status.as_str(),
        software_version: &config.software_version,
        issued_at,
        expires_at,
        sequence,
        ttl_secs: ttl,
    };
    let signature_hex = sign_fields(keypair, &fields).map_err(|_| ())?;
    let public_key_hex = public_key_hex(keypair).map_err(|_| ())?;
    let body = AnnounceBody {
        peer_id: config.peer_id.clone(),
        public_key_hex,
        network_id: config.network_id.clone(),
        chain_id: config.chain_id,
        node_type: config.node_type.as_str().to_string(),
        p2p_multiaddr: p2p_addr,
        rpc_public_url: screened.rpc_public_url,
        service_url: screened.service_url,
        status: status.as_str().to_string(),
        software_version: config.software_version.clone(),
        issued_at,
        expires_at,
        sequence,
        ttl_secs: ttl,
        signature_hex,
    };
    let path = if status == NodeStatus::Offline {
        "/v1/offline"
    } else if known {
        "/v1/heartbeat"
    } else {
        "/v1/announce"
    };
    let url = format!(
        "{}{}",
        config.registry_url.trim_end_matches('/'),
        path
    );
    let response = client.post(&url).json(&body).send().await.map_err(|_| ())?;
    if response.status().as_u16() == 409 {
        if let Ok(value) = response.json::<serde_json::Value>().await {
            if let Some(current) = value.get("expected_after").and_then(|v| v.as_u64()) {
                let _ = write_sequence(&config.sequence_path, current);
            }
        }
        return Err(());
    }
    if response.status().as_u16() == 404 && path == "/v1/heartbeat" {
        let retry = format!(
            "{}{}",
            config.registry_url.trim_end_matches('/'),
            "/v1/announce"
        );
        let again = client.post(&retry).json(&body).send().await.map_err(|_| ())?;
        return if again.status().is_success() {
            Ok(())
        } else {
            Err(())
        };
    }
    if response.status().is_success() {
        Ok(())
    } else {
        Err(())
    }
}

fn next_sequence(path: &Path) -> Result<u64, ()> {
    let current = read_sequence(path)?;
    let next = current.checked_add(1).ok_or(())?;
    write_sequence(path, next)?;
    Ok(next)
}

fn read_sequence(path: &Path) -> Result<u64, ()> {
    match std::fs::read(path) {
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Ok(bytes) if bytes.len() == 8 => {
            let mut buf = [0u8; 8];
            buf.copy_from_slice(&bytes);
            Ok(u64::from_be_bytes(buf))
        }
        _ => Err(()),
    }
}

fn write_sequence(path: &Path, value: u64) -> Result<(), ()> {
    use std::io::Write;
    let name = path.file_name().and_then(|name| name.to_str()).ok_or(())?;
    let parent = path.parent().filter(|dir| !dir.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    let tmp = parent.join(format!(".{name}.tmp"));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(&tmp).map_err(|_| ())?;
    file.write_all(&value.to_be_bytes()).map_err(|_| ())?;
    file.sync_all().map_err(|_| ())?;
    drop(file);
    std::fs::rename(&tmp, path).map_err(|_| ())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_survives_restart_and_corrupt_file_is_not_rotated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p2p.key");
        let first = load_or_create_identity(&path).unwrap();
        let peer = PeerId::from(first.public()).to_string();
        let bytes = std::fs::read(&path).unwrap();
        drop(first);
        let second = load_or_create_identity(&path).unwrap();
        assert_eq!(PeerId::from(second.public()).to_string(), peer);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);

        std::fs::write(&path, b"not-a-key").unwrap();
        assert!(load_or_create_identity(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"not-a-key");
    }

    #[test]
    fn debug_omits_identity_bytes() {
        let key = Keypair::generate_ed25519();
        let config = AnnouncerConfig {
            identity_protobuf: key.to_protobuf_encoding().unwrap(),
            peer_id: PeerId::from(key.public()).to_string(),
            network_id: "rsc-public".into(),
            chain_id: 3,
            node_type: NodeType::Full,
            registry_url: "http://127.0.0.1:8550".into(),
            rpc_public_url: None,
            service_url: None,
            probe_health_url: None,
            software_version: "0.1.0".into(),
            sequence_path: PathBuf::from("announce.seq"),
            heartbeat_secs: 15,
            ttl_secs: 45,
            host_allowlist: Vec::new(),
        };
        let text = format!("{config:?}");
        assert!(!text.contains("identity_protobuf"));
        assert!(!text.to_ascii_lowercase().contains("bearer"));
        let encoded = hex_of(&config.identity_protobuf);
        assert!(!text.contains(&encoded));
    }

    fn hex_of(bytes: &[u8]) -> String {
        crate::util::hex_encode(bytes)
    }

    #[test]
    fn set_p2p_drops_private_and_keeps_public() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p2p.key");
        let key = load_or_create_identity(&path).unwrap();
        let peer = PeerId::from(key.public()).to_string();
        let config = AnnouncerConfig {
            identity_protobuf: key.to_protobuf_encoding().unwrap(),
            peer_id: peer.clone(),
            network_id: "rsc-public".into(),
            chain_id: 3,
            node_type: NodeType::P2p,
            registry_url: "http://127.0.0.1:9".into(),
            rpc_public_url: None,
            service_url: None,
            probe_health_url: None,
            software_version: "0.1.0".into(),
            sequence_path: dir.path().join("announce.seq"),
            heartbeat_secs: 60,
            ttl_secs: 45,
            host_allowlist: Vec::new(),
        };
        let handle = spawn_announcer(config).unwrap();
        handle.set_p2p_addr(Some(format!("/ip4/127.0.0.1/tcp/9000/p2p/{peer}")));
        handle.set_p2p_addr(Some(format!("/ip4/10.0.0.8/tcp/9000/p2p/{peer}")));
        let stored = handle.p2p.lock().unwrap().clone();
        assert!(stored.is_none());
        let public = format!("/ip4/203.0.113.10/tcp/9000/p2p/{peer}");
        handle.set_p2p_addr(Some(public.clone()));
        assert_eq!(handle.p2p.lock().unwrap().clone().as_deref(), Some(public.as_str()));
        handle.shutdown();
    }
}
