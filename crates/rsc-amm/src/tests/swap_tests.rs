//! Tests de cotización y ejecución de swaps.

use crate::accounting::AmmAccounting;
use crate::fees::FeeTracker;
use crate::liquidity::{AddLiquidityParams, LiquidityEngine};
use crate::math::invariant::{amount_out_for_exact_in, validate_invariant};
use crate::pool::{GovernanceAuthority, InMemoryAssetCatalog, PoolPolicy, PoolRegistry};
use crate::pricing::{
    calculate_slippage, calculate_spot_price_normalized, get_quote, validate_quote, UniformDecimals,
};
use crate::swap::{ExactInputSwap, ExactOutputSwap, SwapEngine};
use crate::{AmmError, AssetId, FeeBps, LpShares, PoolId, Reserves, AssetDecimals};
use rsc_core::{AccountId, Amount, BlockHeight, Hash};

fn asset(b: u8) -> AssetId {
    AssetId(Hash([b; 32]))
}
fn account(b: u8) -> AccountId {
    AccountId([b; 32])
}

fn setup_active() -> (PoolRegistry, AmmAccounting, FeeTracker, PoolId, AccountId) {
    let mut catalog = InMemoryAssetCatalog::new();
    catalog.register(asset(1));
    catalog.register(asset(2));
    let mut gov = GovernanceAuthority::new();
    gov.grant(account(0xAA));
    let provider = account(1);
    let trader = account(2);
    let mut pools = PoolRegistry::new(PoolPolicy::default());
    let mut acc = AmmAccounting::new();
    acc.credit(provider, asset(1), Amount(500_000)).unwrap();
    acc.credit(provider, asset(2), Amount(500_000)).unwrap();
    acc.credit(trader, asset(1), Amount(50_000)).unwrap();
    acc.credit(trader, asset(2), Amount(50_000)).unwrap();
    let pool_id = pools
        .create_pool(
            &catalog,
            &(),
            provider,
            asset(1),
            asset(2),
            FeeBps::new(30).unwrap(),
            BlockHeight(1),
        )
        .unwrap();
    LiquidityEngine::add_liquidity(
        &mut pools,
        &mut acc,
        &catalog,
        AddLiquidityParams {
            pool_id,
            provider,
            max_amount_a: Amount(100_000),
            max_amount_b: Amount(100_000),
            min_lp_shares: LpShares(1),
            deadline: None,
            current_block: BlockHeight(2),
            op_id: None,
        },
    )
    .unwrap();
    pools
        .activate_pool(&gov, account(0xAA), pool_id, BlockHeight(3))
        .unwrap();
    let _ = pools.take_events();
    (pools, acc, FeeTracker::new(), pool_id, trader)
}

#[test]
fn swap_preserves_invariant_with_fee() {
    let fee = FeeBps::new(30).unwrap();
    let rin = Amount(5_000_000);
    let rout = Amount(8_000_000);
    let din = Amount(25_000);
    let out = amount_out_for_exact_in(din, rin, rout, fee).unwrap();
    validate_invariant(
        Reserves::new(rin, rout),
        Reserves::new(Amount(rin.0 + din.0), Amount(rout.0 - out.0)),
    )
    .unwrap();
}

#[test]
fn tiny_input_against_deep_pool() {
    let err = amount_out_for_exact_in(
        Amount(1),
        Amount(1_000_000_000_000),
        Amount(1_000_000_000_000),
        FeeBps::new(30).unwrap(),
    );
    match err {
        Ok(Amount(n)) if n <= 1 => {}
        Err(AmmError::InsufficientLiquidity) => {}
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn end_to_end_exact_in_matches_quote() {
    let (mut pools, mut acc, mut fees, pool_id, trader) = setup_active();
    let quote = get_quote(
        &pools,
        pool_id,
        asset(1),
        asset(2),
        Amount(2_000),
        BlockHeight(4),
        Some(BlockHeight(100)),
    )
    .unwrap();
    validate_quote(&quote, BlockHeight(4)).unwrap();
    let slip = calculate_slippage(quote.amount_out, quote.amount_out).unwrap();
    assert_eq!(slip, 0);

    let res = SwapEngine::execute_swap_exact_input(
        &mut pools,
        &mut acc,
        &mut fees,
        ExactInputSwap {
            pool_id,
            trader,
            token_in: asset(1),
            token_out: asset(2),
            amount_in: Amount(2_000),
            min_amount_out: quote.amount_out,
            deadline: Some(BlockHeight(100)),
            current_block: BlockHeight(4),
            op_id: Some(Hash([0xAB; 32])),
        },
    )
    .unwrap();
    assert_eq!(res.amount_out, quote.amount_out);
    let ev = pools.pending_events();
    assert!(ev.iter().any(|e| matches!(e, crate::AmmEvent::SwapExecuted { .. })));
    assert!(ev.iter().any(|e| matches!(e, crate::AmmEvent::FeeCollected { .. })));
    acc.assert_consistent(&pools).unwrap();
}

#[test]
fn insufficient_balance_and_duplicate_op() {
    let (mut pools, mut acc, mut fees, pool_id, trader) = setup_active();
    assert_eq!(
        SwapEngine::execute_swap_exact_input(
            &mut pools,
            &mut acc,
            &mut fees,
            ExactInputSwap {
                pool_id,
                trader,
                token_in: asset(1),
                token_out: asset(2),
                amount_in: Amount(999_999),
                min_amount_out: Amount(1),
                deadline: None,
                current_block: BlockHeight(4),
                op_id: None,
            },
        )
        .unwrap_err(),
        AmmError::InsufficientBalance
    );
    let op = Hash([7; 32]);
    SwapEngine::execute_swap_exact_input(
        &mut pools,
        &mut acc,
        &mut fees,
        ExactInputSwap {
            pool_id,
            trader,
            token_in: asset(1),
            token_out: asset(2),
            amount_in: Amount(100),
            min_amount_out: Amount(1),
            deadline: None,
            current_block: BlockHeight(4),
            op_id: Some(op),
        },
    )
    .unwrap();
    assert_eq!(
        SwapEngine::execute_swap_exact_input(
            &mut pools,
            &mut acc,
            &mut fees,
            ExactInputSwap {
                pool_id,
                trader,
                token_in: asset(1),
                token_out: asset(2),
                amount_in: Amount(100),
                min_amount_out: Amount(1),
                deadline: None,
                current_block: BlockHeight(5),
                op_id: Some(op),
            },
        )
        .unwrap_err(),
        AmmError::DuplicateOperation
    );
}

#[test]
fn different_decimals_spot_helper() {
    let dec = UniformDecimals(6);
    assert_eq!(dec.decimals(&asset(1)), 6);
    let spot18 = calculate_spot_price_normalized(Amount(1_000), Amount(2_000), 18, 18).unwrap();
    let spot6 = calculate_spot_price_normalized(Amount(1_000), Amount(2_000), 6, 6).unwrap();
    assert_eq!(spot18, spot6);
    // 18 vs 6: scale_in/scale_out = 10^12 amplifies out side.
    let mixed =
        calculate_spot_price_normalized(Amount(1_000_000), Amount(2_000_000), 18, 6).unwrap();
    assert!(mixed > spot18);
}

#[test]
fn exact_output_round_trip() {
    let (mut pools, mut acc, mut fees, pool_id, trader) = setup_active();
    let out = Amount(500);
    let r = SwapEngine::execute_swap_exact_output(
        &mut pools,
        &mut acc,
        &mut fees,
        ExactOutputSwap {
            pool_id,
            trader,
            token_in: asset(1),
            token_out: asset(2),
            amount_out: out,
            max_amount_in: Amount(10_000),
            deadline: None,
            current_block: BlockHeight(4),
            op_id: None,
        },
    )
    .unwrap();
    assert_eq!(r.amount_out, out);
    acc.assert_consistent(&pools).unwrap();
}
