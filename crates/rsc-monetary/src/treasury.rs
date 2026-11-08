//! Treasury Engine — reserva comunitaria con vesting y autorización multisig.
//!
//! La tesorería no puede modificar saldos arbitrarios de usuarios. Solo libera
//! su propio presupuesto bajo reglas verificables.

use crate::amount_ext::add;
use crate::error::MonetaryError;
use crate::policy::EmissionCategory;
use crate::supply::SupplyTracker;
use rsc_core::{AccountId, Amount, Hash};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Destino presupuestario interno de tesorería.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum TreasuryPurpose {
    /// Desarrollo del protocolo.
    Development,
    /// Infraestructura de red.
    Infrastructure,
    /// Expansión / ecosistema.
    Expansion,
}

/// Tramo de vesting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VestingCliff {
    /// Epoch/altura desde la cual este tramo es liberable.
    pub unlock_at: u64,
    /// Monto del tramo.
    pub amount: Amount,
}

/// Propuesta de gasto autorizable por multisig.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreasuryProposal {
    /// Identificador de la propuesta.
    pub id: Hash,
    /// Destino.
    pub purpose: TreasuryPurpose,
    /// Beneficiario.
    pub beneficiary: AccountId,
    /// Monto.
    pub amount: Amount,
    /// Epoch mínima (respetar vesting agregado).
    pub executable_at: u64,
    /// Firmantes que aprobaron.
    pub approvals: BTreeSet<AccountId>,
    /// Ejecutada.
    pub executed: bool,
}

/// Motor de tesorería.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreasuryEngine {
    /// Dirección pública canónica de la tesorería (cuenta destino de reservas).
    pub treasury_account: AccountId,
    /// Firmantes autorizados.
    pub signers: BTreeSet<AccountId>,
    /// Umbral multisig (M de N).
    pub threshold: usize,
    /// Calendario de vesting (montos aún en presupuesto bloqueado).
    pub vesting: Vec<VestingCliff>,
    /// Ya liberado del vesting a disponible de tesorería.
    pub vested_available: Amount,
    /// Gastado por propósito.
    pub spent_by_purpose: BTreeMap<TreasuryPurpose, Amount>,
    /// Propuestas.
    pub proposals: BTreeMap<Hash, TreasuryProposal>,
    /// Total emitido desde categoría Treasury.
    pub total_emitted: Amount,
}

impl TreasuryEngine {
    /// Crea la tesorería con signers y umbral.
    pub fn new(
        treasury_account: AccountId,
        signers: BTreeSet<AccountId>,
        threshold: usize,
        vesting: Vec<VestingCliff>,
    ) -> Result<Self, MonetaryError> {
        if threshold == 0 || threshold > signers.len() {
            return Err(MonetaryError::TreasuryUnauthorized);
        }
        let schedule_total = vesting.iter().try_fold(0u128, |acc, c| {
            if c.amount.0 == 0 {
                return Err(MonetaryError::ZeroAmount);
            }
            acc.checked_add(c.amount.0).ok_or(MonetaryError::Overflow)
        })?;
        if schedule_total > EmissionCategory::Treasury.budget() {
            return Err(MonetaryError::CategoryBudgetExhausted);
        }
        Ok(Self {
            treasury_account,
            signers,
            threshold,
            vesting,
            vested_available: Amount::ZERO,
            spent_by_purpose: BTreeMap::new(),
            proposals: BTreeMap::new(),
            total_emitted: Amount::ZERO,
        })
    }

    /// Emite el calendario completo como supply bloqueado (reserva no circulante).
    pub fn seed_locked(&mut self, tracker: &mut SupplyTracker) -> Result<(), MonetaryError> {
        let total = self.vesting.iter().try_fold(Amount::ZERO, |acc, c| {
            add(acc, c.amount)
        })?;
        if total.0 == 0 {
            return Ok(());
        }
        tracker.emit_locked(EmissionCategory::Treasury, total)?;
        self.total_emitted = add(self.total_emitted, total)?;
        Ok(())
    }

    /// Libera tramos de vesting cuyo `unlock_at <= now` hacia disponible.
    pub fn unlock_vested(
        &mut self,
        tracker: &mut SupplyTracker,
        now: u64,
    ) -> Result<Amount, MonetaryError> {
        let mut released = Amount::ZERO;
        let mut remaining = Vec::new();
        for cliff in self.vesting.drain(..) {
            if cliff.unlock_at <= now {
                tracker.unlock(cliff.amount)?;
                released = add(released, cliff.amount)?;
            } else {
                remaining.push(cliff);
            }
        }
        self.vesting = remaining;
        self.vested_available = add(self.vested_available, released)?;
        Ok(released)
    }

    /// Propone un gasto.
    pub fn propose(&mut self, proposal: TreasuryProposal) -> Result<(), MonetaryError> {
        if proposal.amount.0 == 0 {
            return Err(MonetaryError::ZeroAmount);
        }
        if self.proposals.contains_key(&proposal.id) {
            return Err(MonetaryError::TreasuryUnauthorized);
        }
        self.proposals.insert(proposal.id, proposal);
        Ok(())
    }

    /// Aprueba una propuesta (firmante debe estar en el set).
    pub fn approve(
        &mut self,
        proposal_id: Hash,
        signer: AccountId,
    ) -> Result<(), MonetaryError> {
        if !self.signers.contains(&signer) {
            return Err(MonetaryError::TreasuryUnauthorized);
        }
        let p = self
            .proposals
            .get_mut(&proposal_id)
            .ok_or(MonetaryError::TreasuryUnauthorized)?;
        if p.executed {
            return Err(MonetaryError::TreasuryUnauthorized);
        }
        p.approvals.insert(signer);
        Ok(())
    }

    /// Ejecuta si hay umbral y vesting cumplido. Descuenta de `vested_available`.
    ///
    /// El monto sigue en circulación (ya fue unlocked); se registra el gasto.
    pub fn execute(
        &mut self,
        proposal_id: Hash,
        now: u64,
    ) -> Result<TreasuryProposal, MonetaryError> {
        let p = self
            .proposals
            .get_mut(&proposal_id)
            .ok_or(MonetaryError::TreasuryUnauthorized)?;
        if p.executed {
            return Err(MonetaryError::TreasuryUnauthorized);
        }
        if p.approvals.len() < self.threshold {
            return Err(MonetaryError::TreasuryUnauthorized);
        }
        if now < p.executable_at {
            return Err(MonetaryError::TreasuryVestingLocked);
        }
        if p.amount.0 > self.vested_available.0 {
            return Err(MonetaryError::TreasuryBudgetExhausted);
        }
        self.vested_available = Amount(self.vested_available.0 - p.amount.0);
        let spent = self
            .spent_by_purpose
            .get(&p.purpose)
            .copied()
            .unwrap_or(Amount::ZERO);
        self.spent_by_purpose
            .insert(p.purpose, add(spent, p.amount)?);
        p.executed = true;
        Ok(p.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::AMOUNT_SCALE;

    fn id(byte: u8) -> AccountId {
        AccountId([byte; 32])
    }

    #[test]
    fn vesting_and_multisig() {
        let mut tracker = SupplyTracker::new();
        let signers = [id(1), id(2), id(3)].into_iter().collect();
        let vesting = vec![
            VestingCliff {
                unlock_at: 10,
                amount: Amount(100 * AMOUNT_SCALE),
            },
            VestingCliff {
                unlock_at: 20,
                amount: Amount(200 * AMOUNT_SCALE),
            },
        ];
        let mut treasury = TreasuryEngine::new(id(9), signers, 2, vesting).unwrap();
        treasury.seed_locked(&mut tracker).unwrap();
        assert_eq!(tracker.snapshot.locked, Amount(300 * AMOUNT_SCALE));

        assert_eq!(
            treasury.unlock_vested(&mut tracker, 5).unwrap(),
            Amount::ZERO
        );
        assert_eq!(
            treasury.unlock_vested(&mut tracker, 10).unwrap(),
            Amount(100 * AMOUNT_SCALE)
        );

        let pid = Hash([0xab; 32]);
        treasury
            .propose(TreasuryProposal {
                id: pid,
                purpose: TreasuryPurpose::Development,
                beneficiary: id(8),
                amount: Amount(50 * AMOUNT_SCALE),
                executable_at: 10,
                approvals: BTreeSet::new(),
                executed: false,
            })
            .unwrap();
        treasury.approve(pid, id(1)).unwrap();
        assert_eq!(
            treasury.execute(pid, 10).unwrap_err(),
            MonetaryError::TreasuryUnauthorized
        );
        treasury.approve(pid, id(2)).unwrap();
        let done = treasury.execute(pid, 10).unwrap();
        assert!(done.executed);
        assert_eq!(treasury.vested_available, Amount(50 * AMOUNT_SCALE));
    }
}
