//! Criterion benches — Stage 7 performance (motor AMM real).
//!
//! Run: `cargo bench -p rsc-amm --bench amm_swaps --no-default-features`

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use rsc_amm::accounting::AmmAccounting;
use rsc_amm::fees::FeeTracker;
use rsc_amm::liquidity::{AddLiquidityParams, LiquidityEngine};
use rsc_amm::pool::{GovernanceAuthority, InMemoryAssetCatalog, PoolPolicy, PoolRegistry};
use rsc_amm::swap::{ExactInputSwap, SwapEngine};
use rsc_amm::types::{AssetId, FeeBps, LpShares};
use rsc_core::{AccountId, Amount, BlockHeight, Hash};

fn asset(b: u8) -> AssetId {
    AssetId(Hash([b; 32]))
}
fn account(b: u8) -> AccountId {
    AccountId([b; 32])
}

fn primed() -> (PoolRegistry, AmmAccounting, FeeTracker, rsc_amm::PoolId) {
    let mut catalog = InMemoryAssetCatalog::new();
    catalog.register(asset(1));
    catalog.register(asset(2));
    let mut gov = GovernanceAuthority::new();
    gov.grant(account(0xAA));
    let mut pools = PoolRegistry::new(PoolPolicy::default());
    let mut acc = AmmAccounting::new();
    acc.credit(account(1), asset(1), Amount(10_000_000)).unwrap();
    acc.credit(account(1), asset(2), Amount(10_000_000)).unwrap();
    acc.credit(account(2), asset(1), Amount(1_000_000)).unwrap();
    acc.credit(account(2), asset(2), Amount(1_000_000)).unwrap();
    let pool_id = pools
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
    LiquidityEngine::add_liquidity(
        &mut pools,
        &mut acc,
        &catalog,
        AddLiquidityParams {
            pool_id,
            provider: account(1),
            max_amount_a: Amount(1_000_000),
            max_amount_b: Amount(1_000_000),
            min_lp_shares: LpShares(0),
            deadline: None,
            current_block: BlockHeight(1),
            op_id: Some(Hash([1; 32])),
        },
    )
    .unwrap();
    pools
        .activate_pool(&gov, account(0xAA), pool_id, BlockHeight(1))
        .unwrap();
    (pools, acc, FeeTracker::new(), pool_id)
}

fn bench_exact_input(c: &mut Criterion) {
    c.bench_function("swap_exact_input", |b| {
        b.iter_batched(
            primed,
            |(mut pools, mut acc, mut fees, pool_id)| {
                let _ = SwapEngine::execute_swap_exact_input(
                    &mut pools,
                    &mut acc,
                    &mut fees,
                    ExactInputSwap {
                        pool_id,
                        trader: account(2),
                        token_in: asset(1),
                        token_out: asset(2),
                        amount_in: Amount(100),
                        min_amount_out: Amount(1),
                        deadline: None,
                        current_block: BlockHeight(2),
                        op_id: None,
                    },
                );
                black_box(());
            },
            criterion::BatchSize::SmallInput,
        )
    });
}

fn bench_100_swaps(c: &mut Criterion) {
    c.bench_function("swap_exact_input_x100", |b| {
        b.iter(|| {
            let (mut pools, mut acc, mut fees, pool_id) = primed();
            for i in 0u8..100 {
                let _ = SwapEngine::execute_swap_exact_input(
                    &mut pools,
                    &mut acc,
                    &mut fees,
                    ExactInputSwap {
                        pool_id,
                        trader: account(2),
                        token_in: if i % 2 == 0 { asset(1) } else { asset(2) },
                        token_out: if i % 2 == 0 { asset(2) } else { asset(1) },
                        amount_in: Amount(10),
                        min_amount_out: Amount(0),
                        deadline: None,
                        current_block: BlockHeight(u64::from(i) + 2),
                        op_id: Some(Hash([i; 32])),
                    },
                );
            }
            black_box(pools.list_pools().len());
        })
    });
}

criterion_group!(benches, bench_exact_input, bench_100_swaps);
criterion_main!(benches);
