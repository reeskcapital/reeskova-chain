//! Asentamiento de comisiones de protocolo contra el estado de cuentas.
//!
//! La política 30/70 vive aquí. `rsc-state` solo ejecuta montos ya partidos.
//! La identidad del fee pool se publica como `KeyId`; el `AccountId` lo deriva
//! `rsc-state::derive_account_id` en la capa de nodo.

use crate::amount_ext::add;
use crate::burn::{BurnEngine, ProtocolFeeSplit};
use crate::error::MonetaryError;
use crate::policy::{EmissionCategory, MonetaryPolicy};
use crate::schedule::{self, u128_decimal};
use crate::supply::SupplyTracker;
use rsc_core::Amount;
use rsc_crypto::{hash, KeyId, PrivateKey};
use serde::{Deserialize, Serialize};

const FEE_POOL_SEED_DOMAIN: &[u8] = b"RSC-MONETARY-FEE-POOL/V1\0";
const SECURITY_VAULT_SEED_DOMAIN: &[u8] = b"RSC-MONETARY-SECURITY-VAULT/V1\0";

/// Plan de comisión listo para una transición de estado.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolFeePlan {
    /// Comisión bruta cobrada al emisor.
    pub total_fee: Amount,
    /// Parte destruida (30%).
    pub burn_fee: Amount,
    /// Parte acreditada al fee pool (70%).
    pub validator_fee: Amount,
}

impl From<ProtocolFeeSplit> for ProtocolFeePlan {
    fn from(split: ProtocolFeeSplit) -> Self {
        Self {
            total_fee: split.gross,
            burn_fee: split.burned,
            validator_fee: split.to_validators,
        }
    }
}

/// Contabilidad monetaria runtime (supply + quema) serializable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonetaryRuntime {
    /// Política activa.
    pub policy: MonetaryPolicy,
    /// Tracker de supply.
    pub supply: SupplyTracker,
    /// Motor de quema.
    pub burn: BurnEngine,
    /// El calendario de seguridad ya tomó la reserva de minería que quedaba.
    #[serde(default)]
    pub security_armed: bool,
    /// Timestamp del bloque génesis. Cero hasta armar el calendario.
    #[serde(default)]
    pub genesis_timestamp: u64,
    /// Días de cadena ya liquidados. El siguiente día pendiente es este índice.
    #[serde(default)]
    pub settled_days: u64,
    /// Reserva de minería al armar, en unidades mínimas. Texto decimal en JSON.
    #[serde(default, with = "u128_decimal")]
    pub mining_unissued_at_arm: u128,
}

impl MonetaryRuntime {
    /// Runtime en política v1 con contadores a cero.
    pub fn v1() -> Self {
        MonetaryPolicy::assert_distribution_invariants().expect("policy invariants");
        Self {
            policy: MonetaryPolicy::v1(),
            supply: SupplyTracker::new(),
            burn: BurnEngine::new(),
            security_armed: false,
            genesis_timestamp: 0,
            settled_days: 0,
            mining_unissued_at_arm: 0,
        }
    }

    /// Fija el calendario con la reserva de minería que todavía no se emitió.
    ///
    /// Una segunda llamada no cambia la reserva ya armada.
    pub fn arm_security(&mut self, genesis_timestamp: u64) -> Result<(), MonetaryError> {
        if genesis_timestamp == 0 || self.security_armed {
            return Ok(());
        }
        self.mining_unissued_at_arm = self.supply.remaining_budget(EmissionCategory::Mining)?.0;
        self.genesis_timestamp = genesis_timestamp;
        self.settled_days = 0;
        self.security_armed = true;
        Ok(())
    }

    /// Subsidio de minería de los días ya cumplidos y todavía no liquidados.
    ///
    /// `block_timestamp` es el timestamp del bloque que se está sellando.
    /// `payees` es la gente que cobra hoy. Con cero personas no se emite.
    /// Cada día atrasado usa ese mismo recuento.
    pub fn subsidy_due(&self, block_timestamp: u64, payees: u64) -> u128 {
        if !self.security_armed || self.genesis_timestamp == 0 || payees == 0 {
            return 0;
        }
        let day = schedule::chain_day(self.genesis_timestamp, block_timestamp);
        if day <= self.settled_days {
            return 0;
        }
        let elapsed = day - self.settled_days;
        let remaining = self
            .supply
            .remaining_budget(EmissionCategory::Mining)
            .map(|amount| amount.0)
            .unwrap_or(0);
        schedule::subsidy_for_days(remaining, payees, elapsed)
    }

    /// Día de cadena que cubre `block_timestamp`, si el calendario está armado.
    pub fn open_day(&self, block_timestamp: u64) -> Option<u64> {
        if !self.security_armed || self.genesis_timestamp == 0 {
            return None;
        }
        Some(schedule::chain_day(self.genesis_timestamp, block_timestamp))
    }

    /// Marca liquidados los días hasta `day` y emite el subsidio a circulación.
    pub fn note_security_subsidy(&mut self, day: u64, minted: u128) -> Result<(), MonetaryError> {
        if minted > 0 {
            self.supply
                .emit_circulating(EmissionCategory::Mining, Amount(minted))?;
        }
        if day > self.settled_days {
            self.settled_days = day;
        }
        Ok(())
    }

    /// Registra balances de génesis ya puestos en circulación bajo una categoría.
    pub fn bootstrap_genesis_circulation(
        &mut self,
        category: EmissionCategory,
        amount: Amount,
    ) -> Result<(), MonetaryError> {
        if amount.0 == 0 {
            return Ok(());
        }
        self.supply.emit_circulating(category, amount)
    }

    /// Atribuye el supply de génesis según pares `(categoría, monto)`.
    ///
    /// La capa de nodo traduce `GenesisAccount.allocation` a
    /// [`EmissionCategory`] antes de llamar.
    pub fn bootstrap_from_allocations(
        &mut self,
        allocations: &[(EmissionCategory, Amount)],
    ) -> Result<(), MonetaryError> {
        for &(category, amount) in allocations {
            self.bootstrap_genesis_circulation(category, amount)?;
        }
        Ok(())
    }

    /// Atribuye el supply de génesis a presupuestos, en orden de prioridad.
    ///
    /// Reservado para documentos legacy sin categoría. Preferir
    /// [`Self::bootstrap_from_genesis_accounts`].
    pub fn bootstrap_undifferentiated_genesis(
        &mut self,
        total: Amount,
    ) -> Result<(), MonetaryError> {
        if total.0 == 0 {
            return Ok(());
        }
        let order = [
            EmissionCategory::Mining,
            EmissionCategory::Community,
            EmissionCategory::Treasury,
            EmissionCategory::HolderRewards,
            EmissionCategory::OperatorRewards,
        ];
        let mut remaining = total.0;
        for category in order {
            if remaining == 0 {
                break;
            }
            let room = self.supply.remaining_budget(category)?.0;
            let take = remaining.min(room);
            if take > 0 {
                self.bootstrap_genesis_circulation(category, Amount(take))?;
                remaining -= take;
            }
        }
        if remaining > 0 {
            return Err(MonetaryError::MaxSupplyExceeded);
        }
        Ok(())
    }

    /// Planifica el cobro de `minimum_fee` (0 → sin comisión).
    pub fn plan_minimum_fee(
        minimum_fee: u128,
    ) -> Result<Option<ProtocolFeePlan>, MonetaryError> {
        if minimum_fee == 0 {
            return Ok(None);
        }
        Ok(Some(ProtocolFeePlan::from(BurnEngine::split(Amount(
            minimum_fee,
        ))?)))
    }

    /// Aplica la quema contable del plan (tras cobrar en cuentas).
    pub fn settle_protocol_fee(
        &mut self,
        plan: ProtocolFeePlan,
    ) -> Result<ProtocolFeeSplit, MonetaryError> {
        if plan.burn_fee.0 > 0 {
            self.supply.burn_from_circulating(plan.burn_fee)?;
            self.burn.total_burned = add(self.burn.total_burned, plan.burn_fee)?;
        }
        self.burn.total_to_validators =
            add(self.burn.total_to_validators, plan.validator_fee)?;
        Ok(ProtocolFeeSplit {
            gross: plan.total_fee,
            burned: plan.burn_fee,
            to_validators: plan.validator_fee,
        })
    }
}

/// Semilla Ed25519 canónica del fee pool de protocolo (no es una clave de usuario).
pub fn protocol_fee_pool_seed() -> [u8; 32] {
    *hash(FEE_POOL_SEED_DOMAIN).as_bytes()
}

/// Clave privada determinista del fee pool.
pub fn protocol_fee_pool_private_key() -> PrivateKey {
    PrivateKey::from_bytes(&protocol_fee_pool_seed()).expect("seed de 32 bytes")
}

/// `KeyId` del fee pool. El `AccountId` se obtiene con `derive_account_id`.
pub fn protocol_fee_pool_key_id() -> KeyId {
    protocol_fee_pool_private_key().key_id()
}

/// Semilla de la bóveda que guarda el premio de seguridad hasta cobrarlo.
pub fn security_vault_seed() -> [u8; 32] {
    *hash(SECURITY_VAULT_SEED_DOMAIN).as_bytes()
}

/// Clave determinista de la bóveda de seguridad. No es una clave de usuario.
pub fn security_vault_private_key() -> PrivateKey {
    PrivateKey::from_bytes(&security_vault_seed()).expect("seed de 32 bytes")
}

/// `KeyId` de la bóveda. El saldo que no entra en un asiento se queda aquí.
pub fn security_vault_key_id() -> KeyId {
    security_vault_private_key().key_id()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::AMOUNT_SCALE;

    #[test]
    fn plan_splits_30_70() {
        let plan = MonetaryRuntime::plan_minimum_fee(1_000 * AMOUNT_SCALE)
            .unwrap()
            .unwrap();
        assert_eq!(plan.burn_fee, Amount(300 * AMOUNT_SCALE));
        assert_eq!(plan.validator_fee, Amount(700 * AMOUNT_SCALE));
    }

    #[test]
    fn settle_burns_from_circulating_tracker() {
        let mut rt = MonetaryRuntime::v1();
        rt.bootstrap_genesis_circulation(
            EmissionCategory::Community,
            Amount(10_000 * AMOUNT_SCALE),
        )
        .unwrap();
        let plan = MonetaryRuntime::plan_minimum_fee(1_000 * AMOUNT_SCALE)
            .unwrap()
            .unwrap();
        rt.settle_protocol_fee(plan).unwrap();
        assert_eq!(rt.supply.snapshot.burned, Amount(300 * AMOUNT_SCALE));
        assert_eq!(
            rt.supply.snapshot.circulating,
            Amount(9_700 * AMOUNT_SCALE)
        );
        assert_eq!(rt.burn.total_to_validators, Amount(700 * AMOUNT_SCALE));
    }

    #[test]
    fn fee_pool_key_is_stable() {
        assert_eq!(protocol_fee_pool_key_id(), protocol_fee_pool_key_id());
    }

    #[test]
    fn bootstrap_respects_account_categories() {
        let allocations = [
            (EmissionCategory::Mining, Amount(100 * AMOUNT_SCALE)),
            (EmissionCategory::Treasury, Amount(40 * AMOUNT_SCALE)),
        ];
        let mut rt = MonetaryRuntime::v1();
        rt.bootstrap_from_allocations(&allocations).unwrap();
        assert_eq!(
            rt.supply.spent(EmissionCategory::Mining),
            Amount(100 * AMOUNT_SCALE)
        );
        assert_eq!(
            rt.supply.spent(EmissionCategory::Treasury),
            Amount(40 * AMOUNT_SCALE)
        );
        assert_eq!(rt.supply.snapshot.circulating, Amount(140 * AMOUNT_SCALE));
    }
}
