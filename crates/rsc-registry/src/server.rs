//! HTTP del registro.
//!
//! Lectura y anuncio firmado son la API de los nodos. Borrar fichas y forzar
//! la expiración exigen un token y un socket de loopback. El token no se loguea.

use crate::sign::{dash, verify_fields, SignedFields};
use crate::store::{NodeFilter, RecordWrite, RegistryStore, UpsertError};
use crate::types::{NodeStatus, NodeType};
use crate::util::{const_time_eq, unix_now};
use crate::validate::{
    parse_peer_id, screen_endpoints, valid_network_id, valid_version, validate_p2p_multiaddr,
    ValidateError,
};
use axum::extract::{ConnectInfo, FromRequestParts, Path, Query, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::net::TcpListener;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;

#[derive(Clone, Debug)]
pub struct RegistryConfig {
    pub bind: SocketAddr,
    pub db_path: PathBuf,
    pub admin_token: Option<String>,
    pub allow_private: bool,
    pub rpc_hosts: Vec<String>,
    pub min_ttl_secs: u64,
    pub max_ttl_secs: u64,
    pub rate_per_window: u32,
    pub rate_window: Duration,
}

pub struct RegistryServer {
    config: RegistryConfig,
    store: Arc<Mutex<RegistryStore>>,
}

#[derive(Clone)]
struct AppState {
    store: Arc<Mutex<RegistryStore>>,
    admin_token: Option<String>,
    allow_private: bool,
    rpc_hosts: Vec<String>,
    min_ttl_secs: u64,
    max_ttl_secs: u64,
    rate_per_window: u32,
    rate_window: Duration,
    hits: Arc<Mutex<HashMap<String, (Instant, u32)>>>,
}

struct ClientAddr(SocketAddr);

impl<S> FromRequestParts<S> for ClientAddr
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let addr = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|info| info.0)
            .unwrap_or_else(|| SocketAddr::from(([127, 0, 0, 1], 0)));
        Ok(ClientAddr(addr))
    }
}

impl RegistryServer {
    pub fn open(config: RegistryConfig) -> Result<Self, String> {
        let store = RegistryStore::open(&config.db_path).map_err(|_| "database".to_string())?;
        Ok(Self {
            config,
            store: Arc::new(Mutex::new(store)),
        })
    }

    pub fn router(&self) -> Router {
        let state = AppState {
            store: Arc::clone(&self.store),
            admin_token: self.config.admin_token.clone(),
            allow_private: self.config.allow_private,
            rpc_hosts: self.config.rpc_hosts.clone(),
            min_ttl_secs: self.config.min_ttl_secs,
            max_ttl_secs: self.config.max_ttl_secs,
            rate_per_window: self.config.rate_per_window,
            rate_window: self.config.rate_window,
            hits: Arc::new(Mutex::new(HashMap::new())),
        };
        Router::new()
            .route("/health", get(health))
            .route("/v1/nodes", get(list_nodes))
            .route("/v1/nodes/{peer_id}", get(get_node))
            .route("/v1/announce", post(announce))
            .route("/v1/heartbeat", post(heartbeat))
            .route("/v1/offline", post(offline))
            .route("/v1/admin/expire", post(admin_expire))
            .route("/v1/admin/nodes/{peer_id}", delete(admin_delete))
            .layer(RequestBodyLimitLayer::new(8 * 1024))
            .layer(TimeoutLayer::with_status_code(
                StatusCode::REQUEST_TIMEOUT,
                Duration::from_secs(5),
            ))
            .with_state(state)
    }

    pub async fn run(self) -> Result<(), String> {
        let listener = TcpListener::bind(self.config.bind)
            .await
            .map_err(|_| "bind failed".to_string())?;
        let store = Arc::clone(&self.store);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(5)).await;
                if let Ok(mut guard) = store.lock() {
                    let _ = guard.expire(unix_now());
                }
            }
        });
        let app = self.router().into_make_service_with_connect_info::<SocketAddr>();
        eprintln!("registry bind {}", self.config.bind);
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_signal())
            .await
            .map_err(|_| "serve".to_string())
    }
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

async fn health() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({"status":"ok"})))
}

#[derive(Deserialize)]
struct ListQuery {
    network_id: Option<String>,
    chain_id: Option<u32>,
    status: Option<String>,
    node_type: Option<String>,
}

async fn list_nodes(
    State(state): State<AppState>,
    client: ClientAddr,
    Query(query): Query<ListQuery>,
) -> Response {
    if let Err(response) = limit(&state, &client) {
        return response;
    }
    let filter = NodeFilter {
        network_id: query.network_id,
        chain_id: query.chain_id,
        status: query.status,
        node_type: query.node_type,
    };
    let Ok(guard) = state.store.lock() else {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "storage");
    };
    match guard.list(&filter, unix_now()) {
        Ok(nodes) => Json(json!({"nodes": nodes})).into_response(),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "storage"),
    }
}

async fn get_node(
    State(state): State<AppState>,
    client: ClientAddr,
    Path(peer_id): Path<String>,
) -> Response {
    if let Err(response) = limit(&state, &client) {
        return response;
    }
    if parse_peer_id(&peer_id).is_err() {
        return error(StatusCode::BAD_REQUEST, "invalid identity");
    }
    let Ok(guard) = state.store.lock() else {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "storage");
    };
    match guard.get(&peer_id, unix_now()) {
        Ok(Some(node)) => Json(node).into_response(),
        Ok(None) => error(StatusCode::NOT_FOUND, "not found"),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "storage"),
    }
}

#[derive(Debug, Deserialize)]
struct AnnounceRequest {
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

async fn announce(
    State(state): State<AppState>,
    client: ClientAddr,
    Json(body): Json<AnnounceRequest>,
) -> Response {
    write_presence(&state, &client, body, false).await
}

async fn heartbeat(
    State(state): State<AppState>,
    client: ClientAddr,
    Json(body): Json<AnnounceRequest>,
) -> Response {
    write_presence(&state, &client, body, false).await
}

async fn offline(
    State(state): State<AppState>,
    client: ClientAddr,
    Json(body): Json<AnnounceRequest>,
) -> Response {
    if body.status != NodeStatus::Offline.as_str() {
        return error(StatusCode::BAD_REQUEST, "invalid request");
    }
    write_presence(&state, &client, body, true).await
}

async fn write_presence(
    state: &AppState,
    client: &ClientAddr,
    body: AnnounceRequest,
    offline: bool,
) -> Response {
    if let Err(response) = limit(state, client) {
        return response;
    }
    let record = match accept_announce(state, &body, offline) {
        Ok(record) => record,
        Err(response) => return response,
    };
    let now = unix_now();
    let mut guard = match state.store.lock() {
        Ok(guard) => guard,
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "storage"),
    };
    match guard.upsert(&record, now) {
        Ok(()) => Json(json!({
            "peer_id": record.peer_id,
            "status": record.status,
        }))
        .into_response(),
        Err(UpsertError::Stale(stale)) => (
            StatusCode::CONFLICT,
            Json(json!({
                "error": "stale sequence",
                "expected_after": stale.current,
            })),
        )
            .into_response(),
        Err(UpsertError::Identity) => error(StatusCode::FORBIDDEN, "identity mismatch"),
        Err(UpsertError::Storage) => error(StatusCode::INTERNAL_SERVER_ERROR, "storage"),
    }
}

fn accept_announce(
    state: &AppState,
    body: &AnnounceRequest,
    offline: bool,
) -> Result<RecordWrite, Response> {
    if body.peer_id.len() > 128
        || body.public_key_hex.len() > 512
        || body.signature_hex.len() > 256
        || body.sequence == 0
    {
        return Err(error(StatusCode::BAD_REQUEST, "invalid request"));
    }
    valid_network_id(&body.network_id)
        .map_err(|_| error(StatusCode::BAD_REQUEST, "invalid request"))?;
    valid_version(&body.software_version)
        .map_err(|_| error(StatusCode::BAD_REQUEST, "invalid request"))?;
    let kind = NodeType::parse(&body.node_type)
        .ok_or_else(|| error(StatusCode::BAD_REQUEST, "invalid request"))?;
    let status = NodeStatus::parse(&body.status)
        .ok_or_else(|| error(StatusCode::BAD_REQUEST, "invalid request"))?;
    if offline && status != NodeStatus::Offline {
        return Err(error(StatusCode::BAD_REQUEST, "invalid request"));
    }
    if body.ttl_secs < state.min_ttl_secs || body.ttl_secs > state.max_ttl_secs {
        return Err(error(StatusCode::BAD_REQUEST, "invalid request"));
    }
    let now = unix_now();
    if body.issued_at + 120 < now || body.issued_at > now.saturating_add(120) {
        return Err(error(StatusCode::BAD_REQUEST, "invalid request"));
    }
    if body.expires_at != body.issued_at.saturating_add(body.ttl_secs) {
        return Err(error(StatusCode::BAD_REQUEST, "invalid request"));
    }
    let peer = parse_peer_id(&body.peer_id)
        .map_err(|_| error(StatusCode::BAD_REQUEST, "invalid identity"))?;
    let fields = SignedFields {
        peer_id: &body.peer_id,
        network_id: &body.network_id,
        chain_id: body.chain_id,
        node_type: kind.as_str(),
        p2p_multiaddr: &dash(&body.p2p_multiaddr),
        rpc_public_url: &dash(&body.rpc_public_url),
        service_url: &dash(&body.service_url),
        status: status.as_str(),
        software_version: &body.software_version,
        issued_at: body.issued_at,
        expires_at: body.expires_at,
        sequence: body.sequence,
        ttl_secs: body.ttl_secs,
    };
    verify_fields(&body.public_key_hex, &fields, &body.signature_hex)
        .map_err(|_| error(StatusCode::FORBIDDEN, "invalid signature"))?;
    if let Some(addr) = body
        .p2p_multiaddr
        .as_deref()
        .filter(|value| !value.is_empty() && *value != "-")
    {
        validate_p2p_multiaddr(addr, &peer, state.allow_private).map_err(map_validate)?;
    }
    let screened = screen_endpoints(
        kind,
        body.rpc_public_url.as_deref(),
        body.service_url.as_deref(),
        state.allow_private,
        &state.rpc_hosts,
    )
    .map_err(map_validate)?;
    Ok(RecordWrite {
        peer_id: body.peer_id.clone(),
        network_id: body.network_id.clone(),
        chain_id: body.chain_id,
        p2p_multiaddr: body
            .p2p_multiaddr
            .clone()
            .filter(|value| !value.is_empty() && value != "-"),
        rpc_public_url: screened.rpc_public_url,
        service_url: screened.service_url,
        node_type: kind.as_str().to_string(),
        status: status.as_str().to_string(),
        software_version: body.software_version.clone(),
        public_key_hex: body.public_key_hex.clone(),
        sequence: body.sequence,
        ttl_secs: body.ttl_secs,
        primary_rpc: screened.primary_rpc,
        produces_blocks: screened.produces_blocks,
    })
}

fn map_validate(err: ValidateError) -> Response {
    let message = match err {
        ValidateError::PrivateEndpoint => "private endpoint",
        ValidateError::Policy => "announce policy",
        ValidateError::Identity => "invalid identity",
        ValidateError::InvalidAddress | ValidateError::InvalidUrl | ValidateError::Request => {
            "invalid address"
        }
    };
    error(StatusCode::BAD_REQUEST, message)
}

async fn admin_expire(
    State(state): State<AppState>,
    client: ClientAddr,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = authorize(&state, &client, &headers) {
        return response;
    }
    let Ok(mut guard) = state.store.lock() else {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "storage");
    };
    match guard.expire(unix_now()) {
        Ok(expired) => Json(json!({"expired": expired})).into_response(),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "storage"),
    }
}

async fn admin_delete(
    State(state): State<AppState>,
    client: ClientAddr,
    headers: HeaderMap,
    Path(peer_id): Path<String>,
) -> Response {
    if let Err(response) = authorize(&state, &client, &headers) {
        return response;
    }
    if peer_id.len() > 128 || peer_id.contains('/') {
        return error(StatusCode::BAD_REQUEST, "invalid request");
    }
    let Ok(mut guard) = state.store.lock() else {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "storage");
    };
    match guard.delete(&peer_id) {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => error(StatusCode::NOT_FOUND, "not found"),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "storage"),
    }
}

fn authorize(state: &AppState, client: &ClientAddr, headers: &HeaderMap) -> Result<(), Response> {
    if !client.0.ip().is_loopback() {
        return Err(error(StatusCode::NOT_FOUND, "not found"));
    }
    let Some(expected) = state.admin_token.as_deref() else {
        return Err(error(StatusCode::NOT_FOUND, "not found"));
    };
    let Some(header) = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
    else {
        return Err(error(StatusCode::UNAUTHORIZED, "unauthorized"));
    };
    let Some(got) = header.strip_prefix("Bearer ") else {
        return Err(error(StatusCode::UNAUTHORIZED, "unauthorized"));
    };
    if const_time_eq(got.as_bytes(), expected.as_bytes()) {
        Ok(())
    } else {
        Err(error(StatusCode::UNAUTHORIZED, "unauthorized"))
    }
}

fn limit(state: &AppState, client: &ClientAddr) -> Result<(), Response> {
    if state.rate_per_window == 0 {
        return Ok(());
    }
    let key = client.0.ip().to_string();
    let mut hits = state
        .hits
        .lock()
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "storage"))?;
    let now = Instant::now();
    let entry = hits.entry(key).or_insert((now, 0));
    if now.duration_since(entry.0) >= state.rate_window {
        *entry = (now, 0);
    }
    if entry.1 >= state.rate_per_window {
        return Err(error(StatusCode::TOO_MANY_REQUESTS, "rate limited"));
    }
    entry.1 = entry.1.saturating_add(1);
    Ok(())
}

fn error(status: StatusCode, message: &'static str) -> Response {
    (status, Json(json!({"error": message}))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sign::{public_key_hex, sign_fields};
    use crate::util::unix_now;
    use axum::body::Body;
    use http_body_util::BodyExt;
    use libp2p::identity::Keypair;
    use serde_json::Value;
    use tower::ServiceExt;

    fn app() -> Router {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let durable = std::env::temp_dir().join(format!(
            "rsc-registry-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&durable).unwrap();
        let server = RegistryServer::open(RegistryConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            db_path: durable.join("nodes.sqlite"),
            admin_token: Some("test-admin-token".into()),
            allow_private: false,
            rpc_hosts: vec!["rpc.reeskova-chain.xyz".into()],
            min_ttl_secs: 15,
            max_ttl_secs: 120,
            rate_per_window: 100,
            rate_window: Duration::from_secs(60),
        })
        .unwrap();
        server.router()
    }

    fn card(key: &Keypair, status: &str, sequence: u64, rpc: &str) -> Value {
        let peer = libp2p::PeerId::from(key.public()).to_string();
        let issued = unix_now();
        let ttl = 45u64;
        let fields = SignedFields {
            peer_id: &peer,
            network_id: "rsc-public",
            chain_id: 3,
            node_type: "full",
            p2p_multiaddr: "-",
            rpc_public_url: rpc,
            service_url: "-",
            status,
            software_version: "0.1.0",
            issued_at: issued,
            expires_at: issued + ttl,
            sequence,
            ttl_secs: ttl,
        };
        json!({
            "peer_id": peer,
            "public_key_hex": public_key_hex(key).unwrap(),
            "network_id": "rsc-public",
            "chain_id": 3,
            "node_type": "full",
            "p2p_multiaddr": null,
            "rpc_public_url": rpc,
            "service_url": null,
            "status": status,
            "software_version": "0.1.0",
            "issued_at": issued,
            "expires_at": issued + ttl,
            "sequence": sequence,
            "ttl_secs": ttl,
            "signature_hex": sign_fields(key, &fields).unwrap(),
        })
    }

    async fn send(app: &Router, request: axum::http::Request<Body>) -> (StatusCode, Value) {
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        (status, value)
    }

    fn post_json(path: &str, body: &Value) -> axum::http::Request<Body> {
        axum::http::Request::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(body).unwrap()))
            .unwrap()
    }

    #[tokio::test]
    async fn announce_heartbeat_offline_and_reject_forgery() {
        let app = app();
        let key = Keypair::generate_ed25519();
        let peer = libp2p::PeerId::from(key.public()).to_string();
        let rpc = "https://rpc.reeskova-chain.xyz/rpc";
        let (status, body) = send(&app, post_json("/v1/announce", &card(&key, "ONLINE", 1, rpc))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ONLINE");

        let listed = send(
            &app,
            axum::http::Request::builder()
                .uri(format!("/v1/nodes?network_id=rsc-public&chain_id=3"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(listed.0, StatusCode::OK);
        assert_eq!(listed.1["nodes"][0]["peer_id"], peer);
        assert_eq!(listed.1["nodes"][0]["primary_rpc"], true);
        assert!(listed.1["nodes"][0]["produces_blocks"].as_bool().unwrap());

        let (status, _) = send(&app, post_json("/v1/heartbeat", &card(&key, "ONLINE", 2, rpc))).await;
        assert_eq!(status, StatusCode::OK);
        let (status, stale) = send(&app, post_json("/v1/heartbeat", &card(&key, "ONLINE", 2, rpc))).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(stale["error"], "stale sequence");
        assert!(stale["expected_after"].is_number());

        let mut forged = card(&key, "ONLINE", 3, rpc);
        forged["rpc_public_url"] = json!("https://evil.example/rpc");
        let (status, _) = send(&app, post_json("/v1/announce", &forged)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        let other = Keypair::generate_ed25519();
        let mut stolen = card(&other, "ONLINE", 1, rpc);
        stolen["peer_id"] = json!(peer);
        let (status, _) = send(&app, post_json("/v1/announce", &stolen)).await;
        assert!(status == StatusCode::FORBIDDEN || status == StatusCode::BAD_REQUEST);

        let private_body = card(&key, "ONLINE", 4, "http://127.0.0.1:8545/rpc");
        let (status, payload) = send(&app, post_json("/v1/announce", &private_body)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(payload["error"], "private endpoint");

        let mobile_key = Keypair::generate_ed25519();
        let mobile_peer = libp2p::PeerId::from(mobile_key.public()).to_string();
        let issued = unix_now();
        let fields = SignedFields {
            peer_id: &mobile_peer,
            network_id: "rsc-public",
            chain_id: 3,
            node_type: "mobile",
            p2p_multiaddr: "-",
            rpc_public_url: rpc,
            service_url: "-",
            status: "ONLINE",
            software_version: "0.1.0",
            issued_at: issued,
            expires_at: issued + 45,
            sequence: 1,
            ttl_secs: 45,
        };
        let mobile = json!({
            "peer_id": mobile_peer,
            "public_key_hex": public_key_hex(&mobile_key).unwrap(),
            "network_id": "rsc-public",
            "chain_id": 3,
            "node_type": "mobile",
            "status": "ONLINE",
            "software_version": "0.1.0",
            "issued_at": issued,
            "expires_at": issued + 45,
            "sequence": 1,
            "ttl_secs": 45,
            "signature_hex": sign_fields(&mobile_key, &fields).unwrap(),
            "rpc_public_url": rpc,
        });
        let (status, payload) = send(&app, post_json("/v1/announce", &mobile)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(payload["error"], "announce policy");

        let (status, _) = send(&app, post_json("/v1/offline", &card(&key, "OFFLINE", 5, rpc))).await;
        assert_eq!(status, StatusCode::OK);
        let (status, node) = send(
            &app,
            axum::http::Request::builder()
                .uri(format!("/v1/nodes/{peer}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(node["status"], "OFFLINE");
        let rendered = node.to_string();
        assert!(!rendered.contains("test-admin-token"));
    }

    #[tokio::test]
    async fn admin_requires_loopback_and_token() {
        let app = app();
        let mut remote = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/admin/expire")
            .header("authorization", "Bearer test-admin-token")
            .body(Body::empty())
            .unwrap();
        remote
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([8, 8, 8, 8], 9))));
        let (status, _) = send(&app, remote).await;
        assert_eq!(status, StatusCode::NOT_FOUND);

        let missing = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/admin/expire")
            .body(Body::empty())
            .unwrap();
        let (status, _) = send(&app, missing).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        let ok = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/admin/expire")
            .header("authorization", "Bearer test-admin-token")
            .body(Body::empty())
            .unwrap();
        let (status, body) = send(&app, ok).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body["expired"].is_number());
        assert!(!body.to_string().contains("test-admin-token"));
    }
}
