//! Errores estructurales de la cadena. No de consenso, ledger ni red.

use thiserror::Error;

/// Error de estructura, codec o navegación de bloques.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ChainError {
    /// `version` no coincide con la esperada.
    #[error("invalid block version")]
    InvalidBlockVersion,
    /// `chain_id` distinto al de esta red.
    #[error("wrong chain id")]
    WrongChainId,
    /// Genesis mal formado.
    #[error("invalid genesis")]
    InvalidGenesis,
    /// Altura distinta de parent+1.
    #[error("invalid height")]
    InvalidHeight,
    /// `previous_block_hash` no es el hash del padre.
    #[error("invalid parent hash")]
    InvalidParentHash,
    /// Falta el bloque padre.
    #[error("parent not found")]
    ParentNotFound,
    /// `transactions_root` no coincide con el Merkle del bloque.
    #[error("invalid transaction root")]
    InvalidTransactionRoot,
    /// Bytes canónicos por encima del máximo.
    #[error("block too large")]
    BlockTooLarge,
    /// Demasiadas transacciones.
    #[error("too many transactions")]
    TooManyTransactions,
    /// Codec canónico inválido.
    #[error("invalid encoding")]
    InvalidEncoding,
    /// Versión de protocolo no soportada.
    #[error("unsupported protocol version")]
    UnsupportedProtocolVersion,
    /// Hash de bloque no calculable o incorrecto.
    #[error("invalid block hash")]
    InvalidBlockHash,
    /// El bloque ya está en el store.
    #[error("block already exists")]
    BlockAlreadyExists,
    /// El bloque no está en el store.
    #[error("block not found")]
    BlockNotFound,
    /// Fallo del `ChainStore` (sin filtrar el backend).
    #[error("store")]
    Store,
}
