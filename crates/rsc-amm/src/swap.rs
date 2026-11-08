//! Ejecución atómica de swaps.
//!
//! Flujo exact-input / exact-output:
//! validar → calcular → transferir in → transferir out → actualizar reservas
//! → registrar fee métrico → emitir evento.
//!
//! Las fees **permanecen en las reservas** ([`crate::fees`]); no hay vault
//! separado. Snapshot/rollback garantiza atomicidad.

use crate::accounting::AmmAccounting;
use crate::events::AmmEvent;
use crate::fees::{fee_amount, FeeTracker};
use crate::math::amount::{checked_add, checked_sub};
use crate::math::invariant::validate_invariant;
use crate::pool::PoolRegistry;
use crate::pricing::{
    calculate_amount_in, calculate_amount_out, resolve_path, SwapPath,
};
use crate::types::{AssetId, PoolId, Reserves};
use crate::AmmError;
use rsc_core::{AccountId, Amount, BlockHeight, Hash};

/// Parámetros de swap exact-input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExactInputSwap {
    /// Pool.
    pub pool_id: PoolId,
    /// Trader.
    pub trader: AccountId,
    /// Activo de entrada.
    pub token_in: AssetId,
    /// Activo de salida.
    pub token_out: AssetId,
    /// Cantidad exacta de entrada.
    pub amount_in: Amount,
    /// Mínimo de salida aceptable.
    pub min_amount_out: Amount,
    /// Deadline.
    pub deadline: Option<BlockHeight>,
    /// Altura actual.
    pub current_block: BlockHeight,
    /// Anti-replay.
    pub op_id: Option<Hash>,
}

/// Parámetros de swap exact-output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExactOutputSwap {
    /// Pool.
    pub pool_id: PoolId,
    /// Trader.
    pub trader: AccountId,
    /// Activo de entrada.
    pub token_in: AssetId,
    /// Activo de salida.
    pub token_out: AssetId,
    /// Cantidad exacta de salida.
    pub amount_out: Amount,
    /// Máximo de entrada aceptable.
    pub max_amount_in: Amount,
    /// Deadline.
    pub deadline: Option<BlockHeight>,
    /// Altura actual.
    pub current_block: BlockHeight,
    /// Anti-replay.
    pub op_id: Option<Hash>,
}

/// Resultado de un swap ejecutado.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwapResult {
    /// Input efectivo.
    pub amount_in: Amount,
    /// Output efectivo.
    pub amount_out: Amount,
    /// Fee bruto estimado (métrica; fondos en pool).
    pub fee_amount: Amount,
}

/// Motor de swaps.
pub struct SwapEngine;

impl SwapEngine {
    /// Ejecuta exact-input de forma atómica.
    pub fn execute_swap_exact_input(
        pools: &mut PoolRegistry,
        accounting: &mut AmmAccounting,
        fees: &mut FeeTracker,
        params: ExactInputSwap,
    ) -> Result<SwapResult, AmmError> {
        let snap_p = pools.clone();
        let snap_a = accounting.clone();
        let snap_f = fees.clone();
        match Self::exact_input_inner(pools, accounting, fees, params) {
            Ok(r) => Ok(r),
            Err(e) => {
                *pools = snap_p;
                *accounting = snap_a;
                *fees = snap_f;
                Err(e)
            }
        }
    }

    /// Ejecuta exact-output de forma atómica.
    pub fn execute_swap_exact_output(
        pools: &mut PoolRegistry,
        accounting: &mut AmmAccounting,
        fees: &mut FeeTracker,
        params: ExactOutputSwap,
    ) -> Result<SwapResult, AmmError> {
        let snap_p = pools.clone();
        let snap_a = accounting.clone();
        let snap_f = fees.clone();
        match Self::exact_output_inner(pools, accounting, fees, params) {
            Ok(r) => Ok(r),
            Err(e) => {
                *pools = snap_p;
                *accounting = snap_a;
                *fees = snap_f;
                Err(e)
            }
        }
    }

    fn exact_input_inner(
        pools: &mut PoolRegistry,
        accounting: &mut AmmAccounting,
        fees: &mut FeeTracker,
        params: ExactInputSwap,
    ) -> Result<SwapResult, AmmError> {
        validate_swap_deadline(params.deadline, params.current_block)?;
        if let Some(op) = params.op_id {
            accounting.mark_op(op)?;
        }
        let computed = validate_swap_exact_input(pools, accounting, &params)?;
        apply_swap(
            pools,
            accounting,
            fees,
            params.pool_id,
            params.trader,
            params.token_in,
            params.token_out,
            computed,
            params.current_block,
            params.op_id,
        )
    }

    fn exact_output_inner(
        pools: &mut PoolRegistry,
        accounting: &mut AmmAccounting,
        fees: &mut FeeTracker,
        params: ExactOutputSwap,
    ) -> Result<SwapResult, AmmError> {
        validate_swap_deadline(params.deadline, params.current_block)?;
        if let Some(op) = params.op_id {
            accounting.mark_op(op)?;
        }
        let computed = validate_swap_exact_output(pools, accounting, &params)?;
        apply_swap(
            pools,
            accounting,
            fees,
            params.pool_id,
            params.trader,
            params.token_in,
            params.token_out,
            computed,
            params.current_block,
            params.op_id,
        )
    }
}

fn apply_swap(
    pools: &mut PoolRegistry,
    accounting: &mut AmmAccounting,
    fees: &mut FeeTracker,
    pool_id: PoolId,
    trader: AccountId,
    token_in: AssetId,
    token_out: AssetId,
    computed: SwapResult,
    block_height: BlockHeight,
    op_id: Option<Hash>,
) -> Result<SwapResult, AmmError> {
    let pool = pools.get_pool(&pool_id)?;
    let path = resolve_path(pool, token_in, token_out)?;
    let before = pool.reserves;

    transfer_swap_assets(
        accounting,
        pool_id,
        trader,
        token_in,
        token_out,
        computed.amount_in,
        computed.amount_out,
    )?;

    let after = update_pool_reserves(pools, pool_id, &path, computed.amount_in, computed.amount_out)?;
    validate_invariant(before, after)?;

    let breakdown = fees.record(pool_id, token_in, computed.fee_amount)?;
    pools.push_event(AmmEvent::SwapExecuted {
        pool_id,
        trader,
        token_in,
        token_out,
        amount_in: computed.amount_in,
        amount_out: computed.amount_out,
        fee_amount: computed.fee_amount,
        block_height,
        transaction_id: op_id,
    });
    pools.push_event(FeeTracker::fee_collected_event(
        breakdown,
        pool_id,
        token_in,
        block_height,
        op_id,
    ));
    accounting.assert_consistent(pools)?;
    Ok(computed)
}

/// Valida deadline de swap.
pub fn validate_swap_deadline(
    deadline: Option<BlockHeight>,
    current: BlockHeight,
) -> Result<(), AmmError> {
    if let Some(d) = deadline {
        if current.0 > d.0 {
            return Err(AmmError::DeadlineExceeded);
        }
    }
    Ok(())
}

/// Valida mínimo de salida.
pub fn validate_min_amount_out(actual: Amount, min_out: Amount) -> Result<(), AmmError> {
    if actual.0 < min_out.0 {
        return Err(AmmError::SlippageExceeded);
    }
    Ok(())
}

/// Valida máximo de entrada.
pub fn validate_max_amount_in(actual: Amount, max_in: Amount) -> Result<(), AmmError> {
    if actual.0 > max_in.0 {
        return Err(AmmError::SlippageExceeded);
    }
    Ok(())
}

/// Calcula el resultado teórico de un path.
pub fn calculate_swap_result(
    path: &SwapPath,
    amount_in: Amount,
) -> Result<SwapResult, AmmError> {
    let amount_out =
        calculate_amount_out(amount_in, path.reserve_in, path.reserve_out, path.fee)?;
    let fee = fee_amount(amount_in, path.fee)?;
    Ok(SwapResult {
        amount_in,
        amount_out,
        fee_amount: fee,
    })
}

/// Valida un swap exact-input sin mutar estado.
pub fn validate_swap_exact_input(
    pools: &PoolRegistry,
    accounting: &AmmAccounting,
    params: &ExactInputSwap,
) -> Result<SwapResult, AmmError> {
    pools.ensure_swap_allowed(&params.pool_id)?;
    if params.amount_in.0 == 0 {
        return Err(AmmError::ZeroAmount);
    }
    let pool = pools.get_pool(&params.pool_id)?;
    let path = resolve_path(pool, params.token_in, params.token_out)?;
    if accounting.balance_of(&params.trader, &params.token_in).0 < params.amount_in.0 {
        return Err(AmmError::InsufficientBalance);
    }
    let result = calculate_swap_result(&path, params.amount_in)?;
    validate_min_amount_out(result.amount_out, params.min_amount_out)?;
    Ok(result)
}

/// Valida un swap exact-output sin mutar estado.
pub fn validate_swap_exact_output(
    pools: &PoolRegistry,
    accounting: &AmmAccounting,
    params: &ExactOutputSwap,
) -> Result<SwapResult, AmmError> {
    pools.ensure_swap_allowed(&params.pool_id)?;
    if params.amount_out.0 == 0 {
        return Err(AmmError::ZeroAmount);
    }
    let pool = pools.get_pool(&params.pool_id)?;
    let path = resolve_path(pool, params.token_in, params.token_out)?;
    let amount_in =
        calculate_amount_in(params.amount_out, path.reserve_in, path.reserve_out, path.fee)?;
    validate_max_amount_in(amount_in, params.max_amount_in)?;
    if accounting.balance_of(&params.trader, &params.token_in).0 < amount_in.0 {
        return Err(AmmError::InsufficientBalance);
    }
    let fee = fee_amount(amount_in, path.fee)?;
    Ok(SwapResult {
        amount_in,
        amount_out: params.amount_out,
        fee_amount: fee,
    })
}

/// Alias de validación genérica (exact-input).
pub fn validate_swap(
    pools: &PoolRegistry,
    accounting: &AmmAccounting,
    params: &ExactInputSwap,
) -> Result<SwapResult, AmmError> {
    validate_swap_exact_input(pools, accounting, params)
}

/// Actualiza reservas tras un swap.
pub fn update_pool_reserves(
    pools: &mut PoolRegistry,
    pool_id: PoolId,
    path: &SwapPath,
    amount_in: Amount,
    amount_out: Amount,
) -> Result<Reserves, AmmError> {
    let pool = pools.get_pool(&pool_id)?;
    let (new_a, new_b) = if path.in_is_a {
        (
            Amount(checked_add(pool.reserves.reserve_a.0, amount_in.0)?),
            Amount(checked_sub(pool.reserves.reserve_b.0, amount_out.0)?),
        )
    } else {
        (
            Amount(checked_sub(pool.reserves.reserve_a.0, amount_out.0)?),
            Amount(checked_add(pool.reserves.reserve_b.0, amount_in.0)?),
        )
    };
    let after = Reserves::new(new_a, new_b);
    let supply = pool.total_supply;
    let locked = pool.locked_supply;
    pools.record_reserves(&pool_id, after, supply, locked)?;
    Ok(after)
}

/// Transfiere input al pool y output al trader.
pub fn transfer_swap_assets(
    accounting: &mut AmmAccounting,
    pool_id: PoolId,
    trader: AccountId,
    token_in: AssetId,
    token_out: AssetId,
    amount_in: Amount,
    amount_out: Amount,
) -> Result<(), AmmError> {
    accounting.transfer_to_pool(trader, pool_id, token_in, amount_in)?;
    accounting.transfer_from_pool(pool_id, trader, token_out, amount_out)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fees::FeeTracker;
    use crate::liquidity::{AddLiquidityParams, LiquidityEngine};
    use crate::pool::{GovernanceAuthority, InMemoryAssetCatalog, PoolPolicy};
    use crate::pricing::{
        calculate_amount_out, calculate_price_impact, calculate_spot_price, get_quote,
    };
    use crate::types::FeeBps;
    use crate::BPS_DENOMINATOR;
    use proptest::prelude::*;
    use rsc_core::Hash as CoreHash;

    fn asset(b: u8) -> AssetId {
        AssetId(CoreHash([b; 32]))
    }
    fn account(b: u8) -> AccountId {
        AccountId([b; 32])
    }

    fn live_pool() -> (
        PoolRegistry,
        AmmAccounting,
        FeeTracker,
        InMemoryAssetCatalog,
        PoolId,
        AccountId,
    ) {
        let mut catalog = InMemoryAssetCatalog::new();
        catalog.register(asset(1));
        catalog.register(asset(2));
        let mut gov = GovernanceAuthority::new();
        gov.grant(account(0xAA));
        let provider = account(1);
        let trader = account(2);
        let mut pools = PoolRegistry::new(PoolPolicy::default());
        let mut acc = AmmAccounting::new();
        acc.credit(provider, asset(1), Amount(1_000_000)).unwrap();
        acc.credit(provider, asset(2), Amount(1_000_000)).unwrap();
        acc.credit(trader, asset(1), Amount(100_000)).unwrap();
        acc.credit(trader, asset(2), Amount(100_000)).unwrap();
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
                max_amount_a: Amount(200_000),
                max_amount_b: Amount(200_000),
                min_lp_shares: crate::LpShares(1),
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
        (pools, acc, FeeTracker::new(), catalog, pool_id, trader)
    }

    #[test]
    fn exact_input_both_directions() {
        let (mut pools, mut acc, mut fees, _, pool_id, trader) = live_pool();
        let q = get_quote(
            &pools,
            pool_id,
            asset(1),
            asset(2),
            Amount(1_000),
            BlockHeight(4),
            None,
        )
        .unwrap();
        let r = SwapEngine::execute_swap_exact_input(
            &mut pools,
            &mut acc,
            &mut fees,
            ExactInputSwap {
                pool_id,
                trader,
                token_in: asset(1),
                token_out: asset(2),
                amount_in: Amount(1_000),
                min_amount_out: q.amount_out,
                deadline: Some(BlockHeight(10)),
                current_block: BlockHeight(4),
                op_id: Some(CoreHash([1; 32])),
            },
        )
        .unwrap();
        assert_eq!(r.amount_out, q.amount_out);
        assert!(fees.accrued(&pool_id) > 0);

        let r2 = SwapEngine::execute_swap_exact_input(
            &mut pools,
            &mut acc,
            &mut fees,
            ExactInputSwap {
                pool_id,
                trader,
                token_in: asset(2),
                token_out: asset(1),
                amount_in: Amount(500),
                min_amount_out: Amount(1),
                deadline: None,
                current_block: BlockHeight(5),
                op_id: None,
            },
        )
        .unwrap();
        assert!(r2.amount_out.0 > 0);
        acc.assert_consistent(&pools).unwrap();
    }

    #[test]
    fn exact_output_and_max_in_protection() {
        let (mut pools, mut acc, mut fees, _, pool_id, trader) = live_pool();
        let need = SwapEngine::execute_swap_exact_output(
            &mut pools,
            &mut acc,
            &mut fees,
            ExactOutputSwap {
                pool_id,
                trader,
                token_in: asset(1),
                token_out: asset(2),
                amount_out: Amount(1_000),
                max_amount_in: Amount(2_000),
                deadline: None,
                current_block: BlockHeight(4),
                op_id: None,
            },
        )
        .unwrap();
        assert_eq!(need.amount_out.0, 1_000);
        assert!(need.amount_in.0 <= 2_000);

        assert_eq!(
            SwapEngine::execute_swap_exact_output(
                &mut pools,
                &mut acc,
                &mut fees,
                ExactOutputSwap {
                    pool_id,
                    trader,
                    token_in: asset(1),
                    token_out: asset(2),
                    amount_out: Amount(1_000),
                    max_amount_in: Amount(1),
                    deadline: None,
                    current_block: BlockHeight(5),
                    op_id: None,
                },
            )
            .unwrap_err(),
            AmmError::SlippageExceeded
        );
    }

    #[test]
    fn min_out_deadline_paused_zero_duplicate() {
        let (mut pools, mut acc, mut fees, catalog, pool_id, trader) = live_pool();
        let _ = catalog;
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
                    min_amount_out: Amount(u128::MAX),
                    deadline: None,
                    current_block: BlockHeight(4),
                    op_id: None,
                },
            )
            .unwrap_err(),
            AmmError::SlippageExceeded
        );
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
                    min_amount_out: Amount(0),
                    deadline: Some(BlockHeight(1)),
                    current_block: BlockHeight(4),
                    op_id: None,
                },
            )
            .unwrap_err(),
            AmmError::DeadlineExceeded
        );
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
                    amount_in: Amount(0),
                    min_amount_out: Amount(0),
                    deadline: None,
                    current_block: BlockHeight(4),
                    op_id: None,
                },
            )
            .unwrap_err(),
            AmmError::ZeroAmount
        );

        let mut gov = GovernanceAuthority::new();
        gov.grant(account(0xAA));
        pools
            .pause_pool(&gov, account(0xAA), pool_id, BlockHeight(6))
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
                    amount_in: Amount(10),
                    min_amount_out: Amount(0),
                    deadline: None,
                    current_block: BlockHeight(7),
                    op_id: None,
                },
            )
            .unwrap_err(),
            AmmError::SwapNotPermitted
        );
    }

    #[test]
    fn rollback_on_transfer_failure() {
        let (mut pools, mut acc, mut fees, _, pool_id, trader) = live_pool();
        let before = acc.balance_of(&trader, &asset(1));
        let reserves_before = pools.get_pool(&pool_id).unwrap().reserves;
        acc.fail_next_pool_transfer = true;
        let err = SwapEngine::execute_swap_exact_input(
            &mut pools,
            &mut acc,
            &mut fees,
            ExactInputSwap {
                pool_id,
                trader,
                token_in: asset(1),
                token_out: asset(2),
                amount_in: Amount(1_000),
                min_amount_out: Amount(1),
                deadline: None,
                current_block: BlockHeight(4),
                op_id: Some(CoreHash([9; 32])),
            },
        )
        .unwrap_err();
        assert_eq!(err, AmmError::TransferFailed);
        assert_eq!(acc.balance_of(&trader, &asset(1)), before);
        assert_eq!(pools.get_pool(&pool_id).unwrap().reserves, reserves_before);
        assert!(!acc.op_seen(&CoreHash([9; 32])));
        assert_eq!(fees.accrued(&pool_id), 0);
    }

    #[test]
    fn quote_does_not_mutate() {
        let (pools, _, _, _, pool_id, _) = live_pool();
        let before = pools.to_canonical_bytes();
        let _ = get_quote(
            &pools,
            pool_id,
            asset(1),
            asset(2),
            Amount(1_000),
            BlockHeight(4),
            None,
        )
        .unwrap();
        assert_eq!(before, pools.to_canonical_bytes());
    }

    #[test]
    fn repeated_swaps_keep_invariant() {
        let (mut pools, mut acc, mut fees, _, pool_id, trader) = live_pool();
        for i in 0..5u8 {
            SwapEngine::execute_swap_exact_input(
                &mut pools,
                &mut acc,
                &mut fees,
                ExactInputSwap {
                    pool_id,
                    trader,
                    token_in: asset(1),
                    token_out: asset(2),
                    amount_in: Amount(100 + u128::from(i)),
                    min_amount_out: Amount(1),
                    deadline: None,
                    current_block: BlockHeight(10 + u64::from(i)),
                    op_id: Some(CoreHash([i; 32])),
                },
            )
            .unwrap();
        }
        acc.assert_consistent(&pools).unwrap();
    }

    proptest! {
        #[test]
        fn prop_impact_non_negative(
            rin in 10_000u128..200_000,
            rout in 10_000u128..200_000,
            din in 10u128..1_000,
        ) {
            let fee = FeeBps::new(30).unwrap();
            let Ok(out) = calculate_amount_out(Amount(din), Amount(rin), Amount(rout), fee) else {
                return Ok(());
            };
            let spot = calculate_spot_price(Amount(rin), Amount(rout)).unwrap();
            let exec = crate::pricing::calculate_execution_price(Amount(din), out).unwrap();
            let impact = calculate_price_impact(spot, exec).unwrap();
            assert!(impact <= BPS_DENOMINATOR);
        }
    }
}
