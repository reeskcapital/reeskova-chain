//! Tests de gestión de pools (fase 2).

use crate::events::AmmEvent;
use crate::pool::{
    GovernanceAuthority, InMemoryAssetCatalog, PoolIdentity, PoolPolicy, PoolRegistry, PoolStatus,
};
use crate::{AmmError, AssetId, FeeBps, LpShares, Reserves};
use rsc_core::{AccountId, Amount, BlockHeight, Hash};

fn asset(b: u8) -> AssetId {
    AssetId(Hash([b; 32]))
}

fn account(b: u8) -> AccountId {
    AccountId([b; 32])
}

fn harness() -> (PoolRegistry, InMemoryAssetCatalog, GovernanceAuthority) {
    let mut catalog = InMemoryAssetCatalog::new();
    catalog.register(asset(1));
    catalog.register(asset(2));
    let mut gov = GovernanceAuthority::new();
    gov.grant(account(0xAA));
    (PoolRegistry::new(PoolPolicy::default()), catalog, gov)
}

#[test]
fn valid_creation_lookup_and_exists() {
    let (mut reg, catalog, _) = harness();
    let fee = FeeBps::new(30).unwrap();
    let id = reg
        .create_pool(
            &catalog,
            &(),
            account(1),
            asset(2),
            asset(1),
            fee,
            BlockHeight(5),
        )
        .unwrap();
    assert!(reg.pool_exists(asset(1), asset(2), fee));
    assert_eq!(reg.get_pool_by_pair(asset(2), asset(1), fee).unwrap().pool_id, id);
    reg.validate_pool(&id).unwrap();
    assert_eq!(reg.list_pools(), vec![id]);
}

#[test]
fn concurrent_duplicate_creation_is_deterministic() {
    let (mut reg, catalog, _) = harness();
    let fee = FeeBps::new(30).unwrap();
    let first = reg
        .create_pool(
            &catalog,
            &(),
            account(1),
            asset(1),
            asset(2),
            fee,
            BlockHeight(1),
        )
        .unwrap();
    let second = reg.create_pool(
        &catalog,
        &(),
        account(2),
        asset(1),
        asset(2),
        fee,
        BlockHeight(1),
    );
    assert_eq!(second.unwrap_err(), AmmError::PoolAlreadyExists);
    assert_eq!(reg.get_pool(&first).unwrap().creator, account(1));
}

#[test]
fn paused_swap_rejection_and_withdrawal_policy() {
    let (mut reg, catalog, gov) = harness();
    let id = reg
        .create_pool(
            &catalog,
            &(),
            account(1),
            asset(1),
            asset(2),
            FeeBps::new(30).unwrap(),
            BlockHeight(1),
        )
        .unwrap();
    reg.record_reserves(
        &id,
        Reserves::new(Amount(10_000), Amount(10_000)),
        LpShares(10_000),
        LpShares(1_000),
    )
    .unwrap();
    reg.activate_pool(&gov, account(0xAA), id, BlockHeight(2))
        .unwrap();
    reg.pause_pool(&gov, account(0xAA), id, BlockHeight(3))
        .unwrap();
    assert_eq!(
        reg.ensure_swap_allowed(&id).unwrap_err(),
        AmmError::SwapNotPermitted
    );
    assert!(reg.ensure_burn_allowed(&id).is_ok());
    let events = reg.take_events();
    assert!(events.iter().any(|e| matches!(e, AmmEvent::PoolPaused { .. })));
    assert!(events
        .iter()
        .any(|e| matches!(e, AmmEvent::PoolActivated { .. })));
}

#[test]
fn creation_disabled_by_policy() {
    let mut reg = PoolRegistry::new(PoolPolicy {
        permissionless_creation: false,
        min_liquidity: 1_000,
    });
    let mut catalog = InMemoryAssetCatalog::new();
    catalog.register(asset(1));
    catalog.register(asset(2));
    let mut gov = GovernanceAuthority::new();
    gov.grant(account(9));
    assert_eq!(
        reg.create_pool(
            &catalog,
            &gov,
            account(1),
            asset(1),
            asset(2),
            FeeBps::new(30).unwrap(),
            BlockHeight(1),
        )
        .unwrap_err(),
        AmmError::CreationDisabled
    );
    assert!(reg
        .create_pool(
            &catalog,
            &gov,
            account(9),
            asset(1),
            asset(2),
            FeeBps::new(30).unwrap(),
            BlockHeight(1),
        )
        .is_ok());
}

#[test]
fn get_pool_by_pair_missing() {
    let (reg, _, _) = harness();
    assert_eq!(
        reg.get_pool_by_pair(asset(1), asset(2), FeeBps::new(30).unwrap())
            .unwrap_err(),
        AmmError::PoolNotFound
    );
}

#[test]
fn status_helpers() {
    assert!(PoolStatus::Active.allows_swap());
    assert!(!PoolStatus::Paused.allows_swap());
    assert!(PoolStatus::Paused.allows_burn());
    assert!(PoolStatus::Created.allows_burn());
    assert!(PoolStatus::Created.allows_mint());
}

#[test]
fn repeated_execution_same_pool_id() {
    let fee = FeeBps::new(30).unwrap();
    let a = PoolIdentity::new(asset(1), asset(2), fee).unwrap().pool_id();
    for _ in 0..16 {
        assert_eq!(
            a,
            PoolIdentity::new(asset(2), asset(1), fee).unwrap().pool_id()
        );
    }
}
