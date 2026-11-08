//! Modelo de comisiones del AMM.
//!
//! ## Política inicial (Uniswap V2 / producto constante)
//!
//! Las trading fees **permanecen dentro de las reservas del pool**:
//!
//! 1. El trader transfiere el `amount_in` completo a la custodia del pool.
//! 2. La curva usa `amount_in * (BPS - fee_bps) / BPS` para calcular `amount_out`.
//! 3. Las reservas pasan a `reserve_in + amount_in` y `reserve_out - amount_out`.
//!
//! El fee **no** se vuelve a sumar a las reservas ni se mueve a un vault
//! aparte: eso sería doble conteo. Beneficia a los LPs vía el aumento de `k`.
//!
//! ## Protocol fee
//!
//! [`PROTOCOL_FEE_SHARE_BPS`] = 0 en v1: el 100 % del fee reportado es porción
//! LP (implícita en reservas). La API de split queda lista para gobernanza.
//!
//! Los parámetros configurables (`FeeBps` por pool) están separados de
//! [`BPS_DENOMINATOR`] (inmutable).

use crate::constants::{BPS_DENOMINATOR, DEFAULT_SWAP_FEE_BPS, MAX_SWAP_FEE_BPS};
use crate::events::AmmEvent;
use crate::math::amount::mul_div_floor;
use crate::pool::PoolRegistry;
use crate::types::{AssetId, FeeBps, PoolId};
use crate::AmmError;
use rsc_core::{Amount, BlockHeight, Hash};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Parte del fee de swap que iría al protocolo, en bps del fee total.
///
/// `0` = todo el fee queda para LPs (vía reservas). Configurable por hard fork
/// / gobernanza futura; no se hardcodea en la fórmula de swap.
pub const PROTOCOL_FEE_SHARE_BPS: u16 = 0;

/// Valida que el fee esté dentro de la política del protocolo.
pub fn validate_fee(fee: FeeBps) -> Result<(), AmmError> {
    FeeBps::new(fee.get()).map(|_| ())
}

/// Fee por defecto propuesto (30 bps).
pub fn default_swap_fee() -> FeeBps {
    FeeBps::new(DEFAULT_SWAP_FEE_BPS).expect("DEFAULT_SWAP_FEE_BPS is valid")
}

/// Lee el fee configurado de un pool registrado.
pub fn get_pool_fee(pools: &PoolRegistry, pool_id: &PoolId) -> Result<FeeBps, AmmError> {
    Ok(pools.get_pool(pool_id)?.identity.fee_bps)
}

/// Porción de `amount_in` correspondiente al fee: `floor(amount * fee_bps / BPS)`.
///
/// Métrica de reporting. El asiento contable real es el `amount_in` completo
/// hacia el pool; **no** se transfiere este valor a otra cuenta.
pub fn fee_amount(amount_in: Amount, fee: FeeBps) -> Result<Amount, AmmError> {
    validate_fee(fee)?;
    if amount_in.0 == 0 {
        return Err(AmmError::ZeroAmount);
    }
    let f = mul_div_floor(amount_in.0, u128::from(fee.get()), BPS_DENOMINATOR)?;
    Ok(Amount(f))
}

/// Cantidad neta tras restar el fee reportado (aprox. a la curva).
pub fn amount_after_fee(amount_in: Amount, fee: FeeBps) -> Result<Amount, AmmError> {
    validate_fee(fee)?;
    let fee_part = fee_amount(amount_in, fee)?;
    amount_in
        .0
        .checked_sub(fee_part.0)
        .map(Amount)
        .ok_or(AmmError::Underflow)
}

/// Divide un fee total en porción LP y porción protocolo.
///
/// Con [`PROTOCOL_FEE_SHARE_BPS`] = 0: `lp = total`, `protocol = 0`.
pub fn split_fee(total_fee: Amount) -> Result<(Amount, Amount), AmmError> {
    if PROTOCOL_FEE_SHARE_BPS == 0 {
        return Ok((total_fee, Amount::ZERO));
    }
    let protocol = mul_div_floor(
        total_fee.0,
        u128::from(PROTOCOL_FEE_SHARE_BPS),
        BPS_DENOMINATOR,
    )?;
    let lp = total_fee
        .0
        .checked_sub(protocol)
        .ok_or(AmmError::Underflow)?;
    Ok((Amount(lp), Amount(protocol)))
}

/// Tope de fee configurable.
pub fn max_fee_bps() -> u16 {
    MAX_SWAP_FEE_BPS
}

/// Desglose de un fee registrado.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeeBreakdown {
    /// Fee total reportado.
    pub total: Amount,
    /// Porción LP (implícita en reservas).
    pub lp_portion: Amount,
    /// Porción protocolo (0 en v1).
    pub protocol_portion: Amount,
}

/// Contador de fees acumulados (solo métrica; sin custodia ni doble conteo).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeeTracker {
    /// Suma bruta por pool.
    accrued_raw: BTreeMap<PoolId, u128>,
    /// Suma bruta por `(pool, asset_in)`.
    accrued_by_asset: BTreeMap<(PoolId, AssetId), u128>,
}

impl FeeTracker {
    /// Vacío.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registra un fee de swap (no mueve tokens).
    pub fn record(
        &mut self,
        pool_id: PoolId,
        asset_in: AssetId,
        fee: Amount,
    ) -> Result<FeeBreakdown, AmmError> {
        let (lp, protocol) = split_fee(fee)?;
        let entry = self.accrued_raw.entry(pool_id).or_insert(0);
        *entry = entry.checked_add(fee.0).ok_or(AmmError::Overflow)?;
        let by = self
            .accrued_by_asset
            .entry((pool_id, asset_in))
            .or_insert(0);
        *by = by.checked_add(fee.0).ok_or(AmmError::Overflow)?;
        Ok(FeeBreakdown {
            total: fee,
            lp_portion: lp,
            protocol_portion: protocol,
        })
    }

    /// Total acumulado reportado para un pool.
    pub fn accrued(&self, pool_id: &PoolId) -> u128 {
        self.accrued_raw.get(pool_id).copied().unwrap_or(0)
    }

    /// Total acumulado para un par pool/activo de entrada.
    pub fn accrued_for_asset(&self, pool_id: &PoolId, asset: &AssetId) -> u128 {
        self.accrued_by_asset
            .get(&(*pool_id, *asset))
            .copied()
            .unwrap_or(0)
    }

    /// Construye el evento [`AmmEvent::FeeCollected`] (el caller lo empuja
    /// solo tras commit exitoso del swap).
    pub fn fee_collected_event(
        breakdown: FeeBreakdown,
        pool_id: PoolId,
        asset: AssetId,
        block_height: BlockHeight,
        transaction_id: Option<Hash>,
    ) -> AmmEvent {
        AmmEvent::FeeCollected {
            pool_id,
            asset,
            amount: breakdown.total,
            lp_portion: breakdown.lp_portion,
            protocol_portion: breakdown.protocol_portion,
            block_height,
            transaction_id,
        }
    }

    /// Encoding canónico de métricas de fee.
    pub fn to_canonical_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&1u32.to_be_bytes());
        out.extend_from_slice(&(self.accrued_raw.len() as u32).to_be_bytes());
        for (pool, amount) in &self.accrued_raw {
            out.extend_from_slice(&pool.0 .0);
            out.extend_from_slice(&amount.to_be_bytes());
        }
        out.extend_from_slice(&(self.accrued_by_asset.len() as u32).to_be_bytes());
        for ((pool, asset), amount) in &self.accrued_by_asset {
            out.extend_from_slice(&pool.0 .0);
            out.extend_from_slice(&asset.0 .0);
            out.extend_from_slice(&amount.to_be_bytes());
        }
        out
    }

    /// Restaura métricas desde encoding canónico.
    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, AmmError> {
        let mut cur = bytes;
        let schema = read_u32(&mut cur)?;
        if schema != 1 {
            return Err(AmmError::InvalidEncoding);
        }
        let mut tracker = Self::new();
        let n = read_u32(&mut cur)? as usize;
        for _ in 0..n {
            let pool = PoolId(Hash(read_array::<32>(&mut cur)?));
            let amount = read_u128(&mut cur)?;
            tracker.accrued_raw.insert(pool, amount);
        }
        let n2 = read_u32(&mut cur)? as usize;
        for _ in 0..n2 {
            let pool = PoolId(Hash(read_array::<32>(&mut cur)?));
            let asset = AssetId(Hash(read_array::<32>(&mut cur)?));
            let amount = read_u128(&mut cur)?;
            tracker.accrued_by_asset.insert((pool, asset), amount);
        }
        if !cur.is_empty() {
            return Err(AmmError::InvalidEncoding);
        }
        Ok(tracker)
    }
}

fn read_array<const N: usize>(cur: &mut &[u8]) -> Result<[u8; N], AmmError> {
    if cur.len() < N {
        return Err(AmmError::InvalidEncoding);
    }
    let (head, tail) = cur.split_at(N);
    *cur = tail;
    let mut out = [0u8; N];
    out.copy_from_slice(head);
    Ok(out)
}

fn read_u32(cur: &mut &[u8]) -> Result<u32, AmmError> {
    Ok(u32::from_be_bytes(read_array::<4>(cur)?))
}

fn read_u128(cur: &mut &[u8]) -> Result<u128, AmmError> {
    Ok(u128::from_be_bytes(read_array::<16>(cur)?))
}
