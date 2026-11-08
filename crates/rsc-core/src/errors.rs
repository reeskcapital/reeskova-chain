//! Errores de los tipos primitivos. No de consenso, red ni pagos.

use thiserror::Error;

/// Error de formato o aritmética de un tipo de `rsc-core`.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CoreError {
    /// Longitud distinta a la exigida por el tipo.
    #[error("invalid length: expected {expected}, got {actual}")]
    InvalidLength { expected: usize, actual: usize },
    /// Hex mal formado.
    #[error("invalid hex encoding")]
    InvalidHex,
    /// No es un hash de 32 bytes.
    #[error("invalid hash")]
    InvalidHash,
    /// No es un `AccountId` de 32 bytes.
    #[error("invalid account id")]
    InvalidAccountId,
    /// Cantidad no entera / no parseable.
    #[error("invalid amount")]
    InvalidAmount,
    /// Nonce fuera de rango.
    #[error("invalid nonce")]
    InvalidNonce,
    /// `ChainId` no parseable.
    #[error("invalid chain id")]
    InvalidChainId,
    /// `Timestamp` no parseable.
    #[error("invalid timestamp")]
    InvalidTimestamp,
    /// Suma fuera de `u128`/`u64`.
    #[error("arithmetic overflow")]
    Overflow,
    /// Resta por debajo de cero.
    #[error("arithmetic underflow")]
    Underflow,
}
