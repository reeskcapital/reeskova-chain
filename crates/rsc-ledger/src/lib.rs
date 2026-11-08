//! Transacciones canónicas y secuencia lógica del ledger de RSC Chain v1.
//!
//! Este crate define qué transacción ocurrió, cuál es su identidad y en qué
//! orden lógico quedó incluida. No ejecuta la transacción, no modifica
//! balances, no persiste, no crea bloques y no participa en consenso.
//!
//! ```text
//! rsc-core → rsc-crypto → rsc-ledger
//! ```
//!
//! `LedgerRoot` responde cuál es la secuencia de identificadores. No es un
//! `StateRoot`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod ledger;
pub mod transaction;

pub use ledger::{Ledger, LedgerEntry, LedgerError, LedgerRoot};
pub use transaction::{
    Transaction, TransactionError, TransactionId, TransactionPayload, AMM_INSTRUCTION_MAX_LEN,
    TRANSACTION_VERSION_V1,
};
