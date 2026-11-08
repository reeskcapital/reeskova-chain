//! Estado lógico de RSC Chain v1.
//!
//! `rsc-state` interpreta bytes persistidos por `rsc-storage` como cuentas,
//! nonces, balances y una raíz de estado. No ejecuta WASM, no habla con la
//! red y no decide reglas económicas.
//!
//! La raíz es la raíz de un Sparse Merkle Tree de 256 bits indexado por
//! [`AccountId`]. El mismo estado lógico produce el mismo [`StateRoot`] en
//! cualquier nodo.

#![forbid(unsafe_code)]

pub mod account;
pub mod state;

pub use account::{derive_account_id, Account, AccountId, ACCOUNT_ENCODED_LEN, ACCOUNT_VERSION_V1};
pub use state::{
    GenesisAccount, State, StateCommitment, StateOperation, StateRoot, StateSnapshot,
    StateTransition,
};

use rsc_crypto::HashBuilder;

/// Esquema de metadatos de estado. Un valor distinto se rechaza al abrir.
pub(crate) const STATE_SCHEMA_V1: u32 = 1;

/// Profundidad del Sparse Merkle Tree. La hoja está en este nivel.
pub(crate) const TREE_DEPTH: u16 = 256;

/// Prefijo binario de los registros de cuenta.
pub(crate) const ACCOUNT_NAMESPACE: &[u8] = b"RSC/STATE/ACCOUNT/V1/";

/// Prefijo binario de los nodos del árbol.
pub(crate) const SMT_NAMESPACE: &[u8] = b"RSC/STATE/SMT/NODE/V1/";

/// Prefijo binario de los metadatos de estado.
pub(crate) const META_NAMESPACE: &[u8] = b"RSC/STATE/META/V1/";

/// Dominio de derivación de [`AccountId`].
pub(crate) const ACCOUNT_ID_DOMAIN: &[u8] = b"RSC/STATE/ACCOUNT-ID/V1";

/// Dominio del hash de hoja de cuenta.
pub(crate) const ACCOUNT_LEAF_DOMAIN: &[u8] = b"RSC/STATE/SMT/ACCOUNT-LEAF/V1";

/// Dominio del hash de nodo interno. No es la clave de almacenamiento.
pub(crate) const SMT_NODE_DOMAIN: &[u8] = b"RSC/STATE/SMT/NODE/V1";

/// Dominio de la hoja vacía.
pub(crate) const SMT_EMPTY_LEAF_DOMAIN: &[u8] = b"RSC/STATE/SMT/EMPTY-LEAF/V1";

/// SHA-256 de `domain || parts`, delegado en `rsc-crypto`.
pub(crate) fn domain_hash(domain: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut builder = HashBuilder::new();
    builder.update(domain);
    for part in parts {
        builder.update(part);
    }
    builder.finalize().to_bytes()
}

/// Error tipado de estado. No revela claves privadas ni rutas de disco.
#[derive(Debug, thiserror::Error)]
pub enum StateError {
    /// La capa física rechazó la operación. El detalle queda en `source`.
    #[error("storage error")]
    Storage(#[source] rsc_storage::StorageError),
    /// Bytes de cuenta o de nodo que no siguen el encoding canónico.
    #[error("serialization error")]
    Serialization,
    /// La cuenta no está en el estado.
    #[error("account not found")]
    AccountNotFound,
    /// Ya existe una cuenta con ese identificador.
    #[error("account already exists")]
    AccountAlreadyExists,
    /// La cuenta viola un invariante que no tiene un error más específico.
    #[error("invalid account")]
    InvalidAccount,
    /// `version` distinta de [`ACCOUNT_VERSION_V1`].
    #[error("invalid account version")]
    InvalidAccountVersion,
    /// El `AccountId` no coincide con el `OwnerKeyId` o no tiene 32 bytes.
    #[error("invalid account id")]
    InvalidAccountId,
    /// Importe cero. No es una transferencia de valor.
    #[error("invalid amount")]
    InvalidAmount,
    /// Emisor y receptor son la misma cuenta.
    #[error("invalid transfer")]
    InvalidTransfer,
    /// El emisor no tiene balance suficiente.
    #[error("insufficient balance")]
    InsufficientBalance,
    /// El crédito no cabe en `u128`.
    #[error("balance overflow")]
    BalanceOverflow,
    /// El nonce indicado no es el nonce actual de la cuenta.
    #[error("nonce mismatch")]
    NonceMismatch,
    /// El nonce no puede incrementarse sin envolver `u64`.
    #[error("nonce overflow")]
    NonceOverflow,
    /// El esquema persistido no es el de esta versión del protocolo.
    #[error("state schema mismatch")]
    StateSchemaMismatch,
    /// El génesis ya fue aplicado.
    #[error("state already initialized")]
    StateAlreadyInitialized,
    /// La transición exige un estado ya inicializado.
    #[error("state not initialized")]
    StateNotInitialized,
    /// La raíz esperada, o la reconstruida, no coincide con la persistida.
    #[error("state root mismatch")]
    StateRootMismatch,
    /// La versión esperada no es la versión persistida.
    #[error("state version mismatch")]
    StateVersionMismatch,
    /// La versión nueva no es estrictamente mayor que la actual.
    #[error("version not increasing")]
    VersionNotIncreasing,
    /// La transición no contiene operaciones aplicables.
    #[error("invalid transition")]
    InvalidTransition,
    /// Metadatos ausentes, parciales o con longitud ilegal.
    #[error("corrupted metadata")]
    CorruptedMetadata,
    /// No se pudo abrir una vista de lectura.
    #[error("snapshot unavailable")]
    SnapshotUnavailable,
    /// El candado de commit quedó envenenado.
    #[error("concurrency error")]
    Concurrency,
}

impl From<rsc_storage::StorageError> for StateError {
    fn from(value: rsc_storage::StorageError) -> Self {
        Self::Storage(value)
    }
}
