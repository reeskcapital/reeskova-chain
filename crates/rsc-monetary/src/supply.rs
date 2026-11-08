//! Supply Tracker — contabilidad verificable del supply RSK.
//!
//! Distingue: máximo, emitido, circulante, bloqueado y quemado.
//! Regla: `emitted == circulating + locked + burned` (sobre unidades emitidas).

use crate::amount_ext::{add, sub};
use crate::error::MonetaryError;
use crate::policy::{EmissionCategory, MonetaryPolicy, MAX_SUPPLY};
use rsc_core::Amount;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Instantánea contable del supply.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupplySnapshot {
    /// Supply máximo del protocolo.
    pub max_supply: Amount,
    /// Total emitido desde génesis.
    pub emitted: Amount,
    /// En circulación disponible (no bloqueado, no quemado).
    pub circulating: Amount,
    /// Bloqueado (staking, vesting, reservas no liberadas, etc.).
    pub locked: Amount,
    /// Quemado de forma permanente.
    pub burned: Amount,
}

impl SupplySnapshot {
    /// Supply aún no emitido (presupuesto residual global).
    pub fn unissued(&self) -> Result<Amount, MonetaryError> {
        sub(self.max_supply, self.emitted)
    }

    /// Verifica la identidad contable.
    pub fn check_invariant(&self) -> Result<(), MonetaryError> {
        let sum = self
            .circulating
            .0
            .checked_add(self.locked.0)
            .and_then(|v| v.checked_add(self.burned.0))
            .ok_or(MonetaryError::Overflow)?;
        if sum != self.emitted.0 {
            return Err(MonetaryError::InvariantBroken);
        }
        if self.emitted.0 > self.max_supply.0 {
            return Err(MonetaryError::MaxSupplyExceeded);
        }
        Ok(())
    }
}

/// Rastreador autoritativo de supply y presupuestos por categoría.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupplyTracker {
    /// Instantánea agregada.
    pub snapshot: SupplySnapshot,
    /// Unidades emitidas por categoría presupuestaria.
    pub spent_by_category: BTreeMap<EmissionCategory, Amount>,
}

impl Default for SupplyTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl SupplyTracker {
    /// Tracker vacío bajo el supply máximo de política.
    pub fn new() -> Self {
        Self {
            snapshot: SupplySnapshot {
                max_supply: Amount(MAX_SUPPLY),
                emitted: Amount::ZERO,
                circulating: Amount::ZERO,
                locked: Amount::ZERO,
                burned: Amount::ZERO,
            },
            spent_by_category: BTreeMap::new(),
        }
    }

    /// Gasto acumulado de una categoría.
    pub fn spent(&self, category: EmissionCategory) -> Amount {
        self.spent_by_category
            .get(&category)
            .copied()
            .unwrap_or(Amount::ZERO)
    }

    /// Remanente presupuestario de la categoría.
    pub fn remaining_budget(&self, category: EmissionCategory) -> Result<Amount, MonetaryError> {
        sub(Amount(category.budget()), self.spent(category))
    }

    /// Emite `amount` desde `category` como **bloqueado**.
    pub fn emit_locked(
        &mut self,
        category: EmissionCategory,
        amount: Amount,
    ) -> Result<(), MonetaryError> {
        self.emit_internal(category, amount, true)
    }

    /// Emite `amount` directamente a circulación.
    pub fn emit_circulating(
        &mut self,
        category: EmissionCategory,
        amount: Amount,
    ) -> Result<(), MonetaryError> {
        self.emit_internal(category, amount, false)
    }

    fn emit_internal(
        &mut self,
        category: EmissionCategory,
        amount: Amount,
        locked: bool,
    ) -> Result<(), MonetaryError> {
        if amount.0 == 0 {
            return Err(MonetaryError::ZeroAmount);
        }
        MonetaryPolicy::check_max_supply(self.snapshot.emitted, amount)?;
        MonetaryPolicy::check_category_budget(category, self.spent(category), amount)?;
        if let Some(parent) = category.parent() {
            MonetaryPolicy::check_category_budget(parent, self.spent(parent), amount)?;
        }

        self.snapshot.emitted = add(self.snapshot.emitted, amount)?;
        if locked {
            self.snapshot.locked = add(self.snapshot.locked, amount)?;
        } else {
            self.snapshot.circulating = add(self.snapshot.circulating, amount)?;
        }

        let spent = add(self.spent(category), amount)?;
        self.spent_by_category.insert(category, spent);
        if let Some(parent) = category.parent() {
            let parent_spent = add(self.spent(parent), amount)?;
            self.spent_by_category.insert(parent, parent_spent);
        }

        self.snapshot.check_invariant()?;
        Ok(())
    }

    /// Mueve de bloqueado a circulante.
    pub fn unlock(&mut self, amount: Amount) -> Result<(), MonetaryError> {
        if amount.0 == 0 {
            return Err(MonetaryError::ZeroAmount);
        }
        self.snapshot.locked = sub(self.snapshot.locked, amount)?;
        self.snapshot.circulating = add(self.snapshot.circulating, amount)?;
        self.snapshot.check_invariant()?;
        Ok(())
    }

    /// Bloquea supply circulante (p. ej. al stakear).
    pub fn lock(&mut self, amount: Amount) -> Result<(), MonetaryError> {
        if amount.0 == 0 {
            return Err(MonetaryError::ZeroAmount);
        }
        self.snapshot.circulating = sub(self.snapshot.circulating, amount)?;
        self.snapshot.locked = add(self.snapshot.locked, amount)?;
        self.snapshot.check_invariant()?;
        Ok(())
    }

    /// Quema permanente desde circulante (comisiones de protocolo).
    pub fn burn_from_circulating(&mut self, amount: Amount) -> Result<(), MonetaryError> {
        if amount.0 == 0 {
            return Err(MonetaryError::ZeroAmount);
        }
        self.snapshot.circulating = sub(self.snapshot.circulating, amount)?;
        self.snapshot.burned = add(self.snapshot.burned, amount)?;
        self.snapshot.check_invariant()?;
        Ok(())
    }

    /// Quema permanente desde bloqueado (p. ej. slashing irreversible).
    pub fn burn_from_locked(&mut self, amount: Amount) -> Result<(), MonetaryError> {
        if amount.0 == 0 {
            return Err(MonetaryError::ZeroAmount);
        }
        self.snapshot.locked = sub(self.snapshot.locked, amount)?;
        self.snapshot.burned = add(self.snapshot.burned, amount)?;
        self.snapshot.check_invariant()?;
        Ok(())
    }

    /// Instantánea actual.
    pub fn snapshot(&self) -> &SupplySnapshot {
        &self.snapshot
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::AMOUNT_SCALE;

    #[test]
    fn emit_lock_unlock_burn() {
        let mut t = SupplyTracker::new();
        t.emit_locked(EmissionCategory::Community, Amount(1_000 * AMOUNT_SCALE))
            .unwrap();
        assert_eq!(t.snapshot.locked, Amount(1_000 * AMOUNT_SCALE));
        t.unlock(Amount(400 * AMOUNT_SCALE)).unwrap();
        assert_eq!(t.snapshot.circulating, Amount(400 * AMOUNT_SCALE));
        t.burn_from_circulating(Amount(100 * AMOUNT_SCALE)).unwrap();
        assert_eq!(t.snapshot.burned, Amount(100 * AMOUNT_SCALE));
        t.snapshot.check_invariant().unwrap();
    }

    #[test]
    fn cannot_exceed_max_supply() {
        let mut t = SupplyTracker::new();
        let err = t
            .emit_circulating(EmissionCategory::Mining, Amount(MAX_SUPPLY + 1))
            .unwrap_err();
        assert_eq!(err, MonetaryError::MaxSupplyExceeded);
    }

    #[test]
    fn category_budget_enforced() {
        let mut t = SupplyTracker::new();
        let over = Amount(crate::policy::TREASURY_BUDGET + 1);
        let err = t
            .emit_locked(EmissionCategory::Treasury, over)
            .unwrap_err();
        assert_eq!(err, MonetaryError::CategoryBudgetExhausted);
    }
}
