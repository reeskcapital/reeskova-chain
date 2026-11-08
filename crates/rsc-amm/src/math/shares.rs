//! Cálculo de shares LP (liquidez).
//!
//! ## Redondeo
//!
//! - **Mint inicial**: `floor(sqrt(amount_a * amount_b))`. Si `min_liquidity > 0`,
//!   se bloquean `min_liquidity` shares en un sink permanente (no pertenecen a
//!   ningún LP; son liquidez muerta que ancla el pool). El minter recibe
//!   `shares_total - min_liquidity`.
//! - **Mint posterior**: `min( floor(amount_a * supply / reserve_a),
//!   floor(amount_b * supply / reserve_b) )` — redondeo hacia abajo.
//! - **Redeem / withdraw**: `floor(shares * reserve / supply)` en cada activo
//!   — redondeo hacia abajo (favorece al pool frente al retirante).
//!
//! ## Política del sink de liquidez mínima
//!
//! Las `min_liquidity` shares bloqueadas no son transferibles ni redimibles.
//! Contablemente viven en un sink protocolario (burn address / cuenta nula).
//! Esta capa solo calcula cantidades; el asiento contable lo hace la etapa
//! de accounting / ejecución.

use crate::math::amount::{checked_sub, full_mul, isqrt, isqrt_256, mul_div_floor};
use crate::types::{LpShares, Reserves};
use crate::AmmError;
use rsc_core::Amount;

/// Resultado de un mint de liquidez (sin mutar estado).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MintResult {
    /// Shares acuñadas para el proveedor.
    pub minted_to_provider: LpShares,
    /// Shares totales tras el mint (incluye lock si aplica).
    pub total_supply_after: LpShares,
    /// Shares bloqueadas en el sink en este mint (`0` si no es el inicial).
    pub locked_in_sink: LpShares,
}

/// Resultado de un retiro de liquidez (sin mutar estado).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RedeemResult {
    /// Cantidad del activo A a entregar.
    pub amount_a: Amount,
    /// Cantidad del activo B a entregar.
    pub amount_b: Amount,
    /// Supply total tras quemar las shares.
    pub total_supply_after: LpShares,
}

/// Shares iniciales: `isqrt(amount_a * amount_b)`, con lock opcional.
///
/// `min_liquidity == 0` desactiva el bloqueo.
pub fn initial_shares(
    amount_a: Amount,
    amount_b: Amount,
    min_liquidity: u128,
) -> Result<MintResult, AmmError> {
    if amount_a.0 == 0 || amount_b.0 == 0 {
        return Err(AmmError::ZeroAmount);
    }
    let (hi, lo) = full_mul(amount_a.0, amount_b.0);
    let liquidity = if hi == 0 {
        isqrt(lo)
    } else {
        isqrt_256(hi, lo)?
    };

    if liquidity == 0 {
        return Err(AmmError::InsufficientInitialLiquidity);
    }
    if liquidity <= min_liquidity {
        return Err(AmmError::InsufficientInitialLiquidity);
    }

    let to_provider = checked_sub(liquidity, min_liquidity)?;
    Ok(MintResult {
        minted_to_provider: LpShares(to_provider),
        total_supply_after: LpShares(liquidity),
        locked_in_sink: LpShares(min_liquidity),
    })
}

/// Mint posterior proporcional. Usa el mínimo de las dos proporciones (floor).
pub fn mint_shares(
    amount_a: Amount,
    amount_b: Amount,
    reserves: Reserves,
    total_supply: LpShares,
) -> Result<MintResult, AmmError> {
    if amount_a.0 == 0 || amount_b.0 == 0 {
        return Err(AmmError::ZeroAmount);
    }
    if total_supply.0 == 0 {
        return Err(AmmError::ZeroSupply);
    }
    if reserves.reserve_a.0 == 0 || reserves.reserve_b.0 == 0 {
        return Err(AmmError::InvalidReserves);
    }

    let shares_a = mul_div_floor(amount_a.0, total_supply.0, reserves.reserve_a.0)?;
    let shares_b = mul_div_floor(amount_b.0, total_supply.0, reserves.reserve_b.0)?;
    let minted = shares_a.min(shares_b);
    if minted == 0 {
        return Err(AmmError::InsufficientLiquidity);
    }
    let new_supply = total_supply
        .0
        .checked_add(minted)
        .ok_or(AmmError::Overflow)?;
    Ok(MintResult {
        minted_to_provider: LpShares(minted),
        total_supply_after: LpShares(new_supply),
        locked_in_sink: LpShares::ZERO,
    })
}

/// Valor teórico de `shares` en cada activo: `floor(shares * reserve / supply)`.
pub fn share_value(
    shares: LpShares,
    reserves: Reserves,
    total_supply: LpShares,
) -> Result<(Amount, Amount), AmmError> {
    redeem_amounts(shares, reserves, total_supply).map(|r| (r.amount_a, r.amount_b))
}

/// Retiro: cantidades a entregar y supply tras quemar (floor).
pub fn redeem_amounts(
    shares: LpShares,
    reserves: Reserves,
    total_supply: LpShares,
) -> Result<RedeemResult, AmmError> {
    if shares.0 == 0 {
        return Err(AmmError::ZeroAmount);
    }
    if total_supply.0 == 0 {
        return Err(AmmError::ZeroSupply);
    }
    if shares.0 > total_supply.0 {
        return Err(AmmError::InsufficientLiquidity);
    }
    let amount_a = Amount(mul_div_floor(
        shares.0,
        reserves.reserve_a.0,
        total_supply.0,
    )?);
    let amount_b = Amount(mul_div_floor(
        shares.0,
        reserves.reserve_b.0,
        total_supply.0,
    )?);
    let total_supply_after = LpShares(checked_sub(total_supply.0, shares.0)?);
    Ok(RedeemResult {
        amount_a,
        amount_b,
        total_supply_after,
    })
}

/// Participación en basis points: `floor(shares * BPS / total_supply)`.
pub fn ownership_bps(shares: LpShares, total_supply: LpShares) -> Result<u128, AmmError> {
    if total_supply.0 == 0 {
        return Err(AmmError::ZeroSupply);
    }
    mul_div_floor(shares.0, crate::constants::BPS_DENOMINATOR, total_supply.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::DEFAULT_MIN_LIQUIDITY;
    use proptest::prelude::*;

    #[test]
    fn initial_locks_minimum() {
        let m = initial_shares(Amount(10_000), Amount(10_000), DEFAULT_MIN_LIQUIDITY).unwrap();
        assert_eq!(m.locked_in_sink.0, DEFAULT_MIN_LIQUIDITY);
        assert_eq!(
            m.minted_to_provider.0 + m.locked_in_sink.0,
            m.total_supply_after.0
        );
        assert_eq!(m.total_supply_after.0, 10_000);
    }

    #[test]
    fn initial_rejects_too_small() {
        assert_eq!(
            initial_shares(Amount(10), Amount(10), DEFAULT_MIN_LIQUIDITY).unwrap_err(),
            AmmError::InsufficientInitialLiquidity
        );
    }

    #[test]
    fn initial_without_lock() {
        let m = initial_shares(Amount(100), Amount(100), 0).unwrap();
        assert_eq!(m.locked_in_sink.0, 0);
        assert_eq!(m.minted_to_provider.0, 100);
    }

    #[test]
    fn subsequent_mint_uses_min_proportion() {
        let reserves = Reserves::new(Amount(1000), Amount(1000));
        let supply = LpShares(1000);
        let m = mint_shares(Amount(100), Amount(200), reserves, supply).unwrap();
        assert_eq!(m.minted_to_provider.0, 100);
        assert_eq!(m.total_supply_after.0, 1100);
        assert_eq!(m.locked_in_sink.0, 0);
    }

    #[test]
    fn redeem_rounds_down() {
        let reserves = Reserves::new(Amount(1000), Amount(1000));
        let supply = LpShares(1000);
        let r = redeem_amounts(LpShares(1), reserves, supply).unwrap();
        assert_eq!(r.amount_a.0, 1);
        assert_eq!(r.amount_b.0, 1);
        let r = redeem_amounts(LpShares(333), reserves, supply).unwrap();
        assert_eq!(r.amount_a.0, 333);
    }

    #[test]
    fn ownership_bps_full() {
        assert_eq!(
            ownership_bps(LpShares(1), LpShares(1)).unwrap(),
            crate::constants::BPS_DENOMINATOR
        );
        assert_eq!(ownership_bps(LpShares(1), LpShares(2)).unwrap(), 5_000);
    }

    proptest! {
        #[test]
        fn prop_redeem_never_exceeds_reserves(
            shares in 1u128..10_000,
            ra in 1u128..100_000,
            rb in 1u128..100_000,
        ) {
            let supply = shares + 1;
            let r = redeem_amounts(
                LpShares(shares),
                Reserves::new(Amount(ra), Amount(rb)),
                LpShares(supply),
            ).unwrap();
            assert!(r.amount_a.0 <= ra);
            assert!(r.amount_b.0 <= rb);
        }
    }
}
