//! Errores estructurados del AMM.
//!
//! No cubren consenso, red, almacenamiento ni firmas: solo validación de
//! parámetros y aritmética del motor.

use thiserror::Error;

/// Fallo determinista de una operación AMM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum AmmError {
    /// Suma o producto fuera de `u128` / ancho intermedio.
    #[error("arithmetic overflow")]
    Overflow,
    /// Resta por debajo de cero.
    #[error("arithmetic underflow")]
    Underflow,
    /// Divisor cero.
    #[error("division by zero")]
    DivisionByZero,
    /// Cantidad de entrada o salida cero donde se exige positiva.
    #[error("amount must be greater than zero")]
    ZeroAmount,
    /// Reservas inválidas (cero o producto que no cumple la política).
    #[error("invalid pool reserves")]
    InvalidReserves,
    /// Liquidez insuficiente para el swap o el retiro pedido.
    #[error("insufficient liquidity")]
    InsufficientLiquidity,
    /// Fee en basis points fuera del rango permitido.
    #[error("invalid fee in basis points")]
    InvalidFee,
    /// Identificadores de activo iguales o pool mal formado.
    #[error("invalid asset pair")]
    InvalidPair,
    /// Conversión numérica fuera de rango.
    #[error("numeric conversion failed")]
    ConversionFailed,
    /// Liquidez mínima bloqueada supera las shares iniciales.
    #[error("initial liquidity below minimum lock")]
    InsufficientInitialLiquidity,
    /// Supply total de LP es cero donde se requiere positivo.
    #[error("zero total lp supply")]
    ZeroSupply,
    /// Invariante de producto constante violada tras la operación teórica.
    #[error("constant-product invariant violated")]
    InvariantViolation,
    /// Activo no registrado en el catálogo del protocolo.
    #[error("asset is not registered")]
    AssetNotRegistered,
    /// Ya existe un pool con la misma identidad.
    #[error("pool already exists")]
    PoolAlreadyExists,
    /// Pool no encontrado en el registro.
    #[error("pool not found")]
    PoolNotFound,
    /// Estado del pool no admite la operación pedida.
    #[error("invalid pool status for operation")]
    InvalidPoolStatus,
    /// El pool no cumple los requisitos de liquidez inicial para activarse.
    #[error("pool does not meet activation requirements")]
    NotReadyToActivate,
    /// La cuenta no está autorizada para esta acción de gobernanza.
    #[error("unauthorized amm authority")]
    Unauthorized,
    /// La política del protocolo no permite crear pools de forma permissionless.
    #[error("permissionless pool creation is disabled")]
    CreationDisabled,
    /// Operación de swap no permitida en el estado actual del pool.
    #[error("swap not permitted for pool status")]
    SwapNotPermitted,
    /// Aporte de liquidez no permitido en el estado actual del pool.
    #[error("liquidity mint not permitted for pool status")]
    MintNotPermitted,
    /// Retiro de liquidez no permitido en el estado actual del pool.
    #[error("liquidity burn not permitted for pool status")]
    BurnNotPermitted,
    /// Encoding canónico del registro AMM inválido o truncado.
    #[error("invalid amm state encoding")]
    InvalidEncoding,
    /// Saldo de activo insuficiente en la cuenta.
    #[error("insufficient asset balance")]
    InsufficientBalance,
    /// Saldo de LP insuficiente para el retiro.
    #[error("insufficient lp balance")]
    InsufficientLpBalance,
    /// Mínimo de LP o de salida no alcanzado (slippage).
    #[error("slippage / minimum output not met")]
    SlippageExceeded,
    /// Deadline de la operación expirado.
    #[error("operation deadline exceeded")]
    DeadlineExceeded,
    /// Depósito desbalanceado respecto al ratio de reservas (no se acepta exceso).
    #[error("unbalanced liquidity deposit")]
    UnbalancedDeposit,
    /// Intento de retirar shares bloqueadas en el sink.
    #[error("locked liquidity cannot be withdrawn")]
    LockedLiquidity,
    /// Invariante contable violada (reservas ≠ custodia, supply ≠ LP, etc.).
    #[error("accounting invariant violated")]
    AccountingInvariant,
    /// Operación duplicada (mismo `op_id` ya procesado).
    #[error("duplicate operation id")]
    DuplicateOperation,
    /// Transferencia de activo rechazada por el ledger.
    #[error("asset transfer failed")]
    TransferFailed,
    /// Cotización expirada respecto al bloque actual.
    #[error("quote expired")]
    QuoteExpired,
}
