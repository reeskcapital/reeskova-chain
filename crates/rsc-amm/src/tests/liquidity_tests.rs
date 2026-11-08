//! Tests de integración del motor de liquidez.

use crate::accounting::AmmAccounting;
use crate::liquidity::{
    get_pool_liquidity, get_user_liquidity, get_user_lp_balance, AddLiquidityParams,
    LiquidityEngine, RemoveLiquidityParams,
};
use crate::pool::{GovernanceAuthority, InMemoryAssetCatalog, PoolPolicy, PoolRegistry};
use crate::{AmmError, AssetId, FeeBps, LpShares, PoolId, LP_SINK_ACCOUNT};
use rsc_core::{AccountId, Amount, BlockHeight, Hash};

fn asset(b: u8) -> AssetId {
    AssetId(Hash([b; 32]))
}
fn account(b: u8) -> AccountId {
    AccountId([b; 32])
}

fn boot() -> (
    PoolRegistry,
    AmmAccounting,
    InMemoryAssetCatalog,
    GovernanceAuthority,
    PoolId,
    AccountId,
) {
    let mut catalog = InMemoryAssetCatalog::new();
    catalog.register(asset(1));
    catalog.register(asset(2));
    let mut gov = GovernanceAuthority::new();
    gov.grant(account(0xAA));
    let provider = account(1);
    let mut pools = PoolRegistry::new(PoolPolicy::default());
    let mut acc = AmmAccounting::new();
    acc.credit(provider, asset(1), Amount(500_000)).unwrap();
    acc.credit(provider, asset(2), Amount(500_000)).unwrap();
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
    let _ = pools.take_events();
    (pools, acc, catalog, gov, pool_id, provider)
}

#[test]
fn full_withdraw_leaves_sink() {
    let (mut pools, mut acc, catalog, gov, pool_id, provider) = boot();
    LiquidityEngine::add_liquidity(
        &mut pools,
        &mut acc,
        &catalog,
        AddLiquidityParams {
            pool_id,
            provider,
            max_amount_a: Amount(40_000),
            max_amount_b: Amount(40_000),
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
    let lp = get_user_lp_balance(&acc, pool_id, provider);
    LiquidityEngine::remove_liquidity(
        &mut pools,
        &mut acc,
        RemoveLiquidityParams {
            pool_id,
            provider,
            lp_shares: lp,
            min_amount_a: Amount(0),
            min_amount_b: Amount(0),
            deadline: None,
            current_block: BlockHeight(4),
            op_id: None,
        },
    )
    .unwrap();
    assert_eq!(get_user_lp_balance(&acc, pool_id, provider).0, 0);
    let pl = get_pool_liquidity(&pools, pool_id).unwrap();
    assert_eq!(pl.locked_supply.0, crate::DEFAULT_MIN_LIQUIDITY);
    assert_eq!(pl.total_supply, pl.locked_supply);
    assert_eq!(
        acc.lp_balance_of(&pool_id, &LP_SINK_ACCOUNT),
        pl.locked_supply
    );
    acc.assert_consistent(&pools).unwrap();
}

#[test]
fn insufficient_balance_and_bad_pool() {
    let (mut pools, mut acc, catalog, _, pool_id, provider) = boot();
    assert_eq!(
        LiquidityEngine::add_liquidity(
            &mut pools,
            &mut acc,
            &catalog,
            AddLiquidityParams {
                pool_id,
                provider,
                max_amount_a: Amount(9_999_999),
                max_amount_b: Amount(9_999_999),
                min_lp_shares: LpShares(1),
                deadline: None,
                current_block: BlockHeight(2),
                op_id: None,
            },
        )
        .unwrap_err(),
        AmmError::InsufficientBalance
    );
    let fake = crate::PoolId(Hash([0xFF; 32]));
    assert_eq!(
        LiquidityEngine::add_liquidity(
            &mut pools,
            &mut acc,
            &catalog,
            AddLiquidityParams {
                pool_id: fake,
                provider,
                max_amount_a: Amount(100),
                max_amount_b: Amount(100),
                min_lp_shares: LpShares(1),
                deadline: None,
                current_block: BlockHeight(2),
                op_id: None,
            },
        )
        .unwrap_err(),
        AmmError::PoolNotFound
    );
}

#[test]
fn min_lp_slippage_and_deadline() {
    let (mut pools, mut acc, catalog, _, pool_id, provider) = boot();
    assert_eq!(
        LiquidityEngine::add_liquidity(
            &mut pools,
            &mut acc,
            &catalog,
            AddLiquidityParams {
                pool_id,
                provider,
                max_amount_a: Amount(20_000),
                max_amount_b: Amount(20_000),
                min_lp_shares: LpShares(u128::MAX),
                deadline: None,
                current_block: BlockHeight(2),
                op_id: None,
            },
        )
        .unwrap_err(),
        AmmError::SlippageExceeded
    );
    assert_eq!(
        LiquidityEngine::add_liquidity(
            &mut pools,
            &mut acc,
            &catalog,
            AddLiquidityParams {
                pool_id,
                provider,
                max_amount_a: Amount(20_000),
                max_amount_b: Amount(20_000),
                min_lp_shares: LpShares(1),
                deadline: Some(BlockHeight(1)),
                current_block: BlockHeight(2),
                op_id: None,
            },
        )
        .unwrap_err(),
        AmmError::DeadlineExceeded
    );
}

#[test]
fn position_query_matches_shares() {
    let (mut pools, mut acc, catalog, _, pool_id, provider) = boot();
    LiquidityEngine::add_liquidity(
        &mut pools,
        &mut acc,
        &catalog,
        AddLiquidityParams {
            pool_id,
            provider,
            max_amount_a: Amount(25_000),
            max_amount_b: Amount(25_000),
            min_lp_shares: LpShares(1),
            deadline: None,
            current_block: BlockHeight(2),
            op_id: None,
        },
    )
    .unwrap();
    let pos = get_user_liquidity(&pools, &acc, pool_id, provider).unwrap();
    assert_eq!(pos.lp_shares, get_user_lp_balance(&acc, pool_id, provider));
    assert!(pos.value_a.0 > 0 && pos.value_b.0 > 0);
}
