//! Aritmética entera con comprobación de desbordamiento.
//!
//! ## Redondeo
//!
//! - [`mul_div_floor`]: trunca hacia abajo (`floor(a * b / d)`).
//! - [`mul_div_ceil`]: redondea hacia arriba si el resto es distinto de cero.
//!
//! Los productos `a * b` se calculan en 256 bits internos (par `hi`/`lo` de
//! `u128`) para no perder precisión antes de dividir. No se usa `f32`/`f64`
//! ni crates externos de enteros anchos: el workspace trabaja en `u128` y
//! ensancha solo en intermedios.

use crate::AmmError;
use rsc_core::Amount;

/// Modo de redondeo explícito para operaciones de cociente.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rounding {
    /// Hacia abajo (truncación).
    Floor,
    /// Hacia arriba si hay resto.
    Ceil,
}

/// Suma sin wrapping.
pub fn checked_add(a: u128, b: u128) -> Result<u128, AmmError> {
    a.checked_add(b).ok_or(AmmError::Overflow)
}

/// Resta sin ir a negativo.
pub fn checked_sub(a: u128, b: u128) -> Result<u128, AmmError> {
    a.checked_sub(b).ok_or(AmmError::Underflow)
}

/// Multiplicación sin wrapping en `u128`.
pub fn checked_mul(a: u128, b: u128) -> Result<u128, AmmError> {
    a.checked_mul(b).ok_or(AmmError::Overflow)
}

/// División entera (floor). Rechaza divisor cero.
pub fn checked_div(a: u128, b: u128) -> Result<u128, AmmError> {
    if b == 0 {
        return Err(AmmError::DivisionByZero);
    }
    Ok(a / b)
}

/// Producto completo `a * b` como `(hi, lo)` de 256 bits.
///
/// `value = hi * 2^128 + lo`.
pub fn full_mul(a: u128, b: u128) -> (u128, u128) {
    let a_lo = a as u64 as u128;
    let a_hi = a >> 64;
    let b_lo = b as u64 as u128;
    let b_hi = b >> 64;

    let p0 = a_lo * b_lo;
    let p1 = a_lo * b_hi;
    let p2 = a_hi * b_lo;
    let p3 = a_hi * b_hi;

    let mid = (p0 >> 64) + (p1 & 0xffff_ffff_ffff_ffff) + (p2 & 0xffff_ffff_ffff_ffff);
    let lo = (p0 & 0xffff_ffff_ffff_ffff) | (mid << 64);
    let hi = p3 + (p1 >> 64) + (p2 >> 64) + (mid >> 64);
    (hi, lo)
}

/// Compara dos enteros 256-bit `(hi, lo)`.
#[inline]
fn cmp_256(a_hi: u128, a_lo: u128, b_hi: u128, b_lo: u128) -> core::cmp::Ordering {
    (a_hi, a_lo).cmp(&(b_hi, b_lo))
}

/// `floor((hi||lo) / d)` con `hi < d` (cociente en `u128`), por búsqueda binaria.
pub(crate) fn div_256_by_128(hi: u128, lo: u128, d: u128) -> Result<u128, AmmError> {
    if d == 0 {
        return Err(AmmError::DivisionByZero);
    }
    if hi == 0 {
        return Ok(lo / d);
    }
    if hi >= d {
        return Err(AmmError::Overflow);
    }

    let mut low = 0u128;
    let mut high = u128::MAX;
    while low < high {
        // Sesgo hacia arriba sin overflow cuando `high == u128::MAX`.
        let mid = high - ((high - low) >> 1);
        let (ph, pl) = full_mul(mid, d);
        if cmp_256(ph, pl, hi, lo).is_le() {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    Ok(low)
}

/// `q * (d_hi||d_lo)` como entero 384-bit `(hi, mid, lo)`.
fn mul_128_by_256(q: u128, d_hi: u128, d_lo: u128) -> (u128, u128, u128) {
    let (a_hi, a_lo) = full_mul(q, d_lo);
    let (b_hi, b_lo) = full_mul(q, d_hi);
    let (mid, c0) = b_lo.overflowing_add(a_hi);
    let hi = b_hi + u128::from(c0);
    (hi, mid, a_lo)
}

/// `floor((n_hi||n_lo) / (d_hi||d_lo))` con cociente en `u128`.
pub(crate) fn div_256_by_256(
    n_hi: u128,
    n_lo: u128,
    d_hi: u128,
    d_lo: u128,
) -> Result<u128, AmmError> {
    if d_hi == 0 && d_lo == 0 {
        return Err(AmmError::DivisionByZero);
    }
    if d_hi == 0 {
        return div_256_by_128(n_hi, n_lo, d_lo);
    }
    if cmp_256(n_hi, n_lo, d_hi, d_lo).is_lt() {
        return Ok(0);
    }

    let mut low = 0u128;
    let mut high = u128::MAX;
    while low < high {
        let mid = high - ((high - low) >> 1);
        let (ph, pm, pl) = mul_128_by_256(mid, d_hi, d_lo);
        let le = if ph > 0 {
            false
        } else {
            (pm, pl) <= (n_hi, n_lo)
        };
        if le {
            low = mid;
        } else if mid == 0 {
            return Ok(0);
        } else {
            high = mid - 1;
        }
    }
    Ok(low)
}

/// Suma `(hi||lo) + addend` en 256 bits.
pub(crate) fn add_to_256(hi: u128, lo: u128, addend: u128) -> Result<(u128, u128), AmmError> {
    let (lo2, c) = lo.overflowing_add(addend);
    let hi2 = hi.checked_add(u128::from(c)).ok_or(AmmError::Overflow)?;
    Ok((hi2, lo2))
}

/// `floor(a * b / denominator)` con producto de 256 bits.
pub fn mul_div_floor(a: u128, b: u128, denominator: u128) -> Result<u128, AmmError> {
    if denominator == 0 {
        return Err(AmmError::DivisionByZero);
    }
    let (hi, lo) = full_mul(a, b);
    div_256_by_128(hi, lo, denominator)
}

/// `ceil(a * b / denominator)` con producto de 256 bits.
pub fn mul_div_ceil(a: u128, b: u128, denominator: u128) -> Result<u128, AmmError> {
    if denominator == 0 {
        return Err(AmmError::DivisionByZero);
    }
    let (hi, lo) = full_mul(a, b);
    let q = div_256_by_128(hi, lo, denominator)?;
    let (ph, pl) = full_mul(q, denominator);
    if ph != hi || pl != lo {
        checked_add(q, 1)
    } else {
        Ok(q)
    }
}

/// `floor((a * b) / (d_hi||d_lo))` — numerador 256, denominador 256.
pub fn mul_div_floor_wide_den(
    a: u128,
    b: u128,
    d_hi: u128,
    d_lo: u128,
) -> Result<u128, AmmError> {
    let (n_hi, n_lo) = full_mul(a, b);
    div_256_by_256(n_hi, n_lo, d_hi, d_lo)
}

/// `ceil((a * b) / denominator)` ya cubierto; ceil con denom 256-bit.
pub fn mul_div_ceil_wide_den(
    a: u128,
    b: u128,
    d_hi: u128,
    d_lo: u128,
) -> Result<u128, AmmError> {
    let (n_hi, n_lo) = full_mul(a, b);
    let q = div_256_by_256(n_hi, n_lo, d_hi, d_lo)?;
    let (ph, pm, pl) = mul_128_by_256(q, d_hi, d_lo);
    let exact = ph == 0 && pm == n_hi && pl == n_lo;
    if exact {
        Ok(q)
    } else {
        checked_add(q, 1)
    }
}

/// Raíz cuadrada entera floor (`floor(sqrt(x))`).
pub fn isqrt(x: u128) -> u128 {
    x.isqrt()
}

/// `floor(sqrt(hi||lo))` con resultado en `u128`.
pub fn isqrt_256(hi: u128, lo: u128) -> Result<u128, AmmError> {
    if hi == 0 {
        return Ok(isqrt(lo));
    }

    let mut shift = 128u32;
    let mut h = hi;
    while h > 0 {
        h >>= 1;
        shift += 1;
    }
    let mut x = 1u128 << ((shift + 1) / 2).min(127);

    loop {
        let quot = div_256_by_128(hi, lo, x)?;
        let sum = x.checked_add(quot).unwrap_or(u128::MAX);
        let x_new = sum / 2;
        if x_new >= x {
            return Ok(x);
        }
        x = x_new;
    }
}

/// Construye `Amount` desde `u128`.
pub fn amount_from_u128(value: u128) -> Amount {
    Amount(value)
}

/// Vista `u128` de un [`Amount`].
pub fn amount_to_u128(amount: Amount) -> u128 {
    amount.0
}

/// Intenta convertir un valor a `u128`.
pub fn try_into_u128<T: TryInto<u128>>(value: T) -> Result<u128, AmmError> {
    value.try_into().map_err(|_| AmmError::ConversionFailed)
}

/// Helpers sobre [`Amount`].
pub mod amounts {
    use super::*;

    /// Suma de [`Amount`].
    pub fn add(a: Amount, b: Amount) -> Result<Amount, AmmError> {
        checked_add(a.0, b.0).map(Amount)
    }

    /// Resta de [`Amount`].
    pub fn sub(a: Amount, b: Amount) -> Result<Amount, AmmError> {
        checked_sub(a.0, b.0).map(Amount)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn add_sub_mul_div_edges() {
        assert_eq!(checked_add(1, 2).unwrap(), 3);
        assert_eq!(checked_add(u128::MAX, 1).unwrap_err(), AmmError::Overflow);
        assert_eq!(checked_sub(5, 3).unwrap(), 2);
        assert_eq!(checked_sub(1, 2).unwrap_err(), AmmError::Underflow);
        assert_eq!(checked_mul(2, 3).unwrap(), 6);
        assert_eq!(checked_mul(u128::MAX, 2).unwrap_err(), AmmError::Overflow);
        assert_eq!(checked_div(10, 3).unwrap(), 3);
        assert_eq!(checked_div(1, 0).unwrap_err(), AmmError::DivisionByZero);
    }

    #[test]
    fn full_mul_matches_checked_when_fits() {
        let (hi, lo) = full_mul(3, 7);
        assert_eq!(hi, 0);
        assert_eq!(lo, 21);
        let (hi, lo) = full_mul(u128::MAX, u128::MAX);
        assert_eq!(hi, u128::MAX - 1);
        assert_eq!(lo, 1);
    }

    #[test]
    fn mul_div_floor_and_ceil() {
        assert_eq!(mul_div_floor(10, 10, 3).unwrap(), 33);
        assert_eq!(mul_div_ceil(10, 10, 3).unwrap(), 34);
        assert_eq!(mul_div_floor(5, 5, 5).unwrap(), 5);
        assert_eq!(mul_div_ceil(5, 5, 5).unwrap(), 5);
        assert_eq!(
            mul_div_floor(1, 1, 0).unwrap_err(),
            AmmError::DivisionByZero
        );
    }

    #[test]
    fn mul_div_wide_product() {
        let a = 1u128 << 127;
        assert_eq!(mul_div_floor(a, 2, 2).unwrap(), a);
        assert_eq!(
            mul_div_floor(u128::MAX, u128::MAX, u128::MAX).unwrap(),
            u128::MAX
        );
    }

    #[test]
    fn isqrt_basic() {
        assert_eq!(isqrt(0), 0);
        assert_eq!(isqrt(1), 1);
        assert_eq!(isqrt(15), 3);
        assert_eq!(isqrt(16), 4);
        let x = 10_000u128;
        let r = isqrt(x);
        assert!(r * r <= x);
        assert!((r + 1) * (r + 1) > x);
    }

    #[test]
    fn isqrt_max() {
        let r = isqrt(u128::MAX);
        let r2 = r.checked_mul(r).expect("r^2 fits");
        assert!(r2 <= u128::MAX);
        assert!((r + 1).checked_mul(r + 1).is_none());
    }

    #[test]
    fn isqrt_256_matches_isqrt_when_hi_zero() {
        assert_eq!(isqrt_256(0, 144).unwrap(), 12);
        assert_eq!(isqrt_256(1, 0).unwrap(), 1u128 << 64);
    }

    #[test]
    fn deterministic_same_inputs() {
        let a = mul_div_floor(12345, 67890, 42).unwrap();
        let b = mul_div_floor(12345, 67890, 42).unwrap();
        assert_eq!(a, b);
    }

    proptest! {
        #[test]
        fn prop_mul_div_floor_le_ceil(
            a in 0u128..1_000_000,
            b in 0u128..1_000_000,
            d in 1u128..1_000_000
        ) {
            let f = mul_div_floor(a, b, d).unwrap();
            let c = mul_div_ceil(a, b, d).unwrap();
            assert!(f <= c);
            assert!(c - f <= 1);
        }

        #[test]
        fn prop_isqrt(x in 0u128..) {
            let r = isqrt(x);
            assert!(r.checked_mul(r).map(|p| p <= x).unwrap_or(true));
        }

        #[test]
        fn prop_mul_div_matches_when_no_overflow(
            a in 0u128..1_000_000,
            b in 0u128..1_000_000,
            d in 1u128..1_000_000
        ) {
            let slow = a.checked_mul(b).map(|p| p / d).unwrap();
            assert_eq!(mul_div_floor(a, b, d).unwrap(), slow);
        }
    }
}
