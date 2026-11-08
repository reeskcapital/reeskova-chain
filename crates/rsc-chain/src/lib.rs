//! Estructura de bloques y cadena. No consenso, no ledger, no P2P, no DB.
//!
//! ```
//! use rsc_chain::{Block, BlockHeader, Chain, ChainError, ChainStore};
//! ```

#![forbid(unsafe_code)]

pub mod block;
pub mod chain;
pub mod codec;
pub mod errors;
pub mod header;
pub mod merkle;
pub mod rocks_store;
pub mod store;
pub mod validation;

pub use block::{Block, RawTransaction};
pub use chain::{Chain, InsertStatus};
pub use codec::{decode_block, decode_header, encode_block, encode_header, header_hash};
pub use errors::ChainError;
pub use header::BlockHeader;
pub use merkle::{merkle_root, tx_leaf};
pub use rocks_store::RocksDbChainStore;
pub use rsc_core::BlockHash;
pub use store::ChainStore;
pub use validation::{validate_block, ChainValidationContext};
