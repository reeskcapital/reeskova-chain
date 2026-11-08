//! Stage 7 — seguridad matemática y redondeo (motor real, sin floats).

use crate::fees::{fee_amount, split_fee};
use crate::math::amount::{
    checked_add, checked_div, checked_mul, checked_sub, isqrt, isqrt_256, mul_div_ceil,
    mul_div_floor,
};
use crate::math::invariant::{
    amount_in_for_exact_out, amount_out_for_exact_in, validate_invariant,
};
use crate::math::shares::{initial_shares, mint_shares, redeem_amounts};
use crate::types::{FeeBps, LpShares, Reserves};
use crate::AmmError;
use proptest::prelude::*;
use rsc_core::Amount;

#[test]
fn overflow_underflow_divzero_explicit() {
    assert_eq!(checked_add(u128::MAX, 1).unwrap_err(), AmmError::Overflow);
    assert_eq!(checked_sub(0, 1).unwrap_err(), AmmError::Underflow);
    assert_eq!(checked_mul(u128::MAX, 2).unwrap_err(), AmmError::Overflow);
    assert_eq!(checked_div(1, 0).unwrap_err(), AmmError::DivisionByZero);
    assert_eq!(
        mul_div_floor(1, 1, 0).unwrap_err(),
        AmmError::DivisionByZero
    );
}

#[test]
fn zero_and_min_unit_swaps_rejected_or_safe() {
    let r_in = Amount(1_000_000);
    let r_out = Amount(1_000_000);
    let fee = FeeBps::new(30).unwrap();
    assert_eq!(
        amount_out_for_exact_in(Amount(0), r_in, r_out, fee).unwrap_err(),
        AmmError::ZeroAmount
    );
    // Con fee 30 bps, amount_in=1 redondea a salida 0 → InsufficientLiquidity (explícito).
    assert_eq!(
        amount_out_for_exact_in(Amount(1), r_in, r_out, fee).unwrap_err(),
        AmmError::InsufficientLiquidity
    );
    let out = amount_out_for_exact_in(Amount(100), r_in, r_out, fee).unwrap();
    assert!(out.0 > 0 && out.0 < r_out.0);
}

#[test]
fn large_swap_never_drains_full_reserve() {
    let r_in = Amount(1_000_000_000_000);
    let r_out = Amount(1_000_000_000_000);
    let fee = FeeBps::new(30).unwrap();
    let out = amount_out_for_exact_in(Amount(r_in.0 / 2), r_in, r_out, fee).unwrap();
    assert!(out.0 < r_out.0);
    assert!(out.0 > 0);
}

#[test]
fn max_int_mul_div_no_wrap() {
    assert_eq!(
        mul_div_floor(u128::MAX, u128::MAX, u128::MAX).unwrap(),
        u128::MAX
    );
    let _ = mul_div_ceil(u128::MAX / 2, 2, 1).unwrap();
}

#[test]
fn isqrt_bounds_and_256() {
    let r = isqrt(u128::MAX);
    assert!(r.checked_mul(r).is_some());
    assert!((r + 1).checked_mul(r + 1).is_none());
    assert_eq!(isqrt_256(0, 0).unwrap(), 0);
}

#[test]
fn fee_never_exceeds_input_and_split_conserves() {
    let fee = FeeBps::new(30).unwrap();
    let f = fee_amount(Amount(10_000), fee).unwrap();
    assert!(f.0 < 10_000);
    let (lp, proto) = split_fee(f).unwrap();
    assert_eq!(lp.0 + proto.0, f.0);
}

#[test]
fn rounding_floor_never_creates_value_vs_ceil() {
    for (a, b, d) in [(10u128, 10, 3), (1, 1, 2), (999, 999, 7)] {
        let f = mul_div_floor(a, b, d).unwrap();
        let c = mul_div_ceil(a, b, d).unwrap();
        assert!(f <= c);
        assert!(c - f <= 1);
    }
}

#[test]
fn cumulative_tiny_swaps_preserve_invariant_direction() {
    let fee = FeeBps::new(30).unwrap();
    let mut reserve_in = Amount(1_000_000);
    let mut reserve_out = Amount(1_000_000);
    let before = Reserves::new(reserve_in, reserve_out);
    for _ in 0..1_000 {
        let out = amount_out_for_exact_in(Amount(100), reserve_in, reserve_out, fee).unwrap();
        reserve_in = Amount(reserve_in.0 + 100);
        reserve_out = Amount(reserve_out.0 - out.0);
    }
    let after = Reserves::new(reserve_in, reserve_out);
    validate_invariant(before, after).unwrap();
}

#[test]
fn lp_initial_and_redeem_no_free_assets() {
    let mint = initial_shares(Amount(10_000), Amount(10_000), 1_000).unwrap();
    assert!(mint.minted_to_provider.0 + mint.locked_in_sink.0 > 0);
    let redeem = redeem_amounts(
        mint.minted_to_provider,
        Reserves::new(Amount(10_000), Amount(10_000)),
        mint.total_supply_after,
    )
    .unwrap();
    assert!(redeem.amount_a.0 <= 10_000);
    assert!(redeem.amount_b.0 <= 10_000);
}

#[test]
fn exact_out_requires_enough_in_and_rejects_zero() {
    let fee = FeeBps::new(30).unwrap();
    assert_eq!(
        amount_in_for_exact_out(Amount(0), Amount(1000), Amount(1000), fee).unwrap_err(),
        AmmError::ZeroAmount
    );
    let need =
        amount_in_for_exact_out(Amount(100), Amount(10_000), Amount(10_000), fee).unwrap();
    assert!(need.0 > 100);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn prop_exact_in_out_less_than_reserve(
        amount_in in 1u128..1_000_000,
        r_in in 1_000u128..10_000_000,
        r_out in 1_000u128..10_000_000,
        fee_bps in 0u16..500
    ) {
        let fee = FeeBps::new(fee_bps).unwrap();
        let out = amount_out_for_exact_in(Amount(amount_in), Amount(r_in), Amount(r_out), fee).unwrap();
        prop_assert!(out.0 < r_out);
    }

    #[test]
    fn prop_no_silent_overflow_add(a in 0u128.., b in 0u128..) {
        match checked_add(a, b) {
            Ok(s) => prop_assert_eq!(Some(s), a.checked_add(b)),
            Err(AmmError::Overflow) => prop_assert!(a.checked_add(b).is_none()),
            Err(_) => prop_assert!(false),
        }
    }

    #[test]
    fn prop_mul_div_floor_le_true_ratio_when_fits(
        a in 0u128..50_000,
        b in 0u128..50_000,
        d in 1u128..50_000
    ) {
        let f = mul_div_floor(a, b, d).unwrap();
        if let Some(prod) = a.checked_mul(b) {
            prop_assert!(f <= prod / d);
        }
    }

    #[test]
    fn prop_subsequent_mint_min_ratio(
        reserve_a in 1_000u128..1_000_000,
        reserve_b in 1_000u128..1_000_000,
        supply in 1_000u128..1_000_000,
        dep_a in 1u128..100_000,
        dep_b in 1u128..100_000
    ) {
        let reserves = Reserves::new(Amount(reserve_a), Amount(reserve_b));
        let minted = mint_shares(
            Amount(dep_a),
            Amount(dep_b),
            reserves,
            LpShares(supply),
        );
        if let Ok(m) = minted {
            prop_assert!(m.minted_to_provider.0 > 0);
        }
    }
}
