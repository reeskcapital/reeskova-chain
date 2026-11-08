//! Tests de invariantes contables, vault y eventos LP.

use crate::accounting::AmmAccounting;
use crate::liquidity::{
    get_user_lp_balance, AddLiquidityParams, LiquidityEngine, RemoveLiquidityParams,
};
use crate::math::shares::{initial_shares, ownership_bps};
use crate::pool::{InMemoryAssetCatalog, PoolPolicy, PoolRegistry};
use crate::{AssetId, FeeBps, LpShares, AmmEvent, LP_SINK_ACCOUNT, DEFAULT_MIN_LIQUIDITY};
use rsc_core::{AccountId, Amount, BlockHeight, Hash};

#[test]
fn sink_shares_are_explicit_and_not_owned_by_provider() {
    let m = initial_shares(Amount(20_000), Amount(20_000), DEFAULT_MIN_LIQUIDITY).unwrap();
    assert_eq!(m.locked_in_sink.0, DEFAULT_MIN_LIQUIDITY);
    assert_eq!(
        m.minted_to_provider.0 + m.locked_in_sink.0,
        m.total_supply_after.0
    );
    let owned = ownership_bps(m.minted_to_provider, m.total_supply_after).unwrap();
    let full = ownership_bps(m.total_supply_after, m.total_supply_after).unwrap();
    assert!(owned < full);
}

#[test]
fn reserves_match_custody_after_add_remove() {
    let mut catalog = InMemoryAssetCatalog::new();
    let a = AssetId(Hash([1; 32]));
    let b = AssetId(Hash([2; 32]));
    catalog.register(a);
    catalog.register(b);
    let provider = AccountId([3; 32]);
    let mut pools = PoolRegistry::new(PoolPolicy::default());
    let mut acc = AmmAccounting::new();
    acc.credit(provider, a, Amount(100_000)).unwrap();
    acc.credit(provider, b, Amount(100_000)).unwrap();
    let pool_id = pools
        .create_pool(
            &catalog,
            &(),
            provider,
            a,
            b,
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
            max_amount_a: Amount(30_000),
            max_amount_b: Amount(30_000),
            min_lp_shares: LpShares(1),
            deadline: None,
            current_block: BlockHeight(2),
            op_id: None,
        },
    )
    .unwrap();
    acc.reconcile_vault(&pools, &pool_id).unwrap();
    acc.verify_lp_supply(&pools, &pool_id).unwrap();
    acc.assert_consistent(&pools).unwrap();

    let half = LpShares(get_user_lp_balance(&acc, pool_id, provider).0 / 2);
    LiquidityEngine::remove_liquidity(
        &mut pools,
        &mut acc,
        RemoveLiquidityParams {
            pool_id,
            provider,
            lp_shares: half,
            min_amount_a: Amount(0),
            min_amount_b: Amount(0),
            deadline: None,
            current_block: BlockHeight(3),
            op_id: None,
        },
    )
    .unwrap();
    acc.verify_pool_accounting(&pools, &pool_id).unwrap();
    assert!(acc.lp_balance_of(&pool_id, &LP_SINK_ACCOUNT).0 > 0);
}

#[test]
fn insufficient_user_balance_and_lp_burn_events() {
    let mut catalog = InMemoryAssetCatalog::new();
    let a = AssetId(Hash([1; 32]));
    let b = AssetId(Hash([2; 32]));
    catalog.register(a);
    catalog.register(b);
    let provider = AccountId([3; 32]);
    let mut pools = PoolRegistry::new(PoolPolicy::default());
    let mut acc = AmmAccounting::new();
    acc.credit_user(provider, a, Amount(50_000)).unwrap();
    acc.credit_user(provider, b, Amount(50_000)).unwrap();
    let pool_id = pools
        .create_pool(
            &catalog,
            &(),
            provider,
            a,
            b,
            FeeBps::new(30).unwrap(),
            BlockHeight(1),
        )
        .unwrap();
    assert_eq!(
        LiquidityEngine::add_liquidity(
            &mut pools,
            &mut acc,
            &catalog,
            AddLiquidityParams {
                pool_id,
                provider,
                max_amount_a: Amount(60_000),
                max_amount_b: Amount(60_000),
                min_lp_shares: LpShares(1),
                deadline: None,
                current_block: BlockHeight(2),
                op_id: None,
            },
        )
        .unwrap_err(),
        crate::AmmError::InsufficientBalance
    );

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
            deadline: None,
            current_block: BlockHeight(2),
            op_id: Some(Hash([0x11; 32])),
        },
    )
    .unwrap();
    let events = pools.take_events();
    assert!(events
        .iter()
        .any(|e| matches!(e, AmmEvent::LPSharesMinted { to, .. } if *to == provider)));
    assert!(events
        .iter()
        .any(|e| matches!(e, AmmEvent::LPSharesMinted { to, .. } if *to == LP_SINK_ACCOUNT)));
    assert!(events
        .iter()
        .any(|e| matches!(e, AmmEvent::LiquidityAdded { .. })));

    let lp = get_user_lp_balance(&acc, pool_id, provider);
    LiquidityEngine::remove_liquidity(
        &mut pools,
        &mut acc,
        RemoveLiquidityParams {
            pool_id,
            provider,
            lp_shares: LpShares(lp.0 / 2),
            min_amount_a: Amount(0),
            min_amount_b: Amount(0),
            deadline: None,
            current_block: BlockHeight(3),
            op_id: Some(Hash([0x22; 32])),
        },
    )
    .unwrap();
    assert!(pools
        .pending_events()
        .iter()
        .any(|e| matches!(e, AmmEvent::LPSharesBurned { .. })));
}

#[test]
fn atomic_rollback_clears_events() {
    let mut catalog = InMemoryAssetCatalog::new();
    let a = AssetId(Hash([1; 32]));
    let b = AssetId(Hash([2; 32]));
    catalog.register(a);
    catalog.register(b);
    let provider = AccountId([3; 32]);
    let mut pools = PoolRegistry::new(PoolPolicy::default());
    let mut acc = AmmAccounting::new();
    acc.credit(provider, a, Amount(40_000)).unwrap();
    acc.credit(provider, b, Amount(40_000)).unwrap();
    let pool_id = pools
        .create_pool(
            &catalog,
            &(),
            provider,
            a,
            b,
            FeeBps::new(30).unwrap(),
            BlockHeight(1),
        )
        .unwrap();
    let _ = pools.take_events();
    acc.fail_next_pool_transfer = true;
    let err = LiquidityEngine::add_liquidity(
        &mut pools,
        &mut acc,
        &catalog,
        AddLiquidityParams {
            pool_id,
            provider,
            max_amount_a: Amount(20_000),
            max_amount_b: Amount(20_000),
            min_lp_shares: LpShares(1),
            deadline: None,
            current_block: BlockHeight(2),
            op_id: Some(Hash([0x33; 32])),
        },
    )
    .unwrap_err();
    assert_eq!(err, crate::AmmError::TransferFailed);
    assert!(pools.pending_events().is_empty());
    assert!(!acc.op_seen(&Hash([0x33; 32])));
    assert_eq!(pools.get_pool(&pool_id).unwrap().total_supply.0, 0);
}
