//! Persistencia física de RSC Chain.
//!
//! Este crate guarda y recupera bytes. No interpreta cuentas, no calcula
//! raíces de estado y no ejecuta transacciones. `rsc-state` es el dueño del
//! significado lógico.

#![forbid(unsafe_code)]

mod database;
mod snapshots;

pub use database::{
    ColumnFamily, DatabaseConfig, RscDatabase, StorageBatch, StorageError, StorageKey,
    StorageValue, STORAGE_SCHEMA_VERSION,
};
pub use snapshots::{Snapshot, SnapshotError, SnapshotManager, SnapshotManifest, SnapshotMetadata};
