//! Tests de fee (matemática + modelo in-reserve + queries).

use crate::fees::{
    amount_after_fee, default_swap_fee, fee_amount, get_pool_fee, split_fee, validate_fee,
    FeeTracker, PROTOCOL_FEE_SHARE_BPS,
};
use crate::math::invariant::{amount_in_for_exact_out, amount_out_for_exact_in};
use crate::pool::{InMemoryAssetCatalog, PoolPolicy, PoolRegistry};
use crate::{AssetId, FeeBps, PoolId, DEFAULT_SWAP_FEE_BPS, MAX_SWAP_FEE_BPS};
use rsc_core::{AccountId, Amount, BlockHeight, Hash};

#[test]
fn default_fee_matches_constant() {
    assert_eq!(FeeBps::default_swap().get(), DEFAULT_SWAP_FEE_BPS);
    assert_eq!(default_swap_fee().get(), DEFAULT_SWAP_FEE_BPS);
}

#[test]
fn higher_fee_reduces_output() {
    let rin = Amount(100_000);
    let rout = Amount(100_000);
    let din = Amount(1_000);
    let low = amount_out_for_exact_in(din, rin, rout, FeeBps::new(10).unwrap()).unwrap();
    let high = amount_out_for_exact_in(din, rin, rout, FeeBps::new(100).unwrap()).unwrap();
    assert!(low.0 > high.0);
}

#[test]
fn max_fee_still_swappable() {
    let fee = FeeBps::new(MAX_SWAP_FEE_BPS).unwrap();
    let out =
        amount_out_for_exact_in(Amount(1_000), Amount(100_000), Amount(100_000), fee).unwrap();
    assert!(out.0 > 0);
}

#[test]
fn zero_fee_exact_product_path() {
    let need = amount_in_for_exact_out(
        Amount(100),
        Amount(10_000),
        Amount(10_000),
        FeeBps::ZERO,
    )
    .unwrap();
    let got = amount_out_for_exact_in(need, Amount(10_000), Amount(10_000), FeeBps::ZERO).unwrap();
    assert!(got.0 >= 100);
}

#[test]
fn fee_amount_30_bps_and_no_double_count_split() {
    let fee = FeeBps::new(30).unwrap();
    let amount = Amount(10_000);
    let f = fee_amount(amount, fee).unwrap();
    let after = amount_after_fee(amount, fee).unwrap();
    assert_eq!(f.0 + after.0, amount.0);
    assert_eq!(f.0, 30);
    validate_fee(fee).unwrap();
    assert_eq!(PROTOCOL_FEE_SHARE_BPS, 0);
    let (lp, proto) = split_fee(f).unwrap();
    assert_eq!(lp, f);
    assert_eq!(proto.0, 0);
}

#[test]
fn invalid_fee_and_zero_amount() {
    assert!(FeeBps::new(MAX_SWAP_FEE_BPS + 1).is_err());
    assert_eq!(
        fee_amount(Amount::ZERO, FeeBps::new(30).unwrap()).unwrap_err(),
        crate::AmmError::ZeroAmount
    );
}

#[test]
fn fee_tracker_by_asset_and_pool_fee_lookup() {
    let mut catalog = InMemoryAssetCatalog::new();
    let a = AssetId(Hash([1; 32]));
    let b = AssetId(Hash([2; 32]));
    catalog.register(a);
    catalog.register(b);
    let mut pools = PoolRegistry::new(PoolPolicy::default());
    let pid = pools
        .create_pool(
            &catalog,
            &(),
            AccountId([9; 32]),
            a,
            b,
            FeeBps::new(30).unwrap(),
            BlockHeight(1),
        )
        .unwrap();
    assert_eq!(get_pool_fee(&pools, &pid).unwrap().get(), 30);

    let mut t = FeeTracker::new();
    t.record(pid, a, Amount(50)).unwrap();
    t.record(pid, a, Amount(25)).unwrap();
    assert_eq!(t.accrued(&pid), 75);
    assert_eq!(t.accrued_for_asset(&pid, &a), 75);
    assert_eq!(t.accrued_for_asset(&pid, &b), 0);

    let ev = FeeTracker::fee_collected_event(
        crate::fees::FeeBreakdown {
            total: Amount(50),
            lp_portion: Amount(50),
            protocol_portion: Amount(0),
        },
        pid,
        a,
        BlockHeight(2),
        Some(Hash([1; 32])),
    );
    assert!(matches!(ev, crate::AmmEvent::FeeCollected { .. }));
}

#[test]
fn overflow_on_fee_tracker() {
    let mut t = FeeTracker::new();
    let pid = PoolId(Hash([1; 32]));
    t.record(pid, AssetId(Hash([2; 32])), Amount(u128::MAX)).unwrap();
    assert_eq!(
        t.record(pid, AssetId(Hash([2; 32])), Amount(1)).unwrap_err(),
        crate::AmmError::Overflow
    );
}
