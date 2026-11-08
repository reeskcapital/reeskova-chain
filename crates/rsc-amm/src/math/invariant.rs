//! Matemática de producto constante (x * y = k).
//!
//! ## Fórmulas (fee en basis points)
//!
//! Exact-input (`amount_in` → `amount_out`), redondeo **hacia abajo**:
//! ```text
//! amount_in_with_fee = amount_in * (BPS - fee_bps)
//! amount_out = floor( amount_in_with_fee * reserve_out
//!                     / (reserve_in * BPS + amount_in_with_fee) )
//! ```
//!
//! Exact-output (`amount_out` → `amount_in`), redondeo **hacia arriba**:
//! ```text
//! amount_in = ceil( reserve_in * amount_out * BPS
//!                   / ((reserve_out - amount_out) * (BPS - fee_bps)) )
//! ```
//!
//! El denominador `reserve_in * BPS + …` se calcula en 256 bits para soportar
//! reservas cercanas a `u128::MAX`. Ninguna función muta reservas. El fee se
//! pasa como argumento; no se lee [`crate::constants::DEFAULT_SWAP_FEE_BPS`].

use crate::constants::{BPS_DENOMINATOR, PRICE_SCALE};
use crate::math::amount::{
    add_to_256, checked_add, checked_mul, checked_sub, full_mul, mul_div_ceil_wide_den,
    mul_div_floor, mul_div_floor_wide_den,
};
use crate::types::{FeeBps, Reserves, SwapAmounts};
use crate::AmmError;
use rsc_core::Amount;

/// Producto de reservas `(reserve_a * reserve_b)` en 256 bits `(hi, lo)`.
pub fn product(reserves: Reserves) -> Result<(u128, u128), AmmError> {
    validate_reserves(reserves)?;
    Ok(full_mul(reserves.reserve_a.0, reserves.reserve_b.0))
}

/// Exige reservas estrictamente positivas.
pub fn validate_reserves(reserves: Reserves) -> Result<(), AmmError> {
    if reserves.reserve_a.0 == 0 || reserves.reserve_b.0 == 0 {
        return Err(AmmError::InvalidReserves);
    }
    Ok(())
}

/// Comprueba que el producto post ≥ producto pre (en 256 bits).
pub fn validate_invariant(before: Reserves, after: Reserves) -> Result<(), AmmError> {
    let (b_hi, b_lo) = product(before)?;
    let (a_hi, a_lo) = product(after)?;
    if (a_hi, a_lo) < (b_hi, b_lo) {
        return Err(AmmError::InvariantViolation);
    }
    Ok(())
}

/// Precio spot `floor(reserve_out * PRICE_SCALE / reserve_in)`.
pub fn spot_price(reserve_in: Amount, reserve_out: Amount) -> Result<u128, AmmError> {
    if reserve_in.0 == 0 || reserve_out.0 == 0 {
        return Err(AmmError::InvalidReserves);
    }
    mul_div_floor(reserve_out.0, PRICE_SCALE, reserve_in.0)
}

/// Salida para una entrada exacta (swap exact-input).
pub fn amount_out_for_exact_in(
    amount_in: Amount,
    reserve_in: Amount,
    reserve_out: Amount,
    fee: FeeBps,
) -> Result<Amount, AmmError> {
    if amount_in.0 == 0 {
        return Err(AmmError::ZeroAmount);
    }
    if reserve_in.0 == 0 || reserve_out.0 == 0 {
        return Err(AmmError::InvalidReserves);
    }
    let fee_factor = fee.complement();
    if fee_factor == 0 {
        return Err(AmmError::InvalidFee);
    }

    let amount_in_with_fee = checked_mul(amount_in.0, fee_factor)?;
    let (den_hi, den_lo) = {
        let (h, l) = full_mul(reserve_in.0, BPS_DENOMINATOR);
        add_to_256(h, l, amount_in_with_fee)?
    };

    let out = mul_div_floor_wide_den(amount_in_with_fee, reserve_out.0, den_hi, den_lo)?;
    if out == 0 || out >= reserve_out.0 {
        return Err(AmmError::InsufficientLiquidity);
    }
    Ok(Amount(out))
}

/// Entrada requerida para una salida exacta (swap exact-output).
///
/// Redondea hacia arriba para no quedarse corto de `amount_out`.
pub fn amount_in_for_exact_out(
    amount_out: Amount,
    reserve_in: Amount,
    reserve_out: Amount,
    fee: FeeBps,
) -> Result<Amount, AmmError> {
    if amount_out.0 == 0 {
        return Err(AmmError::ZeroAmount);
    }
    if reserve_in.0 == 0 || reserve_out.0 == 0 {
        return Err(AmmError::InvalidReserves);
    }
    if amount_out.0 >= reserve_out.0 {
        return Err(AmmError::InsufficientLiquidity);
    }
    let fee_factor = fee.complement();
    if fee_factor == 0 {
        return Err(AmmError::InvalidFee);
    }

    let reserve_out_remaining = checked_sub(reserve_out.0, amount_out.0)?;
    let (den_hi, den_lo) = full_mul(reserve_out_remaining, fee_factor);

    // numerador = reserve_in * amount_out * BPS
    // = (reserve_in * amount_out) * BPS  — producto hasta 256+ bits vía
    //   primero (hi,lo)=reserve_in*amount_out, luego * BPS con wide ceil.
    let (n_hi, n_lo) = full_mul(reserve_in.0, amount_out.0);
    // ceil( (n_hi||n_lo) * BPS / (den_hi||den_lo) )
    // = ceil_wide( mul_256_by_bps / den )
    let amount_in = if n_hi == 0 {
        // n_lo * BPS puede ser 256-bit
        mul_div_ceil_wide_den(n_lo, BPS_DENOMINATOR, den_hi, den_lo)?
    } else {
        // (n_hi||n_lo) * BPS / den — numerador hasta 384 bits.
        // Usamos: ceil(n * BPS / den) = floor( (n * BPS + den - 1) / den )
        // Calculamos floor(n * BPS / den) y luego ajustamos si no es exacto.
        ceil_mul_bps_div_256(n_hi, n_lo, den_hi, den_lo)?
    };

    if amount_in == 0 {
        return Err(AmmError::InsufficientLiquidity);
    }
    Ok(Amount(amount_in))
}

/// `ceil( (n_hi||n_lo) * BPS / (d_hi||d_lo) )` con cociente en `u128`.
fn ceil_mul_bps_div_256(
    n_hi: u128,
    n_lo: u128,
    d_hi: u128,
    d_lo: u128,
) -> Result<u128, AmmError> {
    // num384 = (n_hi||n_lo) * BPS
    let (a_hi, a_lo) = full_mul(n_lo, BPS_DENOMINATOR);
    let (b_hi, b_lo) = full_mul(n_hi, BPS_DENOMINATOR);
    // 384-bit: (b_hi, b_lo, 0) + (0, a_hi, a_lo)
    let (mid, c0) = b_lo.overflowing_add(a_hi);
    let top = b_hi.checked_add(u128::from(c0)).ok_or(AmmError::Overflow)?;
    // num = (top, mid, a_lo). Para dividir por 256-bit den con cociente u128,
    // buscamos q máximo con q * den <= num.
    let mut low = 0u128;
    let mut high = u128::MAX;
    while low < high {
        let mid_q = high - ((high - low) >> 1);
        let (ph, pm, pl) = {
            let (aa_hi, aa_lo) = full_mul(mid_q, d_lo);
            let (bb_hi, bb_lo) = full_mul(mid_q, d_hi);
            let (m, c) = bb_lo.overflowing_add(aa_hi);
            (bb_hi + u128::from(c), m, aa_lo)
        };
        let le = (ph, pm, pl) <= (top, mid, a_lo);
        if le {
            low = mid_q;
        } else if mid_q == 0 {
            return Ok(0);
        } else {
            high = mid_q - 1;
        }
    }
    let q = low;
    let (ph, pm, pl) = {
        let (aa_hi, aa_lo) = full_mul(q, d_lo);
        let (bb_hi, bb_lo) = full_mul(q, d_hi);
        let (m, c) = bb_lo.overflowing_add(aa_hi);
        (bb_hi + u128::from(c), m, aa_lo)
    };
    if (ph, pm, pl) == (top, mid, a_lo) {
        Ok(q)
    } else {
        checked_add(q, 1)
    }
}

/// Exact-input empaquetado.
pub fn quote_exact_in(
    amount_in: Amount,
    reserve_in: Amount,
    reserve_out: Amount,
    fee: FeeBps,
) -> Result<SwapAmounts, AmmError> {
    let amount_out = amount_out_for_exact_in(amount_in, reserve_in, reserve_out, fee)?;
    Ok(SwapAmounts {
        amount_in,
        amount_out,
    })
}

/// Exact-output empaquetado.
pub fn quote_exact_out(
    amount_out: Amount,
    reserve_in: Amount,
    reserve_out: Amount,
    fee: FeeBps,
) -> Result<SwapAmounts, AmmError> {
    let amount_in = amount_in_for_exact_out(amount_out, reserve_in, reserve_out, fee)?;
    Ok(SwapAmounts {
        amount_in,
        amount_out,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::DEFAULT_SWAP_FEE_BPS;
    use proptest::prelude::*;

    fn fee() -> FeeBps {
        FeeBps::new(DEFAULT_SWAP_FEE_BPS).unwrap()
    }

    #[test]
    fn rejects_zero_reserves_and_amounts() {
        let z = Amount::ZERO;
        let r = Amount(1_000);
        assert_eq!(
            amount_out_for_exact_in(z, r, r, fee()).unwrap_err(),
            AmmError::ZeroAmount
        );
        assert_eq!(
            amount_out_for_exact_in(r, z, r, fee()).unwrap_err(),
            AmmError::InvalidReserves
        );
        assert_eq!(
            validate_reserves(Reserves::new(z, r)).unwrap_err(),
            AmmError::InvalidReserves
        );
    }

    #[test]
    fn exact_in_known_vector() {
        let amount_in = Amount(1_000);
        let reserve_in = Amount(100_000);
        let reserve_out = Amount(100_000);
        let out = amount_out_for_exact_in(amount_in, reserve_in, reserve_out, fee()).unwrap();
        // 997_000_000_000 / 1_009_970_000 = 987
        assert_eq!(out.0, 987);
    }

    #[test]
    fn exact_out_round_trip_covers_out() {
        let reserve_in = Amount(100_000);
        let reserve_out = Amount(100_000);
        let want_out = Amount(500);
        let need_in = amount_in_for_exact_out(want_out, reserve_in, reserve_out, fee()).unwrap();
        let got_out = amount_out_for_exact_in(need_in, reserve_in, reserve_out, fee()).unwrap();
        assert!(got_out.0 >= want_out.0);
    }

    #[test]
    fn invariant_holds_after_theoretical_swap() {
        let reserve_in = Amount(1_000_000);
        let reserve_out = Amount(1_000_000);
        let amount_in = Amount(10_000);
        let out = amount_out_for_exact_in(amount_in, reserve_in, reserve_out, fee()).unwrap();
        let before = Reserves::new(reserve_in, reserve_out);
        let after = Reserves::new(
            Amount(checked_add(reserve_in.0, amount_in.0).unwrap()),
            Amount(checked_sub(reserve_out.0, out.0).unwrap()),
        );
        validate_invariant(before, after).unwrap();
    }

    #[test]
    fn spot_price_scale() {
        let p = spot_price(Amount(2), Amount(8)).unwrap();
        assert_eq!(p, PRICE_SCALE * 4);
    }

    #[test]
    fn fee_increases_input_for_same_out() {
        let rin = Amount(5_000_000);
        let rout = Amount(5_000_000);
        let out = Amount(100_000);
        let with_fee = amount_in_for_exact_out(out, rin, rout, fee()).unwrap();
        let no_fee = amount_in_for_exact_out(out, rin, rout, FeeBps::ZERO).unwrap();
        assert!(with_fee.0 > no_fee.0);
    }

    #[test]
    fn large_and_small_amounts() {
        // Con fee 0, hace falta amount_in >= 2 para obtener out >= 1
        // frente a reservas ~u128::MAX/4 (floor(R/(R+1))=0 si in=1).
        let out = amount_out_for_exact_in(
            Amount(2),
            Amount(u128::MAX / 4),
            Amount(u128::MAX / 4),
            FeeBps::ZERO,
        )
        .unwrap();
        assert!(out.0 >= 1);
    }

    proptest! {
        #[test]
        fn prop_out_less_than_reserve(
            amount_in in 1u128..50_000,
            reserve_in in 50_000u128..500_000,
            reserve_out in 50_000u128..500_000,
        ) {
            let out = amount_out_for_exact_in(
                Amount(amount_in),
                Amount(reserve_in),
                Amount(reserve_out),
                fee(),
            ).unwrap();
            assert!(out.0 < reserve_out);
        }
    }
}
