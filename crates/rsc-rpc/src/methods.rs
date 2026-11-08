//! Métodos RSC-RPC v1.
//!
//! Cada handler valida forma/tamaño y llama a un servicio interno.
//! No hay SQL, consenso, firma ni modificación de transacciones.

#[cfg(feature = "amm-admin-rpc")]
use serde_json::Map;
use serde_json::Value;

#[cfg(feature = "amm-admin-rpc")]
use crate::require_u8;
use crate::{
    decode_hex, map_service_error, named_params, normalize_hex, require_string, require_u64,
    RpcContext, RpcError,
};

/// Metadatos de transporte (headers HTTP) que acompañan al dispatch.
#[derive(Clone, Debug, Default)]
pub struct RpcCallMeta {
    /// Valor del header `x-admin-token` / `x-rsc-admin-token`, si existe.
    pub admin_token: Option<String>,
}

/// Despacha un método `rsc_*`. Un nombre desconocido es `-32601`.
pub async fn dispatch(
    ctx: &RpcContext,
    method: &str,
    params: Option<&Value>,
) -> Result<Value, RpcError> {
    dispatch_with_meta(ctx, method, params, &RpcCallMeta::default()).await
}

/// Igual que [`dispatch`] con token de admin procedente del header HTTP.
pub async fn dispatch_with_meta(
    ctx: &RpcContext,
    method: &str,
    params: Option<&Value>,
    meta: &RpcCallMeta,
) -> Result<Value, RpcError> {
    #[cfg(not(feature = "amm-admin-rpc"))]
    let _ = meta;
    match method {
        "rsc_chainId" => rsc_chain_id(ctx, params).await,
        "rsc_getNodeInfo" => rsc_get_node_info(ctx, params).await,
        "rsc_getStatus" => rsc_get_status(ctx, params).await,
        "rsc_getRpcEndpoints" => rsc_get_rpc_endpoints(ctx, params).await,
        "rsc_getBlock" => rsc_get_block(ctx, params).await,
        "rsc_getBlockByHeight" => rsc_get_block_by_height(ctx, params).await,
        "rsc_getTransaction" => rsc_get_transaction(ctx, params).await,
        "rsc_sendTransaction" => rsc_send_transaction(ctx, params).await,
        "rsc_getAccount" => rsc_get_account(ctx, params).await,
        "rsc_getBalance" => rsc_get_balance(ctx, params).await,
        "rsc_getAssets" => rsc_get_assets(ctx, params).await,
        "rsc_getAssetBalance" => rsc_get_asset_balance(ctx, params).await,
        "rsc_getAssetBalances" => rsc_get_asset_balances(ctx, params).await,
        "rsc_getPools" => rsc_get_pools(ctx, params).await,
        "rsc_getPool" => rsc_get_pool(ctx, params).await,
        "rsc_quoteSwap" => rsc_quote_swap(ctx, params).await,
        "rsc_quoteAddLiquidity" => rsc_quote_add_liquidity(ctx, params).await,
        "rsc_getLpPosition" => rsc_get_lp_position(ctx, params).await,
        "rsc_getValidators" => rsc_get_validators(ctx, params).await,
        "rsc_getFinality" => rsc_get_finality(ctx, params).await,
        "rsc_createWallet" => rsc_create_wallet(ctx, params).await,
        #[cfg(feature = "amm-admin-rpc")]
        "rsc_adminRegisterAsset" => rsc_admin_register_asset(ctx, params, meta).await,
        #[cfg(feature = "amm-admin-rpc")]
        "rsc_adminCreditAsset" => rsc_admin_credit_asset(ctx, params, meta).await,
        _ => Err(RpcError::MethodNotFound),
    }
}

fn reject_unknown_keys(
    map: &serde_json::Map<String, Value>,
    allowed: &[&str],
) -> Result<(), RpcError> {
    if map.keys().any(|k| !allowed.contains(&k.as_str())) {
        return Err(RpcError::InvalidParams);
    }
    Ok(())
}

fn no_params(params: Option<&Value>) -> Result<(), RpcError> {
    let map = named_params(params)?;
    if map.is_empty() {
        Ok(())
    } else {
        Err(RpcError::InvalidParams)
    }
}

/// `rsc_chainId` — lectura. Resultado: string de chain id del nodo.
async fn rsc_chain_id(ctx: &RpcContext, params: Option<&Value>) -> Result<Value, RpcError> {
    no_params(params)?;
    ctx.node
        .chain_id()
        .await
        .map(Value::String)
        .map_err(map_service_error)
}

/// `rsc_getNodeInfo` — lectura. Información pública del proceso.
async fn rsc_get_node_info(ctx: &RpcContext, params: Option<&Value>) -> Result<Value, RpcError> {
    no_params(params)?;
    let info = ctx.node.node_info().await.map_err(map_service_error)?;
    serde_json::to_value(info).map_err(|_| RpcError::InternalError)
}

/// `rsc_createWallet` — el nodo genera la clave y la frase semilla. Params: `{}`.
async fn rsc_create_wallet(ctx: &RpcContext, params: Option<&Value>) -> Result<Value, RpcError> {
    no_params(params)?;
    let wallet = ctx.node.create_wallet().await.map_err(map_service_error)?;
    serde_json::to_value(wallet).map_err(|_| RpcError::InternalError)
}

/// `rsc_getStatus` — lectura. Estado de cadena, no liveness HTTP.
async fn rsc_get_status(ctx: &RpcContext, params: Option<&Value>) -> Result<Value, RpcError> {
    no_params(params)?;
    let status = ctx.node.status().await.map_err(map_service_error)?;
    serde_json::to_value(status).map_err(|_| RpcError::InternalError)
}

/// `rsc_getRpcEndpoints` — lectura. Directorio de RPC conocidos (failover).
async fn rsc_get_rpc_endpoints(
    ctx: &RpcContext,
    params: Option<&Value>,
) -> Result<Value, RpcError> {
    no_params(params)?;
    let endpoints = ctx
        .node
        .rpc_endpoints()
        .await
        .map_err(map_service_error)?;
    serde_json::to_value(endpoints).map_err(|_| RpcError::InternalError)
}

/// `rsc_getBlock` — lectura. Params: `{ "block_hash": hex }`.
async fn rsc_get_block(ctx: &RpcContext, params: Option<&Value>) -> Result<Value, RpcError> {
    let map = named_params(params)?;
    reject_unknown_keys(map, &["block_hash"])?;
    let hash = normalize_hex(require_string(map, "block_hash")?)?;
    let block = ctx
        .chain
        .block_by_hash(&hash)
        .await
        .map_err(map_service_error)?;
    serde_json::to_value(block).map_err(|_| RpcError::InternalError)
}

/// `rsc_getBlockByHeight` — lectura. Params: `{ "height": u64 }`.
async fn rsc_get_block_by_height(
    ctx: &RpcContext,
    params: Option<&Value>,
) -> Result<Value, RpcError> {
    let map = named_params(params)?;
    reject_unknown_keys(map, &["height"])?;
    let height = require_u64(map, "height")?;
    let block = ctx
        .chain
        .block_by_height(height)
        .await
        .map_err(map_service_error)?;
    serde_json::to_value(block).map_err(|_| RpcError::InternalError)
}

/// `rsc_getTransaction` — lectura. Params: `{ "transaction_hash": hex }`.
async fn rsc_get_transaction(ctx: &RpcContext, params: Option<&Value>) -> Result<Value, RpcError> {
    let map = named_params(params)?;
    reject_unknown_keys(map, &["transaction_hash"])?;
    let hash = normalize_hex(require_string(map, "transaction_hash")?)?;
    let tx = ctx
        .chain
        .transaction(&hash)
        .await
        .map_err(map_service_error)?;
    serde_json::to_value(tx).map_err(|_| RpcError::InternalError)
}

/// `rsc_sendTransaction` — escritura protocolaria. Params: `{ "signed_transaction": hex }`.
///
/// La wallet firma. RPC no pide clave, no firma y no reescribe campos.
async fn rsc_send_transaction(ctx: &RpcContext, params: Option<&Value>) -> Result<Value, RpcError> {
    let map = named_params(params)?;
    reject_unknown_keys(map, &["signed_transaction"])?;
    if map.contains_key("private_key")
        || map.contains_key("seed_phrase")
        || map.contains_key("mnemonic")
    {
        return Err(RpcError::InvalidParams);
    }
    let raw = require_string(map, "signed_transaction")?;
    let bytes = decode_hex(raw)?;
    if bytes.is_empty() {
        return Err(RpcError::InvalidParams);
    }
    let result = ctx
        .transaction
        .submit_signed(&bytes)
        .await
        .map_err(map_service_error)?;
    serde_json::to_value(result).map_err(|_| RpcError::InternalError)
}

/// `rsc_getAccount` — lectura. Params: `{ "account": hex }`.
async fn rsc_get_account(ctx: &RpcContext, params: Option<&Value>) -> Result<Value, RpcError> {
    let map = named_params(params)?;
    reject_unknown_keys(map, &["account"])?;
    let account = normalize_hex(require_string(map, "account")?)?;
    let acc = ctx
        .state
        .account(&account)
        .await
        .map_err(map_service_error)?;
    serde_json::to_value(acc).map_err(|_| RpcError::InternalError)
}

/// `rsc_getBalance` — lectura. Params: `{ "account": hex }`.
async fn rsc_get_balance(ctx: &RpcContext, params: Option<&Value>) -> Result<Value, RpcError> {
    let map = named_params(params)?;
    reject_unknown_keys(map, &["account"])?;
    let account = normalize_hex(require_string(map, "account")?)?;
    let bal = ctx
        .state
        .balance(&account)
        .await
        .map_err(map_service_error)?;
    serde_json::to_value(bal).map_err(|_| RpcError::InternalError)
}

/// `rsc_getValidators` — lectura pública del set. Sin mutación.
async fn rsc_get_validators(ctx: &RpcContext, params: Option<&Value>) -> Result<Value, RpcError> {
    no_params(params)?;
    let vals = ctx
        .consensus
        .validators()
        .await
        .map_err(map_service_error)?;
    serde_json::to_value(vals).map_err(|_| RpcError::InternalError)
}

/// `rsc_getFinality` — lectura. RPC no ejecuta finality.
async fn rsc_get_finality(ctx: &RpcContext, params: Option<&Value>) -> Result<Value, RpcError> {
    no_params(params)?;
    let fin = ctx.consensus.finality().await.map_err(map_service_error)?;
    serde_json::to_value(fin).map_err(|_| RpcError::InternalError)
}

fn amm_service(ctx: &RpcContext) -> Result<&std::sync::Arc<dyn crate::AmmService>, RpcError> {
    ctx.amm.as_ref().ok_or(RpcError::ServiceUnavailable)
}

/// `rsc_getAssets` — lectura pública. Params: `{}`.
async fn rsc_get_assets(ctx: &RpcContext, params: Option<&Value>) -> Result<Value, RpcError> {
    no_params(params)?;
    let assets = amm_service(ctx)?
        .assets()
        .await
        .map_err(map_service_error)?;
    serde_json::to_value(assets).map_err(|_| RpcError::InternalError)
}

/// `rsc_getAssetBalance` — lectura. Params: `{ "account", "asset_id" }`.
async fn rsc_get_asset_balance(
    ctx: &RpcContext,
    params: Option<&Value>,
) -> Result<Value, RpcError> {
    let map = named_params(params)?;
    reject_unknown_keys(map, &["account", "asset_id"])?;
    let account = normalize_hex(require_string(map, "account")?)?;
    let asset_id = normalize_hex(require_string(map, "asset_id")?)?;
    let bal = amm_service(ctx)?
        .asset_balance(&account, &asset_id)
        .await
        .map_err(map_service_error)?;
    serde_json::to_value(bal).map_err(|_| RpcError::InternalError)
}

/// `rsc_getAssetBalances` — lectura. Params: `{ "account" }` → mapa asset→balance.
async fn rsc_get_asset_balances(
    ctx: &RpcContext,
    params: Option<&Value>,
) -> Result<Value, RpcError> {
    let map = named_params(params)?;
    reject_unknown_keys(map, &["account"])?;
    let account = normalize_hex(require_string(map, "account")?)?;
    let bals = amm_service(ctx)?
        .asset_balances(&account)
        .await
        .map_err(map_service_error)?;
    serde_json::to_value(bals).map_err(|_| RpcError::InternalError)
}

/// Hex ID de exactamente 32 bytes (64 hex chars tras normalizar).
fn normalize_id32(input: &str) -> Result<String, RpcError> {
    let hex = normalize_hex(input)?;
    if hex.len() != 64 {
        return Err(RpcError::InvalidParams);
    }
    Ok(hex)
}

fn optional_string<'a>(
    map: &'a serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<&'a str>, RpcError> {
    match map.get(key) {
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.as_str())),
        Some(_) => Err(RpcError::InvalidParams),
    }
}

/// Entero decimal sin signo, canónico (`"00"` → `"0"`). Rechaza vacío y no-dígitos.
fn canonical_amount(raw: &str) -> Result<String, RpcError> {
    if raw.is_empty() || raw.len() > 39 || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return Err(RpcError::InvalidParams);
    }
    let value: u128 = raw.parse().map_err(|_| RpcError::InvalidParams)?;
    Ok(value.to_string())
}

/// `rsc_getPools` — lectura pública. Params: `{}`.
async fn rsc_get_pools(ctx: &RpcContext, params: Option<&Value>) -> Result<Value, RpcError> {
    no_params(params)?;
    let pools = amm_service(ctx)?.pools().await.map_err(map_service_error)?;
    serde_json::to_value(pools).map_err(|_| RpcError::InternalError)
}

/// `rsc_getPool` — lectura. Params: `{ "pool_id" }`.
async fn rsc_get_pool(ctx: &RpcContext, params: Option<&Value>) -> Result<Value, RpcError> {
    let map = named_params(params)?;
    reject_unknown_keys(map, &["pool_id"])?;
    let pool_id = normalize_id32(require_string(map, "pool_id")?)?;
    let pool = amm_service(ctx)?
        .pool(&pool_id)
        .await
        .map_err(map_service_error)?;
    serde_json::to_value(pool).map_err(|_| RpcError::InternalError)
}

/// `rsc_quoteSwap` — lectura. Exact-in XOR exact-out.
async fn rsc_quote_swap(ctx: &RpcContext, params: Option<&Value>) -> Result<Value, RpcError> {
    let map = named_params(params)?;
    reject_unknown_keys(
        map,
        &[
            "pool_id",
            "token_in",
            "token_out",
            "amount_in",
            "amount_out",
        ],
    )?;
    let pool_id = normalize_id32(require_string(map, "pool_id")?)?;
    let token_in = normalize_id32(require_string(map, "token_in")?)?;
    let token_out = normalize_id32(require_string(map, "token_out")?)?;
    let amount_in = match optional_string(map, "amount_in")? {
        Some(raw) => Some(canonical_amount(raw)?),
        None => None,
    };
    let amount_out = match optional_string(map, "amount_out")? {
        Some(raw) => Some(canonical_amount(raw)?),
        None => None,
    };
    match (&amount_in, &amount_out) {
        (Some(_), None) | (None, Some(_)) => {}
        _ => return Err(RpcError::InvalidParams),
    }
    let quote = amm_service(ctx)?
        .quote_swap(
            &pool_id,
            &token_in,
            &token_out,
            amount_in.as_deref(),
            amount_out.as_deref(),
        )
        .await
        .map_err(map_service_error)?;
    serde_json::to_value(quote).map_err(|_| RpcError::InternalError)
}

/// `rsc_quoteAddLiquidity` — lectura. Params: `{ pool_id, max_amount_a, max_amount_b }`.
async fn rsc_quote_add_liquidity(
    ctx: &RpcContext,
    params: Option<&Value>,
) -> Result<Value, RpcError> {
    let map = named_params(params)?;
    reject_unknown_keys(map, &["pool_id", "max_amount_a", "max_amount_b"])?;
    let pool_id = normalize_id32(require_string(map, "pool_id")?)?;
    let max_amount_a = canonical_amount(require_string(map, "max_amount_a")?)?;
    let max_amount_b = canonical_amount(require_string(map, "max_amount_b")?)?;
    let quote = amm_service(ctx)?
        .quote_add_liquidity(&pool_id, &max_amount_a, &max_amount_b)
        .await
        .map_err(map_service_error)?;
    serde_json::to_value(quote).map_err(|_| RpcError::InternalError)
}

/// `rsc_getLpPosition` — lectura. Params: `{ pool_id, account }`.
async fn rsc_get_lp_position(ctx: &RpcContext, params: Option<&Value>) -> Result<Value, RpcError> {
    let map = named_params(params)?;
    reject_unknown_keys(map, &["pool_id", "account"])?;
    let pool_id = normalize_id32(require_string(map, "pool_id")?)?;
    let account = normalize_id32(require_string(map, "account")?)?;
    let pos = amm_service(ctx)?
        .lp_position(&pool_id, &account)
        .await
        .map_err(map_service_error)?;
    serde_json::to_value(pos).map_err(|_| RpcError::InternalError)
}

#[cfg(feature = "amm-admin-rpc")]
fn reject_secrets(map: &Map<String, Value>) -> Result<(), RpcError> {
    if map.contains_key("private_key")
        || map.contains_key("seed_phrase")
        || map.contains_key("mnemonic")
    {
        return Err(RpcError::InvalidParams);
    }
    Ok(())
}

#[cfg(feature = "amm-admin-rpc")]
fn check_admin_auth(
    ctx: &RpcContext,
    map: &Map<String, Value>,
    meta: &RpcCallMeta,
) -> Result<(), RpcError> {
    let expected = ctx
        .amm_admin_token
        .as_deref()
        .filter(|t| !t.is_empty())
        .ok_or(RpcError::Unauthorized)?;
    let from_param = match map.get("admin_token") {
        None => None,
        Some(Value::String(s)) => Some(s.as_str()),
        Some(_) => return Err(RpcError::InvalidParams),
    };
    let provided = from_param.or(meta.admin_token.as_deref());
    match provided {
        Some(token) if token == expected => Ok(()),
        _ => Err(RpcError::Unauthorized),
    }
}

/// `rsc_adminRegisterAsset` — testnet only. Params: asset_id, decimals, symbol [, admin_token].
#[cfg(feature = "amm-admin-rpc")]
async fn rsc_admin_register_asset(
    ctx: &RpcContext,
    params: Option<&Value>,
    meta: &RpcCallMeta,
) -> Result<Value, RpcError> {
    let map = named_params(params)?;
    reject_secrets(map)?;
    reject_unknown_keys(map, &["asset_id", "decimals", "symbol", "admin_token"])?;
    check_admin_auth(ctx, map, meta)?;
    let asset_id = normalize_hex(require_string(map, "asset_id")?)?;
    let decimals = require_u8(map, "decimals")?;
    let symbol = require_string(map, "symbol")?;
    if symbol.is_empty() || symbol.len() > 8 {
        return Err(RpcError::InvalidParams);
    }
    amm_service(ctx)?
        .admin_register_asset(&asset_id, decimals, symbol)
        .await
        .map_err(map_service_error)?;
    Ok(Value::Null)
}

/// `rsc_adminCreditAsset` — testnet only. Params: account, asset_id, amount [, admin_token].
#[cfg(feature = "amm-admin-rpc")]
async fn rsc_admin_credit_asset(
    ctx: &RpcContext,
    params: Option<&Value>,
    meta: &RpcCallMeta,
) -> Result<Value, RpcError> {
    let map = named_params(params)?;
    reject_secrets(map)?;
    reject_unknown_keys(map, &["account", "asset_id", "amount", "admin_token"])?;
    check_admin_auth(ctx, map, meta)?;
    let account = normalize_hex(require_string(map, "account")?)?;
    let asset_id = normalize_hex(require_string(map, "asset_id")?)?;
    let amount = require_string(map, "amount")?;
    if amount.is_empty() || !amount.bytes().all(|b| b.is_ascii_digit()) {
        return Err(RpcError::InvalidParams);
    }
    amm_service(ctx)?
        .admin_credit_asset(&account, &asset_id, amount)
        .await
        .map_err(map_service_error)?;
    Ok(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{
        ready_ctx, MockAmm, MockChain, MockConsensus, MockNode, MockState, MockTx,
    };
    use crate::{
        RpcAccount, RpcBalance, RpcContext, RpcEndpoint, RpcFinality, RpcSubmitResult, ServiceError,
    };
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    async fn call(method: &str, params: Value) -> Result<Value, RpcError> {
        dispatch(&ready_ctx(), method, Some(&params)).await
    }

    #[tokio::test]
    async fn get_rpc_endpoints_valid() {
        let mut node = MockNode::default();
        node.chain_id = "3".into();
        node.endpoints = vec![RpcEndpoint {
            url: "http://127.0.0.1:8545/rpc".into(),
            network_id: "devnet".into(),
            chain_id: "3".into(),
            source: "self".into(),
        }];
        let ctx = RpcContext::new(
            Arc::new(node),
            Arc::new(MockChain::default()),
            Arc::new(MockTx::default()),
            Arc::new(MockState::default()),
            Arc::new(MockConsensus::default()),
        );
        let v = dispatch(&ctx, "rsc_getRpcEndpoints", None).await.unwrap();
        assert_eq!(v.as_array().unwrap().len(), 1);
        assert_eq!(v[0]["url"], "http://127.0.0.1:8545/rpc");
        assert_eq!(v[0]["source"], "self");
    }

    #[tokio::test]
    async fn get_rpc_endpoints_rejects_params() {
        assert_eq!(
            call("rsc_getRpcEndpoints", json!({"x": 1}))
                .await
                .unwrap_err(),
            RpcError::InvalidParams
        );
    }

    #[tokio::test]
    async fn chain_id_valid() {
        let v = dispatch(&ready_ctx(), "rsc_chainId", None).await.unwrap();
        assert_eq!(v, json!("3"));
    }

    #[tokio::test]
    async fn chain_id_invalid_params() {
        let err = call("rsc_chainId", json!({"x": 1})).await.unwrap_err();
        assert_eq!(err, RpcError::InvalidParams);
    }

    #[tokio::test]
    async fn chain_id_service_error() {
        let ctx = RpcContext::new(
            Arc::new(MockNode {
                err: Some(ServiceError::Unavailable),
                ..Default::default()
            }),
            Arc::new(MockChain::default()),
            Arc::new(MockTx::default()),
            Arc::new(MockState::default()),
            Arc::new(MockConsensus::default()),
        );
        let err = dispatch(&ctx, "rsc_chainId", None).await.unwrap_err();
        assert_eq!(err, RpcError::ServiceUnavailable);
    }

    #[tokio::test]
    async fn node_info_valid() {
        let v = dispatch(&ready_ctx(), "rsc_getNodeInfo", None)
            .await
            .unwrap();
        assert_eq!(v["node_id"], "node-1");
        assert!(v.get("private_key").is_none());
    }

    #[tokio::test]
    async fn node_info_invalid() {
        assert_eq!(
            call("rsc_getNodeInfo", json!({"a":1})).await.unwrap_err(),
            RpcError::InvalidParams
        );
    }

    #[tokio::test]
    async fn status_valid() {
        let v = dispatch(&ready_ctx(), "rsc_getStatus", None).await.unwrap();
        assert_eq!(v["syncing"], false);
        assert_eq!(v["epoch"], 10);
    }

    #[tokio::test]
    async fn status_service_error() {
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
        assert_eq!(
            dispatch(&ctx, "rsc_getStatus", None).await.unwrap_err(),
            RpcError::InternalError
        );
    }

    #[tokio::test]
    async fn get_block_valid() {
        let v = call(
            "rsc_getBlock",
            json!({"block_hash": "0xAA".to_string() + &"aa".repeat(31)}),
        )
        .await
        .unwrap();
        assert_eq!(v["height"], 100);
    }

    #[tokio::test]
    async fn get_block_invalid_hash() {
        assert_eq!(
            call("rsc_getBlock", json!({"block_hash": "zz"}))
                .await
                .unwrap_err(),
            RpcError::InvalidParams
        );
    }

    #[tokio::test]
    async fn get_block_not_found() {
        let ctx = RpcContext::new(
            Arc::new(MockNode::default()),
            Arc::new(MockChain {
                err: Some(ServiceError::NotFound),
                ..Default::default()
            }),
            Arc::new(MockTx::default()),
            Arc::new(MockState::default()),
            Arc::new(MockConsensus::default()),
        );
        let err = dispatch(
            &ctx,
            "rsc_getBlock",
            Some(&json!({"block_hash": "aa".repeat(32)})),
        )
        .await
        .unwrap_err();
        assert_eq!(err, RpcError::NotFound);
    }

    #[tokio::test]
    async fn get_block_by_height_valid() {
        let v = call("rsc_getBlockByHeight", json!({"height": 100}))
            .await
            .unwrap();
        assert_eq!(v["height"], 100);
    }

    #[tokio::test]
    async fn get_block_by_height_invalid() {
        assert_eq!(
            call("rsc_getBlockByHeight", json!({"height": -1}))
                .await
                .unwrap_err(),
            RpcError::InvalidParams
        );
        assert_eq!(
            call("rsc_getBlockByHeight", json!({"height": "1"}))
                .await
                .unwrap_err(),
            RpcError::InvalidParams
        );
        assert_eq!(
            call("rsc_getBlockByHeight", json!([100]))
                .await
                .unwrap_err(),
            RpcError::InvalidParams
        );
    }

    #[tokio::test]
    async fn get_block_by_height_missing() {
        let ctx = RpcContext::new(
            Arc::new(MockNode::default()),
            Arc::new(MockChain::default()),
            Arc::new(MockTx::default()),
            Arc::new(MockState::default()),
            Arc::new(MockConsensus::default()),
        );
        assert_eq!(
            dispatch(&ctx, "rsc_getBlockByHeight", Some(&json!({"height": 9})))
                .await
                .unwrap_err(),
            RpcError::NotFound
        );
    }

    #[tokio::test]
    async fn get_transaction_valid() {
        let v = call(
            "rsc_getTransaction",
            json!({"transaction_hash": "ee".repeat(32)}),
        )
        .await
        .unwrap();
        assert_eq!(v["status"], "included");
        assert_eq!(v["index"], 0);
    }

    #[tokio::test]
    async fn get_transaction_invalid() {
        assert_eq!(
            call("rsc_getTransaction", json!({})).await.unwrap_err(),
            RpcError::InvalidParams
        );
    }

    #[tokio::test]
    async fn get_transaction_not_found() {
        let ctx = RpcContext::new(
            Arc::new(MockNode::default()),
            Arc::new(MockChain {
                err: Some(ServiceError::NotFound),
                ..Default::default()
            }),
            Arc::new(MockTx::default()),
            Arc::new(MockState::default()),
            Arc::new(MockConsensus::default()),
        );
        assert_eq!(
            dispatch(
                &ctx,
                "rsc_getTransaction",
                Some(&json!({"transaction_hash": "ee".repeat(32)}))
            )
            .await
            .unwrap_err(),
            RpcError::NotFound
        );
    }

    #[tokio::test]
    async fn send_transaction_valid_and_intact() {
        let mock = MockTx {
            result: Some(RpcSubmitResult {
                transaction_hash: "ab".into(),
            }),
            err: None,
            last: Mutex::new(None),
        };
        let stored = Arc::new(mock);
        let ctx = RpcContext::new(
            Arc::new(MockNode::default()),
            Arc::new(MockChain::default()),
            stored.clone(),
            Arc::new(MockState::default()),
            Arc::new(MockConsensus::default()),
        );
        let hex = "01020304";
        let v = dispatch(
            &ctx,
            "rsc_sendTransaction",
            Some(&json!({"signed_transaction": hex})),
        )
        .await
        .unwrap();
        assert_eq!(v["transaction_hash"], "ab");
        let got = stored.last.lock().unwrap().clone().unwrap();
        assert_eq!(got, vec![1, 2, 3, 4]);
    }

    #[tokio::test]
    async fn send_transaction_rejects_private_key_param() {
        let err = call(
            "rsc_sendTransaction",
            json!({"signed_transaction": "aa", "private_key": "secret"}),
        )
        .await
        .unwrap_err();
        assert_eq!(err, RpcError::InvalidParams);
    }

    #[tokio::test]
    async fn send_transaction_invalid_encoding() {
        assert_eq!(
            call("rsc_sendTransaction", json!({"signed_transaction": "gg"}))
                .await
                .unwrap_err(),
            RpcError::InvalidParams
        );
    }

    #[tokio::test]
    async fn send_transaction_rejected() {
        let ctx = RpcContext::new(
            Arc::new(MockNode::default()),
            Arc::new(MockChain::default()),
            Arc::new(MockTx {
                err: Some(ServiceError::InvalidTransaction),
                ..Default::default()
            }),
            Arc::new(MockState::default()),
            Arc::new(MockConsensus::default()),
        );
        assert_eq!(
            dispatch(
                &ctx,
                "rsc_sendTransaction",
                Some(&json!({"signed_transaction": "aa"}))
            )
            .await
            .unwrap_err(),
            RpcError::TransactionRejected
        );
    }

    #[tokio::test]
    async fn get_account_valid() {
        let v = call("rsc_getAccount", json!({"account": "11".repeat(32)}))
            .await
            .unwrap();
        assert_eq!(v["nonce"], 7);
        assert_eq!(v["balance"], "1000");
    }

    #[tokio::test]
    async fn get_account_invalid() {
        assert_eq!(
            call("rsc_getAccount", json!({"account": 1}))
                .await
                .unwrap_err(),
            RpcError::InvalidParams
        );
    }

    #[tokio::test]
    async fn get_account_not_found() {
        let ctx = RpcContext::new(
            Arc::new(MockNode::default()),
            Arc::new(MockChain::default()),
            Arc::new(MockTx::default()),
            Arc::new(MockState::default()),
            Arc::new(MockConsensus::default()),
        );
        assert_eq!(
            dispatch(
                &ctx,
                "rsc_getAccount",
                Some(&json!({"account": "11".repeat(32)}))
            )
            .await
            .unwrap_err(),
            RpcError::NotFound
        );
    }

    #[tokio::test]
    async fn get_balance_valid() {
        let v = call("rsc_getBalance", json!({"account": "11".repeat(32)}))
            .await
            .unwrap();
        assert_eq!(v, json!({"account": "11".repeat(32), "balance": "1000"}));
    }

    #[tokio::test]
    async fn get_balance_invalid() {
        assert_eq!(
            call("rsc_getBalance", json!({})).await.unwrap_err(),
            RpcError::InvalidParams
        );
    }

    #[tokio::test]
    async fn get_balance_service_error() {
        let ctx = RpcContext::new(
            Arc::new(MockNode::default()),
            Arc::new(MockChain::default()),
            Arc::new(MockTx::default()),
            Arc::new(MockState {
                err: Some(ServiceError::InvalidState),
                account: Some(RpcAccount {
                    account: "aa".into(),
                    balance: "0".into(),
                    nonce: 0,
                }),
                balance: Some(RpcBalance {
                    account: "aa".into(),
                    balance: "0".into(),
                }),
            }),
            Arc::new(MockConsensus::default()),
        );
        assert_eq!(
            dispatch(&ctx, "rsc_getBalance", Some(&json!({"account": "aa"})))
                .await
                .unwrap_err(),
            RpcError::InvalidParams
        );
    }

    #[tokio::test]
    async fn get_validators_valid() {
        let v = dispatch(&ready_ctx(), "rsc_getValidators", None)
            .await
            .unwrap();
        assert_eq!(v[0]["validator_id"], "v1");
        assert!(v[0].get("private_key").is_none());
    }

    #[tokio::test]
    async fn get_validators_invalid() {
        assert_eq!(
            call("rsc_getValidators", json!({"x":1})).await.unwrap_err(),
            RpcError::InvalidParams
        );
    }

    #[tokio::test]
    async fn get_finality_valid() {
        let v = dispatch(&ready_ctx(), "rsc_getFinality", None)
            .await
            .unwrap();
        assert_eq!(v["view"], 3);
        assert_eq!(v["epoch"], 10);
        assert_eq!(v["finalized_height"], 97);
        assert_eq!(v["prepare"], "01");
        assert_eq!(v["precommit"], "02");
        assert_eq!(v["commit"], "03");
    }

    #[tokio::test]
    async fn get_finality_unavailable() {
        let ctx = RpcContext::new(
            Arc::new(MockNode::default()),
            Arc::new(MockChain::default()),
            Arc::new(MockTx::default()),
            Arc::new(MockState::default()),
            Arc::new(MockConsensus {
                finality: Some(RpcFinality {
                    finalized_height: 1,
                    finalized_hash: "aa".into(),
                    epoch: 0,
                    view: 0,
                    prepare: String::new(),
                    precommit: String::new(),
                    commit: String::new(),
                }),
                err: Some(ServiceError::ConsensusUnavailable),
                ..Default::default()
            }),
        );
        assert_eq!(
            dispatch(&ctx, "rsc_getFinality", None).await.unwrap_err(),
            RpcError::ServiceUnavailable
        );
    }

    #[tokio::test]
    async fn unknown_method() {
        assert_eq!(
            dispatch(&ready_ctx(), "eth_getBlock", None)
                .await
                .unwrap_err(),
            RpcError::MethodNotFound
        );
    }

    #[tokio::test]
    async fn method_determinism() {
        let a = dispatch(&ready_ctx(), "rsc_getStatus", None).await.unwrap();
        let b = dispatch(&ready_ctx(), "rsc_getStatus", None).await.unwrap();
        assert_eq!(a, b);
    }

    fn amm_ctx(mock: MockAmm) -> RpcContext {
        ready_ctx().with_amm(Arc::new(mock))
    }

    fn sample_asset() -> crate::RpcAsset {
        crate::RpcAsset {
            asset_id: "aa".repeat(32),
            decimals: 6,
            symbol: "tUSDC".into(),
        }
    }

    #[tokio::test]
    async fn get_assets_ok_without_auth() {
        let ctx = amm_ctx(MockAmm {
            assets: vec![sample_asset()],
            ..Default::default()
        });
        let v = dispatch(&ctx, "rsc_getAssets", Some(&json!({})))
            .await
            .unwrap();
        assert_eq!(v[0]["symbol"], "tUSDC");
        assert_eq!(v[0]["decimals"], 6);
    }

    #[tokio::test]
    async fn get_asset_balance_ok_without_auth() {
        let ctx = amm_ctx(MockAmm {
            balance: Some(crate::RpcAssetBalance {
                balance: "1000000".into(),
            }),
            ..Default::default()
        });
        let v = dispatch(
            &ctx,
            "rsc_getAssetBalance",
            Some(&json!({
                "account": "11".repeat(32),
                "asset_id": "aa".repeat(32),
            })),
        )
        .await
        .unwrap();
        assert_eq!(v, json!({"balance": "1000000"}));
    }

    #[tokio::test]
    async fn get_asset_balances_ok_without_auth() {
        let mut balances = std::collections::BTreeMap::new();
        balances.insert("aa".repeat(32), "42".into());
        let ctx = amm_ctx(MockAmm {
            balances,
            ..Default::default()
        });
        let v = dispatch(
            &ctx,
            "rsc_getAssetBalances",
            Some(&json!({"account": "11".repeat(32)})),
        )
        .await
        .unwrap();
        assert_eq!(v["aa".repeat(32)], "42");
    }

    #[tokio::test]
    async fn get_assets_unavailable_without_service() {
        assert_eq!(
            dispatch(&ready_ctx(), "rsc_getAssets", Some(&json!({})))
                .await
                .unwrap_err(),
            RpcError::ServiceUnavailable
        );
    }

    fn sample_pool() -> crate::RpcPool {
        crate::RpcPool {
            pool_id: "cc".repeat(32),
            asset_a: "aa".repeat(32),
            asset_b: "bb".repeat(32),
            fee_bps: 30,
            status: "active".into(),
            reserve_a: "1000".into(),
            reserve_b: "2000".into(),
            total_supply: "1414".into(),
            locked_supply: "1000".into(),
            decimals_a: 18,
            decimals_b: 6,
        }
    }

    #[tokio::test]
    async fn get_pools_ok_without_auth() {
        let ctx = amm_ctx(MockAmm {
            pools: vec![sample_pool()],
            ..Default::default()
        });
        let v = dispatch(&ctx, "rsc_getPools", Some(&json!({})))
            .await
            .unwrap();
        assert_eq!(v[0]["status"], "active");
        assert_eq!(v[0]["fee_bps"], 30);
        assert_eq!(v[0]["decimals_a"], 18);
    }

    #[tokio::test]
    async fn get_pool_ok_and_unknown_key_rejected() {
        let ctx = amm_ctx(MockAmm {
            pool: Some(sample_pool()),
            ..Default::default()
        });
        let v = dispatch(
            &ctx,
            "rsc_getPool",
            Some(&json!({ "pool_id": "cc".repeat(32) })),
        )
        .await
        .unwrap();
        assert_eq!(v["reserve_a"], "1000");
        assert_eq!(
            dispatch(
                &ctx,
                "rsc_getPool",
                Some(&json!({ "pool_id": "cc".repeat(32), "extra": 1 })),
            )
            .await
            .unwrap_err(),
            RpcError::InvalidParams
        );
    }

    #[tokio::test]
    async fn get_pool_not_found() {
        let ctx = amm_ctx(MockAmm {
            err: Some(ServiceError::NotFound),
            ..Default::default()
        });
        let err = dispatch(
            &ctx,
            "rsc_getPool",
            Some(&json!({ "pool_id": "cc".repeat(32) })),
        )
        .await
        .unwrap_err();
        assert_eq!(err, RpcError::NotFound);
        assert_eq!(err.code(), -32000);
    }

    #[tokio::test]
    async fn quote_rejects_bad_amount_short_id_and_unknown_key() {
        let mock = Arc::new(MockAmm::default());
        let ctx = ready_ctx().with_amm(Arc::clone(&mock) as Arc<dyn crate::AmmService>);
        assert_eq!(
            dispatch(
                &ctx,
                "rsc_quoteSwap",
                Some(&json!({
                    "pool_id": "cc",
                    "token_in": "aa".repeat(32),
                    "token_out": "bb".repeat(32),
                    "amount_in": "1",
                })),
            )
            .await
            .unwrap_err()
            .code(),
            -32602
        );
        assert_eq!(
            dispatch(
                &ctx,
                "rsc_quoteSwap",
                Some(&json!({
                    "pool_id": "cc".repeat(32),
                    "token_in": "aa".repeat(32),
                    "token_out": "bb".repeat(32),
                    "amount_in": "12x",
                })),
            )
            .await
            .unwrap_err(),
            RpcError::InvalidParams
        );
        assert_eq!(
            dispatch(&ctx, "rsc_getPools", Some(&json!({"extra": true})))
                .await
                .unwrap_err(),
            RpcError::InvalidParams
        );
        assert!(mock.quote_swap_calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn quote_swap_exact_in_xor_exact_out() {
        let mock = Arc::new(MockAmm {
            swap_quote: Some(crate::RpcSwapQuote {
                pool_id: "cc".repeat(32),
                token_in: "aa".repeat(32),
                token_out: "bb".repeat(32),
                amount_in: "100".into(),
                amount_out: "99".into(),
                fee_amount: "1".into(),
                execution_price: "990000".into(),
                spot_price: "1000000".into(),
                spot_price_normalized: "1000000000000000000".into(),
                price_impact_bps: "10".into(),
                decimals_in: 18,
                decimals_out: 6,
                quote_block: 7,
            }),
            ..Default::default()
        });
        let ctx = ready_ctx().with_amm(Arc::clone(&mock) as Arc<dyn crate::AmmService>);
        let v = dispatch(
            &ctx,
            "rsc_quoteSwap",
            Some(&json!({
                "pool_id": "cc".repeat(32),
                "token_in": "aa".repeat(32),
                "token_out": "bb".repeat(32),
                "amount_in": "100",
            })),
        )
        .await
        .unwrap();
        assert_eq!(v["amount_out"], "99");
        assert_eq!(
            dispatch(
                &ctx,
                "rsc_quoteSwap",
                Some(&json!({
                    "pool_id": "cc".repeat(32),
                    "token_in": "aa".repeat(32),
                    "token_out": "bb".repeat(32),
                    "amount_in": "100",
                    "amount_out": "99",
                })),
            )
            .await
            .unwrap_err(),
            RpcError::InvalidParams
        );
        assert_eq!(
            dispatch(
                &ctx,
                "rsc_quoteSwap",
                Some(&json!({
                    "pool_id": "cc".repeat(32),
                    "token_in": "aa".repeat(32),
                    "token_out": "bb".repeat(32),
                })),
            )
            .await
            .unwrap_err(),
            RpcError::InvalidParams
        );
        let calls = mock.quote_swap_calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].3.as_deref(), Some("100"));
        assert!(calls[0].4.is_none());
    }

    #[tokio::test]
    async fn quote_add_liquidity_and_lp_position_ok() {
        let ctx = amm_ctx(MockAmm {
            add_liquidity_quote: Some(crate::RpcAddLiquidityQuote {
                pool_id: "cc".repeat(32),
                amount_a: "50".into(),
                amount_b: "100".into(),
            }),
            lp_position: Some(crate::RpcLpPosition {
                pool_id: "cc".repeat(32),
                account: "11".repeat(32),
                lp_shares: "42".into(),
            }),
            ..Default::default()
        });
        let q = dispatch(
            &ctx,
            "rsc_quoteAddLiquidity",
            Some(&json!({
                "pool_id": "cc".repeat(32),
                "max_amount_a": "100",
                "max_amount_b": "200",
            })),
        )
        .await
        .unwrap();
        assert_eq!(q["amount_a"], "50");
        let p = dispatch(
            &ctx,
            "rsc_getLpPosition",
            Some(&json!({
                "pool_id": "cc".repeat(32),
                "account": "11".repeat(32),
            })),
        )
        .await
        .unwrap();
        assert_eq!(p["lp_shares"], "42");
    }

    #[tokio::test]
    #[cfg(not(feature = "amm-admin-rpc"))]
    async fn admin_methods_absent_without_feature() {
        assert_eq!(
            dispatch(
                &ready_ctx(),
                "rsc_adminCreditAsset",
                Some(&json!({
                    "account": "11".repeat(32),
                    "asset_id": "aa".repeat(32),
                    "amount": "1",
                }))
            )
            .await
            .unwrap_err(),
            RpcError::MethodNotFound
        );
        assert_eq!(
            dispatch(
                &ready_ctx(),
                "rsc_adminRegisterAsset",
                Some(&json!({
                    "asset_id": "aa".repeat(32),
                    "decimals": 6,
                    "symbol": "tUSDC",
                }))
            )
            .await
            .unwrap_err(),
            RpcError::MethodNotFound
        );
    }

    #[cfg(feature = "amm-admin-rpc")]
    mod admin {
        use super::*;

        fn admin_ctx(mock: MockAmm, token: &str) -> RpcContext {
            amm_ctx(mock).with_amm_admin_token(token)
        }

        #[tokio::test]
        async fn credit_without_token_rejected() {
            let ctx = admin_ctx(MockAmm::default(), "secret");
            let err = dispatch(
                &ctx,
                "rsc_adminCreditAsset",
                Some(&json!({
                    "account": "11".repeat(32),
                    "asset_id": "aa".repeat(32),
                    "amount": "1000000",
                })),
            )
            .await
            .unwrap_err();
            assert_eq!(err, RpcError::Unauthorized);
            assert_eq!(err.code(), -32001);
        }

        #[tokio::test]
        async fn credit_unregistered_asset_mapped() {
            let ctx = admin_ctx(
                MockAmm {
                    err: Some(ServiceError::InvalidState),
                    ..Default::default()
                },
                "secret",
            );
            let err = dispatch(
                &ctx,
                "rsc_adminCreditAsset",
                Some(&json!({
                    "account": "11".repeat(32),
                    "asset_id": "aa".repeat(32),
                    "amount": "1000000",
                    "admin_token": "secret",
                })),
            )
            .await
            .unwrap_err();
            assert_eq!(err, RpcError::InvalidParams);
        }

        #[tokio::test]
        async fn register_and_credit_with_param_token() {
            let mock = Arc::new(MockAmm::default());
            let ctx = ready_ctx()
                .with_amm(Arc::clone(&mock) as Arc<dyn crate::AmmService>)
                .with_amm_admin_token("secret");
            let reg = dispatch(
                &ctx,
                "rsc_adminRegisterAsset",
                Some(&json!({
                    "asset_id": "aa".repeat(32),
                    "decimals": 6,
                    "symbol": "tUSDC",
                    "admin_token": "secret",
                })),
            )
            .await
            .unwrap();
            assert_eq!(reg, Value::Null);
            let credit = dispatch(
                &ctx,
                "rsc_adminCreditAsset",
                Some(&json!({
                    "account": "11".repeat(32),
                    "asset_id": "aa".repeat(32),
                    "amount": "1000000",
                    "admin_token": "secret",
                })),
            )
            .await
            .unwrap();
            assert_eq!(credit, Value::Null);
            let calls = mock.credit_calls.lock().unwrap();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].2, "1000000");
        }

        #[tokio::test]
        async fn credit_with_header_token() {
            let ctx = admin_ctx(MockAmm::default(), "hdr-secret");
            let meta = RpcCallMeta {
                admin_token: Some("hdr-secret".into()),
            };
            let v = dispatch_with_meta(
                &ctx,
                "rsc_adminCreditAsset",
                Some(&json!({
                    "account": "11".repeat(32),
                    "asset_id": "aa".repeat(32),
                    "amount": "1",
                })),
                &meta,
            )
            .await
            .unwrap();
            assert_eq!(v, Value::Null);
        }

        #[tokio::test]
        async fn rejects_private_key() {
            let ctx = admin_ctx(MockAmm::default(), "secret");
            assert_eq!(
                dispatch(
                    &ctx,
                    "rsc_adminCreditAsset",
                    Some(&json!({
                        "account": "11".repeat(32),
                        "asset_id": "aa".repeat(32),
                        "amount": "1",
                        "admin_token": "secret",
                        "private_key": "dead",
                    })),
                )
                .await
                .unwrap_err(),
                RpcError::InvalidParams
            );
        }
    }
}
