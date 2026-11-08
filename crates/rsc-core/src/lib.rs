//! Tipos, errores y constantes primitivas de RSC.
//!
//! ```
//! use rsc_core::{Amount, BlockHeight, Hash};
//! ```

#![forbid(unsafe_code)]

pub mod constants;
pub mod errors;
pub mod types;

pub use constants::*;
pub use errors::CoreError;
pub use types::{
    AccountId, Amount, BlockHash, BlockHeight, ChainId, Hash, Nonce, PublicKey, Signature,
    StateRoot, Timestamp, TransactionId,
};
