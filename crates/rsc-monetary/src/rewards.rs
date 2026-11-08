//! Validator Rewards — liberación de recompensas desde presupuestos existentes.
//!
//! No crea supply ilimitado. Emite solo desde `HolderRewards` / `OperatorRewards`
//! (sub-presupuestos de Validadores) o reparte fees ya circulantes.

use crate::amount_ext::add;
use crate::error::MonetaryError;
use crate::policy::EmissionCategory;
use crate::staking::{StakingEngine, ValidatorId};
use crate::supply::SupplyTracker;
use rsc_core::{AccountId, Amount};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Crédito de recompensa pendiente de cobro off-chain / ledger.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RewardBalances {
    /// Recompensas acumuladas por cuenta.
    pub pending: BTreeMap<AccountId, Amount>,
}

impl RewardBalances {
    /// Abona a una cuenta.
    pub fn credit(&mut self, account: AccountId, amount: Amount) -> Result<(), MonetaryError> {
        if amount.0 == 0 {
            return Err(MonetaryError::ZeroAmount);
        }
        let cur = self.pending.get(&account).copied().unwrap_or(Amount::ZERO);
        self.pending.insert(account, add(cur, amount)?);
        Ok(())
    }

    /// Consume el pendiente (para transferirlo al ledger de saldos).
    pub fn take(&mut self, account: &AccountId) -> Amount {
        self.pending.remove(account).unwrap_or(Amount::ZERO)
    }
}

/// Motor de recompensas de validadores y holders.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidatorRewards {
    /// Saldos pendientes de recompensa.
    pub balances: RewardBalances,
    /// Total liberado desde presupuesto de holders.
    pub issued_holder_budget: Amount,
    /// Total liberado desde presupuesto de operadores.
    pub issued_operator_budget: Amount,
    /// Total repartido desde fees de protocolo (ya circulantes).
    pub distributed_from_fees: Amount,
}

impl ValidatorRewards {
    /// Motor vacío.
    pub fn new() -> Self {
        Self::default()
    }

    /// Libera recompensas de holders desde el presupuesto (entra bloqueado→luego
    /// se acredita como pendiente; el supply se emite a circulación para el
    /// cobro). Proporcional al stake activo por validador/delegador.
    pub fn release_holder_rewards(
        &mut self,
        tracker: &mut SupplyTracker,
        staking: &StakingEngine,
        total: Amount,
    ) -> Result<(), MonetaryError> {
        if total.0 == 0 {
            return Err(MonetaryError::ZeroAmount);
        }
        if staking.total_active.0 == 0 {
            return Err(MonetaryError::InsufficientStake);
        }
        tracker.emit_circulating(EmissionCategory::HolderRewards, total)?;
        self.issued_holder_budget = add(self.issued_holder_budget, total)?;

        for pos in staking.positions.values() {
            if pos.active.0 == 0 {
                continue;
            }
            let share = mul_div(pos.active.0, total.0, staking.total_active.0)?;
            if share > 0 {
                self.balances
                    .credit(pos.delegator, Amount(share))?;
            }
        }
        Ok(())
    }

    /// Libera recompensa de operador desde presupuesto.
    pub fn release_operator_reward(
        &mut self,
        tracker: &mut SupplyTracker,
        operator: ValidatorId,
        amount: Amount,
    ) -> Result<(), MonetaryError> {
        tracker.emit_circulating(EmissionCategory::OperatorRewards, amount)?;
        self.issued_operator_budget = add(self.issued_operator_budget, amount)?;
        self.balances.credit(operator, amount)?;
        Ok(())
    }

    /// Reparte la parte de fees de protocolo (70%) entre operadores
    /// proporcional al stake activo. No emite supply nuevo.
    pub fn distribute_protocol_fees(
        &mut self,
        staking: &StakingEngine,
        amount: Amount,
    ) -> Result<(), MonetaryError> {
        if amount.0 == 0 {
            return Err(MonetaryError::ZeroAmount);
        }
        if staking.total_active.0 == 0 {
            return Err(MonetaryError::InsufficientStake);
        }
        // Agrupa stake por validador.
        let mut by_validator: BTreeMap<ValidatorId, Amount> = BTreeMap::new();
        for pos in staking.positions.values() {
            let cur = by_validator
                .get(&pos.validator)
                .copied()
                .unwrap_or(Amount::ZERO);
            by_validator.insert(pos.validator, add(cur, pos.active)?);
        }
        for (validator, stake) in by_validator {
            let share = mul_div(stake.0, amount.0, staking.total_active.0)?;
            if share > 0 {
                self.balances.credit(validator, Amount(share))?;
            }
        }
        self.distributed_from_fees = add(self.distributed_from_fees, amount)?;
        Ok(())
    }
}

/// `floor(a * b / d)` evitando overflow intermedio cuando es posible.
fn mul_div(a: u128, b: u128, d: u128) -> Result<u128, MonetaryError> {
    if d == 0 {
        return Err(MonetaryError::InvariantBroken);
    }
    if let Some(product) = a.checked_mul(b) {
        return Ok(product / d);
    }
    let g = gcd(a, d);
    let a = a / g;
    let d = d / g;
    let g = gcd(b, d);
    let b = b / g;
    let d = d / g;
    a.checked_mul(b)
        .ok_or(MonetaryError::Overflow)
        .map(|v| v / d)
}

fn gcd(mut x: u128, mut y: u128) -> u128 {
    while y != 0 {
        let t = y;
        y = x % y;
        x = t;
    }
    x
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::AMOUNT_SCALE;
    use crate::staking::StakingParams;

    fn id(byte: u8) -> AccountId {
        AccountId([byte; 32])
    }

    #[test]
    fn holder_rewards_proportional() {
        let mut tracker = SupplyTracker::new();
        tracker
            .emit_circulating(EmissionCategory::Community, Amount(1_000 * AMOUNT_SCALE))
            .unwrap();
        let mut staking = StakingEngine::new(StakingParams::default());
        let v = id(1);
        staking.set_validator(v, true);
        staking
            .stake(&mut tracker, id(2), v, Amount(300 * AMOUNT_SCALE))
            .unwrap();
        staking
            .stake(&mut tracker, id(3), v, Amount(100 * AMOUNT_SCALE))
            .unwrap();

        let mut rewards = ValidatorRewards::new();
        rewards
            .release_holder_rewards(&mut tracker, &staking, Amount(400 * AMOUNT_SCALE))
            .unwrap();
        assert_eq!(
            rewards.balances.pending.get(&id(2)).copied().unwrap(),
            Amount(300 * AMOUNT_SCALE)
        );
        assert_eq!(
            rewards.balances.pending.get(&id(3)).copied().unwrap(),
            Amount(100 * AMOUNT_SCALE)
        );
    }
}
