//! Cotizaciones y precios de solo lectura.
//!
//! Ninguna función muta pools ni accounting. Las fórmulas de swap viven en
//! [`crate::math::invariant`]; aquí se ensamblan cotizaciones, impacto y
//! slippage.
//!
//! ## Precio e impacto
//!
//! - **Spot (raw)**: `floor(reserve_out * PRICE_SCALE / reserve_in)`.
//! - **Spot normalizado**: ajusta por decimales de cada activo.
//! - **Execution price**: `floor(amount_out * PRICE_SCALE / amount_in)`.
//! - **Price impact (bps)**: desviación execution vs spot (mismo espacio).
//! - **Slippage (bps)**: holgura del usuario entre esperado y mínimo aceptado
//!   (protección de tx; distinto del impacto de mercado).

use crate::constants::{BPS_DENOMINATOR, PRICE_SCALE};
use crate::fees::fee_amount;
use crate::math::amount::mul_div_floor;
use crate::math::invariant::{
    amount_in_for_exact_out, amount_out_for_exact_in, spot_price, validate_reserves,
};
use crate::pool::{LiquidityPool, PoolRegistry};
use crate::types::{AssetId, FeeBps, PoolId};
use crate::AmmError;
use rsc_core::{Amount, BlockHeight};
use serde::{Deserialize, Serialize};

/// Decimales por defecto cuando el catálogo no declara otro valor.
pub const DEFAULT_ASSET_DECIMALS: u8 = 18;

/// Catálogo de decimales de activos (0..=38).
pub trait AssetDecimals {
    /// Decimales del activo.
    fn decimals(&self, asset: &AssetId) -> u8;
}

/// Todos los activos con el mismo número de decimales.
#[derive(Clone, Copy, Debug)]
pub struct UniformDecimals(pub u8);

impl AssetDecimals for UniformDecimals {
    fn decimals(&self, _asset: &AssetId) -> u8 {
        self.0
    }
}

impl Default for UniformDecimals {
    fn default() -> Self {
        Self(DEFAULT_ASSET_DECIMALS)
    }
}

/// Cotización determinista para un snapshot de estado.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwapQuote {
    /// Pool.
    pub pool_id: PoolId,
    /// Activo de entrada.
    pub token_in: AssetId,
    /// Activo de salida.
    pub token_out: AssetId,
    /// Cantidad de entrada.
    pub amount_in: Amount,
    /// Cantidad de salida esperada / pedida.
    pub amount_out: Amount,
    /// Fee bruto estimado sobre el input.
    pub fee_amount: Amount,
    /// Precio de ejecución raw: `out * PRICE_SCALE / in`.
    pub execution_price: u128,
    /// Precio spot raw antes del trade.
    pub spot_price: u128,
    /// Impacto de precio en basis points.
    pub price_impact_bps: u128,
    /// Altura del snapshot cotizado.
    pub quote_block: BlockHeight,
    /// Expiración opcional del quote.
    pub expires_at: Option<BlockHeight>,
}

/// Dirección resuelta dentro del pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwapPath {
    /// Activo in.
    pub token_in: AssetId,
    /// Activo out.
    pub token_out: AssetId,
    /// Reserva del lado in.
    pub reserve_in: Amount,
    /// Reserva del lado out.
    pub reserve_out: Amount,
    /// Fee del pool.
    pub fee: FeeBps,
    /// `true` si `token_in` es el activo A canónico.
    pub in_is_a: bool,
}

/// Resuelve el path A↔B del pool.
pub fn resolve_path(
    pool: &LiquidityPool,
    token_in: AssetId,
    token_out: AssetId,
) -> Result<SwapPath, AmmError> {
    let a = pool.identity.asset_a;
    let b = pool.identity.asset_b;
    if token_in == a && token_out == b {
        validate_reserves(pool.reserves)?;
        Ok(SwapPath {
            token_in,
            token_out,
            reserve_in: pool.reserves.reserve_a,
            reserve_out: pool.reserves.reserve_b,
            fee: pool.identity.fee_bps,
            in_is_a: true,
        })
    } else if token_in == b && token_out == a {
        validate_reserves(pool.reserves)?;
        Ok(SwapPath {
            token_in,
            token_out,
            reserve_in: pool.reserves.reserve_b,
            reserve_out: pool.reserves.reserve_a,
            fee: pool.identity.fee_bps,
            in_is_a: false,
        })
    } else {
        Err(AmmError::InvalidPair)
    }
}

/// `10^decimals` con tope seguro.
pub fn pow10(decimals: u8) -> Result<u128, AmmError> {
    if decimals > 38 {
        return Err(AmmError::ConversionFailed);
    }
    Ok(10u128.pow(u32::from(decimals)))
}

/// Cotización exact-input de solo lectura.
pub fn get_quote(
    pools: &PoolRegistry,
    pool_id: PoolId,
    token_in: AssetId,
    token_out: AssetId,
    amount_in: Amount,
    quote_block: BlockHeight,
    expires_at: Option<BlockHeight>,
) -> Result<SwapQuote, AmmError> {
    let pool = pools.get_pool(&pool_id)?;
    pools.ensure_swap_allowed(&pool_id)?;
    let path = resolve_path(pool, token_in, token_out)?;
    let amount_out = calculate_amount_out(amount_in, path.reserve_in, path.reserve_out, path.fee)?;
    build_quote(
        pool_id,
        path,
        amount_in,
        amount_out,
        quote_block,
        expires_at,
    )
}

/// Cotización exact-output.
pub fn get_quote_exact_out(
    pools: &PoolRegistry,
    pool_id: PoolId,
    token_in: AssetId,
    token_out: AssetId,
    amount_out: Amount,
    quote_block: BlockHeight,
    expires_at: Option<BlockHeight>,
) -> Result<SwapQuote, AmmError> {
    let pool = pools.get_pool(&pool_id)?;
    pools.ensure_swap_allowed(&pool_id)?;
    let path = resolve_path(pool, token_in, token_out)?;
    let amount_in = calculate_amount_in(amount_out, path.reserve_in, path.reserve_out, path.fee)?;
    build_quote(
        pool_id,
        path,
        amount_in,
        amount_out,
        quote_block,
        expires_at,
    )
}

fn build_quote(
    pool_id: PoolId,
    path: SwapPath,
    amount_in: Amount,
    amount_out: Amount,
    quote_block: BlockHeight,
    expires_at: Option<BlockHeight>,
) -> Result<SwapQuote, AmmError> {
    let fee = fee_amount(amount_in, path.fee)?;
    let spot = calculate_spot_price(path.reserve_in, path.reserve_out)?;
    let exec = calculate_execution_price(amount_in, amount_out)?;
    let impact = calculate_price_impact(spot, exec)?;
    let quote = SwapQuote {
        pool_id,
        token_in: path.token_in,
        token_out: path.token_out,
        amount_in,
        amount_out,
        fee_amount: fee,
        execution_price: exec,
        spot_price: spot,
        price_impact_bps: impact,
        quote_block,
        expires_at,
    };
    validate_quote(&quote, quote_block)?;
    Ok(quote)
}

/// Wrapper de la matemática exact-input.
pub fn calculate_amount_out(
    amount_in: Amount,
    reserve_in: Amount,
    reserve_out: Amount,
    fee: FeeBps,
) -> Result<Amount, AmmError> {
    amount_out_for_exact_in(amount_in, reserve_in, reserve_out, fee)
}

/// Wrapper de la matemática exact-output.
pub fn calculate_amount_in(
    amount_out: Amount,
    reserve_in: Amount,
    reserve_out: Amount,
    fee: FeeBps,
) -> Result<Amount, AmmError> {
    amount_in_for_exact_out(amount_out, reserve_in, reserve_out, fee)
}

/// Spot raw.
pub fn calculate_spot_price(reserve_in: Amount, reserve_out: Amount) -> Result<u128, AmmError> {
    spot_price(reserve_in, reserve_out)
}

/// Spot normalizado por decimales.
///
/// `floor(reserve_out * 10^dec_in * PRICE_SCALE / (reserve_in * 10^dec_out))`.
pub fn calculate_spot_price_normalized(
    reserve_in: Amount,
    reserve_out: Amount,
    decimals_in: u8,
    decimals_out: u8,
) -> Result<u128, AmmError> {
    if reserve_in.0 == 0 || reserve_out.0 == 0 {
        return Err(AmmError::InvalidReserves);
    }
    let scale_in = pow10(decimals_in)?;
    let scale_out = pow10(decimals_out)?;
    // (reserve_out * scale_in / scale_out) * PRICE_SCALE / reserve_in
    let adjusted = mul_div_floor(reserve_out.0, scale_in, scale_out)?;
    mul_div_floor(adjusted, PRICE_SCALE, reserve_in.0)
}

/// Precio de ejecución raw.
pub fn calculate_execution_price(amount_in: Amount, amount_out: Amount) -> Result<u128, AmmError> {
    if amount_in.0 == 0 {
        return Err(AmmError::ZeroAmount);
    }
    mul_div_floor(amount_out.0, PRICE_SCALE, amount_in.0)
}

/// Impacto en bps respecto al spot.
pub fn calculate_price_impact(spot: u128, execution: u128) -> Result<u128, AmmError> {
    if spot == 0 {
        return Err(AmmError::InvalidReserves);
    }
    if execution >= spot {
        return Ok(0);
    }
    mul_div_floor(spot - execution, BPS_DENOMINATOR, spot)
}

/// Slippage en bps del usuario (`expected` vs `min_out`).
pub fn calculate_slippage(expected_out: Amount, min_out: Amount) -> Result<u128, AmmError> {
    if expected_out.0 == 0 {
        return Err(AmmError::ZeroAmount);
    }
    if min_out.0 > expected_out.0 {
        return Err(AmmError::SlippageExceeded);
    }
    mul_div_floor(expected_out.0 - min_out.0, BPS_DENOMINATOR, expected_out.0)
}

/// Valida coherencia y vigencia de un quote.
pub fn validate_quote(quote: &SwapQuote, current_block: BlockHeight) -> Result<(), AmmError> {
    if quote.amount_in.0 == 0 || quote.amount_out.0 == 0 {
        return Err(AmmError::ZeroAmount);
    }
    if quote.token_in == quote.token_out {
        return Err(AmmError::InvalidPair);
    }
    if let Some(exp) = quote.expires_at {
        if current_block.0 > exp.0 {
            return Err(AmmError::QuoteExpired);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Reserves;
    use rsc_core::Hash;

    #[test]
    fn spot_execution_impact() {
        let spot = calculate_spot_price(Amount(1000), Amount(2000)).unwrap();
        assert_eq!(spot, PRICE_SCALE * 2);
        let exec = calculate_execution_price(Amount(100), Amount(180)).unwrap();
        assert!(calculate_price_impact(spot, exec).unwrap() > 0);
    }

    #[test]
    fn slippage_one_percent() {
        assert_eq!(
            calculate_slippage(Amount(1000), Amount(990)).unwrap(),
            100
        );
    }

    #[test]
    fn equal_decimals_normalized_matches_raw() {
        let ra = Amount(1_000);
        let rb = Amount(2_000);
        assert_eq!(
            calculate_spot_price(ra, rb).unwrap(),
            calculate_spot_price_normalized(ra, rb, 18, 18).unwrap()
        );
    }

    #[test]
    fn resolve_path_rejects_foreign_asset() {
        let pool = LiquidityPool {
            pool_id: PoolId(Hash([1; 32])),
            identity: crate::pool::PoolIdentity {
                asset_a: AssetId(Hash([1; 32])),
                asset_b: AssetId(Hash([2; 32])),
                fee_bps: FeeBps::new(30).unwrap(),
                version: crate::AMM_VERSION,
            },
            reserves: Reserves::new(Amount(10), Amount(10)),
            total_supply: crate::LpShares(10),
            locked_supply: crate::LpShares(0),
            status: crate::PoolStatus::Active,
            created_at: BlockHeight(0),
            creator: rsc_core::AccountId([0; 32]),
        };
        assert_eq!(
            resolve_path(&pool, AssetId(Hash([1; 32])), AssetId(Hash([3; 32])))
                .unwrap_err(),
            AmmError::InvalidPair
        );
    }
}
