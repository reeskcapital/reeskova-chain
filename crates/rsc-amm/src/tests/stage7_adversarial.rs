//! Stage 7 — adversarial / seguridad de transacciones del motor AMM.

use super::stage7_harness::{asset_rsk, asset_usdc, Stage7World};
use crate::liquidity::{AddLiquidityParams, LiquidityEngine, RemoveLiquidityParams};
use crate::pool::PoolIdentity;
use crate::swap::{ExactInputSwap, ExactOutputSwap, SwapEngine};
use crate::types::{FeeBps, LpShares};
use crate::AmmError;
use crate::LP_SINK_ACCOUNT;
use rsc_core::{Amount, BlockHeight, Hash};

#[test]
fn replay_same_op_id_rejected() {
    let mut w = Stage7World::bootstrap(100_000, 100_000);
    let op = Hash([9; 32]);
    SwapEngine::execute_swap_exact_input(
        &mut w.pools,
        &mut w.accounting,
        &mut w.fees,
        ExactInputSwap {
            pool_id: w.pool_id,
            trader: w.trader,
            token_in: asset_rsk(),
            token_out: asset_usdc(),
            amount_in: Amount(100),
            min_amount_out: Amount(1),
            deadline: None,
            current_block: BlockHeight(2),
            op_id: Some(op),
        },
    )
    .unwrap();
    let err = SwapEngine::execute_swap_exact_input(
        &mut w.pools,
        &mut w.accounting,
        &mut w.fees,
        ExactInputSwap {
            pool_id: w.pool_id,
            trader: w.trader,
            token_in: asset_rsk(),
            token_out: asset_usdc(),
            amount_in: Amount(100),
            min_amount_out: Amount(1),
            deadline: None,
            current_block: BlockHeight(2),
            op_id: Some(op),
        },
    )
    .unwrap_err();
    assert_eq!(err, AmmError::DuplicateOperation);
    w.assert_accounting_integrity();
}

#[test]
fn slippage_and_deadline_and_paused() {
    let mut w = Stage7World::bootstrap(100_000, 100_000);
    let err = SwapEngine::execute_swap_exact_input(
        &mut w.pools,
        &mut w.accounting,
        &mut w.fees,
        ExactInputSwap {
            pool_id: w.pool_id,
            trader: w.trader,
            token_in: asset_rsk(),
            token_out: asset_usdc(),
            amount_in: Amount(1_000),
            min_amount_out: Amount(u128::MAX),
            deadline: None,
            current_block: BlockHeight(2),
            op_id: None,
        },
    )
    .unwrap_err();
    assert_eq!(err, AmmError::SlippageExceeded);

    let err = SwapEngine::execute_swap_exact_input(
        &mut w.pools,
        &mut w.accounting,
        &mut w.fees,
        ExactInputSwap {
            pool_id: w.pool_id,
            trader: w.trader,
            token_in: asset_rsk(),
            token_out: asset_usdc(),
            amount_in: Amount(10),
            min_amount_out: Amount(1),
            deadline: Some(BlockHeight(1)),
            current_block: BlockHeight(5),
            op_id: None,
        },
    )
    .unwrap_err();
    assert_eq!(err, AmmError::DeadlineExceeded);

    w.pools
        .pause_pool(&w.gov, w.authority, w.pool_id, BlockHeight(3))
        .unwrap();
    let err = SwapEngine::execute_swap_exact_input(
        &mut w.pools,
        &mut w.accounting,
        &mut w.fees,
        ExactInputSwap {
            pool_id: w.pool_id,
            trader: w.trader,
            token_in: asset_rsk(),
            token_out: asset_usdc(),
            amount_in: Amount(10),
            min_amount_out: Amount(1),
            deadline: None,
            current_block: BlockHeight(4),
            op_id: None,
        },
    )
    .unwrap_err();
    assert_eq!(err, AmmError::SwapNotPermitted);
    w.assert_accounting_integrity();
}

#[test]
fn duplicate_pool_and_reversed_pair_same_id() {
    let mut w = Stage7World::bootstrap(50_000, 50_000);
    let fee = FeeBps::new(30).unwrap();
    let id1 = PoolIdentity::new(asset_rsk(), asset_usdc(), fee)
        .unwrap()
        .pool_id();
    let id2 = PoolIdentity::new(asset_usdc(), asset_rsk(), fee)
        .unwrap()
        .pool_id();
    assert_eq!(id1, id2);
    assert_eq!(id1, w.pool_id);
    let err = w
        .pools
        .create_pool(
            &w.catalog,
            &(),
            w.provider,
            asset_usdc(),
            asset_rsk(),
            fee,
            BlockHeight(9),
        )
        .unwrap_err();
    assert_eq!(err, AmmError::PoolAlreadyExists);
}

#[test]
fn insufficient_balance_and_reserve_exhaustion_rollback() {
    let mut w = Stage7World::bootstrap(20_000, 20_000);
    let before = w.canonical_fingerprint();
    let err = SwapEngine::execute_swap_exact_input(
        &mut w.pools,
        &mut w.accounting,
        &mut w.fees,
        ExactInputSwap {
            pool_id: w.pool_id,
            trader: w.trader,
            token_in: asset_rsk(),
            token_out: asset_usdc(),
            amount_in: Amount(u128::MAX / 4),
            min_amount_out: Amount(1),
            deadline: None,
            current_block: BlockHeight(2),
            op_id: None,
        },
    )
    .unwrap_err();
    assert!(matches!(
        err,
        AmmError::InsufficientBalance | AmmError::InsufficientLiquidity | AmmError::Overflow
    ));
    assert_eq!(w.canonical_fingerprint(), before);
}

#[test]
fn cannot_burn_locked_sink_shares() {
    let mut w = Stage7World::bootstrap(30_000, 30_000);
    let locked = w
        .accounting
        .lp_balance_of(&w.pool_id, &LP_SINK_ACCOUNT);
    assert!(locked.0 > 0);
    let err = LiquidityEngine::remove_liquidity(
        &mut w.pools,
        &mut w.accounting,
        RemoveLiquidityParams {
            pool_id: w.pool_id,
            provider: LP_SINK_ACCOUNT,
            lp_shares: LpShares(1),
            min_amount_a: Amount(0),
            min_amount_b: Amount(0),
            deadline: None,
            current_block: BlockHeight(2),
            op_id: None,
        },
    )
    .unwrap_err();
    assert!(matches!(
        err,
        AmmError::LockedLiquidity | AmmError::InsufficientLpBalance
    ));
}

#[test]
fn unbalanced_deposit_rejected_no_partial_credit() {
    let mut w = Stage7World::bootstrap(40_000, 40_000);
    let before = w.canonical_fingerprint();
    let err = LiquidityEngine::add_liquidity(
        &mut w.pools,
        &mut w.accounting,
        &w.catalog,
        AddLiquidityParams {
            pool_id: w.pool_id,
            provider: w.provider,
            max_amount_a: Amount(10_000),
            max_amount_b: Amount(1),
            min_lp_shares: LpShares(0),
            deadline: None,
            current_block: BlockHeight(2),
            op_id: Some(Hash([0xAB; 32])),
        },
    )
    .unwrap_err();
    assert!(matches!(
        err,
        AmmError::UnbalancedDeposit | AmmError::SlippageExceeded | AmmError::ZeroAmount
    ));
    assert_eq!(w.canonical_fingerprint(), before);
}

#[test]
fn exact_output_max_in_and_competing_swaps() {
    let mut w = Stage7World::bootstrap(80_000, 80_000);
    SwapEngine::execute_swap_exact_output(
        &mut w.pools,
        &mut w.accounting,
        &mut w.fees,
        ExactOutputSwap {
            pool_id: w.pool_id,
            trader: w.trader,
            token_in: asset_rsk(),
            token_out: asset_usdc(),
            amount_out: Amount(50),
            max_amount_in: Amount(10_000),
            deadline: None,
            current_block: BlockHeight(2),
            op_id: Some(Hash([0xC1; 32])),
        },
    )
    .unwrap();
    // Segunda tx compite por el mismo pool.
    SwapEngine::execute_swap_exact_input(
        &mut w.pools,
        &mut w.accounting,
        &mut w.fees,
        ExactInputSwap {
            pool_id: w.pool_id,
            trader: w.trader,
            token_in: asset_usdc(),
            token_out: asset_rsk(),
            amount_in: Amount(40),
            min_amount_out: Amount(1),
            deadline: None,
            current_block: BlockHeight(2),
            op_id: Some(Hash([0xC2; 32])),
        },
    )
    .unwrap();
    w.assert_accounting_integrity();
}

#[test]
fn repeated_full_cycle_withdraw_after_swaps() {
    let mut w = Stage7World::bootstrap(60_000, 60_000);
    for i in 0u8..20 {
        SwapEngine::execute_swap_exact_input(
            &mut w.pools,
            &mut w.accounting,
            &mut w.fees,
            ExactInputSwap {
                pool_id: w.pool_id,
                trader: w.trader,
                token_in: if i % 2 == 0 {
                    asset_rsk()
                } else {
                    asset_usdc()
                },
                token_out: if i % 2 == 0 {
                    asset_usdc()
                } else {
                    asset_rsk()
                },
                amount_in: Amount(25),
                min_amount_out: Amount(1),
                deadline: None,
                current_block: BlockHeight(u64::from(i) + 2),
                op_id: Some({
                    let mut h = [0xD0; 32];
                    h[31] = i;
                    Hash(h)
                }),
            },
        )
        .unwrap();
    }
    let lp = w.accounting.lp_balance_of(&w.pool_id, &w.provider);
    LiquidityEngine::remove_liquidity(
        &mut w.pools,
        &mut w.accounting,
        RemoveLiquidityParams {
            pool_id: w.pool_id,
            provider: w.provider,
            lp_shares: LpShares(lp.0 / 2),
            min_amount_a: Amount(0),
            min_amount_b: Amount(0),
            deadline: None,
            current_block: BlockHeight(50),
            op_id: Some(Hash([0xEE; 32])),
        },
    )
    .unwrap();
    w.assert_accounting_integrity();
}
