//! Harness compartido Stage 7 (motor AMM real, sin floats).

use crate::accounting::AmmAccounting;
use crate::fees::FeeTracker;
use crate::liquidity::{AddLiquidityParams, LiquidityEngine};
use crate::pool::{GovernanceAuthority, InMemoryAssetCatalog, PoolPolicy, PoolRegistry};
use crate::types::{AssetId, FeeBps, LpShares, PoolId};
use crate::LP_SINK_ACCOUNT;
use rsc_core::{AccountId, Amount, BlockHeight, Hash};

/// Activo de prueba estilo RSK (sin valor monetario real).
pub fn asset_rsk() -> AssetId {
    AssetId(Hash([0x52; 32])) // 'R'
}

/// Activo de prueba estilo USDC (sin valor monetario real).
pub fn asset_usdc() -> AssetId {
    AssetId(Hash([0x55; 32])) // 'U'
}

pub fn account(b: u8) -> AccountId {
    AccountId([b; 32])
}

/// Mundo AMM listo: pool activo RSK/USDC con liquidez inicial.
pub struct Stage7World {
    pub pools: PoolRegistry,
    pub accounting: AmmAccounting,
    pub fees: FeeTracker,
    pub catalog: InMemoryAssetCatalog,
    pub gov: GovernanceAuthority,
    pub pool_id: PoolId,
    pub provider: AccountId,
    pub trader: AccountId,
    pub authority: AccountId,
}

impl Stage7World {
    /// Génesis determinista de laboratorio.
    pub fn bootstrap(initial_a: u128, initial_b: u128) -> Self {
        let mut catalog = InMemoryAssetCatalog::new();
        catalog.register(asset_rsk());
        catalog.register(asset_usdc());
        let authority = account(0xAA);
        let provider = account(0x10);
        let trader = account(0x20);
        let mut gov = GovernanceAuthority::new();
        gov.grant(authority);

        let mut pools = PoolRegistry::new(PoolPolicy::default());
        let mut accounting = AmmAccounting::new();
        accounting
            .credit(provider, asset_rsk(), Amount(initial_a.saturating_mul(10)))
            .unwrap();
        accounting
            .credit(provider, asset_usdc(), Amount(initial_b.saturating_mul(10)))
            .unwrap();
        accounting
            .credit(trader, asset_rsk(), Amount(initial_a))
            .unwrap();
        accounting
            .credit(trader, asset_usdc(), Amount(initial_b))
            .unwrap();

        let pool_id = pools
            .create_pool(
                &catalog,
                &(),
                provider,
                asset_rsk(),
                asset_usdc(),
                FeeBps::new(30).unwrap(),
                BlockHeight(1),
            )
            .unwrap();
        LiquidityEngine::add_liquidity(
            &mut pools,
            &mut accounting,
            &catalog,
            AddLiquidityParams {
                pool_id,
                provider,
                max_amount_a: Amount(initial_a),
                max_amount_b: Amount(initial_b),
                min_lp_shares: LpShares(0),
                deadline: None,
                current_block: BlockHeight(1),
                op_id: Some(Hash([0xB0; 32])),
            },
        )
        .unwrap();
        pools
            .activate_pool(&gov, authority, pool_id, BlockHeight(1))
            .unwrap();
        let _ = pools.take_events();

        Self {
            pools,
            accounting,
            fees: FeeTracker::new(),
            catalog,
            gov,
            pool_id,
            provider,
            trader,
            authority,
        }
    }

    /// Invariantes contables post-operación (falla el test si se rompen).
    pub fn assert_accounting_integrity(&self) {
        self.accounting
            .assert_consistent(&self.pools)
            .expect("accounting invariant");
        let pool = self.pools.get_pool(&self.pool_id).unwrap();
        let sum_lp = self.accounting.total_lp_tracked(&self.pool_id);
        assert_eq!(
            sum_lp, pool.total_supply.0,
            "LP supply != sum(user LP + locked)"
        );
        assert_eq!(
            self.accounting
                .lp_balance_of(&self.pool_id, &LP_SINK_ACCOUNT)
                .0,
            pool.locked_supply.0,
            "locked LP mismatch"
        );
        assert_eq!(
            self.accounting
                .pool_balance(&self.pool_id, &pool.identity.asset_a),
            pool.reserves.reserve_a
        );
        assert_eq!(
            self.accounting
                .pool_balance(&self.pool_id, &pool.identity.asset_b),
            pool.reserves.reserve_b
        );
    }

    /// Snapshot canónico para comparar réplicas / validadores.
    pub fn canonical_fingerprint(&self) -> Vec<u8> {
        let mut out = self.pools.to_canonical_bytes();
        out.extend_from_slice(&self.accounting.to_canonical_bytes());
        out.extend_from_slice(&self.fees.to_canonical_bytes());
        out
    }
}
