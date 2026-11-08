//! Burn Engine — quema parcial de comisiones de protocolo.
//!
//! Solo aplica a comisiones del protocolo de red. Las fees del AMM para LPs
//! son un sistema distinto y no pasan por este motor.

use crate::amount_ext::add;
use crate::error::MonetaryError;
use crate::policy::{BPS_DENOMINATOR, PROTOCOL_FEE_BURN_BPS, PROTOCOL_FEE_VALIDATOR_BPS};
use crate::supply::SupplyTracker;
use rsc_core::Amount;
use serde::{Deserialize, Serialize};

/// Resultado de repartir una comisión de protocolo.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolFeeSplit {
    /// Comisión bruta recibida.
    pub gross: Amount,
    /// Parte destruida permanentemente (30%).
    pub burned: Amount,
    /// Parte destinada a validadores (70%).
    pub to_validators: Amount,
}

/// Motor de quema de comisiones de protocolo.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BurnEngine {
    /// Total histórico quemado vía este motor.
    pub total_burned: Amount,
    /// Total histórico asignado a validadores desde fees.
    pub total_to_validators: Amount,
}

impl BurnEngine {
    /// Nuevo motor vacío.
    pub fn new() -> Self {
        Self::default()
    }

    /// Calcula el split 30/70 sin mutar estado.
    ///
    /// El residuo de redondeo se asigna a validadores; la quema nunca se
    /// redondea al alza.
    pub fn split(gross: Amount) -> Result<ProtocolFeeSplit, MonetaryError> {
        if gross.0 == 0 {
            return Err(MonetaryError::ZeroAmount);
        }
        let burned = gross
            .0
            .checked_mul(u128::from(PROTOCOL_FEE_BURN_BPS))
            .ok_or(MonetaryError::Overflow)?
            / u128::from(BPS_DENOMINATOR);
        let to_validators = gross
            .0
            .checked_sub(burned)
            .ok_or(MonetaryError::Overflow)?;
        debug_assert_eq!(burned + to_validators, gross.0);
        debug_assert_eq!(
            PROTOCOL_FEE_BURN_BPS + PROTOCOL_FEE_VALIDATOR_BPS,
            BPS_DENOMINATOR
        );
        Ok(ProtocolFeeSplit {
            gross,
            burned: Amount(burned),
            to_validators: Amount(to_validators),
        })
    }

    /// Aplica el split: quema desde circulante y contabiliza la parte a validadores.
    pub fn apply_protocol_fee(
        &mut self,
        tracker: &mut SupplyTracker,
        gross: Amount,
    ) -> Result<ProtocolFeeSplit, MonetaryError> {
        let split = Self::split(gross)?;
        if split.burned.0 > 0 {
            tracker.burn_from_circulating(split.burned)?;
            self.total_burned = add(self.total_burned, split.burned)?;
        }
        self.total_to_validators = add(self.total_to_validators, split.to_validators)?;
        Ok(split)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{EmissionCategory, AMOUNT_SCALE};
    use crate::supply::SupplyTracker;

    #[test]
    fn split_1000_rsk() {
        let gross = Amount(1_000 * AMOUNT_SCALE);
        let split = BurnEngine::split(gross).unwrap();
        assert_eq!(split.burned, Amount(300 * AMOUNT_SCALE));
        assert_eq!(split.to_validators, Amount(700 * AMOUNT_SCALE));
    }

    #[test]
    fn apply_burns_from_circulating() {
        let mut tracker = SupplyTracker::new();
        tracker
            .emit_circulating(EmissionCategory::Community, Amount(1_000 * AMOUNT_SCALE))
            .unwrap();
        let mut burn = BurnEngine::new();
        let split = burn
            .apply_protocol_fee(&mut tracker, Amount(1_000 * AMOUNT_SCALE))
            .unwrap();
        assert_eq!(split.burned, Amount(300 * AMOUNT_SCALE));
        assert_eq!(tracker.snapshot.burned, Amount(300 * AMOUNT_SCALE));
        assert_eq!(tracker.snapshot.circulating, Amount(700 * AMOUNT_SCALE));
        assert_eq!(burn.total_burned, Amount(300 * AMOUNT_SCALE));
    }
}
