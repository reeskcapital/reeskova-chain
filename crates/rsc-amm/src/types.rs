//! Tipos de datos del AMM.
//!
//! Reutiliza [`rsc_core::Amount`] (`u128`) para saldos y cantidades. No
//! redefine balances nativos ni introduce floats.

use crate::constants::{BPS_DENOMINATOR, DEFAULT_SWAP_FEE_BPS, MAX_SWAP_FEE_BPS};
use crate::AmmError;
use core::fmt;
use core::str::FromStr;
use rsc_core::{Amount, Hash};
use serde::{Deserialize, Serialize};

/// Identificador de activo negociable en el AMM.
///
/// RSC v1 solo tiene saldo nativo en el ledger; este tipo prepara el índice
/// de pools multi-activo sin duplicar `Amount`. La semántica concreta del
/// bytes (hash de metadata, id de mint, etc.) la fija la capa de integración.
#[derive(
    Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct AssetId(pub Hash);

impl AssetId {
    /// Valor nulo.
    pub const ZERO: Self = Self(Hash::ZERO);

    /// Construye desde 32 bytes.
    pub fn from_slice(slice: &[u8]) -> Result<Self, AmmError> {
        Hash::from_slice(slice)
            .map(Self)
            .map_err(|_| AmmError::ConversionFailed)
    }

    /// Hex con o sin `0x`.
    pub fn from_hex(s: &str) -> Result<Self, AmmError> {
        Hash::from_hex(s)
            .map(Self)
            .map_err(|_| AmmError::ConversionFailed)
    }

    /// Hex en minúsculas con prefijo `0x`.
    pub fn to_hex(&self) -> String {
        self.0.to_hex()
    }
}

impl fmt::Debug for AssetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AssetId({})", self.to_hex())
    }
}

impl fmt::Display for AssetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl FromStr for AssetId {
    type Err = AmmError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_hex(s)
    }
}

/// Identificador de pool.
#[derive(
    Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct PoolId(pub Hash);

impl PoolId {
    /// Valor nulo.
    pub const ZERO: Self = Self(Hash::ZERO);

    /// Hex con prefijo `0x`.
    pub fn to_hex(&self) -> String {
        self.0.to_hex()
    }
}

impl fmt::Debug for PoolId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PoolId({})", self.to_hex())
    }
}

impl fmt::Display for PoolId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// Par canónico de activos (`asset_a < asset_b` por orden de bytes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PoolKey {
    /// Activo menor en orden lexicográfico.
    pub asset_a: AssetId,
    /// Activo mayor en orden lexicográfico.
    pub asset_b: AssetId,
}

impl PoolKey {
    /// Ordena el par. Rechaza activos iguales.
    pub fn new(left: AssetId, right: AssetId) -> Result<Self, AmmError> {
        if left == right {
            return Err(AmmError::InvalidPair);
        }
        if left < right {
            Ok(Self {
                asset_a: left,
                asset_b: right,
            })
        } else {
            Ok(Self {
                asset_a: right,
                asset_b: left,
            })
        }
    }
}

/// Fee de swap en basis points respecto a [`BPS_DENOMINATOR`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FeeBps(pub u16);

impl FeeBps {
    /// Fee cero.
    pub const ZERO: Self = Self(0);

    /// Default propuesto del protocolo.
    pub const fn default_swap() -> Self {
        Self(DEFAULT_SWAP_FEE_BPS)
    }

    /// Valida contra el tope de política y el denominador BPS.
    ///
    /// Exige `fee < BPS_DENOMINATOR` y `fee <= MAX_SWAP_FEE_BPS`.
    pub fn new(bps: u16) -> Result<Self, AmmError> {
        if u128::from(bps) >= BPS_DENOMINATOR || bps > MAX_SWAP_FEE_BPS {
            return Err(AmmError::InvalidFee);
        }
        Ok(Self(bps))
    }

    /// Valor crudo en basis points.
    pub const fn get(self) -> u16 {
        self.0
    }

    /// Factor `(BPS - fee)` como `u128`.
    pub fn complement(self) -> u128 {
        BPS_DENOMINATOR - u128::from(self.0)
    }
}

impl Default for FeeBps {
    fn default() -> Self {
        Self::default_swap()
    }
}

/// Reservas de un pool de dos activos. No posee identidad ni mutadores.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Reserves {
    /// Reserva del activo A.
    pub reserve_a: Amount,
    /// Reserva del activo B.
    pub reserve_b: Amount,
}

impl Reserves {
    /// Construye reservas. No valida ceros (eso lo hace la capa matemática).
    pub const fn new(reserve_a: Amount, reserve_b: Amount) -> Self {
        Self {
            reserve_a,
            reserve_b,
        }
    }
}

/// Shares de proveedor de liquidez. Misma unidad entera que [`Amount`].
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct LpShares(pub u128);

impl LpShares {
    /// Cero.
    pub const ZERO: Self = Self(0);

    /// Vista como [`Amount`].
    pub const fn as_amount(self) -> Amount {
        Amount(self.0)
    }

    /// Desde [`Amount`].
    pub const fn from_amount(amount: Amount) -> Self {
        Self(amount.0)
    }
}

/// Resultado de un cálculo de swap (solo cantidades; sin mutar el pool).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SwapAmounts {
    /// Cantidad de entrada (bruta, antes de fee implícito en la fórmula).
    pub amount_in: Amount,
    /// Cantidad de salida.
    pub amount_out: Amount,
}

/// Parámetros de protocolo configurables (gobernanza / génesis AMM).
///
/// No son constantes matemáticas. Las funciones puras reciben los campos
/// que necesitan de forma explícita.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolParams {
    /// Fee de swap en basis points.
    pub swap_fee_bps: FeeBps,
    /// Liquidez mínima a bloquear al crear el pool. `0` desactiva el lock.
    pub min_liquidity: u128,
    /// Tope de slippage en bps (capa de ejecución). `None` = sin tope aquí.
    pub max_slippage_bps: Option<u16>,
}

impl Default for ProtocolParams {
    fn default() -> Self {
        Self {
            swap_fee_bps: FeeBps::default_swap(),
            min_liquidity: crate::constants::DEFAULT_MIN_LIQUIDITY,
            max_slippage_bps: Some(crate::constants::DEFAULT_MAX_SLIPPAGE_BPS),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::{BPS_DENOMINATOR, MAX_SWAP_FEE_BPS};

    #[test]
    fn pool_key_orders_and_rejects_equal() {
        let a = AssetId(Hash([1; 32]));
        let b = AssetId(Hash([2; 32]));
        let key = PoolKey::new(b, a).unwrap();
        assert_eq!(key.asset_a, a);
        assert_eq!(key.asset_b, b);
        assert_eq!(PoolKey::new(a, a).unwrap_err(), AmmError::InvalidPair);
    }

    #[test]
    fn fee_bps_bounds() {
        assert!(FeeBps::new(30).is_ok());
        assert_eq!(
            FeeBps::new(MAX_SWAP_FEE_BPS + 1).unwrap_err(),
            AmmError::InvalidFee
        );
        assert_eq!(
            FeeBps::new(BPS_DENOMINATOR as u16).unwrap_err(),
            AmmError::InvalidFee
        );
    }
}
