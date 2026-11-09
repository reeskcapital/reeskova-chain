//! RSC-RPC v1: JSON-RPC 2.0 sobre HTTP.
//!
//! Esta capa traduce peticiones externas a llamadas de servicios internos.
//! No ejecuta consenso, no firma, no accede a storage y no custodia claves.
//!
//! Transporte: `POST /rpc` (JSON-RPC), `GET /health` (liveness) y `GET /network`
//! (directorio de endpoints RPC conocidos). Los hashes públicos usan hexadecimal
//! minúsculo. Los balances van como string.
//!
//! La red no depende de una única URL fija: cada nodo puede anunciar su RPC y
//! devolver semillas vía [`rsc_getRpcEndpoints`](methods::dispatch). Los
//! clientes prueban varios endpoints (failover).
//!
//! # Métodos v1
//!
//! | Método | Acceso | Parámetros nombrados | Resultado |
//! |---|---|---|---|
//! | `rsc_chainId` | lectura | ninguno | `string` |
//! | `rsc_getNodeInfo` | lectura | ninguno | [`RpcNodeInfo`] |
//! | `rsc_getStatus` | lectura | ninguno | [`RpcStatus`] |
//! | `rsc_getRpcEndpoints` | lectura | ninguno | lista de [`RpcEndpoint`] |
//! | `rsc_getBlock` | lectura | `block_hash` | [`RpcBlock`] |
//! | `rsc_getBlockByHeight` | lectura | `height` | [`RpcBlock`] |
//! | `rsc_getTransaction` | lectura | `transaction_hash` | [`RpcTransaction`] |
//! | `rsc_sendTransaction` | escritura | `signed_transaction` | [`RpcSubmitResult`] |
//! | `rsc_getAccount` | lectura | `account` | [`RpcAccount`] |
//! | `rsc_getBalance` | lectura | `account` | [`RpcBalance`] |
//! | `rsc_getAssets` | lectura | ninguno | lista de [`RpcAsset`] |
//! | `rsc_getAssetBalance` | lectura | `account`, `asset_id` | [`RpcAssetBalance`] |
//! | `rsc_getAssetBalances` | lectura | `account` | mapa `asset_id` → balance |
//! | `rsc_getPools` | lectura | ninguno | lista de [`RpcPool`] |
//! | `rsc_getPool` | lectura | `pool_id` | [`RpcPool`] |
//! | `rsc_quoteSwap` | lectura | `pool_id`, `token_in`, `token_out`, `amount_in` XOR `amount_out` | [`RpcSwapQuote`] |
//! | `rsc_quoteAddLiquidity` | lectura | `pool_id`, `max_amount_a`, `max_amount_b` | [`RpcAddLiquidityQuote`] |
//! | `rsc_getLpPosition` | lectura | `pool_id`, `account` | [`RpcLpPosition`] |
//! | `rsc_getValidators` | lectura | ninguno | lista de [`RpcValidator`] |
//! | `rsc_getFinality` | lectura | ninguno | [`RpcFinality`] |
//! | `rsc_adminRegisterAsset` | admin (feature) | `asset_id`, `decimals`, `symbol` | `null` |
//! | `rsc_adminCreditAsset` | admin (feature) | `account`, `asset_id`, `amount` | `null` |
//!
//! Errores de protocolo: `-32700`, `-32600`, `-32601`, `-32602`, `-32603`.
//! Errores de servidor: `-32000` not found, `-32001` unauthorized, `-32003` too large,
//! `-32004` timeout, `-32005` unavailable, `-32006` transaction rejected.
//!
//! `rsc_sendTransaction` exige una transacción ya firmada. Nunca acepta
//! `private_key`, `seed_phrase` ni `mnemonic`. Los métodos `rsc_admin*` solo
//! existen con feature `amm-admin-rpc` (testnet); requieren `admin_token`
//! (param o header `x-admin-token`) y nunca aceptan `private_key`.
//!
//! Las lecturas de pools (`rsc_getPools`, `rsc_getPool`, `rsc_quoteSwap`,
//! `rsc_quoteAddLiquidity`, `rsc_getLpPosition`) no escriben estado ni firman.
//! Ejecutar un swap o un cambio de liquidez sigue siendo una transacción
//! firmada enviada con `rsc_sendTransaction`. Identificadores: hex minúsculo
//! de 32 bytes. Cantidades: string entero. Un pool ausente es `-32000`.
//! Parámetros desconocidos, o `amount_in` junto con `amount_out`, son `-32602`.

pub mod methods;
pub mod server;

pub use server::RpcServer;

use std::collections::BTreeMap;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Versión JSON-RPC admitida. No se acepta 1.0.
pub const JSONRPC_VERSION: &str = "2.0";

/// Códigos JSON-RPC 2.0 y rango de servidor `-32000..-32099`.
pub const PARSE_ERROR: i32 = -32700;
pub const INVALID_REQUEST: i32 = -32600;
pub const METHOD_NOT_FOUND: i32 = -32601;
pub const INVALID_PARAMS: i32 = -32602;
pub const INTERNAL_ERROR: i32 = -32603;
pub const ERR_NOT_FOUND: i32 = -32000;
pub const ERR_UNAUTHORIZED: i32 = -32001;
pub const ERR_RATE_LIMITED: i32 = -32002;
pub const ERR_REQUEST_TOO_LARGE: i32 = -32003;
pub const ERR_TIMEOUT: i32 = -32004;
pub const ERR_UNAVAILABLE: i32 = -32005;
pub const ERR_TX_REJECTED: i32 = -32006;

/// Identificador JSON-RPC 2.0. Se preserva tal cual en la respuesta.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RpcId {
    String(String),
    Number(serde_json::Number),
    Null,
}

/// Petición JSON-RPC 2.0. `id = None` es una notification.
#[derive(Clone, Debug, PartialEq)]
pub struct RpcRequest {
    pub jsonrpc: String,
    pub method: String,
    pub params: Option<Value>,
    pub id: Option<RpcId>,
}

/// Respuesta JSON-RPC 2.0. Exactamente `result` o `error`, nunca ambos.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RpcResponse {
    Success(RpcSuccess),
    Error(RpcFailure),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RpcSuccess {
    pub jsonrpc: String,
    pub id: RpcId,
    pub result: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RpcFailure {
    pub jsonrpc: String,
    pub id: RpcId,
    pub error: RpcErrorObject,
}

/// Cuerpo de error público. Sin `data` para no filtrar detalles internos.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcErrorObject {
    pub code: i32,
    pub message: String,
}

/// Errores de protocolo y de aplicación. Los mensajes públicos están fijados.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RpcError {
    ParseError,
    InvalidRequest,
    MethodNotFound,
    InvalidParams,
    InternalError,
    NotFound,
    Unauthorized,
    RateLimited,
    RequestTooLarge,
    Timeout,
    ServiceUnavailable,
    TransactionRejected,
}

impl RpcError {
    pub fn code(self) -> i32 {
        match self {
            Self::ParseError => PARSE_ERROR,
            Self::InvalidRequest => INVALID_REQUEST,
            Self::MethodNotFound => METHOD_NOT_FOUND,
            Self::InvalidParams => INVALID_PARAMS,
            Self::InternalError => INTERNAL_ERROR,
            Self::NotFound => ERR_NOT_FOUND,
            Self::Unauthorized => ERR_UNAUTHORIZED,
            Self::RateLimited => ERR_RATE_LIMITED,
            Self::RequestTooLarge => ERR_REQUEST_TOO_LARGE,
            Self::Timeout => ERR_TIMEOUT,
            Self::ServiceUnavailable => ERR_UNAVAILABLE,
            Self::TransactionRejected => ERR_TX_REJECTED,
        }
    }

    /// Mensaje estable y sanitizado. Nunca incluye el `Display` interno.
    pub fn public_message(self) -> &'static str {
        match self {
            Self::ParseError => "parse error",
            Self::InvalidRequest => "invalid request",
            Self::MethodNotFound => "method not found",
            Self::InvalidParams => "invalid params",
            Self::InternalError => "internal error",
            Self::NotFound => "not found",
            Self::Unauthorized => "unauthorized",
            Self::RateLimited => "rate limited",
            Self::RequestTooLarge => "request too large",
            Self::Timeout => "timeout",
            Self::ServiceUnavailable => "service unavailable",
            Self::TransactionRejected => "transaction rejected",
        }
    }

    pub fn object(self) -> RpcErrorObject {
        RpcErrorObject {
            code: self.code(),
            message: self.public_message().to_owned(),
        }
    }
}

/// Error al arrancar o servir HTTP. No se envía a clientes RPC.
#[derive(Debug, thiserror::Error)]
pub enum RpcServerError {
    #[error("failed to bind RPC listener")]
    Bind(#[source] std::io::Error),
    #[error("RPC server stopped unexpectedly")]
    Serve(#[source] std::io::Error),
}

/// Error de un servicio interno. Se mapea en un único sitio a [`RpcError`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceError {
    NotFound,
    Unavailable,
    InvalidTransaction,
    InvalidState,
    ConsensusUnavailable,
    Internal,
}

/// Tabla única de mapping interno → RPC.
pub fn map_service_error(err: ServiceError) -> RpcError {
    match err {
        ServiceError::NotFound => RpcError::NotFound,
        ServiceError::Unavailable | ServiceError::ConsensusUnavailable => {
            RpcError::ServiceUnavailable
        }
        ServiceError::InvalidTransaction => RpcError::TransactionRejected,
        ServiceError::InvalidState => RpcError::InvalidParams,
        ServiceError::Internal => RpcError::InternalError,
    }
}

/// Configuración del servidor HTTP.
///
/// Defaults: bind `127.0.0.1:8545`, body 1 MiB, timeout 10s, batch 100,
/// CORS deshabilitado (`allowed_origins` vacío). No usa `0.0.0.0` por defecto.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RpcConfig {
    pub bind_address: SocketAddr,
    pub max_body_size: usize,
    pub request_timeout: Duration,
    pub max_batch_size: usize,
    pub allowed_origins: Vec<String>,
}

impl Default for RpcConfig {
    fn default() -> Self {
        Self {
            bind_address: SocketAddr::from(([127, 0, 0, 1], 8545)),
            max_body_size: 1_048_576,
            request_timeout: Duration::from_secs(10),
            max_batch_size: 100,
            allowed_origins: Vec::new(),
        }
    }
}

/// Contexto de dependencias. RPC no posee implementaciones de cadena.
#[derive(Clone)]
pub struct RpcContext {
    pub node: Arc<dyn NodeService>,
    pub chain: Arc<dyn ChainService>,
    pub transaction: Arc<dyn TransactionService>,
    pub state: Arc<dyn StateService>,
    pub consensus: Arc<dyn ConsensusService>,
    /// Ledger de activos / AMM. `None` → lecturas de asset responden unavailable.
    pub amm: Option<Arc<dyn AmmService>>,
    /// Token esperado para `rsc_admin*` (testnet). Solo con feature `amm-admin-rpc`.
    #[cfg(feature = "amm-admin-rpc")]
    pub amm_admin_token: Option<String>,
}

impl RpcContext {
    pub fn new(
        node: Arc<dyn NodeService>,
        chain: Arc<dyn ChainService>,
        transaction: Arc<dyn TransactionService>,
        state: Arc<dyn StateService>,
        consensus: Arc<dyn ConsensusService>,
    ) -> Self {
        Self {
            node,
            chain,
            transaction,
            state,
            consensus,
            amm: None,
            #[cfg(feature = "amm-admin-rpc")]
            amm_admin_token: None,
        }
    }

    /// Conecta el servicio de activos / AMM (lecturas públicas).
    pub fn with_amm(mut self, amm: Arc<dyn AmmService>) -> Self {
        self.amm = Some(amm);
        self
    }

    /// Token de admin para mint/register en testnet (`AMM_ADMIN_TOKEN` / flag).
    #[cfg(feature = "amm-admin-rpc")]
    pub fn with_amm_admin_token(mut self, token: impl Into<String>) -> Self {
        let token = token.into();
        self.amm_admin_token = if token.is_empty() { None } else { Some(token) };
        self
    }
}

/// Información pública del proceso. Sin claves ni rutas.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcNodeInfo {
    pub node_id: String,
    pub version: String,
    pub network_id: String,
    pub chain_id: String,
    pub protocol_version: String,
    pub sync_status: String,
    pub latest_height: u64,
    pub latest_block_hash: String,
}

/// Endpoint RPC conocido (semilla, este nodo u otro peer).
///
/// `url` es la URL completa del `POST /rpc` (p. ej. `http://127.0.0.1:8545/rpc`).
/// Los clientes deben probar varios y no asumir un único host fijo.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcEndpoint {
    pub url: String,
    pub network_id: String,
    pub chain_id: String,
    /// `self` | `seed` | `peer` | `file`.
    pub source: String,
}

/// Normaliza una URL de RPC: trim, sin barra final salvo el path `/rpc`.
pub fn normalize_rpc_url(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return None;
    }
    let mut url = trimmed.trim_end_matches('/').to_string();
    if !url.to_ascii_lowercase().ends_with("/rpc") {
        url.push_str("/rpc");
    }
    Some(url)
}

/// Fusiona endpoints por URL (primera aparición gana).
pub fn merge_rpc_endpoints(entries: impl IntoIterator<Item = RpcEndpoint>) -> Vec<RpcEndpoint> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for entry in entries {
        let Some(url) = normalize_rpc_url(&entry.url) else {
            continue;
        };
        if !seen.insert(url.clone()) {
            continue;
        }
        out.push(RpcEndpoint {
            url,
            network_id: entry.network_id,
            chain_id: entry.chain_id,
            source: entry.source,
        });
    }
    out
}

/// Estado operativo del nodo (no liveness HTTP).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcStatus {
    pub syncing: bool,
    pub latest_height: u64,
    pub latest_block_hash: String,
    pub epoch: u64,
    pub finalized_height: u64,
}

/// Representación RPC de un bloque. Campos existentes en la cadena actual.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcBlock {
    pub hash: String,
    pub height: u64,
    pub parent_hash: String,
    pub timestamp: u64,
    pub transaction_root: String,
    pub transactions: Vec<String>,
}

/// Transacción pública. Sin precio fiat ni gas inventado.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcTransaction {
    pub hash: String,
    pub raw: String,
    pub status: String,
    pub block_height: Option<u64>,
    pub block_hash: Option<String>,
    pub index: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcAccount {
    pub account: String,
    pub balance: String,
    pub nonce: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcBalance {
    pub account: String,
    pub balance: String,
}

/// Activo registrado en el ledger AMM / assets.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcAsset {
    pub asset_id: String,
    pub decimals: u8,
    pub symbol: String,
}

/// Saldo de un activo tipado. Amount como string decimal entero.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcAssetBalance {
    pub balance: String,
}

/// Pool AMM. Amounts/shares como string entero; IDs hex minúsculo 32 bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcPool {
    pub pool_id: String,
    pub asset_a: String,
    pub asset_b: String,
    pub fee_bps: u16,
    /// `"created"` | `"active"` | `"paused"`.
    pub status: String,
    pub reserve_a: String,
    pub reserve_b: String,
    pub total_supply: String,
    pub locked_supply: String,
    pub decimals_a: u8,
    pub decimals_b: u8,
}

/// Cotización de swap (solo lectura). Precios e impacto como string entero.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcSwapQuote {
    pub pool_id: String,
    pub token_in: String,
    pub token_out: String,
    pub amount_in: String,
    pub amount_out: String,
    pub fee_amount: String,
    pub execution_price: String,
    pub spot_price: String,
    /// Spot ajustado con los decimales del ledger de cada activo.
    pub spot_price_normalized: String,
    pub price_impact_bps: String,
    pub decimals_in: u8,
    pub decimals_out: u8,
    pub quote_block: u64,
}

/// Cotización de depósito balanceado ≤ máximos.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcAddLiquidityQuote {
    pub pool_id: String,
    pub amount_a: String,
    pub amount_b: String,
}

/// Posición LP de una cuenta en un pool.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcLpPosition {
    pub pool_id: String,
    pub account: String,
    pub lp_shares: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcValidator {
    pub validator_id: String,
    pub public_key: String,
    pub voting_power: u64,
    pub status: String,
}

/// Finalidad publicada.
///
/// `finalized_hash` es el hash del bloque. `finalized_height`, `epoch` y `view`
/// identifican la prueba. `prepare`, `precommit` y `commit` son el hexadecimal
/// minúsculo del encuadre de cada QC, el mismo que persiste el validador.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcFinality {
    pub finalized_height: u64,
    pub finalized_hash: String,
    pub epoch: u64,
    pub view: u64,
    pub prepare: String,
    pub precommit: String,
    pub commit: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcSubmitResult {
    pub transaction_hash: String,
}

/// Wallet generada por el nodo. `signing_key` y `seed_phrase` se entregan una sola vez.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcWallet {
    pub address: String,
    pub account_id: String,
    pub public_key: String,
    pub signing_key: String,
    pub seed_phrase: String,
}

/// Nombre público ligado a una wallet. No incluye frase ni clave.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcNameRecord {
    pub name: String,
    pub address: String,
    pub account_id: String,
    pub public_key: String,
}

/// Future object-safe de un servicio interno.
pub type ServiceFut<'a, T> = Pin<Box<dyn Future<Output = Result<T, ServiceError>> + Send + 'a>>;

/// Servicio de identidad y estado operativo del nodo.
pub trait NodeService: Send + Sync {
    fn chain_id(&self) -> ServiceFut<'_, String>;
    fn node_info(&self) -> ServiceFut<'_, RpcNodeInfo>;
    fn status(&self) -> ServiceFut<'_, RpcStatus>;

    /// Directorio de endpoints RPC conocidos (este nodo + semillas + peers).
    ///
    /// Default: lista vacía. Los clientes usan failover; la red no depende de
    /// un único DNS fijo.
    fn rpc_endpoints(&self) -> ServiceFut<'_, Vec<RpcEndpoint>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    /// El nodo genera la wallet. Quien no la implementa no la ofrece.
    fn create_wallet(&self) -> ServiceFut<'_, RpcWallet> {
        Box::pin(async { Err(ServiceError::Unavailable) })
    }

    /// Crea una wallet y reserva `name`. El nombre repetido se rechaza.
    /// `password` solo se usa para armar el verificador; no se guarda ni firma.
    fn register_named_wallet(&self, _name: &str, _password: &str) -> ServiceFut<'_, RpcWallet> {
        Box::pin(async { Err(ServiceError::Unavailable) })
    }

    /// Abre la wallet del nombre si la frase y la contraseña son las de esa cuenta.
    fn open_named_wallet(
        &self,
        _name: &str,
        _phrase: &str,
        _password: &str,
    ) -> ServiceFut<'_, RpcWallet> {
        Box::pin(async { Err(ServiceError::Unavailable) })
    }

    /// Datos públicos del nombre. Sin frase ni clave.
    fn lookup_name(&self, _name: &str) -> ServiceFut<'_, RpcNameRecord> {
        Box::pin(async { Err(ServiceError::Unavailable) })
    }
}

/// Consultas de bloques y transacciones. Sin acceso a DB desde RPC.
pub trait ChainService: Send + Sync {
    fn block_by_hash(&self, block_hash: &str) -> ServiceFut<'_, RpcBlock>;
    fn block_by_height(&self, height: u64) -> ServiceFut<'_, RpcBlock>;
    fn transaction(&self, tx_hash: &str) -> ServiceFut<'_, RpcTransaction>;
}

/// Recepción de transacciones ya firmadas. RPC no modifica el payload.
pub trait TransactionService: Send + Sync {
    fn submit_signed(&self, signed_tx: &[u8]) -> ServiceFut<'_, RpcSubmitResult>;
}

/// Consultas de estado. Solo lectura.
pub trait StateService: Send + Sync {
    fn account(&self, account: &str) -> ServiceFut<'_, RpcAccount>;
    fn balance(&self, account: &str) -> ServiceFut<'_, RpcBalance>;
}

/// Consultas (y, en testnet, mint) del ledger / pools AMM.
///
/// Lecturas públicas. `admin_*` solo se invocan desde RPC con feature
/// `amm-admin-rpc` y token; producción usa génesis / tx firmada.
/// Cotizar o listar pools no muta reservas ni `state_root`.
pub trait AmmService: Send + Sync {
    fn assets(&self) -> ServiceFut<'_, Vec<RpcAsset>>;
    fn asset_balance(&self, account: &str, asset_id: &str) -> ServiceFut<'_, RpcAssetBalance>;
    /// Mapa `asset_id` hex → balance string (activos registrados).
    fn asset_balances(&self, account: &str) -> ServiceFut<'_, BTreeMap<String, String>>;

    fn pools(&self) -> ServiceFut<'_, Vec<RpcPool>>;
    fn pool(&self, pool_id: &str) -> ServiceFut<'_, RpcPool>;
    /// Exact-in (`amount_in`) XOR exact-out (`amount_out`).
    fn quote_swap(
        &self,
        pool_id: &str,
        token_in: &str,
        token_out: &str,
        amount_in: Option<&str>,
        amount_out: Option<&str>,
    ) -> ServiceFut<'_, RpcSwapQuote>;
    fn quote_add_liquidity(
        &self,
        pool_id: &str,
        max_amount_a: &str,
        max_amount_b: &str,
    ) -> ServiceFut<'_, RpcAddLiquidityQuote>;
    fn lp_position(&self, pool_id: &str, account: &str) -> ServiceFut<'_, RpcLpPosition>;

    /// Registra un activo (host / testnet admin). Default: unavailable.
    fn admin_register_asset(
        &self,
        _asset_id: &str,
        _decimals: u8,
        _symbol: &str,
    ) -> ServiceFut<'_, ()> {
        Box::pin(async { Err(ServiceError::Unavailable) })
    }

    /// Acredita saldo de activo (host / testnet admin). Default: unavailable.
    fn admin_credit_asset(
        &self,
        _account: &str,
        _asset_id: &str,
        _amount: &str,
    ) -> ServiceFut<'_, ()> {
        Box::pin(async { Err(ServiceError::Unavailable) })
    }
}

/// Consultas públicas de consenso. RPC no vota ni finaliza.
pub trait ConsensusService: Send + Sync {
    fn validators(&self) -> ServiceFut<'_, Vec<RpcValidator>>;
    fn finality(&self) -> ServiceFut<'_, RpcFinality>;
}

pub fn success(id: RpcId, result: Value) -> RpcResponse {
    RpcResponse::Success(RpcSuccess {
        jsonrpc: JSONRPC_VERSION.to_owned(),
        id,
        result,
    })
}

pub fn failure(id: RpcId, err: RpcError) -> RpcResponse {
    RpcResponse::Error(RpcFailure {
        jsonrpc: JSONRPC_VERSION.to_owned(),
        id,
        error: err.object(),
    })
}

/// Parsea un objeto JSON-RPC. `jsonrpc` debe ser exactamente `"2.0"`.
pub fn parse_request(value: &Value) -> Result<RpcRequest, RpcError> {
    let obj = value.as_object().ok_or(RpcError::InvalidRequest)?;
    match obj.get("jsonrpc") {
        Some(Value::String(v)) if v == JSONRPC_VERSION => {}
        _ => return Err(RpcError::InvalidRequest),
    }
    let method = match obj.get("method") {
        Some(Value::String(m)) if !m.is_empty() => m.clone(),
        _ => return Err(RpcError::InvalidRequest),
    };
    let params = match obj.get("params") {
        None => None,
        Some(Value::Null) => None,
        Some(v) if v.is_object() || v.is_array() => Some(v.clone()),
        Some(_) => return Err(RpcError::InvalidParams),
    };
    let id = match obj.get("id") {
        None => None,
        Some(Value::Null) => Some(RpcId::Null),
        Some(Value::String(s)) => Some(RpcId::String(s.clone())),
        Some(Value::Number(n)) => Some(RpcId::Number(n.clone())),
        Some(_) => return Err(RpcError::InvalidRequest),
    };
    Ok(RpcRequest {
        jsonrpc: JSONRPC_VERSION.to_owned(),
        method,
        params,
        id,
    })
}

/// Parámetros nombrados. Arrays posicionales no se infieren.
pub fn named_params(params: Option<&Value>) -> Result<&Map<String, Value>, RpcError> {
    static EMPTY: std::sync::LazyLock<Map<String, Value>> = std::sync::LazyLock::new(Map::new);
    match params {
        None => Ok(&EMPTY),
        Some(Value::Object(map)) => Ok(map),
        _ => Err(RpcError::InvalidParams),
    }
}

pub fn require_string<'a>(map: &'a Map<String, Value>, key: &str) -> Result<&'a str, RpcError> {
    match map.get(key) {
        Some(Value::String(s)) => Ok(s.as_str()),
        _ => Err(RpcError::InvalidParams),
    }
}

pub fn require_u64(map: &Map<String, Value>, key: &str) -> Result<u64, RpcError> {
    match map.get(key) {
        Some(Value::Number(n)) => n.as_u64().ok_or(RpcError::InvalidParams),
        _ => Err(RpcError::InvalidParams),
    }
}

pub fn require_u8(map: &Map<String, Value>, key: &str) -> Result<u8, RpcError> {
    let v = require_u64(map, key)?;
    u8::try_from(v).map_err(|_| RpcError::InvalidParams)
}

/// Hexadecimal minúsculo. Prefijo `0x` opcional en entrada.
pub fn normalize_hex(input: &str) -> Result<String, RpcError> {
    let raw = input.strip_prefix("0x").unwrap_or(input);
    if raw.is_empty() || !raw.len().is_multiple_of(2) {
        return Err(RpcError::InvalidParams);
    }
    if !raw.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(RpcError::InvalidParams);
    }
    Ok(raw.to_ascii_lowercase())
}

pub fn decode_hex(input: &str) -> Result<Vec<u8>, RpcError> {
    let hex = normalize_hex(input)?;
    let mut out = Vec::with_capacity(hex.len() / 2);
    let bytes = hex.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let hi = hex_val(bytes[i])?;
        let lo = hex_val(bytes[i + 1])?;
        out.push((hi << 4) | lo);
        i += 2;
    }
    Ok(out)
}

fn hex_val(b: u8) -> Result<u8, RpcError> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        _ => Err(RpcError::InvalidParams),
    }
}

pub fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct MockNode {
        pub chain_id: String,
        pub info: Option<RpcNodeInfo>,
        pub status: Option<RpcStatus>,
        pub endpoints: Vec<RpcEndpoint>,
        pub err: Option<ServiceError>,
        pub delay: Option<Duration>,
    }

    impl NodeService for MockNode {
        fn chain_id(&self) -> ServiceFut<'_, String> {
            ready(self.delay, self.err, Ok(self.chain_id.clone()))
        }
        fn node_info(&self) -> ServiceFut<'_, RpcNodeInfo> {
            ready(
                self.delay,
                self.err,
                self.info.clone().ok_or(ServiceError::Internal),
            )
        }
        fn status(&self) -> ServiceFut<'_, RpcStatus> {
            ready(
                self.delay,
                self.err,
                self.status.clone().ok_or(ServiceError::Internal),
            )
        }
        fn rpc_endpoints(&self) -> ServiceFut<'_, Vec<RpcEndpoint>> {
            ready(self.delay, self.err, Ok(self.endpoints.clone()))
        }
    }

    #[derive(Default)]
    pub struct MockChain {
        pub block: Option<RpcBlock>,
        pub tx: Option<RpcTransaction>,
        pub err: Option<ServiceError>,
    }

    impl ChainService for MockChain {
        fn block_by_hash(&self, _: &str) -> ServiceFut<'_, RpcBlock> {
            ready(
                None,
                self.err,
                self.block.clone().ok_or(ServiceError::NotFound),
            )
        }
        fn block_by_height(&self, _: u64) -> ServiceFut<'_, RpcBlock> {
            ready(
                None,
                self.err,
                self.block.clone().ok_or(ServiceError::NotFound),
            )
        }
        fn transaction(&self, _: &str) -> ServiceFut<'_, RpcTransaction> {
            ready(
                None,
                self.err,
                self.tx.clone().ok_or(ServiceError::NotFound),
            )
        }
    }

    #[derive(Default)]
    pub struct MockTx {
        pub result: Option<RpcSubmitResult>,
        pub err: Option<ServiceError>,
        pub last: Mutex<Option<Vec<u8>>>,
    }

    impl TransactionService for MockTx {
        fn submit_signed(&self, signed_tx: &[u8]) -> ServiceFut<'_, RpcSubmitResult> {
            if let Ok(mut g) = self.last.lock() {
                *g = Some(signed_tx.to_vec());
            }
            ready(
                None,
                self.err,
                self.result.clone().ok_or(ServiceError::Internal),
            )
        }
    }

    #[derive(Default)]
    pub struct MockState {
        pub account: Option<RpcAccount>,
        pub balance: Option<RpcBalance>,
        pub err: Option<ServiceError>,
    }

    impl StateService for MockState {
        fn account(&self, _: &str) -> ServiceFut<'_, RpcAccount> {
            ready(
                None,
                self.err,
                self.account.clone().ok_or(ServiceError::NotFound),
            )
        }
        fn balance(&self, _: &str) -> ServiceFut<'_, RpcBalance> {
            ready(
                None,
                self.err,
                self.balance.clone().ok_or(ServiceError::NotFound),
            )
        }
    }

    #[derive(Default)]
    pub struct MockConsensus {
        pub validators: Vec<RpcValidator>,
        pub finality: Option<RpcFinality>,
        pub err: Option<ServiceError>,
    }

    impl ConsensusService for MockConsensus {
        fn validators(&self) -> ServiceFut<'_, Vec<RpcValidator>> {
            ready(None, self.err, Ok(self.validators.clone()))
        }
        fn finality(&self) -> ServiceFut<'_, RpcFinality> {
            ready(
                None,
                self.err,
                self.finality.clone().ok_or(ServiceError::Unavailable),
            )
        }
    }

    #[derive(Default)]
    pub struct MockAmm {
        pub assets: Vec<RpcAsset>,
        pub balance: Option<RpcAssetBalance>,
        pub balances: BTreeMap<String, String>,
        pub pools: Vec<RpcPool>,
        pub pool: Option<RpcPool>,
        pub swap_quote: Option<RpcSwapQuote>,
        pub add_liquidity_quote: Option<RpcAddLiquidityQuote>,
        pub lp_position: Option<RpcLpPosition>,
        pub err: Option<ServiceError>,
        pub register_calls: Mutex<Vec<(String, u8, String)>>,
        pub credit_calls: Mutex<Vec<(String, String, String)>>,
        pub quote_swap_calls: Mutex<Vec<(String, String, String, Option<String>, Option<String>)>>,
    }

    impl AmmService for MockAmm {
        fn assets(&self) -> ServiceFut<'_, Vec<RpcAsset>> {
            ready(None, self.err, Ok(self.assets.clone()))
        }
        fn asset_balance(&self, _: &str, _: &str) -> ServiceFut<'_, RpcAssetBalance> {
            ready(
                None,
                self.err,
                self.balance.clone().ok_or(ServiceError::NotFound),
            )
        }
        fn asset_balances(&self, _: &str) -> ServiceFut<'_, BTreeMap<String, String>> {
            ready(None, self.err, Ok(self.balances.clone()))
        }
        fn pools(&self) -> ServiceFut<'_, Vec<RpcPool>> {
            ready(None, self.err, Ok(self.pools.clone()))
        }
        fn pool(&self, _: &str) -> ServiceFut<'_, RpcPool> {
            ready(
                None,
                self.err,
                self.pool.clone().ok_or(ServiceError::NotFound),
            )
        }
        fn quote_swap(
            &self,
            pool_id: &str,
            token_in: &str,
            token_out: &str,
            amount_in: Option<&str>,
            amount_out: Option<&str>,
        ) -> ServiceFut<'_, RpcSwapQuote> {
            if let Ok(mut g) = self.quote_swap_calls.lock() {
                g.push((
                    pool_id.to_owned(),
                    token_in.to_owned(),
                    token_out.to_owned(),
                    amount_in.map(str::to_owned),
                    amount_out.map(str::to_owned),
                ));
            }
            ready(
                None,
                self.err,
                self.swap_quote.clone().ok_or(ServiceError::NotFound),
            )
        }
        fn quote_add_liquidity(
            &self,
            _: &str,
            _: &str,
            _: &str,
        ) -> ServiceFut<'_, RpcAddLiquidityQuote> {
            ready(
                None,
                self.err,
                self.add_liquidity_quote
                    .clone()
                    .ok_or(ServiceError::NotFound),
            )
        }
        fn lp_position(&self, _: &str, _: &str) -> ServiceFut<'_, RpcLpPosition> {
            ready(
                None,
                self.err,
                self.lp_position.clone().ok_or(ServiceError::NotFound),
            )
        }
        fn admin_register_asset(
            &self,
            asset_id: &str,
            decimals: u8,
            symbol: &str,
        ) -> ServiceFut<'_, ()> {
            if let Ok(mut g) = self.register_calls.lock() {
                g.push((asset_id.to_owned(), decimals, symbol.to_owned()));
            }
            ready(None, self.err, Ok(()))
        }
        fn admin_credit_asset(
            &self,
            account: &str,
            asset_id: &str,
            amount: &str,
        ) -> ServiceFut<'_, ()> {
            if let Ok(mut g) = self.credit_calls.lock() {
                g.push((account.to_owned(), asset_id.to_owned(), amount.to_owned()));
            }
            ready(None, self.err, Ok(()))
        }
    }

    fn ready<T: Send + 'static>(
        delay: Option<Duration>,
        err: Option<ServiceError>,
        value: Result<T, ServiceError>,
    ) -> ServiceFut<'static, T> {
        Box::pin(async move {
            if let Some(d) = delay {
                tokio::time::sleep(d).await;
            }
            if let Some(e) = err {
                return Err(e);
            }
            value
        })
    }

    pub fn sample_block() -> RpcBlock {
        RpcBlock {
            hash: "aa".repeat(32),
            height: 100,
            parent_hash: "bb".repeat(32),
            timestamp: 1_700_000_000,
            transaction_root: "cc".repeat(32),
            transactions: vec!["dd".repeat(8)],
        }
    }

    pub fn sample_tx() -> RpcTransaction {
        RpcTransaction {
            hash: "ee".repeat(32),
            raw: "ff".repeat(8),
            status: "included".into(),
            block_height: Some(100),
            block_hash: Some("aa".repeat(32)),
            index: Some(0),
        }
    }

    pub fn sample_info() -> RpcNodeInfo {
        RpcNodeInfo {
            node_id: "node-1".into(),
            version: "0.1.0".into(),
            network_id: "devnet".into(),
            chain_id: "3".into(),
            protocol_version: "1".into(),
            sync_status: "synced".into(),
            latest_height: 100,
            latest_block_hash: "aa".repeat(32),
        }
    }

    pub fn sample_status() -> RpcStatus {
        RpcStatus {
            syncing: false,
            latest_height: 100,
            latest_block_hash: "aa".repeat(32),
            epoch: 10,
            finalized_height: 97,
        }
    }

    pub fn ready_ctx() -> RpcContext {
        RpcContext::new(
            Arc::new(MockNode {
                chain_id: "3".into(),
                info: Some(sample_info()),
                status: Some(sample_status()),
                endpoints: Vec::new(),
                err: None,
                delay: None,
            }),
            Arc::new(MockChain {
                block: Some(sample_block()),
                tx: Some(sample_tx()),
                err: None,
            }),
            Arc::new(MockTx {
                result: Some(RpcSubmitResult {
                    transaction_hash: "ee".repeat(32),
                }),
                err: None,
                last: Mutex::new(None),
            }),
            Arc::new(MockState {
                account: Some(RpcAccount {
                    account: "11".repeat(32),
                    balance: "1000".into(),
                    nonce: 7,
                }),
                balance: Some(RpcBalance {
                    account: "11".repeat(32),
                    balance: "1000".into(),
                }),
                err: None,
            }),
            Arc::new(MockConsensus {
                validators: vec![RpcValidator {
                    validator_id: "v1".into(),
                    public_key: "22".repeat(32),
                    voting_power: 10,
                    status: "active".into(),
                }],
                finality: Some(RpcFinality {
                    finalized_height: 97,
                    finalized_hash: "aa".repeat(32),
                    epoch: 10,
                    view: 3,
                    prepare: "01".into(),
                    precommit: "02".into(),
                    commit: "03".into(),
                }),
                err: None,
            }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn valid_request() {
        let v = json!({"jsonrpc":"2.0","method":"rsc_chainId","id":1});
        let r = parse_request(&v).unwrap();
        assert_eq!(r.method, "rsc_chainId");
        assert_eq!(r.id, Some(RpcId::Number(1.into())));
    }

    #[test]
    fn invalid_json_is_parse_error_via_serde() {
        assert!(serde_json::from_str::<Value>("{").is_err());
    }

    #[test]
    fn invalid_request_not_object() {
        assert_eq!(parse_request(&json!([])), Err(RpcError::InvalidRequest));
        assert_eq!(parse_request(&json!("x")), Err(RpcError::InvalidRequest));
    }

    #[test]
    fn wrong_jsonrpc_version() {
        let v = json!({"jsonrpc":"1.0","method":"rsc_chainId","id":1});
        assert_eq!(parse_request(&v), Err(RpcError::InvalidRequest));
    }

    #[test]
    fn missing_method() {
        let v = json!({"jsonrpc":"2.0","id":1});
        assert_eq!(parse_request(&v), Err(RpcError::InvalidRequest));
    }

    #[test]
    fn method_non_string() {
        let v = json!({"jsonrpc":"2.0","method":1,"id":1});
        assert_eq!(parse_request(&v), Err(RpcError::InvalidRequest));
    }

    #[test]
    fn missing_jsonrpc() {
        let v = json!({"method":"rsc_chainId","id":1});
        assert_eq!(parse_request(&v), Err(RpcError::InvalidRequest));
    }

    #[test]
    fn invalid_id_type() {
        let v = json!({"jsonrpc":"2.0","method":"rsc_chainId","id":true});
        assert_eq!(parse_request(&v), Err(RpcError::InvalidRequest));
    }

    #[test]
    fn params_malformed() {
        let v = json!({"jsonrpc":"2.0","method":"rsc_chainId","params":"x","id":1});
        assert_eq!(parse_request(&v), Err(RpcError::InvalidParams));
    }

    #[test]
    fn string_id() {
        let v = json!({"jsonrpc":"2.0","method":"m","id":"abc"});
        assert_eq!(
            parse_request(&v).unwrap().id,
            Some(RpcId::String("abc".into()))
        );
    }

    #[test]
    fn integer_id() {
        let v = json!({"jsonrpc":"2.0","method":"m","id":42});
        assert_eq!(
            parse_request(&v).unwrap().id,
            Some(RpcId::Number(42.into()))
        );
    }

    #[test]
    fn null_id() {
        let v = json!({"jsonrpc":"2.0","method":"m","id":null});
        assert_eq!(parse_request(&v).unwrap().id, Some(RpcId::Null));
    }

    #[test]
    fn id_roundtrip() {
        for id in [
            RpcId::String("x".into()),
            RpcId::Number(7.into()),
            RpcId::Null,
        ] {
            let v = serde_json::to_value(&id).unwrap();
            let back: RpcId = serde_json::from_value(v).unwrap();
            assert_eq!(back, id);
        }
    }

    #[test]
    fn response_preserves_id() {
        let id = RpcId::String("keep".into());
        let s = serde_json::to_value(success(id.clone(), json!(true))).unwrap();
        assert_eq!(s["id"], "keep");
        let e = serde_json::to_value(failure(id, RpcError::NotFound)).unwrap();
        assert_eq!(e["id"], "keep");
    }

    #[test]
    fn success_response() {
        let v = serde_json::to_value(success(RpcId::Null, json!({"ok":true}))).unwrap();
        assert_eq!(v["jsonrpc"], "2.0");
        assert!(v.get("result").is_some());
        assert!(v.get("error").is_none());
    }

    #[test]
    fn error_response() {
        let v = serde_json::to_value(failure(RpcId::Null, RpcError::MethodNotFound)).unwrap();
        assert_eq!(v["error"]["code"], METHOD_NOT_FOUND);
        assert!(v.get("result").is_none());
    }

    #[test]
    fn response_never_has_result_and_error() {
        let s = serde_json::to_value(success(RpcId::Null, json!(1))).unwrap();
        let e = serde_json::to_value(failure(RpcId::Null, RpcError::InternalError)).unwrap();
        assert!(s.get("result").is_some() && s.get("error").is_none());
        assert!(e.get("error").is_some() && e.get("result").is_none());
    }

    #[test]
    fn error_mapping_standard() {
        assert_eq!(RpcError::ParseError.code(), -32700);
        assert_eq!(RpcError::InvalidRequest.code(), -32600);
        assert_eq!(RpcError::MethodNotFound.code(), -32601);
        assert_eq!(RpcError::InvalidParams.code(), -32602);
        assert_eq!(RpcError::InternalError.code(), -32603);
    }

    #[test]
    fn error_mapping_application() {
        assert_eq!(
            map_service_error(ServiceError::NotFound),
            RpcError::NotFound
        );
        assert_eq!(
            map_service_error(ServiceError::Unavailable),
            RpcError::ServiceUnavailable
        );
        assert_eq!(
            map_service_error(ServiceError::InvalidTransaction),
            RpcError::TransactionRejected
        );
        assert_eq!(RpcError::NotFound.code(), -32000);
        assert_eq!(RpcError::ServiceUnavailable.code(), -32005);
    }

    #[test]
    fn public_errors_have_no_secrets() {
        for e in [
            RpcError::InternalError,
            RpcError::NotFound,
            RpcError::ServiceUnavailable,
        ] {
            let msg = e.public_message();
            assert!(!msg.contains("password"));
            assert!(!msg.contains("private"));
            assert!(!msg.contains("mnemonic"));
            assert!(!msg.contains('/'));
        }
    }

    #[test]
    fn normalize_hex_lowercase() {
        assert_eq!(normalize_hex("0xAaBb").unwrap(), "aabb");
        assert!(normalize_hex("zz").is_err());
        assert!(normalize_hex("abc").is_err());
        assert!(normalize_hex("").is_err());
    }

    #[test]
    fn decode_encode_hex() {
        let b = decode_hex("0x0a0b").unwrap();
        assert_eq!(b, vec![10, 11]);
        assert_eq!(encode_hex(&b), "0a0b");
    }

    #[test]
    fn positional_params_rejected() {
        assert_eq!(
            named_params(Some(&json!([1]))),
            Err(RpcError::InvalidParams)
        );
    }

    #[test]
    fn require_u64_rejects_negative_and_overflow() {
        let mut m = Map::new();
        m.insert("height".into(), json!(-1));
        assert_eq!(require_u64(&m, "height"), Err(RpcError::InvalidParams));
        m.insert("height".into(), json!(u64::MAX));
        assert_eq!(require_u64(&m, "height").unwrap(), u64::MAX);
        m.insert("height".into(), json!(1.5));
        assert_eq!(require_u64(&m, "height"), Err(RpcError::InvalidParams));
    }

    #[test]
    fn dto_determinism() {
        let b = test_support::sample_block();
        let a = serde_json::to_string(&b).unwrap();
        let c = serde_json::to_string(&b).unwrap();
        assert_eq!(a, c);
        assert_eq!(serde_json::from_str::<RpcBlock>(&a).unwrap(), b);
    }

    #[test]
    fn normalize_and_merge_rpc_endpoints() {
        assert_eq!(
            normalize_rpc_url("http://127.0.0.1:8545"),
            Some("http://127.0.0.1:8545/rpc".into())
        );
        assert_eq!(
            normalize_rpc_url("https://a.example/rpc/"),
            Some("https://a.example/rpc".into())
        );
        assert!(normalize_rpc_url("ftp://x").is_none());
        let merged = merge_rpc_endpoints([
            RpcEndpoint {
                url: "http://127.0.0.1:8545".into(),
                network_id: "devnet".into(),
                chain_id: "3".into(),
                source: "self".into(),
            },
            RpcEndpoint {
                url: "http://127.0.0.1:8545/rpc".into(),
                network_id: "devnet".into(),
                chain_id: "3".into(),
                source: "seed".into(),
            },
            RpcEndpoint {
                url: "https://peer.example/rpc".into(),
                network_id: "devnet".into(),
                chain_id: "3".into(),
                source: "peer".into(),
            },
        ]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].source, "self");
        assert_eq!(merged[1].url, "https://peer.example/rpc");
    }
}
