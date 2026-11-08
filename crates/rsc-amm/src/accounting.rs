//! Contabilidad de activos y LP del AMM.
//!
//! ## Rol en RSC Chain
//!
//! En la vía nativa ([`crate::native`]), los saldos de usuario y vaults son
//! autoritativos en `rsc-assets`. Este módulo actúa como working set del motor
//! (más LP shares y anti-replay). Tras cada instrucción se hace flush hacia
//! `rsc-assets` en el mismo `StorageBatch`. Nunca usar saldos de frontend /
//! indexer como fuente de verdad.
//!
//! ## Qué rastrea
//!
//! - Saldos libres `(AccountId, AssetId)`.
//! - Custodia / vault del pool `(PoolId, AssetId)` ≡ reservas registradas.
//! - LP `(PoolId, AccountId)`; el sink usa [`LP_SINK_ACCOUNT`].
//!
//! ## Invariantes
//!
//! 1. `pool_balance(pool, asset_*) == reserve_*`.
//! 2. `sum(lp_balances[pool]) == total_supply` (incluye sink).
//! 3. `lp_balance(pool, LP_SINK_ACCOUNT) == locked_supply`.
//! 4. Fees retenidos **no** aparecen como saldos aparte (viven en reservas).
//!
//! La atomicidad la imponen [`crate::liquidity`] / [`crate::swap`] con
//! snapshot/rollback.

use crate::constants::LP_SINK_ACCOUNT;
use crate::pool::PoolRegistry;
use crate::types::{AssetId, LpShares, PoolId, Reserves};
use crate::AmmError;
use rsc_core::{AccountId, Amount, Hash};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Ledger de saldos del AMM.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AmmAccounting {
    balances: BTreeMap<(AccountId, AssetId), Amount>,
    pool_custody: BTreeMap<(PoolId, AssetId), Amount>,
    lp_balances: BTreeMap<(PoolId, AccountId), LpShares>,
    /// Operaciones ya aplicadas (anti-replay a nivel AMM).
    processed_ops: BTreeSet<Hash>,
    /// Si `true`, la próxima transferencia a custodia falla (tests de rollback).
    #[serde(skip)]
    pub fail_next_pool_transfer: bool,
}

impl AmmAccounting {
    /// Ledger vacío.
    pub fn new() -> Self {
        Self::default()
    }

    /// Abona saldo libre de usuario.
    pub fn credit(
        &mut self,
        account: AccountId,
        asset: AssetId,
        amount: Amount,
    ) -> Result<(), AmmError> {
        self.credit_user(account, asset, amount)
    }

    /// Alias explícito: crédito a cuenta de usuario.
    pub fn credit_user(
        &mut self,
        account: AccountId,
        asset: AssetId,
        amount: Amount,
    ) -> Result<(), AmmError> {
        if amount.0 == 0 {
            return Err(AmmError::ZeroAmount);
        }
        let entry = self.balances.entry((account, asset)).or_insert(Amount::ZERO);
        entry.0 = entry.0.checked_add(amount.0).ok_or(AmmError::Overflow)?;
        Ok(())
    }

    /// Debita saldo libre de usuario.
    pub fn debit(
        &mut self,
        account: AccountId,
        asset: AssetId,
        amount: Amount,
    ) -> Result<(), AmmError> {
        self.debit_user(account, asset, amount)
    }

    /// Alias explícito: débito de cuenta de usuario.
    pub fn debit_user(
        &mut self,
        account: AccountId,
        asset: AssetId,
        amount: Amount,
    ) -> Result<(), AmmError> {
        if amount.0 == 0 {
            return Err(AmmError::ZeroAmount);
        }
        let bal = self.balance_of(&account, &asset);
        if bal.0 < amount.0 {
            return Err(AmmError::InsufficientBalance);
        }
        let new = bal.0 - amount.0;
        if new == 0 {
            self.balances.remove(&(account, asset));
        } else {
            self.balances.insert((account, asset), Amount(new));
        }
        Ok(())
    }

    /// Saldo libre de un usuario.
    pub fn balance_of(&self, account: &AccountId, asset: &AssetId) -> Amount {
        self.balances
            .get(&(*account, *asset))
            .copied()
            .unwrap_or(Amount::ZERO)
    }

    /// Custodia / vault del pool para un activo.
    pub fn pool_balance(&self, pool_id: &PoolId, asset: &AssetId) -> Amount {
        self.pool_custody
            .get(&(*pool_id, *asset))
            .copied()
            .unwrap_or(Amount::ZERO)
    }

    /// Alias: crédito directo al vault del pool (sin pasar por usuario).
    pub fn credit_pool(
        &mut self,
        pool_id: PoolId,
        asset: AssetId,
        amount: Amount,
    ) -> Result<(), AmmError> {
        if amount.0 == 0 {
            return Err(AmmError::ZeroAmount);
        }
        let entry = self
            .pool_custody
            .entry((pool_id, asset))
            .or_insert(Amount::ZERO);
        entry.0 = entry.0.checked_add(amount.0).ok_or(AmmError::Overflow)?;
        Ok(())
    }

    /// Alias: débito directo del vault del pool.
    pub fn debit_pool(
        &mut self,
        pool_id: PoolId,
        asset: AssetId,
        amount: Amount,
    ) -> Result<(), AmmError> {
        if amount.0 == 0 {
            return Err(AmmError::ZeroAmount);
        }
        let cust = self.pool_balance(&pool_id, &asset);
        if cust.0 < amount.0 {
            return Err(AmmError::InsufficientLiquidity);
        }
        let new_c = cust.0 - amount.0;
        if new_c == 0 {
            self.pool_custody.remove(&(pool_id, asset));
        } else {
            self.pool_custody.insert((pool_id, asset), Amount(new_c));
        }
        Ok(())
    }

    /// Saldo LP de una cuenta en un pool (incluye sink si se consulta).
    pub fn lp_balance_of(&self, pool_id: &PoolId, account: &AccountId) -> LpShares {
        self.lp_balances
            .get(&(*pool_id, *account))
            .copied()
            .unwrap_or(LpShares::ZERO)
    }

    /// ¿Ya se procesó este `transaction_id` / op?
    pub fn op_seen(&self, op_id: &Hash) -> bool {
        self.processed_ops.contains(op_id)
    }

    /// Marca un id de transacción como procesado.
    pub fn mark_op(&mut self, op_id: Hash) -> Result<(), AmmError> {
        if !self.processed_ops.insert(op_id) {
            return Err(AmmError::DuplicateOperation);
        }
        Ok(())
    }

    /// Transfiere de usuario a custodia del pool.
    pub fn transfer_to_pool(
        &mut self,
        from: AccountId,
        pool_id: PoolId,
        asset: AssetId,
        amount: Amount,
    ) -> Result<(), AmmError> {
        if self.fail_next_pool_transfer {
            self.fail_next_pool_transfer = false;
            return Err(AmmError::TransferFailed);
        }
        self.debit_user(from, asset, amount)?;
        self.credit_pool(pool_id, asset, amount)?;
        Ok(())
    }

    /// Transfiere de custodia del pool a usuario.
    pub fn transfer_from_pool(
        &mut self,
        pool_id: PoolId,
        to: AccountId,
        asset: AssetId,
        amount: Amount,
    ) -> Result<(), AmmError> {
        self.debit_pool(pool_id, asset, amount)?;
        self.credit_user(to, asset, amount)?;
        Ok(())
    }

    /// Acuña LP a una cuenta (proveedor o sink).
    pub fn mint_lp(
        &mut self,
        pool_id: PoolId,
        to: AccountId,
        shares: LpShares,
    ) -> Result<(), AmmError> {
        if shares.0 == 0 {
            return Ok(());
        }
        let entry = self
            .lp_balances
            .entry((pool_id, to))
            .or_insert(LpShares::ZERO);
        entry.0 = entry.0.checked_add(shares.0).ok_or(AmmError::Overflow)?;
        Ok(())
    }

    /// Quema LP de un proveedor. No puede quemar el sink vía esta API.
    pub fn burn_lp(
        &mut self,
        pool_id: PoolId,
        from: AccountId,
        shares: LpShares,
    ) -> Result<(), AmmError> {
        if from == LP_SINK_ACCOUNT {
            return Err(AmmError::LockedLiquidity);
        }
        if shares.0 == 0 {
            return Err(AmmError::ZeroAmount);
        }
        let bal = self.lp_balance_of(&pool_id, &from);
        if bal.0 < shares.0 {
            return Err(AmmError::InsufficientLpBalance);
        }
        let new = bal.0 - shares.0;
        if new == 0 {
            self.lp_balances.remove(&(pool_id, from));
        } else {
            self.lp_balances.insert((pool_id, from), LpShares(new));
        }
        Ok(())
    }

    /// Suma de LP de un pool (todos los holders, incl. sink).
    pub fn total_lp_tracked(&self, pool_id: &PoolId) -> u128 {
        self.lp_balances
            .iter()
            .filter(|((p, _), _)| p == pool_id)
            .map(|(_, s)| s.0)
            .fold(0u128, |acc, x| acc.saturating_add(x))
    }

    /// Lee las reservas registradas en el registry (vista de protocolo).
    pub fn read_pool_reserves(
        pools: &PoolRegistry,
        pool_id: &PoolId,
    ) -> Result<Reserves, AmmError> {
        Ok(pools.get_pool(pool_id)?.reserves)
    }

    /// Verifica que el supply LP del registry coincida con el ledger.
    pub fn verify_lp_supply(
        &self,
        pools: &PoolRegistry,
        pool_id: &PoolId,
    ) -> Result<(), AmmError> {
        let pool = pools.get_pool(pool_id)?;
        if self.total_lp_tracked(pool_id) != pool.total_supply.0 {
            return Err(AmmError::AccountingInvariant);
        }
        if self.lp_balance_of(pool_id, &LP_SINK_ACCOUNT) != pool.locked_supply {
            return Err(AmmError::AccountingInvariant);
        }
        Ok(())
    }

    /// Reconcilia vault (custodia) vs reservas registradas de un pool.
    pub fn reconcile_vault(
        &self,
        pools: &PoolRegistry,
        pool_id: &PoolId,
    ) -> Result<(), AmmError> {
        let pool = pools.get_pool(pool_id)?;
        let a = pool.identity.asset_a;
        let b = pool.identity.asset_b;
        if self.pool_balance(pool_id, &a) != pool.reserves.reserve_a {
            return Err(AmmError::AccountingInvariant);
        }
        if self.pool_balance(pool_id, &b) != pool.reserves.reserve_b {
            return Err(AmmError::AccountingInvariant);
        }
        Ok(())
    }

    /// Verifica contabilidad completa de un pool (vault + LP).
    pub fn verify_pool_accounting(
        &self,
        pools: &PoolRegistry,
        pool_id: &PoolId,
    ) -> Result<(), AmmError> {
        self.reconcile_vault(pools, pool_id)?;
        self.verify_lp_supply(pools, pool_id)?;
        Ok(())
    }

    /// Verifica invariantes de todos los pools.
    pub fn assert_consistent(&self, pools: &PoolRegistry) -> Result<(), AmmError> {
        for pool_id in pools.list_pools() {
            self.verify_pool_accounting(pools, &pool_id)?;
        }
        Ok(())
    }

    /// Encoding canónico (LP + ops; saldos de activo viven en `rsc-assets`).
    pub fn to_canonical_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&1u32.to_be_bytes()); // schema
        out.extend_from_slice(&(self.lp_balances.len() as u32).to_be_bytes());
        for ((pool, account), shares) in &self.lp_balances {
            out.extend_from_slice(&pool.0 .0);
            out.extend_from_slice(&account.0);
            out.extend_from_slice(&shares.0.to_be_bytes());
        }
        out.extend_from_slice(&(self.processed_ops.len() as u32).to_be_bytes());
        for op in &self.processed_ops {
            out.extend_from_slice(&op.0);
        }
        out
    }

    /// Restaura LP + ops. Los saldos de activo parten vacíos (sync desde assets).
    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, AmmError> {
        let mut cur = bytes;
        let schema = read_u32(&mut cur)?;
        if schema != 1 {
            return Err(AmmError::InvalidEncoding);
        }
        let mut accounting = Self::new();
        let n_lp = read_u32(&mut cur)? as usize;
        for _ in 0..n_lp {
            let pool = PoolId(Hash(read_array::<32>(&mut cur)?));
            let account = AccountId(read_array::<32>(&mut cur)?);
            let shares = LpShares(read_u128(&mut cur)?);
            if shares.0 == 0 {
                return Err(AmmError::InvalidEncoding);
            }
            accounting.lp_balances.insert((pool, account), shares);
        }
        let n_ops = read_u32(&mut cur)? as usize;
        for _ in 0..n_ops {
            accounting
                .processed_ops
                .insert(Hash(read_array::<32>(&mut cur)?));
        }
        if !cur.is_empty() {
            return Err(AmmError::InvalidEncoding);
        }
        Ok(accounting)
    }

    /// Vista de saldos de usuario (sync hacia `rsc-assets`).
    pub fn user_balances_map(&self) -> &BTreeMap<(AccountId, AssetId), Amount> {
        &self.balances
    }

    /// Vista de custodia de pool (sync hacia `rsc-assets`).
    pub fn pool_custody_map(&self) -> &BTreeMap<(PoolId, AssetId), Amount> {
        &self.pool_custody
    }

    /// Sustituye saldos libres y custodia desde el ledger de activos.
    pub fn replace_asset_balances(
        &mut self,
        balances: BTreeMap<(AccountId, AssetId), Amount>,
        custody: BTreeMap<(PoolId, AssetId), Amount>,
    ) {
        self.balances = balances;
        self.pool_custody = custody;
    }

    /// Política Stage 7: transferencias no solicitadas al vault **no** se
    /// interpretan como reservas AMM. Solo `transfer_to_pool` / mint path
    /// actualizan custodia reconciliada con [`PoolRegistry::record_reserves`].
    ///
    /// Cualquier crédito externo debe quedar fuera de `pool_custody` hasta
    /// una operación de protocolo explícita (hoy: no hay “sweep” automático).
    pub fn policy_unsolicited_vault_credits() -> &'static str {
        "unsolicited vault credits are ignored by AMM reserves until an explicit protocol op"
    }
}

fn read_array<const N: usize>(cur: &mut &[u8]) -> Result<[u8; N], AmmError> {
    if cur.len() < N {
        return Err(AmmError::InvalidEncoding);
    }
    let (head, tail) = cur.split_at(N);
    *cur = tail;
    let mut out = [0u8; N];
    out.copy_from_slice(head);
    Ok(out)
}

fn read_u32(cur: &mut &[u8]) -> Result<u32, AmmError> {
    Ok(u32::from_be_bytes(read_array::<4>(cur)?))
}

fn read_u128(cur: &mut &[u8]) -> Result<u128, AmmError> {
    Ok(u128::from_be_bytes(read_array::<16>(cur)?))
}

/// Posición de liquidez de un usuario en un pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiquidityPosition {
    /// Pool.
    pub pool_id: PoolId,
    /// Proveedor.
    pub provider: AccountId,
    /// Shares LP del proveedor (sin sink).
    pub lp_shares: LpShares,
    /// Valor teórico en activo A (`floor`).
    pub value_a: Amount,
    /// Valor teórico en activo B (`floor`).
    pub value_b: Amount,
}

/// Vista agregada de liquidez del pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PoolLiquidity {
    /// Pool.
    pub pool_id: PoolId,
    /// Reserva A.
    pub reserve_a: Amount,
    /// Reserva B.
    pub reserve_b: Amount,
    /// Supply total de LP.
    pub total_supply: LpShares,
    /// LP bloqueado en sink.
    pub locked_supply: LpShares,
}
