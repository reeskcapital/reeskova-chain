//! Staking Engine — staking delegado abierto a holders.
//!
//! No crea RSK. Solo bloquea/desbloquea supply ya emitido y registra
//! delegaciones. Las recompensas las libera `ValidatorRewards` desde
//! presupuestos existentes.

use crate::amount_ext::{add, sub};
use crate::error::MonetaryError;
use crate::supply::SupplyTracker;
use rsc_core::{AccountId, Amount};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Identidad de validador a efectos de staking (AccountId del operador).
pub type ValidatorId = AccountId;

/// Posición de stake de un delegador hacia un validador.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StakePosition {
    /// Delegador.
    pub delegator: AccountId,
    /// Validador elegido.
    pub validator: ValidatorId,
    /// Monto actualmente bloqueado en stake activo.
    pub active: Amount,
    /// Monto en cola de salida.
    pub unbonding: Amount,
    /// Altura/epoch en la que el unbonding queda liberable.
    pub unbond_release_at: u64,
}

/// Parámetros de staking.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StakingParams {
    /// Epochs/bloques de desbloqueo tras solicitar retiro.
    pub unbonding_period: u64,
}

impl Default for StakingParams {
    fn default() -> Self {
        Self {
            unbonding_period: 28,
        }
    }
}

/// Motor de staking delegado.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StakingEngine {
    /// Parámetros.
    pub params: StakingParams,
    /// Validadores admitidos (operadores registrados).
    pub validators: BTreeMap<ValidatorId, bool>,
    /// Posiciones indexadas por (delegator, validator).
    pub positions: BTreeMap<(AccountId, ValidatorId), StakePosition>,
    /// Total activo en staking.
    pub total_active: Amount,
    /// Total en unbonding.
    pub total_unbonding: Amount,
}

impl Default for StakingEngine {
    fn default() -> Self {
        Self::new(StakingParams::default())
    }
}

impl StakingEngine {
    /// Motor nuevo.
    pub fn new(params: StakingParams) -> Self {
        Self {
            params,
            validators: BTreeMap::new(),
            positions: BTreeMap::new(),
            total_active: Amount::ZERO,
            total_unbonding: Amount::ZERO,
        }
    }

    /// Registra o actualiza elegibilidad de un validador.
    pub fn set_validator(&mut self, id: ValidatorId, eligible: bool) {
        self.validators.insert(id, eligible);
    }

    fn is_eligible(&self, id: &ValidatorId) -> bool {
        self.validators.get(id).copied().unwrap_or(false)
    }

    /// Deposita y delega RSK circulante hacia un validador.
    pub fn stake(
        &mut self,
        tracker: &mut SupplyTracker,
        delegator: AccountId,
        validator: ValidatorId,
        amount: Amount,
    ) -> Result<(), MonetaryError> {
        if amount.0 == 0 {
            return Err(MonetaryError::ZeroAmount);
        }
        if !self.is_eligible(&validator) {
            return Err(MonetaryError::InvalidValidator);
        }
        tracker.lock(amount)?;
        let key = (delegator, validator);
        let entry = self.positions.entry(key).or_insert(StakePosition {
            delegator,
            validator,
            active: Amount::ZERO,
            unbonding: Amount::ZERO,
            unbond_release_at: 0,
        });
        entry.active = add(entry.active, amount)?;
        self.total_active = add(self.total_active, amount)?;
        Ok(())
    }

    /// Solicita retiro: pasa de activo a unbonding.
    pub fn request_unstake(
        &mut self,
        delegator: AccountId,
        validator: ValidatorId,
        amount: Amount,
        now: u64,
    ) -> Result<(), MonetaryError> {
        if amount.0 == 0 {
            return Err(MonetaryError::ZeroAmount);
        }
        let pos = self
            .positions
            .get_mut(&(delegator, validator))
            .ok_or(MonetaryError::InsufficientStake)?;
        pos.active = sub(pos.active, amount).map_err(|_| MonetaryError::InsufficientStake)?;
        pos.unbonding = add(pos.unbonding, amount)?;
        pos.unbond_release_at = now.saturating_add(self.params.unbonding_period);
        self.total_active =
            sub(self.total_active, amount).map_err(|_| MonetaryError::InsufficientStake)?;
        self.total_unbonding = add(self.total_unbonding, amount)?;
        Ok(())
    }

    /// Completa el retiro tras el periodo de unbonding → circulante.
    pub fn complete_unstake(
        &mut self,
        tracker: &mut SupplyTracker,
        delegator: AccountId,
        validator: ValidatorId,
        now: u64,
    ) -> Result<Amount, MonetaryError> {
        let pos = self
            .positions
            .get_mut(&(delegator, validator))
            .ok_or(MonetaryError::InsufficientStake)?;
        if pos.unbonding.0 == 0 {
            return Err(MonetaryError::InsufficientStake);
        }
        if now < pos.unbond_release_at {
            return Err(MonetaryError::UnstakeLockActive);
        }
        let amount = pos.unbonding;
        pos.unbonding = Amount::ZERO;
        pos.unbond_release_at = 0;
        self.total_unbonding = sub(self.total_unbonding, amount)?;
        tracker.unlock(amount)?;
        if pos.active.0 == 0 && pos.unbonding.0 == 0 {
            self.positions.remove(&(delegator, validator));
        }
        Ok(amount)
    }

    /// Slash solo del stake del operador (protege delegadores ajenos).
    pub fn slash_operator(
        &mut self,
        tracker: &mut SupplyTracker,
        operator: ValidatorId,
        amount: Amount,
    ) -> Result<(), MonetaryError> {
        if amount.0 == 0 {
            return Err(MonetaryError::ZeroAmount);
        }
        let pos = self
            .positions
            .get_mut(&(operator, operator))
            .ok_or(MonetaryError::InsufficientStake)?;
        pos.active = sub(pos.active, amount).map_err(|_| MonetaryError::InsufficientStake)?;
        self.total_active =
            sub(self.total_active, amount).map_err(|_| MonetaryError::InsufficientStake)?;
        tracker.burn_from_locked(amount)?;
        Ok(())
    }

    /// Stake activo total hacia un validador.
    pub fn validator_active_stake(&self, validator: &ValidatorId) -> Amount {
        self.positions
            .values()
            .filter(|p| p.validator == *validator)
            .fold(Amount::ZERO, |acc, p| add(acc, p.active).unwrap_or(acc))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{EmissionCategory, AMOUNT_SCALE};

    fn id(byte: u8) -> AccountId {
        AccountId([byte; 32])
    }

    #[test]
    fn stake_unstake_flow() {
        let mut tracker = SupplyTracker::new();
        tracker
            .emit_circulating(EmissionCategory::Community, Amount(1_000 * AMOUNT_SCALE))
            .unwrap();
        let mut staking = StakingEngine::default();
        let v = id(1);
        let u = id(2);
        staking.set_validator(v, true);
        staking
            .stake(&mut tracker, u, v, Amount(100 * AMOUNT_SCALE))
            .unwrap();
        assert_eq!(tracker.snapshot.locked, Amount(100 * AMOUNT_SCALE));
        staking
            .request_unstake(u, v, Amount(100 * AMOUNT_SCALE), 0)
            .unwrap();
        assert_eq!(
            staking.complete_unstake(&mut tracker, u, v, 10).unwrap_err(),
            MonetaryError::UnstakeLockActive
        );
        let released = staking.complete_unstake(&mut tracker, u, v, 28).unwrap();
        assert_eq!(released, Amount(100 * AMOUNT_SCALE));
        assert_eq!(tracker.snapshot.circulating, Amount(1_000 * AMOUNT_SCALE));
    }
}
