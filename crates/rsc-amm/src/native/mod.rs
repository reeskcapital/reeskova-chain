//! Integración nativa del AMM con el ledger, estado y almacenamiento RSC.
//!
//! - Instrucciones canónicas en transacciones [`rsc_ledger::TransactionPayload::Amm`].
//! - Saldos autoritativos en [`rsc_assets::AssetLedger`].
//! - Nonce / anti-replay de cuenta vía [`rsc_state::StateOperation::ConsumeNonce`].
//! - Persistencia AMM + assets + estado en un único [`rsc_storage::StorageBatch`].
//! - Eventos emitidos solo tras commit exitoso (CF Receipts).

mod bridge;
mod instruction;
mod runtime;
mod store;

pub use bridge::{amm_asset_id, assets_asset_id, AssetsCatalog};
pub use instruction::AmmInstruction;
pub use runtime::{
    sign_amm_tx, AmmExecutionResult, AmmRuntime, BlockContext, NativeAmmError,
};
pub use store::{AmmStore, AMM_EVENTS_KEY_PREFIX, AMM_STORE_KEYS};

#[cfg(test)]
mod stage7_native;
#[cfg(test)]
mod tests;
