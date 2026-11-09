//! Transporte HTTP de RSC-RPC v1.
//!
//! `POST /rpc` — JSON-RPC 2.0 (single o batch).
//! `GET /health` — liveness del proceso, no sync de cadena.
//! `GET /network` — directorio JSON de endpoints RPC conocidos (discovery).
//!
//! Política HTTP: una petición JSON-RPC bien formada responde `200` con
//! `result` o `error` en el cuerpo. Fallos de transporte (cuerpo enorme,
//! timeout, content-type) usan códigos HTTP. Los errores de aplicación no
//! cambian de status por método.

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Json;
use axum::Router;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;

use crate::methods::{dispatch_with_meta, RpcCallMeta};
use crate::{
    failure, parse_request, success, RpcConfig, RpcContext, RpcError, RpcId, RpcResponse,
    RpcServerError, JSONRPC_VERSION,
};

#[derive(Clone)]
struct AppState {
    ctx: RpcContext,
    config: RpcConfig,
}

/// Servidor HTTP JSON-RPC.
///
/// Construye el router, aplica límites/timeout/CORS y sirve `POST /rpc`,
/// `GET /health` y `GET /network`. El apagado es por señal del proceso, no por RPC.
pub struct RpcServer {
    context: RpcContext,
    config: RpcConfig,
}

impl RpcServer {
    pub fn new(context: RpcContext, config: RpcConfig) -> Self {
        Self { context, config }
    }

    pub fn router(&self) -> Router {
        build_router(self.context.clone(), self.config.clone())
    }

    /// Bind + serve con shutdown por señal. No existe RPC de apagado.
    pub async fn run(self) -> Result<(), RpcServerError> {
        let listener = TcpListener::bind(self.config.bind_address)
            .await
            .map_err(RpcServerError::Bind)?;
        axum::serve(listener, self.router())
            .with_graceful_shutdown(shutdown_signal())
            .await
            .map_err(RpcServerError::Serve)
    }
}

pub fn build_router(ctx: RpcContext, config: RpcConfig) -> Router {
    let max_body = config.max_body_size;
    let timeout = config.request_timeout;
    let cors = cors_layer(&config);
    let state = AppState { ctx, config };
    let mut router = Router::new()
        .route("/rpc", post(rpc_entry).get(method_not_allowed))
        .route("/health", get(health).post(method_not_allowed))
        .route("/network", get(network_directory).post(method_not_allowed))
        .route("/swap", get(swap_index))
        .route("/swap/pools", get(swap_pools))
        .route("/swap/pool", get(swap_pool))
        .route("/swap/assets", get(swap_assets))
        .route("/swap/balances", get(swap_balances))
        .route("/swap/balance", get(swap_balance))
        .route("/swap/position", get(swap_position))
        .route("/swap/quote", post(swap_quote))
        .route("/swap/liquidity", post(swap_liquidity))
        .route("/swap/submit", post(swap_submit))
        .route("/wallet", get(wallet_lookup))
        .route("/wallet/create", post(wallet_create))
        .route("/wallet/login", post(wallet_login))
        .layer(RequestBodyLimitLayer::new(max_body))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            timeout,
        ))
        .with_state(state);
    if let Some(layer) = cors {
        router = router.layer(layer);
    }
    router
}

fn cors_layer(config: &RpcConfig) -> Option<CorsLayer> {
    if config.allowed_origins.is_empty() {
        return None;
    }
    let origins: Result<Vec<_>, _> = config
        .allowed_origins
        .iter()
        .map(|o| o.parse::<axum::http::HeaderValue>())
        .collect();
    let origins = origins.ok()?;
    Some(
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(origins))
            .allow_methods([Method::POST, Method::GET])
            .allow_headers([header::CONTENT_TYPE]),
    )
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {}
        _ = term => {}
    }
}

/// Liveness: el proceso responde. No implica sync ni validador activo.
async fn health() -> impl IntoResponse {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        "{\"status\":\"ok\"}",
    )
}

/// Discovery HTTP: mismos endpoints que `rsc_getRpcEndpoints`.
async fn network_directory(State(state): State<AppState>) -> Response {
    match state.ctx.node.rpc_endpoints().await {
        Ok(endpoints) => {
            let body = match serde_json::to_string(&json!({ "endpoints": endpoints })) {
                Ok(body) => body,
                Err(_) => {
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
            };
            (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "application/json")],
                body,
            )
                .into_response()
        }
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}

async fn method_not_allowed() -> impl IntoResponse {
    StatusCode::METHOD_NOT_ALLOWED
}

/// API HTTP del swap (AMM). Misma cadena que `/rpc`, rutas propias para la app.
async fn swap_index() -> impl IntoResponse {
    Json(json!({
        "name": "rsc-swap",
        "pools": "GET /swap/pools",
        "pool": "GET /swap/pool?pool_id=",
        "assets": "GET /swap/assets",
        "balances": "GET /swap/balances?account=",
        "balance": "GET /swap/balance?account=&asset_id=",
        "position": "GET /swap/position?pool_id=&account=",
        "quote": "POST /swap/quote",
        "liquidity": "POST /swap/liquidity",
        "submit": "POST /swap/submit"
    }))
}

#[derive(Deserialize)]
struct PoolQuery {
    pool_id: String,
}

#[derive(Deserialize)]
struct AccountQuery {
    account: String,
}

#[derive(Deserialize)]
struct BalanceQuery {
    account: String,
    asset_id: String,
}

#[derive(Deserialize)]
struct PositionQuery {
    pool_id: String,
    account: String,
}

async fn swap_pools(State(state): State<AppState>) -> Response {
    swap_call(&state, "rsc_getPools", json!({})).await
}

async fn swap_assets(State(state): State<AppState>) -> Response {
    swap_call(&state, "rsc_getAssets", json!({})).await
}

async fn swap_pool(State(state): State<AppState>, Query(q): Query<PoolQuery>) -> Response {
    swap_call(&state, "rsc_getPool", json!({ "pool_id": q.pool_id })).await
}

async fn swap_balances(State(state): State<AppState>, Query(q): Query<AccountQuery>) -> Response {
    swap_call(&state, "rsc_getAssetBalances", json!({ "account": q.account })).await
}

async fn swap_balance(State(state): State<AppState>, Query(q): Query<BalanceQuery>) -> Response {
    swap_call(
        &state,
        "rsc_getAssetBalance",
        json!({ "account": q.account, "asset_id": q.asset_id }),
    )
    .await
}

async fn swap_position(State(state): State<AppState>, Query(q): Query<PositionQuery>) -> Response {
    swap_call(
        &state,
        "rsc_getLpPosition",
        json!({ "pool_id": q.pool_id, "account": q.account }),
    )
    .await
}

async fn swap_quote(State(state): State<AppState>, Json(body): Json<Value>) -> Response {
    swap_call(&state, "rsc_quoteSwap", body).await
}

async fn swap_liquidity(State(state): State<AppState>, Json(body): Json<Value>) -> Response {
    swap_call(&state, "rsc_quoteAddLiquidity", body).await
}

async fn swap_submit(State(state): State<AppState>, Json(body): Json<Value>) -> Response {
    swap_call(&state, "rsc_sendTransaction", body).await
}

#[derive(Deserialize)]
struct NameQuery {
    name: String,
}

async fn wallet_lookup(State(state): State<AppState>, Query(q): Query<NameQuery>) -> Response {
    swap_call(&state, "rsc_lookupName", json!({ "name": q.name })).await
}

async fn wallet_create(State(state): State<AppState>, Json(body): Json<Value>) -> Response {
    swap_call(&state, "rsc_createNamedWallet", body).await
}

async fn wallet_login(State(state): State<AppState>, Json(body): Json<Value>) -> Response {
    swap_call(&state, "rsc_openWallet", body).await
}

async fn swap_call(state: &AppState, method: &str, params: Value) -> Response {
    match crate::methods::dispatch_with_meta(
        &state.ctx,
        method,
        Some(&params),
        &RpcCallMeta::default(),
    )
    .await
    {
        Ok(result) => (StatusCode::OK, Json(json!({ "ok": true, "result": result }))).into_response(),
        Err(err) => {
            let status = match err {
                RpcError::NotFound => StatusCode::NOT_FOUND,
                RpcError::InvalidParams | RpcError::ParseError => StatusCode::BAD_REQUEST,
                RpcError::ServiceUnavailable => StatusCode::SERVICE_UNAVAILABLE,
                _ => StatusCode::BAD_REQUEST,
            };
            (status, Json(failure(crate::RpcId::Null, err))).into_response()
        }
    }
}

async fn rpc_entry(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    if !content_type_ok(&headers) {
        return StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response();
    }
    let meta = RpcCallMeta {
        admin_token: extract_admin_token(&headers),
    };
    match tokio::time::timeout(
        state.config.request_timeout,
        handle_rpc(&state, &meta, body),
    )
    .await
    {
        Ok(resp) => resp,
        Err(_) => json_rpc_http(failure(RpcId::Null, RpcError::Timeout)),
    }
}

fn extract_admin_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-admin-token")
        .or_else(|| headers.get("x-rsc-admin-token"))
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_owned())
}

fn content_type_ok(headers: &HeaderMap) -> bool {
    match headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
    {
        None => true,
        Some(v) => {
            let base = v.split(';').next().unwrap_or(v).trim();
            base.eq_ignore_ascii_case("application/json")
        }
    }
}

async fn handle_rpc(state: &AppState, meta: &RpcCallMeta, body: Bytes) -> Response {
    if body.len() > state.config.max_body_size {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            json_error_bytes(RpcError::RequestTooLarge),
        )
            .into_response();
    }
    let value: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return json_rpc_http(failure(RpcId::Null, RpcError::ParseError)),
    };
    match value {
        Value::Array(items) => handle_batch(state, meta, items).await,
        other => handle_one(state, meta, other).await,
    }
}

async fn handle_one(state: &AppState, meta: &RpcCallMeta, value: Value) -> Response {
    match execute(state, meta, value).await {
        Outcome::Response(r) => json_rpc_http(r),
        Outcome::Notification => empty_ok(),
    }
}

async fn handle_batch(state: &AppState, meta: &RpcCallMeta, items: Vec<Value>) -> Response {
    if items.is_empty() {
        return json_rpc_http(failure(RpcId::Null, RpcError::InvalidRequest));
    }
    if items.len() > state.config.max_batch_size {
        return json_rpc_http(failure(RpcId::Null, RpcError::RequestTooLarge));
    }
    let mut out = Vec::new();
    for item in items {
        match execute(state, meta, item).await {
            Outcome::Response(r) => out.push(r),
            Outcome::Notification => {}
        }
    }
    if out.is_empty() {
        return empty_ok();
    }
    let values: Vec<Value> = out
        .into_iter()
        .filter_map(|r| serde_json::to_value(r).ok())
        .collect();
    json_bytes(StatusCode::OK, &Value::Array(values))
}

enum Outcome {
    Response(RpcResponse),
    Notification,
}

async fn execute(state: &AppState, meta: &RpcCallMeta, value: Value) -> Outcome {
    let req = match parse_request(&value) {
        Ok(r) => r,
        Err(e) => {
            let has_id = value.as_object().is_some_and(|o| o.contains_key("id"));
            if !has_id && value.is_object() {
                return Outcome::Notification;
            }
            return Outcome::Response(failure(extract_id(&value).unwrap_or(RpcId::Null), e));
        }
    };
    match req.id {
        None => {
            let _ = dispatch_with_meta(&state.ctx, &req.method, req.params.as_ref(), meta).await;
            Outcome::Notification
        }
        Some(id) => {
            match dispatch_with_meta(&state.ctx, &req.method, req.params.as_ref(), meta).await {
                Ok(result) => Outcome::Response(success(id, result)),
                Err(err) => Outcome::Response(failure(id, err)),
            }
        }
    }
}

fn extract_id(value: &Value) -> Option<RpcId> {
    match value.get("id")? {
        Value::Null => Some(RpcId::Null),
        Value::String(s) => Some(RpcId::String(s.clone())),
        Value::Number(n) => Some(RpcId::Number(n.clone())),
        _ => None,
    }
}

fn json_rpc_http(resp: RpcResponse) -> Response {
    match serde_json::to_value(&resp) {
        Ok(v) => json_bytes(StatusCode::OK, &v),
        Err(_) => json_bytes(
            StatusCode::OK,
            &json!({
                "jsonrpc": JSONRPC_VERSION,
                "id": null,
                "error": RpcError::InternalError.object(),
            }),
        ),
    }
}

fn json_error_bytes(err: RpcError) -> ([(header::HeaderName, &'static str); 1], String) {
    let body = json!({
        "jsonrpc": JSONRPC_VERSION,
        "id": null,
        "error": err.object(),
    });
    (
        [(header::CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
}

fn json_bytes(status: StatusCode, value: &Value) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "application/json")],
        value.to_string(),
    )
        .into_response()
}

fn empty_ok() -> Response {
    (StatusCode::OK, "").into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{ready_ctx, MockChain, MockConsensus, MockNode, MockState, MockTx};
    use crate::{RpcContext, ServiceError};
    use axum::body::{to_bytes, Body};
    use axum::http::Request;
    use serde_json::{json, Value};
    use std::sync::Arc;
    use std::time::Duration;
    use tower::ServiceExt;

    fn cfg() -> RpcConfig {
        RpcConfig {
            max_body_size: 2048,
            request_timeout: Duration::from_millis(200),
            max_batch_size: 4,
            allowed_origins: vec!["https://wallet.example".into()],
            ..RpcConfig::default()
        }
    }

    fn app() -> Router {
        build_router(ready_ctx(), cfg())
    }

    async fn post_rpc(app: Router, body: &str) -> (StatusCode, Value) {
        let res = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/rpc")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_owned()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = res.status();
        let bytes = to_bytes(res.into_body(), 64 * 1024).await.unwrap();
        let v = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or(Value::String(String::from_utf8_lossy(&bytes).into_owned()))
        };
        (status, v)
    }

    #[tokio::test]
    async fn test_health_returns_success() {
        let res = app()
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let bytes = to_bytes(res.into_body(), 1024).await.unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert_eq!(text, "{\"status\":\"ok\"}");
    }

    #[tokio::test]
    async fn test_health_does_not_expose_internal_data() {
        let res = app()
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = to_bytes(res.into_body(), 1024).await.unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(!text.contains("password"));
        assert!(!text.contains("private"));
        assert!(!text.contains("mnemonic"));
        assert!(!text.contains("target/"));
        assert!(!text.contains("postgres"));
    }

    #[tokio::test]
    async fn post_rpc_success() {
        let (st, v) = post_rpc(app(), r#"{"jsonrpc":"2.0","method":"rsc_chainId","id":1}"#).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v["result"], "3");
        assert_eq!(v["id"], 1);
        assert!(v.get("error").is_none());
    }

    #[tokio::test]
    async fn wrong_http_method() {
        let res = app()
            .oneshot(Request::builder().uri("/rpc").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn wrong_content_type() {
        let res = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/rpc")
                    .header("content-type", "text/plain")
                    .body(Body::from(
                        r#"{"jsonrpc":"2.0","method":"rsc_chainId","id":1}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }

    #[tokio::test]
    async fn test_malformed_json() {
        let (st, v) = post_rpc(app(), "{").await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v["error"]["code"], -32700);
        assert_eq!(v["id"], Value::Null);
    }

    #[tokio::test]
    async fn test_invalid_jsonrpc_version() {
        let (st, v) = post_rpc(app(), r#"{"jsonrpc":"1.0","method":"rsc_chainId","id":1}"#).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v["error"]["code"], -32600);
    }

    #[tokio::test]
    async fn test_unknown_method() {
        let (_, v) = post_rpc(
            app(),
            r#"{"jsonrpc":"2.0","method":"rsc_forceBlock","id":1}"#,
        )
        .await;
        assert_eq!(v["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn test_invalid_params() {
        let (_, v) = post_rpc(
            app(),
            r#"{"jsonrpc":"2.0","method":"rsc_getBlockByHeight","params":{"height":-1},"id":1}"#,
        )
        .await;
        assert_eq!(v["error"]["code"], -32602);
    }

    #[tokio::test]
    async fn test_request_size_limit() {
        let huge = format!(
            r#"{{"jsonrpc":"2.0","method":"rsc_chainId","id":1,"pad":"{}"}}"#,
            "x".repeat(4000)
        );
        let res = app()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/rpc")
                    .header("content-type", "application/json")
                    .body(Body::from(huge))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn test_batch_size_limit() {
        let items: Vec<Value> = (0..8)
            .map(|i| json!({"jsonrpc":"2.0","method":"rsc_chainId","id":i}))
            .collect();
        let (_, v) = post_rpc(app(), &Value::Array(items).to_string()).await;
        assert_eq!(v["error"]["code"], -32003);
    }

    #[tokio::test]
    async fn test_timeout() {
        let ctx = RpcContext::new(
            Arc::new(MockNode {
                chain_id: "3".into(),
                delay: Some(Duration::from_secs(2)),
                ..Default::default()
            }),
            Arc::new(MockChain::default()),
            Arc::new(MockTx::default()),
            Arc::new(MockState::default()),
            Arc::new(MockConsensus::default()),
        );
        let mut c = cfg();
        c.request_timeout = Duration::from_millis(40);
        let router = build_router(ctx, c);
        let (st, v) = post_rpc(router, r#"{"jsonrpc":"2.0","method":"rsc_chainId","id":9}"#).await;
        assert!(st == StatusCode::OK || st == StatusCode::REQUEST_TIMEOUT);
        if st == StatusCode::OK {
            assert_eq!(v["error"]["code"], -32004);
        }
    }

    #[tokio::test]
    async fn valid_batch() {
        let body = json!([
            {"jsonrpc":"2.0","method":"rsc_chainId","id":1},
            {"jsonrpc":"2.0","method":"rsc_getStatus","id":2}
        ])
        .to_string();
        let (st, v) = post_rpc(app(), &body).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v.as_array().unwrap().len(), 2);
        assert_eq!(v[0]["id"], 1);
        assert_eq!(v[1]["id"], 2);
    }

    #[tokio::test]
    async fn mixed_batch() {
        let body = json!([
            {"jsonrpc":"2.0","method":"rsc_chainId","id":1},
            {"jsonrpc":"2.0","method":"nope","id":2}
        ])
        .to_string();
        let (_, v) = post_rpc(app(), &body).await;
        assert_eq!(v[0]["result"], "3");
        assert_eq!(v[1]["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn empty_batch() {
        let (_, v) = post_rpc(app(), "[]").await;
        assert_eq!(v["error"]["code"], -32600);
    }

    #[tokio::test]
    async fn invalid_element_in_batch() {
        let body = json!([
            {"jsonrpc":"2.0","method":"rsc_chainId","id":1},
            "nope"
        ])
        .to_string();
        let (_, v) = post_rpc(app(), &body).await;
        assert_eq!(v[0]["result"], "3");
        assert_eq!(v[1]["error"]["code"], -32600);
    }

    #[tokio::test]
    async fn notifications_only_batch() {
        let body = json!([
            {"jsonrpc":"2.0","method":"rsc_chainId"},
            {"jsonrpc":"2.0","method":"rsc_getStatus"}
        ])
        .to_string();
        let (st, v) = post_rpc(app(), &body).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v, Value::Null);
    }

    #[tokio::test]
    async fn batch_with_notifications() {
        let body = json!([
            {"jsonrpc":"2.0","method":"rsc_chainId"},
            {"jsonrpc":"2.0","method":"rsc_getStatus","id":5}
        ])
        .to_string();
        let (_, v) = post_rpc(app(), &body).await;
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["id"], 5);
    }

    #[tokio::test]
    async fn batch_id_correlation() {
        let body = json!([
            {"jsonrpc":"2.0","method":"rsc_chainId","id":"a"},
            {"jsonrpc":"2.0","method":"rsc_chainId","id":7}
        ])
        .to_string();
        let (_, v) = post_rpc(app(), &body).await;
        assert_eq!(v[0]["id"], "a");
        assert_eq!(v[1]["id"], 7);
    }

    #[tokio::test]
    async fn notification_no_body() {
        let (st, v) = post_rpc(app(), r#"{"jsonrpc":"2.0","method":"rsc_chainId"}"#).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v, Value::Null);
    }

    #[tokio::test]
    async fn test_no_private_key_in_error() {
        let ctx = RpcContext::new(
            Arc::new(MockNode {
                err: Some(ServiceError::Internal),
                ..Default::default()
            }),
            Arc::new(MockChain::default()),
            Arc::new(MockTx::default()),
            Arc::new(MockState::default()),
            Arc::new(MockConsensus::default()),
        );
        let (_, v) = post_rpc(
            build_router(ctx, cfg()),
            r#"{"jsonrpc":"2.0","method":"rsc_getNodeInfo","id":1}"#,
        )
        .await;
        let s = v.to_string();
        assert!(!s.contains("private_key"));
        assert!(!s.contains("mnemonic"));
        assert_eq!(v["error"]["message"], "internal error");
    }

    #[tokio::test]
    async fn test_no_secret_leak() {
        let (_, v) = post_rpc(
            app(),
            r#"{"jsonrpc":"2.0","method":"rsc_getNodeInfo","id":1}"#,
        )
        .await;
        let s = v.to_string();
        assert!(!s.contains("password"));
        assert!(!s.contains("seed"));
        assert!(!s.contains("/home/"));
        assert!(v["result"].get("node_id").is_some());
    }

    #[tokio::test]
    async fn huge_numeric_height() {
        let (_, v) = post_rpc(
            app(),
            r#"{"jsonrpc":"2.0","method":"rsc_getBlockByHeight","params":{"height":999999999999999999999},"id":1}"#,
        )
        .await;
        assert_eq!(v["error"]["code"], -32602);
    }

    #[tokio::test]
    async fn cors_not_wildcard() {
        let res = app()
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri("/rpc")
                    .header("origin", "https://evil.example")
                    .header("access-control-request-method", "POST")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let acao = res.headers().get("access-control-allow-origin");
        assert!(acao.is_none() || acao.unwrap() != "*");
    }
}
