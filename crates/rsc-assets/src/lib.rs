//! Ledger autoritativo multi-activo de RSC Chain.
//!
//! RSC v1 solo modela saldo nativo en `rsc-state`. Este crate es el ledger de
//! tokens negociables (incl. el nativo tipado como [`NATIVE_ASSET_ID`]) que
//! consumen el AMM y otros módulos de protocolo. Los saldos de frontend /
//! indexer no son fuente de verdad.
//!
//! Persistencia: claves bajo `RSC/ASSETS/V1/` en la CF Default de
//! [`rsc_storage::RscDatabase`], escritas vía [`rsc_storage::StorageBatch`]
//! junto al resto de la transición.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod error;
mod ledger;
mod types;

pub use error::AssetsError;
pub use ledger::{AssetLedger, ASSETS_KEY_PREFIX};
pub use types::{
    default_native_metadata, native_asset_id, AssetId, AssetMetadata, ASSETS_SCHEMA_VERSION,
};
