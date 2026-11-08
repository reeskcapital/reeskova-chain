//! Errores del ledger de activos.

use thiserror::Error;

/// Fallo determinista del ledger de activos.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum AssetsError {
    /// Encoding canónico inválido o truncado.
    #[error("invalid assets encoding")]
    InvalidEncoding,
    /// Activo no registrado.
    #[error("asset is not registered")]
    AssetNotRegistered,
    /// Activo ya registrado.
    #[error("asset already registered")]
    AssetAlreadyRegistered,
    /// Cantidad cero donde se exige positiva.
    #[error("amount must be greater than zero")]
    ZeroAmount,
    /// Saldo insuficiente.
    #[error("insufficient asset balance")]
    InsufficientBalance,
    /// Overflow aritmético.
    #[error("arithmetic overflow")]
    Overflow,
    /// Emisor y receptor iguales.
    #[error("invalid transfer")]
    InvalidTransfer,
    /// Transferencia no autorizada (cuenta origen distinta del firmante).
    #[error("transfer not authorized")]
    Unauthorized,
    /// Esquema de persistencia distinto.
    #[error("assets schema mismatch")]
    SchemaMismatch,
    /// Error de almacenamiento.
    #[error("storage error")]
    Storage,
}
