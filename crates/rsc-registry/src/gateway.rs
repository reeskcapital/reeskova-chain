//! Gateway HTTP. Reenvía método y cuerpo sin reescribir JSON-RPC.
//!
//! El backend principal es el que configura el operador. Un failover solo
//! entra si antes se leyó el `rsc_chainId` del primario y el suplente
//! responde el mismo valor. No se reparte carga entre cadenas distintas.

use crate::util::sanitize_log;
use crate::validate::validate_operator_backend;
use axum::body::Bytes;
use axum::extract::{ConnectInfo, FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Router;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tokio::net::TcpListener;

#[derive(Clone, Debug)]
pub struct GatewayConfig {
    pub rpc_backends: Vec<String>,
    pub mobile_backend: Option<String>,
    pub community_backend: Option<String>,
    pub timeout: Duration,
    pub health_ttl: Duration,
    pub rate_per_window: u32,
    pub rate_window: Duration,
    pub max_body: usize,
}

struct HealthCache {
    checked: Option<Instant>,
    primary_ok: bool,
    chain_id: Option<String>,
}

#[derive(Clone)]
struct Gw {
    config: GatewayConfig,
    client: reqwest::Client,
    health: std::sync::Arc<Mutex<HealthCache>>,
    hits: std::sync::Arc<Mutex<HashMap<String, (Instant, u32)>>>,
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

pub fn validate_gateway(config: &GatewayConfig, self_port: Option<u16>) -> Result<(), String> {
    if config.rpc_backends.is_empty() {
        return Err("falta backend rpc".into());
    }
    for backend in config
        .rpc_backends
        .iter()
        .chain(config.mobile_backend.iter())
        .chain(config.community_backend.iter())
    {
        validate_operator_backend(backend, self_port).map_err(|_| "backend inválido".to_string())?;
    }
    Ok(())
}

pub fn router(config: GatewayConfig) -> Result<Router, String> {
    validate_gateway(&config, None)?;
    let client = reqwest::Client::builder()
        .timeout(config.timeout)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "cliente".to_string())?;
    let state = Gw {
        config,
        client,
        health: std::sync::Arc::new(Mutex::new(HealthCache {
            checked: None,
            primary_ok: false,
            chain_id: None,
        })),
        hits: std::sync::Arc::new(Mutex::new(HashMap::new())),
    };
    Ok(Router::new().fallback(proxy).with_state(state))
}

pub async fn serve(bind: SocketAddr, config: GatewayConfig) -> Result<(), String> {
    if !bind.ip().is_loopback() && std::env::var("RSC_GATEWAY_ALLOW_PUBLIC_BIND").ok().as_deref() != Some("1")
    {
        return Err("el bind debe ser loopback".into());
    }
    validate_gateway(&config, Some(bind.port()))?;
    let listener = TcpListener::bind(bind).await.map_err(|_| "bind failed".to_string())?;
    let app = router(config)?.into_make_service_with_connect_info::<SocketAddr>();
    eprintln!("gateway bind {bind}");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .map_err(|_| "serve".to_string())
}

async fn proxy(
    State(state): State<Gw>,
    client: ClientAddr,
    req: Request,
) -> Response {
    if let Err(response) = limit(&state, &client) {
        return response;
    }
    let path = req.uri().path().to_string();
    if path.starts_with("/admin") || path.starts_with("/v1/admin") || path.contains("://") || path.contains("..")
    {
        return error(StatusCode::NOT_FOUND, "not found");
    }
    if req.headers().contains_key("x-rsc-gateway") {
        return error(StatusCode::LOOP_DETECTED, "proxy loop");
    }
    if !matches!(
        *req.method(),
        Method::GET | Method::POST | Method::HEAD | Method::OPTIONS
    ) {
        return error(StatusCode::METHOD_NOT_ALLOWED, "method not allowed");
    }
    let kind = match route_kind(&path) {
        Some(kind) => kind,
        None => return error(StatusCode::NOT_FOUND, "not found"),
    };
    let (parts, body) = req.into_parts();
    let bytes = match axum::body::to_bytes(body, state.config.max_body).await {
        Ok(bytes) => bytes,
        Err(_) => return error(StatusCode::PAYLOAD_TOO_LARGE, "request too large"),
    };
    if path == "/rpc" && blocks_admin_method(&bytes) {
        return error(StatusCode::FORBIDDEN, "unauthorized");
    }
    let backend = match kind {
        RouteKind::Rpc => match pick_rpc(&state).await {
            Some(backend) => backend,
            None => return error(StatusCode::SERVICE_UNAVAILABLE, "backend unavailable"),
        },
        RouteKind::Mobile => match state.config.mobile_backend.clone() {
            Some(backend) => backend,
            None => return error(StatusCode::SERVICE_UNAVAILABLE, "backend unavailable"),
        },
        RouteKind::Community => match state.config.community_backend.clone() {
            Some(backend) => backend,
            None => return error(StatusCode::SERVICE_UNAVAILABLE, "backend unavailable"),
        },
    };
    if !healthy_enough(&state, &backend, kind).await {
        return error(StatusCode::SERVICE_UNAVAILABLE, "backend unavailable");
    }
    forward(&state, &backend, &path, parts.uri.query(), &parts.method, &parts.headers, bytes).await
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RouteKind {
    Rpc,
    Mobile,
    Community,
}

fn route_kind(path: &str) -> Option<RouteKind> {
    if path == "/rpc" || path == "/health" || path == "/network" || path == "/swap" || path.starts_with("/swap/")
        || path == "/wallet" || path.starts_with("/wallet/")
    {
        Some(RouteKind::Rpc)
    } else if path == "/mobile" || path.starts_with("/mobile/") {
        Some(RouteKind::Mobile)
    } else if path == "/community" || path.starts_with("/community/") {
        Some(RouteKind::Community)
    } else {
        None
    }
}

async fn healthy_enough(state: &Gw, backend: &str, kind: RouteKind) -> bool {
    match kind {
        RouteKind::Rpc => refresh_rpc(state).await,
        RouteKind::Mobile => probe_ready(&state.client, backend, "/mobile").await,
        RouteKind::Community => probe_ready(&state.client, backend, "/community").await,
    }
}

async fn probe_ready(client: &reqwest::Client, backend: &str, path: &str) -> bool {
    let url = format!("{}{}", backend.trim_end_matches('/'), path);
    match client
        .get(url)
        .header("x-rsc-gateway", "1")
        .send()
        .await
    {
        Ok(response) => response.status().is_success(),
        Err(_) => false,
    }
}

async fn pick_rpc(state: &Gw) -> Option<String> {
    if !refresh_rpc(state).await {
        return None;
    }
    let primary_ok = state.health.lock().ok()?.primary_ok;
    if primary_ok {
        return state.config.rpc_backends.first().cloned();
    }
    let expected = state.health.lock().ok()?.chain_id.clone()?;
    for backend in state.config.rpc_backends.iter().skip(1) {
        if probe_health(&state.client, backend).await {
            if fetch_chain_id(&state.client, backend).await.as_deref() == Some(expected.as_str()) {
                return Some(backend.clone());
            }
            eprintln!("gateway failover rechazado: chain id distinta");
        }
    }
    None
}

async fn refresh_rpc(state: &Gw) -> bool {
    let fresh = state
        .health
        .lock()
        .ok()
        .and_then(|cache| cache.checked)
        .map(|checked| checked.elapsed() < state.config.health_ttl && !state.config.health_ttl.is_zero())
        .unwrap_or(false);
    if fresh {
        return state
            .health
            .lock()
            .map(|cache| cache.primary_ok || (state.config.rpc_backends.len() > 1 && cache.chain_id.is_some()))
            .unwrap_or(false);
    }
    let Some(primary) = state.config.rpc_backends.first().cloned() else {
        return false;
    };
    let ok = probe_health(&state.client, &primary).await;
    let chain_id = if ok {
        fetch_chain_id(&state.client, &primary).await
    } else {
        None
    };
    if let Ok(mut cache) = state.health.lock() {
        if chain_id.is_some() {
            cache.chain_id = chain_id;
        }
        cache.primary_ok = ok;
        cache.checked = Some(Instant::now());
        let failover = state.config.rpc_backends.len() > 1 && cache.chain_id.is_some();
        return ok || failover;
    }
    false
}

async fn probe_health(client: &reqwest::Client, backend: &str) -> bool {
    let url = format!("{}/health", backend.trim_end_matches('/'));
    let Ok(response) = client
        .get(url)
        .header("x-rsc-gateway", "1")
        .send()
        .await
    else {
        return false;
    };
    if !response.status().is_success() {
        return false;
    }
    let Ok(bytes) = response.bytes().await else {
        return false;
    };
    serde_json::from_slice::<Value>(&bytes)
        .ok()
        .and_then(|value| value.get("status")?.as_str().map(|status| status == "ok"))
        .unwrap_or(false)
}

async fn fetch_chain_id(client: &reqwest::Client, backend: &str) -> Option<String> {
    let url = format!("{}/rpc", backend.trim_end_matches('/'));
    let response = client
        .post(url)
        .header("content-type", "application/json")
        .header("x-rsc-gateway", "1")
        .body(r#"{"jsonrpc":"2.0","id":1,"method":"rsc_chainId","params":[]}"#)
        .send()
        .await
        .ok()?;
    if !response.status().is_success() {
        return None;
    }
    let bytes = response.bytes().await.ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    value.get("result")?.as_str().map(str::to_string)
}

async fn forward(
    state: &Gw,
    backend: &str,
    path: &str,
    query: Option<&str>,
    method: &Method,
    headers: &HeaderMap,
    body: Bytes,
) -> Response {
    let mut url = format!("{}{}", backend.trim_end_matches('/'), path);
    if let Some(query) = query {
        if query.contains("token=") || query.contains("://") {
            return error(StatusCode::BAD_REQUEST, "invalid request");
        }
        url.push('?');
        url.push_str(query);
    }
    let method = method.clone();
    let mut builder = state
        .client
        .request(method, &url)
        .header("x-rsc-gateway", "1")
        .body(body.to_vec());
    if let Some(value) = headers.get(axum::http::header::CONTENT_TYPE) {
        builder = builder.header(axum::http::header::CONTENT_TYPE, value);
    }
    if let Some(value) = headers.get(axum::http::header::ACCEPT) {
        builder = builder.header(axum::http::header::ACCEPT, value);
    }
    match builder.send().await {
        Ok(response) => {
            let status = StatusCode::from_u16(response.status().as_u16())
                .unwrap_or(StatusCode::BAD_GATEWAY);
            let content_type = response
                .headers()
                .get(axum::http::header::CONTENT_TYPE)
                .cloned();
            match response.bytes().await {
                Ok(bytes) => {
                    let mut out = Response::new(axum::body::Body::from(bytes));
                    *out.status_mut() = status;
                    if let Some(content_type) = content_type {
                        if let Ok(value) = HeaderValue::from_bytes(content_type.as_bytes()) {
                            out.headers_mut().insert(axum::http::header::CONTENT_TYPE, value);
                        }
                    }
                    out
                }
                Err(_) => {
                    eprintln!("gateway upstream error {}", sanitize_log(backend));
                    error(StatusCode::SERVICE_UNAVAILABLE, "backend unavailable")
                }
            }
        }
        Err(_) => {
            eprintln!("gateway upstream unavailable {}", sanitize_log(backend));
            error(StatusCode::SERVICE_UNAVAILABLE, "backend unavailable")
        }
    }
}

fn blocks_admin_method(body: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    let admin = |value: &Value| {
        value
            .get("method")
            .and_then(Value::as_str)
            .map(|method| method.starts_with("rsc_admin"))
            .unwrap_or(false)
    };
    if let Some(batch) = value.as_array() {
        batch.iter().any(admin)
    } else {
        admin(&value)
    }
}

fn limit(state: &Gw, client: &ClientAddr) -> Result<(), Response> {
    if state.config.rate_per_window == 0 {
        return Ok(());
    }
    let mut hits = state
        .hits
        .lock()
        .map_err(|_| error(StatusCode::INTERNAL_SERVER_ERROR, "storage"))?;
    let now = Instant::now();
    let entry = hits.entry(client.0.ip().to_string()).or_insert((now, 0));
    if now.duration_since(entry.0) >= state.config.rate_window {
        *entry = (now, 0);
    }
    if entry.1 >= state.config.rate_per_window {
        return Err(error(StatusCode::TOO_MANY_REQUESTS, "rate limited"));
    }
    entry.1 = entry.1.saturating_add(1);
    Ok(())
}

fn error(status: StatusCode, message: &'static str) -> Response {
    (status, axum::Json(json!({"error": message}))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::{get, post};
    use axum::Json;
    use std::sync::atomic::{AtomicU32, Ordering};

    async fn serve_app(app: Router) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
                .await
                .unwrap();
        });
        addr
    }

    fn rpc_app(chain: &'static str, fail_health_after: u32) -> Router {
        let hits = std::sync::Arc::new(AtomicU32::new(0));
        let hits_health = std::sync::Arc::clone(&hits);
        Router::new()
            .route(
                "/health",
                get(move || {
                    let hits = std::sync::Arc::clone(&hits_health);
                    async move {
                        let n = hits.fetch_add(1, Ordering::SeqCst);
                        if fail_health_after > 0 && n >= fail_health_after {
                            (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"status":"down"})))
                        } else {
                            (StatusCode::OK, Json(json!({"status":"ok"})))
                        }
                    }
                }),
            )
            .route(
                "/rpc",
                post(move |body: Bytes| async move {
                    if body.windows(11).any(|window| window == b"rsc_chainId") {
                        Json(json!({"jsonrpc":"2.0","id":1,"result": chain})).into_response()
                    } else if body.windows(9).any(|window| window == b"rsc_admin") {
                        Json(json!({"jsonrpc":"2.0","id":1,"result":"should-not-pass"})).into_response()
                    } else {
                        (
                            StatusCode::OK,
                            [(axum::http::header::CONTENT_TYPE, "application/json")],
                            body,
                        )
                            .into_response()
                    }
                }),
            )
    }

    #[tokio::test]
    async fn proxies_json_rpc_unchanged_and_blocks_admin_and_loops() {
        let backend = serve_app(rpc_app("3", 0)).await;
        let gateway = serve_app(
            router(GatewayConfig {
                rpc_backends: vec![format!("http://{backend}")],
                mobile_backend: None,
                community_backend: None,
                timeout: Duration::from_secs(2),
                health_ttl: Duration::from_secs(30),
                rate_per_window: 20,
                rate_window: Duration::from_secs(60),
                max_body: 1024 * 1024,
            })
            .unwrap(),
        )
        .await;
        let client = reqwest::Client::new();
        let payload = r#"{"jsonrpc":"2.0","id":9,"method":"rsc_getStatus","params":[]}"#;
        let response = client
            .post(format!("http://{gateway}/rpc"))
            .header("content-type", "application/json")
            .body(payload)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let bytes = response.bytes().await.unwrap();
        assert_eq!(bytes.as_ref(), payload.as_bytes());

        let admin = client
            .post(format!("http://{gateway}/rpc"))
            .header("content-type", "application/json")
            .body(r#"{"jsonrpc":"2.0","id":1,"method":"rsc_adminCreditAsset","params":{}}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(admin.status(), 403);
        let admin_body = admin.text().await.unwrap();
        assert!(!admin_body.contains("should-not-pass"));
        assert!(!admin_body.contains("\"result\""));

        let looped = client
            .get(format!("http://{gateway}/health"))
            .header("x-rsc-gateway", "1")
            .send()
            .await
            .unwrap();
        assert_eq!(looped.status(), 508);

        let hidden = client
            .get(format!("http://{gateway}/v1/admin/expire"))
            .send()
            .await
            .unwrap();
        assert_eq!(hidden.status(), 404);
    }

    #[tokio::test]
    async fn unhealthy_primary_fails_over_only_with_same_chain() {
        let primary = serve_app(rpc_app("3", 1)).await;
        let same = serve_app(rpc_app("3", 0)).await;
        let gateway = serve_app(
            router(GatewayConfig {
                rpc_backends: vec![format!("http://{primary}"), format!("http://{same}")],
                mobile_backend: None,
                community_backend: None,
                timeout: Duration::from_secs(2),
                health_ttl: Duration::ZERO,
                rate_per_window: 50,
                rate_window: Duration::from_secs(60),
                max_body: 1024 * 1024,
            })
            .unwrap(),
        )
        .await;
        let client = reqwest::Client::new();
        let payload = r#"{"jsonrpc":"2.0","id":9,"method":"rsc_getStatus","params":[]}"#;
        let first = client
            .post(format!("http://{gateway}/rpc"))
            .body(payload)
            .send()
            .await
            .unwrap();
        assert_eq!(first.status(), 200);
        assert_eq!(first.bytes().await.unwrap().as_ref(), payload.as_bytes());

        let second = client
            .post(format!("http://{gateway}/rpc"))
            .body(payload)
            .send()
            .await
            .unwrap();
        assert_eq!(second.status(), 200);
        assert_eq!(second.bytes().await.unwrap().as_ref(), payload.as_bytes());
    }

    #[tokio::test]
    async fn refuses_failover_when_chain_id_differs() {
        let primary = serve_app(rpc_app("3", 1)).await;
        let other = serve_app(rpc_app("9", 0)).await;
        let gateway = serve_app(
            router(GatewayConfig {
                rpc_backends: vec![format!("http://{primary}"), format!("http://{other}")],
                mobile_backend: None,
                community_backend: None,
                timeout: Duration::from_secs(2),
                health_ttl: Duration::ZERO,
                rate_per_window: 50,
                rate_window: Duration::from_secs(60),
                max_body: 1024 * 1024,
            })
            .unwrap(),
        )
        .await;
        let client = reqwest::Client::new();
        let payload = r#"{"jsonrpc":"2.0","id":9,"method":"rsc_getStatus","params":[]}"#;
        let first = client
            .post(format!("http://{gateway}/rpc"))
            .body(payload)
            .send()
            .await
            .unwrap();
        assert_eq!(first.status(), 200);
        let second = client
            .post(format!("http://{gateway}/rpc"))
            .body(payload)
            .send()
            .await
            .unwrap();
        assert_eq!(second.status(), 503);
        let text = second.text().await.unwrap();
        assert!(text.contains("backend unavailable"));
        assert!(!text.contains("\"result\""));
    }

    #[test]
    fn rejects_metadata_backend_and_self_port() {
        let config = GatewayConfig {
            rpc_backends: vec!["http://169.254.169.254".into()],
            mobile_backend: None,
            community_backend: None,
            timeout: Duration::from_secs(1),
            health_ttl: Duration::from_secs(1),
            rate_per_window: 1,
            rate_window: Duration::from_secs(60),
            max_body: 1024,
        };
        assert!(validate_gateway(&config, Some(8544)).is_err());
        let looped = GatewayConfig {
            rpc_backends: vec!["http://127.0.0.1:8544".into()],
            ..config
        };
        assert!(validate_gateway(&looped, Some(8544)).is_err());
    }

    #[tokio::test]
    async fn mobile_health_is_its_own_path() {
        let mobile = serve_app(
            Router::new().route("/mobile", get(|| async { Json(json!({"linked": false})) })),
        )
        .await;
        let down = "http://127.0.0.1:1";
        let gateway = serve_app(
            router(GatewayConfig {
                rpc_backends: vec![down.into()],
                mobile_backend: Some(format!("http://{mobile}")),
                community_backend: Some(down.into()),
                timeout: Duration::from_secs(2),
                health_ttl: Duration::from_secs(30),
                rate_per_window: 20,
                rate_window: Duration::from_secs(60),
                max_body: 1024 * 1024,
            })
            .unwrap(),
        )
        .await;
        let client = reqwest::Client::new();
        let ok = client
            .get(format!("http://{gateway}/mobile"))
            .send()
            .await
            .unwrap();
        assert_eq!(ok.status(), 200);
        let community = client
            .get(format!("http://{gateway}/community"))
            .send()
            .await
            .unwrap();
        assert_eq!(community.status(), 503);
        assert!(community.text().await.unwrap().contains("backend unavailable"));
    }
}
