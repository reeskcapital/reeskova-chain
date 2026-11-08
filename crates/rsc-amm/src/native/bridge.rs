//! Puente entre [`rsc_assets::AssetLedger`] y el motor AMM in-memory.

use crate::accounting::AmmAccounting;
use crate::pool::AssetCatalog;
use crate::types::{AssetId, PoolId};
use crate::AmmError;
use rsc_assets::{AssetId as AssetsAssetId, AssetLedger};
use rsc_core::{AccountId, Amount, Hash};
use std::collections::BTreeMap;

/// Convierte id de activo AMM → assets.
pub fn assets_asset_id(id: AssetId) -> AssetsAssetId {
    AssetsAssetId(id.0)
}

/// Convierte id de activo assets → AMM.
pub fn amm_asset_id(id: AssetsAssetId) -> AssetId {
    AssetId(id.0)
}

/// Catálogo respaldado por el ledger autoritativo de activos.
pub struct AssetsCatalog<'a> {
    ledger: &'a AssetLedger,
}

impl<'a> AssetsCatalog<'a> {
    /// Vista de solo lectura.
    pub fn new(ledger: &'a AssetLedger) -> Self {
        Self { ledger }
    }
}

impl AssetCatalog for AssetsCatalog<'_> {
    fn is_registered(&self, asset: &AssetId) -> bool {
        self.ledger.is_registered(&assets_asset_id(*asset))
    }
}

/// Carga saldos de usuario y vaults desde `rsc-assets` hacia [`AmmAccounting`].
pub fn hydrate_accounting(accounting: &mut AmmAccounting, assets: &AssetLedger) {
    let mut balances = BTreeMap::new();
    for ((account, asset), amount) in assets.user_balances() {
        if *amount > 0 {
            balances.insert((*account, amm_asset_id(*asset)), Amount(*amount));
        }
    }
    let mut custody = BTreeMap::new();
    for ((pool_hash, asset), amount) in assets.vault_balances() {
        if *amount > 0 {
            custody.insert(
                (PoolId(*pool_hash), amm_asset_id(*asset)),
                Amount(*amount),
            );
        }
    }
    accounting.replace_asset_balances(balances, custody);
}

/// Escribe saldos de usuario y vaults desde [`AmmAccounting`] hacia `rsc-assets`.
pub fn flush_accounting(
    accounting: &AmmAccounting,
    assets: &mut AssetLedger,
) -> Result<(), AmmError> {
    let mut balances = BTreeMap::new();
    for ((account, asset), amount) in accounting.user_balances_map() {
        if amount.0 > 0 {
            balances.insert((*account, assets_asset_id(*asset)), amount.0);
        }
    }
    let mut vaults = BTreeMap::new();
    for ((pool, asset), amount) in accounting.pool_custody_map() {
        if amount.0 > 0 {
            vaults.insert((pool.0, assets_asset_id(*asset)), amount.0);
        }
    }
    assets
        .replace_balances(balances, vaults)
        .map_err(map_assets_err)
}

/// Mapea errores de assets a errores AMM.
pub fn map_assets_err(err: rsc_assets::AssetsError) -> AmmError {
    match err {
        rsc_assets::AssetsError::AssetNotRegistered => AmmError::AssetNotRegistered,
        rsc_assets::AssetsError::InsufficientBalance => AmmError::InsufficientBalance,
        rsc_assets::AssetsError::ZeroAmount => AmmError::ZeroAmount,
        rsc_assets::AssetsError::Overflow => AmmError::Overflow,
        rsc_assets::AssetsError::Unauthorized | rsc_assets::AssetsError::InvalidTransfer => {
            AmmError::TransferFailed
        }
        rsc_assets::AssetsError::InvalidEncoding
        | rsc_assets::AssetsError::SchemaMismatch
        | rsc_assets::AssetsError::Storage
        | rsc_assets::AssetsError::AssetAlreadyRegistered => AmmError::InvalidEncoding,
    }
}

/// Helper: id de vault documentado (no usado como AccountId de estado).
#[allow(dead_code)]
pub fn vault_account(pool_id: &PoolId, asset: &AssetId) -> AccountId {
    AssetLedger::vault_account_id(&pool_id.0, &assets_asset_id(*asset))
}

/// Hash de pool como clave de vault.
pub fn pool_vault_key(pool_id: &PoolId) -> Hash {
    pool_id.0
}
