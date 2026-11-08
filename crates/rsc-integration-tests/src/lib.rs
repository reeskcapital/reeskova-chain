//! Harness de integración. No añade reglas de protocolo.
//!
//! Reexporta los doce crates para que el árbol de dependencias del workspace
//! los muestre juntos. El flujo vive en `tests/integration_12_crates.rs`.

#![forbid(unsafe_code)]

pub use rsc_chain;
pub use rsc_consensus;
pub use rsc_core;
pub use rsc_crypto;
pub use rsc_execution;
pub use rsc_genesis;
pub use rsc_ledger;
pub use rsc_mempool;
pub use rsc_p2p;
pub use rsc_rpc;
pub use rsc_state;
pub use rsc_storage;
