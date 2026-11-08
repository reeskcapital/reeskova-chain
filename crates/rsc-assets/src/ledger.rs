//! Ledger de saldos multi-activo con encoding canónico y persistencia.

use crate::error::AssetsError;
use crate::types::{
    default_native_metadata, native_asset_id, AssetId, AssetMetadata, ASSETS_SCHEMA_VERSION,
};
use rsc_core::{AccountId, Amount, Hash};
use rsc_crypto::hash;
#[cfg(feature = "persist")]
use rsc_storage::{ColumnFamily, RscDatabase, StorageBatch};
use std::collections::BTreeMap;

/// Prefijo de clave física del ledger.
pub const ASSETS_KEY_PREFIX: &[u8] = b"RSC/ASSETS/V1/LEDGER";

/// Prefijo de dominio para cuentas vault de pool.
const DOMAIN_VAULT: &[u8] = b"RSC-ASSETS-VAULT/V1\0";

/// Ledger autoritativo de activos negociables.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AssetLedger {
    meta: BTreeMap<AssetId, AssetMetadata>,
    /// Saldos libres de usuario `(account, asset)`.
    balances: BTreeMap<(AccountId, AssetId), u128>,
    /// Custodia de vaults AMM `(pool_id_hash, asset)`.
    vaults: BTreeMap<(Hash, AssetId), u128>,
}

impl AssetLedger {
    /// Ledger vacío.
    pub fn new() -> Self {
        Self::default()
    }

    /// Ledger con el activo nativo tipado ya registrado.
    pub fn with_native() -> Self {
        let mut ledger = Self::new();
        let _ = ledger.register(native_asset_id(), default_native_metadata());
        ledger
    }

    /// Registra un activo. Rechaza duplicados.
    pub fn register(&mut self, asset: AssetId, meta: AssetMetadata) -> Result<(), AssetsError> {
        if self.meta.contains_key(&asset) {
            return Err(AssetsError::AssetAlreadyRegistered);
        }
        self.meta.insert(asset, meta);
        Ok(())
    }

    /// `true` si el activo está registrado.
    pub fn is_registered(&self, asset: &AssetId) -> bool {
        self.meta.contains_key(asset)
    }

    /// Metadatos de un activo registrado.
    pub fn metadata(&self, asset: &AssetId) -> Result<AssetMetadata, AssetsError> {
        self.meta
            .get(asset)
            .copied()
            .ok_or(AssetsError::AssetNotRegistered)
    }

    /// Decimales de un activo registrado.
    pub fn decimals(&self, asset: &AssetId) -> Result<u8, AssetsError> {
        Ok(self.metadata(asset)?.decimals)
    }

    /// Saldo libre de una cuenta.
    pub fn balance_of(&self, account: &AccountId, asset: &AssetId) -> Amount {
        self.balances
            .get(&(*account, *asset))
            .copied()
            .map(Amount)
            .unwrap_or(Amount::ZERO)
    }

    /// Saldo de custodia de un vault de pool.
    pub fn vault_balance(&self, pool_id: &Hash, asset: &AssetId) -> Amount {
        self.vaults
            .get(&(*pool_id, *asset))
            .copied()
            .map(Amount)
            .unwrap_or(Amount::ZERO)
    }

    /// Identificador determinista de cuenta vault (documentación / puentes).
    pub fn vault_account_id(pool_id: &Hash, asset: &AssetId) -> AccountId {
        let mut preimage = Vec::with_capacity(DOMAIN_VAULT.len() + 64);
        preimage.extend_from_slice(DOMAIN_VAULT);
        preimage.extend_from_slice(&pool_id.0);
        preimage.extend_from_slice(asset.as_bytes());
        let digest = hash(&preimage);
        AccountId(*digest.as_bytes())
    }

    /// Abona saldo de usuario (génesis / mint de protocolo). Requiere activo registrado.
    pub fn credit(
        &mut self,
        account: AccountId,
        asset: AssetId,
        amount: Amount,
    ) -> Result<(), AssetsError> {
        self.require_registered(&asset)?;
        if amount.0 == 0 {
            return Err(AssetsError::ZeroAmount);
        }
        let entry = self.balances.entry((account, asset)).or_insert(0);
        *entry = entry
            .checked_add(amount.0)
            .ok_or(AssetsError::Overflow)?;
        Ok(())
    }

    /// Debita saldo de usuario.
    pub fn debit(
        &mut self,
        account: AccountId,
        asset: AssetId,
        amount: Amount,
    ) -> Result<(), AssetsError> {
        self.require_registered(&asset)?;
        if amount.0 == 0 {
            return Err(AssetsError::ZeroAmount);
        }
        let bal = self.balance_of(&account, &asset);
        if bal.0 < amount.0 {
            return Err(AssetsError::InsufficientBalance);
        }
        let new = bal.0 - amount.0;
        if new == 0 {
            self.balances.remove(&(account, asset));
        } else {
            self.balances.insert((account, asset), new);
        }
        Ok(())
    }

    /// Transferencia atómica entre cuentas de usuario.
    ///
    /// `authority` debe coincidir con `from` (permiso de gasto).
    pub fn transfer(
        &mut self,
        authority: &AccountId,
        from: AccountId,
        to: AccountId,
        asset: AssetId,
        amount: Amount,
    ) -> Result<(), AssetsError> {
        if authority != &from {
            return Err(AssetsError::Unauthorized);
        }
        if from == to {
            return Err(AssetsError::InvalidTransfer);
        }
        self.debit(from, asset, amount)?;
        self.credit(to, asset, amount)?;
        Ok(())
    }

    /// Mueve fondos de usuario a vault de pool.
    pub fn transfer_to_vault(
        &mut self,
        from: AccountId,
        pool_id: Hash,
        asset: AssetId,
        amount: Amount,
    ) -> Result<(), AssetsError> {
        self.debit(from, asset, amount)?;
        self.credit_vault(pool_id, asset, amount)?;
        Ok(())
    }

    /// Mueve fondos de vault de pool a usuario.
    pub fn transfer_from_vault(
        &mut self,
        pool_id: Hash,
        to: AccountId,
        asset: AssetId,
        amount: Amount,
    ) -> Result<(), AssetsError> {
        self.debit_vault(pool_id, asset, amount)?;
        self.credit(to, asset, amount)?;
        Ok(())
    }

    /// Crédito directo a vault.
    pub fn credit_vault(
        &mut self,
        pool_id: Hash,
        asset: AssetId,
        amount: Amount,
    ) -> Result<(), AssetsError> {
        self.require_registered(&asset)?;
        if amount.0 == 0 {
            return Err(AssetsError::ZeroAmount);
        }
        let entry = self.vaults.entry((pool_id, asset)).or_insert(0);
        *entry = entry
            .checked_add(amount.0)
            .ok_or(AssetsError::Overflow)?;
        Ok(())
    }

    /// Débito directo de vault.
    pub fn debit_vault(
        &mut self,
        pool_id: Hash,
        asset: AssetId,
        amount: Amount,
    ) -> Result<(), AssetsError> {
        self.require_registered(&asset)?;
        if amount.0 == 0 {
            return Err(AssetsError::ZeroAmount);
        }
        let bal = self.vault_balance(&pool_id, &asset);
        if bal.0 < amount.0 {
            return Err(AssetsError::InsufficientBalance);
        }
        let new = bal.0 - amount.0;
        if new == 0 {
            self.vaults.remove(&(pool_id, asset));
        } else {
            self.vaults.insert((pool_id, asset), new);
        }
        Ok(())
    }

    /// Encoding canónico big-endian.
    pub fn to_canonical_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&ASSETS_SCHEMA_VERSION.to_be_bytes());
        out.extend_from_slice(&(self.meta.len() as u32).to_be_bytes());
        for (id, meta) in &self.meta {
            out.extend_from_slice(id.as_bytes());
            out.push(meta.decimals);
            out.extend_from_slice(&meta.symbol);
        }
        out.extend_from_slice(&(self.balances.len() as u32).to_be_bytes());
        for ((account, asset), amount) in &self.balances {
            out.extend_from_slice(&account.0);
            out.extend_from_slice(asset.as_bytes());
            out.extend_from_slice(&amount.to_be_bytes());
        }
        out.extend_from_slice(&(self.vaults.len() as u32).to_be_bytes());
        for ((pool, asset), amount) in &self.vaults {
            out.extend_from_slice(&pool.0);
            out.extend_from_slice(asset.as_bytes());
            out.extend_from_slice(&amount.to_be_bytes());
        }
        out
    }

    /// Restaura desde encoding canónico.
    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, AssetsError> {
        let mut cur = bytes;
        let version = read_u32(&mut cur)?;
        if version != ASSETS_SCHEMA_VERSION {
            return Err(AssetsError::SchemaMismatch);
        }
        let mut ledger = Self::new();
        let n_meta = read_u32(&mut cur)? as usize;
        for _ in 0..n_meta {
            let id = AssetId(Hash(read_array::<32>(&mut cur)?));
            let decimals = read_u8(&mut cur)?;
            let symbol = read_array::<8>(&mut cur)?;
            ledger.meta.insert(
                id,
                AssetMetadata {
                    decimals,
                    symbol,
                },
            );
        }
        let n_bal = read_u32(&mut cur)? as usize;
        for _ in 0..n_bal {
            let account = AccountId(read_array::<32>(&mut cur)?);
            let asset = AssetId(Hash(read_array::<32>(&mut cur)?));
            let amount = read_u128(&mut cur)?;
            if amount == 0 || !ledger.meta.contains_key(&asset) {
                return Err(AssetsError::InvalidEncoding);
            }
            ledger.balances.insert((account, asset), amount);
        }
        let n_vault = read_u32(&mut cur)? as usize;
        for _ in 0..n_vault {
            let pool = Hash(read_array::<32>(&mut cur)?);
            let asset = AssetId(Hash(read_array::<32>(&mut cur)?));
            let amount = read_u128(&mut cur)?;
            if amount == 0 || !ledger.meta.contains_key(&asset) {
                return Err(AssetsError::InvalidEncoding);
            }
            ledger.vaults.insert((pool, asset), amount);
        }
        if !cur.is_empty() {
            return Err(AssetsError::InvalidEncoding);
        }
        Ok(ledger)
    }

    /// Carga el ledger desde la base (o vacío si no hay clave).
    #[cfg(feature = "persist")]
    pub fn load(db: &RscDatabase) -> Result<Self, AssetsError> {
        match db
            .get(ColumnFamily::Default, ASSETS_KEY_PREFIX)
            .map_err(|_| AssetsError::Storage)?
        {
            None => Ok(Self::new()),
            Some(value) => Self::from_canonical_bytes(value.as_bytes()),
        }
    }

    /// Programa la escritura del ledger en un lote (sin `write`).
    #[cfg(feature = "persist")]
    pub fn stage(&self, batch: &mut StorageBatch) {
        batch.put(
            ColumnFamily::Default,
            ASSETS_KEY_PREFIX,
            &self.to_canonical_bytes(),
        );
    }

    /// Iterador de metadatos (orden canónico).
    pub fn registered_assets(&self) -> impl Iterator<Item = (AssetId, AssetMetadata)> + '_ {
        self.meta.iter().map(|(k, v)| (*k, *v))
    }

    /// Vista de saldos de usuario (tests / sync AMM).
    pub fn user_balances(&self) -> &BTreeMap<(AccountId, AssetId), u128> {
        &self.balances
    }

    /// Vista de vaults (tests / sync AMM).
    pub fn vault_balances(&self) -> &BTreeMap<(Hash, AssetId), u128> {
        &self.vaults
    }

    /// Sustituye saldos de usuario y vaults (sync desde el motor AMM).
    pub fn replace_balances(
        &mut self,
        balances: BTreeMap<(AccountId, AssetId), u128>,
        vaults: BTreeMap<(Hash, AssetId), u128>,
    ) -> Result<(), AssetsError> {
        for ((_, asset), _) in &balances {
            self.require_registered(asset)?;
        }
        for ((_, asset), _) in &vaults {
            self.require_registered(asset)?;
        }
        self.balances = balances;
        self.vaults = vaults;
        Ok(())
    }

    fn require_registered(&self, asset: &AssetId) -> Result<(), AssetsError> {
        if self.is_registered(asset) {
            Ok(())
        } else {
            Err(AssetsError::AssetNotRegistered)
        }
    }
}

fn read_array<const N: usize>(cur: &mut &[u8]) -> Result<[u8; N], AssetsError> {
    if cur.len() < N {
        return Err(AssetsError::InvalidEncoding);
    }
    let (head, tail) = cur.split_at(N);
    *cur = tail;
    let mut out = [0u8; N];
    out.copy_from_slice(head);
    Ok(out)
}

fn read_u8(cur: &mut &[u8]) -> Result<u8, AssetsError> {
    Ok(read_array::<1>(cur)?[0])
}

fn read_u32(cur: &mut &[u8]) -> Result<u32, AssetsError> {
    Ok(u32::from_be_bytes(read_array::<4>(cur)?))
}

fn read_u128(cur: &mut &[u8]) -> Result<u128, AssetsError> {
    Ok(u128::from_be_bytes(read_array::<16>(cur)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(b: u8) -> AssetId {
        AssetId(Hash([b; 32]))
    }

    fn account(b: u8) -> AccountId {
        AccountId([b; 32])
    }

    #[test]
    fn transfer_and_roundtrip() {
        let mut ledger = AssetLedger::new();
        let a = asset(1);
        ledger.register(a, AssetMetadata::new(6, "AAA")).unwrap();
        ledger.credit(account(1), a, Amount(100)).unwrap();
        ledger
            .transfer(&account(1), account(1), account(2), a, Amount(40))
            .unwrap();
        assert_eq!(ledger.balance_of(&account(1), &a), Amount(60));
        assert_eq!(ledger.balance_of(&account(2), &a), Amount(40));
        let bytes = ledger.to_canonical_bytes();
        let restored = AssetLedger::from_canonical_bytes(&bytes).unwrap();
        assert_eq!(restored, ledger);
    }

    #[test]
    fn vault_transfer_roundtrip_encoding() {
        let mut ledger = AssetLedger::with_native();
        let native = native_asset_id();
        let pool = Hash([9; 32]);
        ledger.credit(account(1), native, Amount(50)).unwrap();
        ledger
            .transfer_to_vault(account(1), pool, native, Amount(20))
            .unwrap();
        assert_eq!(ledger.vault_balance(&pool, &native), Amount(20));
        assert_eq!(ledger.balance_of(&account(1), &native), Amount(30));
        ledger
            .transfer_from_vault(pool, account(2), native, Amount(5))
            .unwrap();
        assert_eq!(ledger.vault_balance(&pool, &native), Amount(15));
        assert_eq!(ledger.balance_of(&account(2), &native), Amount(5));
        let restored = AssetLedger::from_canonical_bytes(&ledger.to_canonical_bytes()).unwrap();
        assert_eq!(restored, ledger);
    }

    #[test]
    fn unauthorized_transfer_rejected() {
        let mut ledger = AssetLedger::with_native();
        let native = native_asset_id();
        ledger.credit(account(1), native, Amount(10)).unwrap();
        assert_eq!(
            ledger.transfer(&account(2), account(1), account(3), native, Amount(1)),
            Err(AssetsError::Unauthorized)
        );
    }

    #[test]
    fn insufficient_balance_and_zero_and_duplicate_register() {
        let mut ledger = AssetLedger::new();
        let a = asset(3);
        ledger.register(a, AssetMetadata::new(8, "BBB")).unwrap();
        assert_eq!(
            ledger.register(a, AssetMetadata::new(8, "BBB")),
            Err(AssetsError::AssetAlreadyRegistered)
        );
        assert_eq!(
            ledger.credit(account(1), a, Amount(0)),
            Err(AssetsError::ZeroAmount)
        );
        ledger.credit(account(1), a, Amount(5)).unwrap();
        assert_eq!(
            ledger.debit(account(1), a, Amount(6)),
            Err(AssetsError::InsufficientBalance)
        );
        assert_eq!(
            ledger.transfer(&account(1), account(1), account(1), a, Amount(1)),
            Err(AssetsError::InvalidTransfer)
        );
    }

    #[test]
    fn unregistered_asset_rejected() {
        let mut ledger = AssetLedger::new();
        let a = asset(4);
        assert_eq!(
            ledger.credit(account(1), a, Amount(1)),
            Err(AssetsError::AssetNotRegistered)
        );
        assert_eq!(ledger.decimals(&a), Err(AssetsError::AssetNotRegistered));
    }

    #[test]
    fn native_asset_stable_and_decimals() {
        let a = native_asset_id();
        let b = native_asset_id();
        assert_eq!(a, b);
        let ledger = AssetLedger::with_native();
        assert_eq!(ledger.decimals(&a).unwrap(), 18);
        assert!(ledger.is_registered(&a));
    }

    #[test]
    fn replace_balances_requires_registered() {
        let mut ledger = AssetLedger::with_native();
        let native = native_asset_id();
        let mut bal = BTreeMap::new();
        bal.insert((account(1), native), 42u128);
        ledger
            .replace_balances(bal, BTreeMap::new())
            .unwrap();
        assert_eq!(ledger.balance_of(&account(1), &native), Amount(42));
        let mut bad = BTreeMap::new();
        bad.insert((account(1), asset(9)), 1u128);
        assert_eq!(
            ledger.replace_balances(bad, BTreeMap::new()),
            Err(AssetsError::AssetNotRegistered)
        );
    }

    #[test]
    fn vault_account_id_deterministic() {
        let pool = Hash([1; 32]);
        let a = asset(2);
        assert_eq!(
            AssetLedger::vault_account_id(&pool, &a),
            AssetLedger::vault_account_id(&pool, &a)
        );
        assert_ne!(
            AssetLedger::vault_account_id(&pool, &a),
            AssetLedger::vault_account_id(&Hash([2; 32]), &a)
        );
    }

    #[cfg(feature = "persist")]
    #[test]
    fn vault_transfer_and_persist() {
        use rsc_storage::DatabaseConfig;
        use tempfile::tempdir;

        let dir = tempdir().unwrap();
        let db = RscDatabase::open(
            dir.path(),
            DatabaseConfig {
                max_value_size: 1 << 20,
                network_id: "test".into(),
                chain_id: "test".into(),
            },
        )
        .unwrap();
        let mut ledger = AssetLedger::with_native();
        let native = native_asset_id();
        let pool = Hash([9; 32]);
        ledger.credit(account(1), native, Amount(50)).unwrap();
        ledger
            .transfer_to_vault(account(1), pool, native, Amount(20))
            .unwrap();
        let mut batch = StorageBatch::new();
        ledger.stage(&mut batch);
        db.write(batch).unwrap();
        let loaded = AssetLedger::load(&db).unwrap();
        assert_eq!(loaded.vault_balance(&pool, &native), Amount(20));
        assert_eq!(loaded.balance_of(&account(1), &native), Amount(30));
    }
}
