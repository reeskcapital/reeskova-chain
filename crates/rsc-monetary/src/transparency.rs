//! Dashboard de transparencia monetaria — vista verificable de la economía RSK.

use crate::burn::BurnEngine;
use crate::policy::{
    EmissionCategory, EmissionPhase, MonetaryPolicy, COMMUNITY_BUDGET, MAX_SUPPLY, MINING_BUDGET,
    TREASURY_BUDGET, VALIDATOR_BUDGET, CURRENCY_SYMBOL, MONETARY_POLICY_VERSION,
};
use crate::rewards::ValidatorRewards;
use crate::staking::StakingEngine;
use crate::supply::{SupplySnapshot, SupplyTracker};
use crate::treasury::TreasuryEngine;
use rsc_core::Amount;
use serde::{Deserialize, Serialize};

/// Informe público consolidado.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonetaryTransparencyReport {
    /// Versión de política.
    pub policy_version: u32,
    /// Moneda.
    pub currency: String,
    /// Fase de emisión.
    pub phase: EmissionPhase,
    /// Contabilidad de supply.
    pub supply: SupplySnapshot,
    /// Presupuestos: límite vs gastado.
    pub budgets: Vec<BudgetLine>,
    /// Totales de quema por fees.
    pub burned_via_protocol_fees: Amount,
    /// Fees asignados a validadores (histórico del burn engine).
    pub fees_to_validators: Amount,
    /// Stake activo total.
    pub staking_active: Amount,
    /// Stake en unbonding.
    pub staking_unbonding: Amount,
    /// Disponible liberado de tesorería (vested).
    pub treasury_available: Amount,
    /// Recompensas pendientes agregadas.
    pub pending_rewards: Amount,
}

/// Línea de presupuesto.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetLine {
    /// Categoría.
    pub category: EmissionCategory,
    /// Techo.
    pub budget: Amount,
    /// Emitido/gastado.
    pub spent: Amount,
    /// Remanente.
    pub remaining: Amount,
}

/// Estado monetario agregado para generar el informe.
pub struct MonetarySystem {
    /// Política activa.
    pub policy: MonetaryPolicy,
    /// Tracker de supply.
    pub supply: SupplyTracker,
    /// Quema.
    pub burn: BurnEngine,
    /// Staking.
    pub staking: StakingEngine,
    /// Recompensas.
    pub rewards: ValidatorRewards,
    /// Tesorería (opcional hasta seed).
    pub treasury: Option<TreasuryEngine>,
}

impl MonetarySystem {
    /// Sistema en política v1, fase Launch, contadores a cero.
    pub fn v1_genesis() -> Self {
        MonetaryPolicy::assert_distribution_invariants().expect("policy invariants");
        Self {
            policy: MonetaryPolicy::v1(),
            supply: SupplyTracker::new(),
            burn: BurnEngine::new(),
            staking: StakingEngine::default(),
            rewards: ValidatorRewards::new(),
            treasury: None,
        }
    }

    /// Genera el informe de transparencia.
    pub fn transparency_report(&self) -> MonetaryTransparencyReport {
        let categories = [
            EmissionCategory::Mining,
            EmissionCategory::Validators,
            EmissionCategory::HolderRewards,
            EmissionCategory::OperatorRewards,
            EmissionCategory::Community,
            EmissionCategory::Treasury,
        ];
        let budgets = categories
            .into_iter()
            .map(|category| {
                let budget = Amount(category.budget());
                let spent = self.supply.spent(category);
                let remaining = Amount(budget.0.saturating_sub(spent.0));
                BudgetLine {
                    category,
                    budget,
                    spent,
                    remaining,
                }
            })
            .collect();

        let pending_rewards = self
            .rewards
            .balances
            .pending
            .values()
            .fold(0u128, |acc, a| acc.saturating_add(a.0));

        let treasury_available = self
            .treasury
            .as_ref()
            .map(|t| t.vested_available)
            .unwrap_or(Amount::ZERO);

        MonetaryTransparencyReport {
            policy_version: MONETARY_POLICY_VERSION,
            currency: CURRENCY_SYMBOL.to_owned(),
            phase: self.policy.phase,
            supply: self.supply.snapshot.clone(),
            budgets,
            burned_via_protocol_fees: self.burn.total_burned,
            fees_to_validators: self.burn.total_to_validators,
            staking_active: self.staking.total_active,
            staking_unbonding: self.staking.total_unbonding,
            treasury_available,
            pending_rewards: Amount(pending_rewards),
        }
    }

    /// Resumen estático de la distribución oficial (sin estado runtime).
    pub fn official_allocation_table() -> Vec<(EmissionCategory, Amount)> {
        vec![
            (EmissionCategory::Mining, Amount(MINING_BUDGET)),
            (EmissionCategory::Validators, Amount(VALIDATOR_BUDGET)),
            (EmissionCategory::Community, Amount(COMMUNITY_BUDGET)),
            (EmissionCategory::Treasury, Amount(TREASURY_BUDGET)),
        ]
    }

    /// Supply máximo declarado.
    pub fn max_supply() -> Amount {
        Amount(MAX_SUPPLY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_at_genesis_is_zeroed() {
        let sys = MonetarySystem::v1_genesis();
        let report = sys.transparency_report();
        assert_eq!(report.currency, "RSK");
        assert_eq!(report.supply.max_supply, Amount(MAX_SUPPLY));
        assert_eq!(report.supply.emitted, Amount::ZERO);
        assert_eq!(report.supply.burned, Amount::ZERO);
        assert_eq!(report.budgets.len(), 6);
        assert_eq!(
            MonetarySystem::official_allocation_table()
                .iter()
                .map(|(_, a)| a.0)
                .sum::<u128>(),
            MAX_SUPPLY
        );
    }
}
