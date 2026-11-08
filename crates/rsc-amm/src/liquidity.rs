//! Motor de depósitos y retiros de liquidez.
//!
//! Transiciones atómicas sobre [`PoolRegistry`] + [`AmmAccounting`]:
//! validación → cálculo → transferencias → mint/burn LP → reservas → evento.
//! Si cualquier paso falla, se restauran ambos estados (snapshot).
//!
//! No usa floats. Reutiliza [`crate::math::shares`]. No crea un token LP
//! transferible aparte: las shares viven en el ledger AMM por cuenta.

use crate::accounting::{AmmAccounting, LiquidityPosition, PoolLiquidity};
use crate::constants::LP_SINK_ACCOUNT;
use crate::events::AmmEvent;
use crate::math::amount::{checked_add, checked_sub, mul_div_floor};
use crate::math::shares::{self, MintResult, RedeemResult};
use crate::pool::{AssetCatalog, PoolRegistry};
use crate::types::{AssetId, LpShares, PoolId, Reserves};
use crate::AmmError;
use rsc_core::{AccountId, Amount, BlockHeight, Hash};

/// Parámetros de `add_liquidity`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddLiquidityParams {
    /// Pool destino.
    pub pool_id: PoolId,
    /// Proveedor.
    pub provider: AccountId,
    /// Máximo de activo A (canónico del pool) a depositar.
    pub max_amount_a: Amount,
    /// Máximo de activo B a depositar.
    pub max_amount_b: Amount,
    /// Mínimo de LP que debe recibir el proveedor.
    pub min_lp_shares: LpShares,
    /// Deadline opcional; si `current_block > deadline` se rechaza.
    pub deadline: Option<BlockHeight>,
    /// Altura actual de la transición.
    pub current_block: BlockHeight,
    /// Id de operación anti-replay (p. ej. hash de tx).
    pub op_id: Option<Hash>,
}

/// Resultado de un depósito.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AddLiquidityResult {
    /// Cantidades efectivamente depositadas (sin exceso).
    pub amount_a: Amount,
    /// Cantidad B depositada.
    pub amount_b: Amount,
    /// LP acuñados al proveedor.
    pub lp_minted: LpShares,
    /// LP bloqueados en el sink (solo bootstrap).
    pub lp_locked: LpShares,
}

/// Parámetros de `remove_liquidity`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoveLiquidityParams {
    /// Pool.
    pub pool_id: PoolId,
    /// Proveedor.
    pub provider: AccountId,
    /// Shares a quemar.
    pub lp_shares: LpShares,
    /// Mínimo de A a recibir.
    pub min_amount_a: Amount,
    /// Mínimo de B a recibir.
    pub min_amount_b: Amount,
    /// Deadline opcional.
    pub deadline: Option<BlockHeight>,
    /// Altura actual.
    pub current_block: BlockHeight,
    /// Id anti-replay.
    pub op_id: Option<Hash>,
}

/// Resultado de un retiro.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RemoveLiquidityResult {
    /// A recibido.
    pub amount_a: Amount,
    /// B recibido.
    pub amount_b: Amount,
    /// LP quemados.
    pub lp_burned: LpShares,
}

/// Motor de liquidez (stateless; el estado vive en registry + accounting).
pub struct LiquidityEngine;

impl LiquidityEngine {
    /// Deposita liquidez de forma atómica.
    pub fn add_liquidity<C: AssetCatalog>(
        pools: &mut PoolRegistry,
        accounting: &mut AmmAccounting,
        catalog: &C,
        params: AddLiquidityParams,
    ) -> Result<AddLiquidityResult, AmmError> {
        let snap_p = pools.clone();
        let snap_a = accounting.clone();
        match Self::add_liquidity_inner(pools, accounting, catalog, params) {
            Ok(r) => Ok(r),
            Err(e) => {
                *pools = snap_p;
                *accounting = snap_a;
                Err(e)
            }
        }
    }

    /// Retira liquidez de forma atómica (permitido en pools pausados).
    pub fn remove_liquidity(
        pools: &mut PoolRegistry,
        accounting: &mut AmmAccounting,
        params: RemoveLiquidityParams,
    ) -> Result<RemoveLiquidityResult, AmmError> {
        let snap_p = pools.clone();
        let snap_a = accounting.clone();
        match Self::remove_liquidity_inner(pools, accounting, params) {
            Ok(r) => Ok(r),
            Err(e) => {
                *pools = snap_p;
                *accounting = snap_a;
                Err(e)
            }
        }
    }

    fn add_liquidity_inner<C: AssetCatalog>(
        pools: &mut PoolRegistry,
        accounting: &mut AmmAccounting,
        catalog: &C,
        params: AddLiquidityParams,
    ) -> Result<AddLiquidityResult, AmmError> {
        check_deadline(params.deadline, params.current_block)?;
        if let Some(op) = params.op_id {
            accounting.mark_op(op)?;
        }

        let plan = validate_deposit(pools, accounting, catalog, &params)?;
        let (asset_a, asset_b, reserve_a, reserve_b, locked_supply, total_supply) = {
            let pool = pools.get_pool(&params.pool_id)?;
            (
                pool.identity.asset_a,
                pool.identity.asset_b,
                pool.reserves.reserve_a.0,
                pool.reserves.reserve_b.0,
                pool.locked_supply.0,
                pool.total_supply.0,
            )
        };

        accounting.transfer_to_pool(
            params.provider,
            params.pool_id,
            asset_a,
            plan.amount_a,
        )?;
        accounting.transfer_to_pool(
            params.provider,
            params.pool_id,
            asset_b,
            plan.amount_b,
        )?;

        accounting.mint_lp(params.pool_id, params.provider, plan.lp_minted)?;
        if plan.lp_minted.0 > 0 {
            pools.push_event(AmmEvent::LPSharesMinted {
                pool_id: params.pool_id,
                to: params.provider,
                shares: plan.lp_minted,
                block_height: params.current_block,
                transaction_id: params.op_id,
            });
        }
        if plan.lp_locked.0 > 0 {
            accounting.mint_lp(params.pool_id, LP_SINK_ACCOUNT, plan.lp_locked)?;
            pools.push_event(AmmEvent::LPSharesMinted {
                pool_id: params.pool_id,
                to: LP_SINK_ACCOUNT,
                shares: plan.lp_locked,
                block_height: params.current_block,
                transaction_id: params.op_id,
            });
        }

        let new_reserves = Reserves::new(
            Amount(checked_add(reserve_a, plan.amount_a.0)?),
            Amount(checked_add(reserve_b, plan.amount_b.0)?),
        );
        let new_locked = LpShares(checked_add(locked_supply, plan.lp_locked.0)?);
        let minted_total = plan
            .lp_minted
            .0
            .checked_add(plan.lp_locked.0)
            .ok_or(AmmError::Overflow)?;
        let new_supply = LpShares(checked_add(total_supply, minted_total)?);

        pools.record_reserves(&params.pool_id, new_reserves, new_supply, new_locked)?;
        pools.push_event(AmmEvent::LiquidityAdded {
            pool_id: params.pool_id,
            provider: params.provider,
            amount_a: plan.amount_a,
            amount_b: plan.amount_b,
            lp_minted: plan.lp_minted,
            lp_locked: plan.lp_locked,
            block_height: params.current_block,
            transaction_id: params.op_id,
        });

        accounting.assert_consistent(pools)?;
        Ok(AddLiquidityResult {
            amount_a: plan.amount_a,
            amount_b: plan.amount_b,
            lp_minted: plan.lp_minted,
            lp_locked: plan.lp_locked,
        })
    }

    fn remove_liquidity_inner(
        pools: &mut PoolRegistry,
        accounting: &mut AmmAccounting,
        params: RemoveLiquidityParams,
    ) -> Result<RemoveLiquidityResult, AmmError> {
        check_deadline(params.deadline, params.current_block)?;
        if let Some(op) = params.op_id {
            accounting.mark_op(op)?;
        }

        let plan = validate_withdrawal(pools, accounting, &params)?;
        let (asset_a, asset_b, reserve_a, reserve_b, locked) = {
            let pool = pools.get_pool(&params.pool_id)?;
            (
                pool.identity.asset_a,
                pool.identity.asset_b,
                pool.reserves.reserve_a.0,
                pool.reserves.reserve_b.0,
                pool.locked_supply,
            )
        };

        accounting.burn_lp(params.pool_id, params.provider, params.lp_shares)?;
        pools.push_event(AmmEvent::LPSharesBurned {
            pool_id: params.pool_id,
            from: params.provider,
            shares: params.lp_shares,
            block_height: params.current_block,
            transaction_id: params.op_id,
        });

        let new_reserves = Reserves::new(
            Amount(checked_sub(reserve_a, plan.amount_a.0)?),
            Amount(checked_sub(reserve_b, plan.amount_b.0)?),
        );
        let new_supply = plan.total_supply_after;

        pools.record_reserves(&params.pool_id, new_reserves, new_supply, locked)?;

        accounting.transfer_from_pool(params.pool_id, params.provider, asset_a, plan.amount_a)?;
        accounting.transfer_from_pool(params.pool_id, params.provider, asset_b, plan.amount_b)?;

        pools.push_event(AmmEvent::LiquidityRemoved {
            pool_id: params.pool_id,
            provider: params.provider,
            amount_a: plan.amount_a,
            amount_b: plan.amount_b,
            lp_burned: params.lp_shares,
            block_height: params.current_block,
            transaction_id: params.op_id,
        });

        accounting.assert_consistent(pools)?;
        Ok(RemoveLiquidityResult {
            amount_a: plan.amount_a,
            amount_b: plan.amount_b,
            lp_burned: params.lp_shares,
        })
    }
}

/// Plan validado de depósito (sin efectos).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DepositPlan {
    /// Aporte A exacto.
    pub amount_a: Amount,
    /// Aporte B exacto.
    pub amount_b: Amount,
    /// LP al proveedor.
    pub lp_minted: LpShares,
    /// LP al sink.
    pub lp_locked: LpShares,
}

/// Plan validado de retiro (sin efectos).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WithdrawPlan {
    /// Salida A.
    pub amount_a: Amount,
    /// Salida B.
    pub amount_b: Amount,
    /// Supply tras quemar.
    pub total_supply_after: LpShares,
}

fn check_deadline(deadline: Option<BlockHeight>, current: BlockHeight) -> Result<(), AmmError> {
    if let Some(d) = deadline {
        if current.0 > d.0 {
            return Err(AmmError::DeadlineExceeded);
        }
    }
    Ok(())
}

/// Valida un depósito y calcula cantidades/shares exactas.
///
/// Política: **no se acepta exceso**. Los `max_amount_*` deben coincidir con
/// el aporte óptimo al ratio actual; si no, [`AmmError::UnbalancedDeposit`].
pub fn validate_deposit<C: AssetCatalog>(
    pools: &PoolRegistry,
    accounting: &AmmAccounting,
    catalog: &C,
    params: &AddLiquidityParams,
) -> Result<DepositPlan, AmmError> {
    pools.ensure_mint_allowed(&params.pool_id)?;
    let pool = pools.get_pool(&params.pool_id)?;
    let asset_a = pool.identity.asset_a;
    let asset_b = pool.identity.asset_b;
    if !catalog.is_registered(&asset_a) || !catalog.is_registered(&asset_b) {
        return Err(AmmError::AssetNotRegistered);
    }
    if params.max_amount_a.0 == 0 || params.max_amount_b.0 == 0 {
        return Err(AmmError::ZeroAmount);
    }
    if accounting.balance_of(&params.provider, &asset_a).0 < params.max_amount_a.0
        || accounting.balance_of(&params.provider, &asset_b).0 < params.max_amount_b.0
    {
        return Err(AmmError::InsufficientBalance);
    }

    let (amount_a, amount_b, mint) = if pool.total_supply.0 == 0 {
        let mint = calculate_initial_shares(
            params.max_amount_a,
            params.max_amount_b,
            pools.policy().min_liquidity,
        )?;
        (params.max_amount_a, params.max_amount_b, mint)
    } else {
        let (opt_a, opt_b) = quote_balanced_deposit(
            params.max_amount_a,
            params.max_amount_b,
            pool.reserves,
        )?;
        if opt_a != params.max_amount_a || opt_b != params.max_amount_b {
            return Err(AmmError::UnbalancedDeposit);
        }
        let mint = calculate_liquidity_shares(
            opt_a,
            opt_b,
            pool.reserves,
            pool.total_supply,
        )?;
        (opt_a, opt_b, mint)
    };

    if mint.minted_to_provider.0 < params.min_lp_shares.0 {
        return Err(AmmError::SlippageExceeded);
    }

    Ok(DepositPlan {
        amount_a,
        amount_b,
        lp_minted: mint.minted_to_provider,
        lp_locked: mint.locked_in_sink,
    })
}

/// Valida un retiro.
pub fn validate_withdrawal(
    pools: &PoolRegistry,
    accounting: &AmmAccounting,
    params: &RemoveLiquidityParams,
) -> Result<WithdrawPlan, AmmError> {
    pools.ensure_burn_allowed(&params.pool_id)?;
    if params.provider == LP_SINK_ACCOUNT {
        return Err(AmmError::LockedLiquidity);
    }
    if params.lp_shares.0 == 0 {
        return Err(AmmError::ZeroAmount);
    }
    let pool = pools.get_pool(&params.pool_id)?;
    let owned = accounting.lp_balance_of(&params.pool_id, &params.provider);
    if owned.0 < params.lp_shares.0 {
        return Err(AmmError::InsufficientLpBalance);
    }
    // No se puede quemar por encima del supply menos el sink (conservación).
    let withdrawable_supply = checked_sub(pool.total_supply.0, pool.locked_supply.0)?;
    if params.lp_shares.0 > withdrawable_supply {
        return Err(AmmError::LockedLiquidity);
    }

    let redeem = calculate_withdrawal(params.lp_shares, pool.reserves, pool.total_supply)?;
    if redeem.amount_a.0 < params.min_amount_a.0 || redeem.amount_b.0 < params.min_amount_b.0 {
        return Err(AmmError::SlippageExceeded);
    }
    if redeem.amount_a.0 > pool.reserves.reserve_a.0
        || redeem.amount_b.0 > pool.reserves.reserve_b.0
    {
        return Err(AmmError::InsufficientLiquidity);
    }

    Ok(WithdrawPlan {
        amount_a: redeem.amount_a,
        amount_b: redeem.amount_b,
        total_supply_after: redeem.total_supply_after,
    })
}

/// LP inicial: `floor(sqrt(a*b))` con lock de `min_liquidity`.
pub fn calculate_initial_shares(
    amount_a: Amount,
    amount_b: Amount,
    min_liquidity: u128,
) -> Result<MintResult, AmmError> {
    shares::initial_shares(amount_a, amount_b, min_liquidity)
}

/// LP posterior: `min` de proporciones.
pub fn calculate_liquidity_shares(
    amount_a: Amount,
    amount_b: Amount,
    reserves: Reserves,
    total_supply: LpShares,
) -> Result<MintResult, AmmError> {
    shares::mint_shares(amount_a, amount_b, reserves, total_supply)
}

/// Montos de retiro proporcionales (floor).
pub fn calculate_withdrawal(
    lp_shares: LpShares,
    reserves: Reserves,
    total_supply: LpShares,
) -> Result<RedeemResult, AmmError> {
    shares::redeem_amounts(lp_shares, reserves, total_supply)
}

/// Cotiza el aporte balanceado ≤ máximos (estilo Uniswap V2).
pub fn quote_balanced_deposit(
    max_a: Amount,
    max_b: Amount,
    reserves: Reserves,
) -> Result<(Amount, Amount), AmmError> {
    if reserves.reserve_a.0 == 0 || reserves.reserve_b.0 == 0 {
        return Err(AmmError::InvalidReserves);
    }
    let b_opt = mul_div_floor(max_a.0, reserves.reserve_b.0, reserves.reserve_a.0)?;
    if b_opt <= max_b.0 {
        if b_opt == 0 {
            return Err(AmmError::UnbalancedDeposit);
        }
        Ok((max_a, Amount(b_opt)))
    } else {
        let a_opt = mul_div_floor(max_b.0, reserves.reserve_a.0, reserves.reserve_b.0)?;
        if a_opt == 0 {
            return Err(AmmError::UnbalancedDeposit);
        }
        Ok((Amount(a_opt), max_b))
    }
}

/// Posición del usuario.
pub fn get_user_liquidity(
    pools: &PoolRegistry,
    accounting: &AmmAccounting,
    pool_id: PoolId,
    provider: AccountId,
) -> Result<LiquidityPosition, AmmError> {
    let pool = pools.get_pool(&pool_id)?;
    let lp = accounting.lp_balance_of(&pool_id, &provider);
    let (value_a, value_b) = if lp.0 == 0 || pool.total_supply.0 == 0 {
        (Amount::ZERO, Amount::ZERO)
    } else {
        shares::share_value(lp, pool.reserves, pool.total_supply)?
    };
    Ok(LiquidityPosition {
        pool_id,
        provider,
        lp_shares: lp,
        value_a,
        value_b,
    })
}

/// Liquidez agregada del pool.
pub fn get_pool_liquidity(pools: &PoolRegistry, pool_id: PoolId) -> Result<PoolLiquidity, AmmError> {
    let pool = pools.get_pool(&pool_id)?;
    Ok(PoolLiquidity {
        pool_id,
        reserve_a: pool.reserves.reserve_a,
        reserve_b: pool.reserves.reserve_b,
        total_supply: pool.total_supply,
        locked_supply: pool.locked_supply,
    })
}

/// Saldo LP del usuario.
pub fn get_user_lp_balance(
    accounting: &AmmAccounting,
    pool_id: PoolId,
    provider: AccountId,
) -> LpShares {
    accounting.lp_balance_of(&pool_id, &provider)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool::{GovernanceAuthority, InMemoryAssetCatalog, PoolPolicy};
    use crate::types::FeeBps;
    use crate::DEFAULT_MIN_LIQUIDITY;
    use proptest::prelude::*;
    use rsc_core::Hash as CoreHash;

    fn asset(b: u8) -> AssetId {
        AssetId(CoreHash([b; 32]))
    }
    fn account(b: u8) -> AccountId {
        AccountId([b; 32])
    }

    fn ready_pool() -> (
        PoolRegistry,
        AmmAccounting,
        InMemoryAssetCatalog,
        GovernanceAuthority,
        PoolId,
    ) {
        let mut catalog = InMemoryAssetCatalog::new();
        catalog.register(asset(1));
        catalog.register(asset(2));
        let mut gov = GovernanceAuthority::new();
        gov.grant(account(0xAA));
        let mut pools = PoolRegistry::new(PoolPolicy::default());
        let mut accounting = AmmAccounting::new();
        let provider = account(1);
        accounting
            .credit(provider, asset(1), Amount(1_000_000))
            .unwrap();
        accounting
            .credit(provider, asset(2), Amount(1_000_000))
            .unwrap();
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
        (pools, accounting, catalog, gov, pool_id)
    }

    #[test]
    fn initial_and_subsequent_deposit_withdraw() {
        let (mut pools, mut acc, catalog, gov, pool_id) = ready_pool();
        let provider = account(1);
        let add = LiquidityEngine::add_liquidity(
            &mut pools,
            &mut acc,
            &catalog,
            AddLiquidityParams {
                pool_id,
                provider,
                max_amount_a: Amount(50_000),
                max_amount_b: Amount(50_000),
                min_lp_shares: LpShares(1),
                deadline: None,
                current_block: BlockHeight(2),
                op_id: None,
            },
        )
        .unwrap();
        assert!(add.lp_minted.0 > 0);
        assert_eq!(add.lp_locked.0, DEFAULT_MIN_LIQUIDITY);
        pools
            .activate_pool(&gov, account(0xAA), pool_id, BlockHeight(3))
            .unwrap();

        let add2 = LiquidityEngine::add_liquidity(
            &mut pools,
            &mut acc,
            &catalog,
            AddLiquidityParams {
                pool_id,
                provider,
                max_amount_a: Amount(5_000),
                max_amount_b: Amount(5_000),
                min_lp_shares: LpShares(1),
                deadline: None,
                current_block: BlockHeight(4),
                op_id: None,
            },
        )
        .unwrap();
        assert_eq!(add2.lp_locked.0, 0);

        let lp = get_user_lp_balance(&acc, pool_id, provider);
        let out = LiquidityEngine::remove_liquidity(
            &mut pools,
            &mut acc,
            RemoveLiquidityParams {
                pool_id,
                provider,
                lp_shares: LpShares(add2.lp_minted.0),
                min_amount_a: Amount(0),
                min_amount_b: Amount(0),
                deadline: None,
                current_block: BlockHeight(5),
                op_id: None,
            },
        )
        .unwrap();
        assert!(out.amount_a.0 > 0);
        assert_eq!(
            get_user_lp_balance(&acc, pool_id, provider).0,
            lp.0 - add2.lp_minted.0
        );
        acc.assert_consistent(&pools).unwrap();
    }

    #[test]
    fn unbalanced_and_zero_rejected() {
        let (mut pools, mut acc, catalog, gov, pool_id) = ready_pool();
        let provider = account(1);
        LiquidityEngine::add_liquidity(
            &mut pools,
            &mut acc,
            &catalog,
            AddLiquidityParams {
                pool_id,
                provider,
                max_amount_a: Amount(50_000),
                max_amount_b: Amount(50_000),
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
        assert_eq!(
            LiquidityEngine::add_liquidity(
                &mut pools,
                &mut acc,
                &catalog,
                AddLiquidityParams {
                    pool_id,
                    provider,
                    max_amount_a: Amount(100),
                    max_amount_b: Amount(200),
                    min_lp_shares: LpShares(1),
                    deadline: None,
                    current_block: BlockHeight(4),
                    op_id: None,
                },
            )
            .unwrap_err(),
            AmmError::UnbalancedDeposit
        );
        assert_eq!(
            LiquidityEngine::add_liquidity(
                &mut pools,
                &mut acc,
                &catalog,
                AddLiquidityParams {
                    pool_id,
                    provider,
                    max_amount_a: Amount(0),
                    max_amount_b: Amount(100),
                    min_lp_shares: LpShares(0),
                    deadline: None,
                    current_block: BlockHeight(4),
                    op_id: None,
                },
            )
            .unwrap_err(),
            AmmError::ZeroAmount
        );
    }

    #[test]
    fn rollback_on_transfer_failure() {
        let (mut pools, mut acc, catalog, _, pool_id) = ready_pool();
        let provider = account(1);
        let before_a = acc.balance_of(&provider, &asset(1));
        let before_pools = pools.clone();
        acc.fail_next_pool_transfer = true;
        let err = LiquidityEngine::add_liquidity(
            &mut pools,
            &mut acc,
            &catalog,
            AddLiquidityParams {
                pool_id,
                provider,
                max_amount_a: Amount(50_000),
                max_amount_b: Amount(50_000),
                min_lp_shares: LpShares(1),
                deadline: None,
                current_block: BlockHeight(2),
                op_id: Some(CoreHash([7; 32])),
            },
        )
        .unwrap_err();
        assert_eq!(err, AmmError::TransferFailed);
        assert_eq!(acc.balance_of(&provider, &asset(1)), before_a);
        assert_eq!(pools.get_pool(&pool_id).unwrap().total_supply, LpShares::ZERO);
        assert!(!acc.op_seen(&CoreHash([7; 32])));
        assert_eq!(
            pools.get_pool(&pool_id).unwrap().reserves,
            before_pools.get_pool(&pool_id).unwrap().reserves
        );
    }

    #[test]
    fn paused_allows_withdraw_not_add() {
        let (mut pools, mut acc, catalog, gov, pool_id) = ready_pool();
        let provider = account(1);
        LiquidityEngine::add_liquidity(
            &mut pools,
            &mut acc,
            &catalog,
            AddLiquidityParams {
                pool_id,
                provider,
                max_amount_a: Amount(50_000),
                max_amount_b: Amount(50_000),
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
        pools
            .pause_pool(&gov, account(0xAA), pool_id, BlockHeight(4))
            .unwrap();
        assert_eq!(
            LiquidityEngine::add_liquidity(
                &mut pools,
                &mut acc,
                &catalog,
                AddLiquidityParams {
                    pool_id,
                    provider,
                    max_amount_a: Amount(1_000),
                    max_amount_b: Amount(1_000),
                    min_lp_shares: LpShares(1),
                    deadline: None,
                    current_block: BlockHeight(5),
                    op_id: None,
                },
            )
            .unwrap_err(),
            AmmError::MintNotPermitted
        );
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
                current_block: BlockHeight(5),
                op_id: None,
            },
        )
        .unwrap();
        acc.assert_consistent(&pools).unwrap();
    }

    #[test]
    fn cannot_withdraw_locked_or_duplicate_op() {
        let (mut pools, mut acc, catalog, _, pool_id) = ready_pool();
        let provider = account(1);
        let op = CoreHash([9; 32]);
        LiquidityEngine::add_liquidity(
            &mut pools,
            &mut acc,
            &catalog,
            AddLiquidityParams {
                pool_id,
                provider,
                max_amount_a: Amount(50_000),
                max_amount_b: Amount(50_000),
                min_lp_shares: LpShares(1),
                deadline: None,
                current_block: BlockHeight(2),
                op_id: Some(op),
            },
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
                    max_amount_a: Amount(1_000),
                    max_amount_b: Amount(1_000),
                    min_lp_shares: LpShares(1),
                    deadline: None,
                    current_block: BlockHeight(3),
                    op_id: Some(op),
                },
            )
            .unwrap_err(),
            AmmError::DuplicateOperation
        );
        // Intentar quemar más que withdrawable (supply - locked)
        let pool = pools.get_pool(&pool_id).unwrap();
        let owned = get_user_lp_balance(&acc, pool_id, provider);
        assert!(owned.0 + pool.locked_supply.0 == pool.total_supply.0);
        assert_eq!(
            LiquidityEngine::remove_liquidity(
                &mut pools,
                &mut acc,
                RemoveLiquidityParams {
                    pool_id,
                    provider: LP_SINK_ACCOUNT,
                    lp_shares: LpShares(1),
                    min_amount_a: Amount(0),
                    min_amount_b: Amount(0),
                    deadline: None,
                    current_block: BlockHeight(4),
                    op_id: None,
                },
            )
            .unwrap_err(),
            AmmError::LockedLiquidity
        );
    }

    proptest! {
        #[test]
        fn prop_withdraw_never_exceeds_reserves(
            shares in 1u128..5_000,
            ra in 10_000u128..100_000,
            rb in 10_000u128..100_000,
        ) {
            let supply = shares + DEFAULT_MIN_LIQUIDITY + 1;
            let r = calculate_withdrawal(
                LpShares(shares),
                Reserves::new(Amount(ra), Amount(rb)),
                LpShares(supply),
            ).unwrap();
            assert!(r.amount_a.0 <= ra);
            assert!(r.amount_b.0 <= rb);
        }
    }
}
