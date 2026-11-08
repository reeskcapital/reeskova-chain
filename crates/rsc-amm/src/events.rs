//! Eventos tipados y deterministas del AMM.
//!
//! Se acumulan en [`crate::pool::PoolRegistry`] durante una transición y solo
//! quedan visibles tras commit exitoso (las fallas hacen rollback del registry
//! junto con accounting). No incluyen relojes locales ni datos no canónicos.
//!
//! ## Consumo futuro
//!
//! No existe aún `rsc-indexer` ni un bus de eventos en `rsc-rpc`. Los eventos
//! usan `serde` + [`AmmEvent::canonical_bytes`] para que RPC/indexer los
//! indexen sin reinterpretar saldos (el ledger AMM sigue siendo la fuente
//! de verdad).

use crate::types::{AssetId, FeeBps, LpShares, PoolId};
use crate::AmmError;
use rsc_core::{AccountId, Amount, BlockHeight, Hash};
use serde::{Deserialize, Serialize};

/// Identificador de transacción / operación determinista (`Hash` de 32 bytes).
///
/// Compatible con [`rsc_core::TransactionId`] a nivel de bytes.
pub type TxId = Hash;

/// Evento emitido por una operación AMM confirmada.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AmmEvent {
    /// Pool registrado (sin liquidez ni activación).
    PoolCreated {
        /// Identidad determinista del pool.
        pool_id: PoolId,
        /// Activo A canónico.
        asset_a: AssetId,
        /// Activo B canónico.
        asset_b: AssetId,
        /// Fee de swap en basis points (forma parte de la identidad).
        fee_bps: FeeBps,
        /// Cuenta que solicitó la creación.
        creator: AccountId,
        /// Altura de bloque de la transición.
        block_height: BlockHeight,
    },
    /// Pool pasado a activo.
    PoolActivated {
        /// Identidad del pool.
        pool_id: PoolId,
        /// Autoridad de gobernanza / emergencia que activó.
        authority: AccountId,
        /// Altura de bloque de la transición.
        block_height: BlockHeight,
    },
    /// Pool pausado.
    PoolPaused {
        /// Identidad del pool.
        pool_id: PoolId,
        /// Autoridad de gobernanza / emergencia que pausó.
        authority: AccountId,
        /// Altura de bloque de la transición.
        block_height: BlockHeight,
    },
    /// Liquidez depositada.
    LiquidityAdded {
        /// Pool.
        pool_id: PoolId,
        /// Proveedor.
        provider: AccountId,
        /// Cantidad del activo A depositada.
        amount_a: Amount,
        /// Cantidad del activo B depositada.
        amount_b: Amount,
        /// Shares acuñadas al proveedor (sin el sink).
        lp_minted: LpShares,
        /// Shares bloqueadas en el sink en este aporte.
        lp_locked: LpShares,
        /// Altura de bloque.
        block_height: BlockHeight,
        /// Id de transacción / operación.
        transaction_id: Option<TxId>,
    },
    /// Liquidez retirada.
    LiquidityRemoved {
        /// Pool.
        pool_id: PoolId,
        /// Proveedor.
        provider: AccountId,
        /// Cantidad del activo A retirada.
        amount_a: Amount,
        /// Cantidad del activo B retirada.
        amount_b: Amount,
        /// Shares quemadas.
        lp_burned: LpShares,
        /// Altura de bloque.
        block_height: BlockHeight,
        /// Id de transacción / operación.
        transaction_id: Option<TxId>,
    },
    /// Swap ejecutado.
    SwapExecuted {
        /// Pool.
        pool_id: PoolId,
        /// Trader.
        trader: AccountId,
        /// Activo de entrada.
        token_in: AssetId,
        /// Activo de salida.
        token_out: AssetId,
        /// Cantidad de entrada.
        amount_in: Amount,
        /// Cantidad de salida.
        amount_out: Amount,
        /// Fee bruto (retenido en reservas; no es un vault aparte).
        fee_amount: Amount,
        /// Altura de bloque.
        block_height: BlockHeight,
        /// Id de transacción.
        transaction_id: Option<TxId>,
    },
    /// Fee de swap contabilizado (métrica; fondos ya en reservas).
    FeeCollected {
        /// Pool.
        pool_id: PoolId,
        /// Activo sobre el que se midió el fee (input del swap).
        asset: AssetId,
        /// Monto de fee reportado.
        amount: Amount,
        /// Porción LP (hoy = `amount`; protocol fee = 0).
        lp_portion: Amount,
        /// Porción protocolo (hoy = 0; retenido en pool vía LPs).
        protocol_portion: Amount,
        /// Altura de bloque.
        block_height: BlockHeight,
        /// Id de transacción.
        transaction_id: Option<TxId>,
    },
    /// LP shares acuñadas a una cuenta (proveedor o sink).
    LPSharesMinted {
        /// Pool.
        pool_id: PoolId,
        /// Receptor.
        to: AccountId,
        /// Shares.
        shares: LpShares,
        /// Altura de bloque.
        block_height: BlockHeight,
        /// Id de transacción.
        transaction_id: Option<TxId>,
    },
    /// LP shares quemadas de un proveedor.
    LPSharesBurned {
        /// Pool.
        pool_id: PoolId,
        /// Cuenta que quema.
        from: AccountId,
        /// Shares.
        shares: LpShares,
        /// Altura de bloque.
        block_height: BlockHeight,
        /// Id de transacción.
        transaction_id: Option<TxId>,
    },
}

impl AmmEvent {
    /// Discriminante estable u8 para encoding canónico.
    pub fn tag(&self) -> u8 {
        match self {
            Self::PoolCreated { .. } => 1,
            Self::PoolActivated { .. } => 2,
            Self::PoolPaused { .. } => 3,
            Self::LiquidityAdded { .. } => 4,
            Self::LiquidityRemoved { .. } => 5,
            Self::SwapExecuted { .. } => 6,
            Self::FeeCollected { .. } => 7,
            Self::LPSharesMinted { .. } => 8,
            Self::LPSharesBurned { .. } => 9,
        }
    }

    /// Altura de bloque asociada.
    pub fn block_height(&self) -> BlockHeight {
        match self {
            Self::PoolCreated { block_height, .. }
            | Self::PoolActivated { block_height, .. }
            | Self::PoolPaused { block_height, .. }
            | Self::LiquidityAdded { block_height, .. }
            | Self::LiquidityRemoved { block_height, .. }
            | Self::SwapExecuted { block_height, .. }
            | Self::FeeCollected { block_height, .. }
            | Self::LPSharesMinted { block_height, .. }
            | Self::LPSharesBurned { block_height, .. } => *block_height,
        }
    }

    /// Id de transacción si la variante lo porta.
    pub fn transaction_id(&self) -> Option<TxId> {
        match self {
            Self::LiquidityAdded { transaction_id, .. }
            | Self::LiquidityRemoved { transaction_id, .. }
            | Self::SwapExecuted { transaction_id, .. }
            | Self::FeeCollected { transaction_id, .. }
            | Self::LPSharesMinted { transaction_id, .. }
            | Self::LPSharesBurned { transaction_id, .. } => *transaction_id,
            _ => None,
        }
    }

    /// Serialización JSON determinista (campos en orden de struct / serde).
    pub fn to_json(&self) -> Result<String, AmmError> {
        serde_json::to_string(self).map_err(|_| AmmError::InvalidEncoding)
    }

    /// Encoding canónico big-endian mínimo (tag + block + opcional tx id).
    ///
    /// Suficiente para indexación por altura/tx; el detalle completo va en JSON
    /// o en el encoding del host.
    pub fn canonical_header_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(1 + 8 + 1 + 32);
        out.push(self.tag());
        out.extend_from_slice(&self.block_height().0.to_be_bytes());
        match self.transaction_id() {
            Some(txid) => {
                out.push(1);
                out.extend_from_slice(&txid.0);
            }
            None => out.push(0),
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_roundtrip_swap_executed() {
        let e = AmmEvent::SwapExecuted {
            pool_id: PoolId(Hash([1; 32])),
            trader: AccountId([2; 32]),
            token_in: AssetId(Hash([3; 32])),
            token_out: AssetId(Hash([4; 32])),
            amount_in: Amount(100),
            amount_out: Amount(90),
            fee_amount: Amount(1),
            block_height: BlockHeight(7),
            transaction_id: Some(Hash([9; 32])),
        };
        let s = e.to_json().unwrap();
        let back: AmmEvent = serde_json::from_str(&s).unwrap();
        assert_eq!(e, back);
        assert_eq!(e.tag(), 6);
        assert_eq!(e.transaction_id(), Some(Hash([9; 32])));
    }

    #[test]
    fn header_bytes_deterministic() {
        let e = AmmEvent::FeeCollected {
            pool_id: PoolId(Hash([1; 32])),
            asset: AssetId(Hash([2; 32])),
            amount: Amount(3),
            lp_portion: Amount(3),
            protocol_portion: Amount(0),
            block_height: BlockHeight(10),
            transaction_id: None,
        };
        assert_eq!(e.canonical_header_bytes(), e.canonical_header_bytes());
    }
}
