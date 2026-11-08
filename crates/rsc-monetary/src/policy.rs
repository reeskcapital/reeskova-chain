//! Política monetaria fundamental RSK v1.0.
//!
//! Constantes verificables. No emiten ni mueven fondos; solo definen los
//! límites que el resto de motores deben respetar.

use crate::error::MonetaryError;
use rsc_core::Amount;
use serde::{Deserialize, Serialize};

/// Versión de la política monetaria.
pub const MONETARY_POLICY_VERSION: u32 = 1;

/// Símbolo canónico de la moneda nativa.
pub const CURRENCY_SYMBOL: &str = "RSK";

/// Decimales de presentación (alineados con el activo nativo tipado).
pub const AMOUNT_DECIMALS: u8 = 18;

/// Unidades mínimas por 1 RSK (`10^18`).
pub const AMOUNT_SCALE: u128 = 1_000_000_000_000_000_000;

/// Supply máximo en RSK enteros (antes de escala).
pub const MAX_SUPPLY_WHOLE: u128 = 35_999_999;

/// Supply máximo en unidades mínimas.
pub const MAX_SUPPLY: u128 = MAX_SUPPLY_WHOLE * AMOUNT_SCALE;

/// Presupuesto minería: 21_599_999 RSK.
pub const MINING_BUDGET: u128 = 21_599_999 * AMOUNT_SCALE;

/// Presupuesto validadores / staking: 7_200_000 RSK.
pub const VALIDATOR_BUDGET: u128 = 7_200_000 * AMOUNT_SCALE;

/// Presupuesto comunidad: 5_400_000 RSK.
pub const COMMUNITY_BUDGET: u128 = 5_400_000 * AMOUNT_SCALE;

/// Presupuesto tesorería: 1_800_000 RSK.
pub const TREASURY_BUDGET: u128 = 1_800_000 * AMOUNT_SCALE;

/// Dentro de validadores: recompensas de holders (5_400_000 RSK).
pub const HOLDER_REWARDS_BUDGET: u128 = 5_400_000 * AMOUNT_SCALE;

/// Dentro de validadores: operadores (1_800_000 RSK).
pub const OPERATOR_REWARDS_BUDGET: u128 = 1_800_000 * AMOUNT_SCALE;

/// Porción de comisión de protocolo quemada (basis points). 30%.
pub const PROTOCOL_FEE_BURN_BPS: u16 = 3_000;

/// Porción de comisión de protocolo a validadores (basis points). 70%.
pub const PROTOCOL_FEE_VALIDATOR_BPS: u16 = 7_000;

/// Denominador de basis points.
pub const BPS_DENOMINATOR: u16 = 10_000;

/// Categoría presupuestaria de emisión.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum EmissionCategory {
    /// Reserva de minería.
    Mining,
    /// Reserva de validadores / staking (presupuesto agregado).
    Validators,
    /// Sub-presupuesto: recompensas a holders que stakean.
    HolderRewards,
    /// Sub-presupuesto: operadores de validadores.
    OperatorRewards,
    /// Reserva comunitaria.
    Community,
    /// Tesorería (desarrollo / infraestructura).
    Treasury,
}

impl EmissionCategory {
    /// Techo presupuestario de la categoría en unidades mínimas.
    pub const fn budget(self) -> u128 {
        match self {
            Self::Mining => MINING_BUDGET,
            Self::Validators => VALIDATOR_BUDGET,
            Self::HolderRewards => HOLDER_REWARDS_BUDGET,
            Self::OperatorRewards => OPERATOR_REWARDS_BUDGET,
            Self::Community => COMMUNITY_BUDGET,
            Self::Treasury => TREASURY_BUDGET,
        }
    }

    /// Categoría padre de presupuesto (si aplica).
    pub const fn parent(self) -> Option<Self> {
        match self {
            Self::HolderRewards | Self::OperatorRewards => Some(Self::Validators),
            _ => None,
        }
    }
}


/// Fase de emisión decreciente.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EmissionPhase {
    /// Lanzamiento: distribución inicial, AMM y recompensas comunidad.
    Launch,
    /// Expansión: liberación progresiva de minería y staking.
    Expansion,
    /// Madurez: menos emisión nueva; más peso de comisiones.
    Maturity,
    /// Sostenible: recompensas mayormente por comisiones + reservas restantes.
    Sustainable,
}

/// Documento inmutable de política monetaria v1.0.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonetaryPolicy {
    /// Versión de la política.
    pub version: u32,
    /// Símbolo de la moneda.
    pub currency: String,
    /// Decimales.
    pub decimals: u8,
    /// Supply máximo en unidades mínimas.
    pub max_supply: Amount,
    /// Fase actual de emisión.
    pub phase: EmissionPhase,
}

impl MonetaryPolicy {
    /// Política canónica v1.0 en fase de lanzamiento.
    pub fn v1() -> Self {
        Self {
            version: MONETARY_POLICY_VERSION,
            currency: CURRENCY_SYMBOL.to_owned(),
            decimals: AMOUNT_DECIMALS,
            max_supply: Amount(MAX_SUPPLY),
            phase: EmissionPhase::Launch,
        }
    }

    /// Convierte RSK enteros a unidades mínimas.
    pub fn whole_to_amount(whole: u128) -> Result<Amount, MonetaryError> {
        whole
            .checked_mul(AMOUNT_SCALE)
            .map(Amount)
            .ok_or(MonetaryError::Overflow)
    }

    /// Verifica que `emitted + amount` no supera el supply máximo.
    pub fn check_max_supply(emitted: Amount, amount: Amount) -> Result<(), MonetaryError> {
        let next = emitted
            .0
            .checked_add(amount.0)
            .ok_or(MonetaryError::Overflow)?;
        if next > MAX_SUPPLY {
            return Err(MonetaryError::MaxSupplyExceeded);
        }
        Ok(())
    }

    /// Verifica presupuesto de categoría: `spent + amount <= budget`.
    pub fn check_category_budget(
        category: EmissionCategory,
        spent: Amount,
        amount: Amount,
    ) -> Result<(), MonetaryError> {
        if amount.0 == 0 {
            return Err(MonetaryError::ZeroAmount);
        }
        let next = spent
            .0
            .checked_add(amount.0)
            .ok_or(MonetaryError::Overflow)?;
        if next > category.budget() {
            return Err(MonetaryError::CategoryBudgetExhausted);
        }
        Ok(())
    }

    /// Avanza de fase (solo hacia adelante).
    pub fn advance_phase(&mut self) -> Result<(), MonetaryError> {
        self.phase = match self.phase {
            EmissionPhase::Launch => EmissionPhase::Expansion,
            EmissionPhase::Expansion => EmissionPhase::Maturity,
            EmissionPhase::Maturity => EmissionPhase::Sustainable,
            EmissionPhase::Sustainable => return Err(MonetaryError::EmissionPhaseDenied),
        };
        Ok(())
    }

    /// Invariante de distribución: las cuatro categorías suman el máximo;
    /// holders + operadores = validadores.
    pub fn assert_distribution_invariants() -> Result<(), MonetaryError> {
        let top = MINING_BUDGET
            .checked_add(VALIDATOR_BUDGET)
            .and_then(|v| v.checked_add(COMMUNITY_BUDGET))
            .and_then(|v| v.checked_add(TREASURY_BUDGET))
            .ok_or(MonetaryError::Overflow)?;
        if top != MAX_SUPPLY {
            return Err(MonetaryError::InvariantBroken);
        }
        let nested = HOLDER_REWARDS_BUDGET
            .checked_add(OPERATOR_REWARDS_BUDGET)
            .ok_or(MonetaryError::Overflow)?;
        if nested != VALIDATOR_BUDGET {
            return Err(MonetaryError::InvariantBroken);
        }
        if PROTOCOL_FEE_BURN_BPS + PROTOCOL_FEE_VALIDATOR_BPS != BPS_DENOMINATOR {
            return Err(MonetaryError::InvariantBroken);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distribution_sums_to_max_supply() {
        MonetaryPolicy::assert_distribution_invariants().unwrap();
        assert_eq!(
            MINING_BUDGET + VALIDATOR_BUDGET + COMMUNITY_BUDGET + TREASURY_BUDGET,
            MAX_SUPPLY
        );
    }

    #[test]
    fn whole_conversion() {
        assert_eq!(
            MonetaryPolicy::whole_to_amount(1).unwrap(),
            Amount(AMOUNT_SCALE)
        );
        assert_eq!(
            MonetaryPolicy::whole_to_amount(MAX_SUPPLY_WHOLE).unwrap(),
            Amount(MAX_SUPPLY)
        );
    }

    #[test]
    fn amount_scale_is_1e18() {
        // Debe coincidir con `rsc_genesis::RSK_AMOUNT_SCALE`.
        assert_eq!(AMOUNT_SCALE, 1_000_000_000_000_000_000);
        assert_eq!(AMOUNT_DECIMALS, 18);
    }

    #[test]
    fn budgets_match_published_policy_numbers() {
        // Mantener alineado con `rsc_genesis::RSK_*_BUDGET`.
        assert_eq!(AMOUNT_SCALE, 1_000_000_000_000_000_000);
        assert_eq!(MAX_SUPPLY_WHOLE, 35_999_999);
        assert_eq!(MINING_BUDGET / AMOUNT_SCALE, 21_599_999);
        assert_eq!(VALIDATOR_BUDGET / AMOUNT_SCALE, 7_200_000);
        assert_eq!(COMMUNITY_BUDGET / AMOUNT_SCALE, 5_400_000);
        assert_eq!(TREASURY_BUDGET / AMOUNT_SCALE, 1_800_000);
        assert_eq!(HOLDER_REWARDS_BUDGET / AMOUNT_SCALE, 5_400_000);
        assert_eq!(OPERATOR_REWARDS_BUDGET / AMOUNT_SCALE, 1_800_000);
    }

    #[test]
    fn max_supply_guard() {
        MonetaryPolicy::check_max_supply(Amount(MAX_SUPPLY), Amount(1))
            .unwrap_err();
        MonetaryPolicy::check_max_supply(Amount(MAX_SUPPLY - 1), Amount(1)).unwrap();
    }
}
