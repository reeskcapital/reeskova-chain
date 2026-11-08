//! Errores deterministas de la política monetaria.

use thiserror::Error;

/// Fallo de reglas monetarias. Ninguno crea RSK por encima del supply máximo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum MonetaryError {
    /// Cantidad cero donde se exige positiva.
    #[error("amount must be greater than zero")]
    ZeroAmount,
    /// Overflow aritmético.
    #[error("arithmetic overflow")]
    Overflow,
    /// Emisión supera el presupuesto de la categoría.
    #[error("category budget exhausted")]
    CategoryBudgetExhausted,
    /// Emisión supera el supply máximo.
    #[error("max supply exceeded")]
    MaxSupplyExceeded,
    /// No hay suficiente supply emitido/circulante/bloqueado para la operación.
    #[error("insufficient supply")]
    InsufficientSupply,
    /// Categoría de emisión inválida para la operación.
    #[error("invalid emission category")]
    InvalidCategory,
    /// Fase de emisión no permite esta liberación.
    #[error("emission not allowed in current phase")]
    EmissionPhaseDenied,
    /// Stake insuficiente o inexistente.
    #[error("insufficient stake")]
    InsufficientStake,
    /// Periodo de desbloqueo aún no cumplido.
    #[error("unstake lock active")]
    UnstakeLockActive,
    /// Validador no registrado o no elegible.
    #[error("invalid validator")]
    InvalidValidator,
    /// Tesorería: vesting aún no liberado.
    #[error("treasury vesting locked")]
    TreasuryVestingLocked,
    /// Tesorería: umbral multisig no alcanzado.
    #[error("treasury authorization failed")]
    TreasuryUnauthorized,
    /// Saldo de presupuesto de tesorería insuficiente.
    #[error("treasury budget exhausted")]
    TreasuryBudgetExhausted,
    /// Estado inconsistente del tracker.
    #[error("supply accounting invariant broken")]
    InvariantBroken,
}
