//! Persistencia del subestado AMM en `rsc-storage`.

use crate::accounting::AmmAccounting;
use crate::events::AmmEvent;
use crate::fees::FeeTracker;
use crate::pool::{PoolPolicy, PoolRegistry};
use crate::AmmError;
use rsc_storage::{ColumnFamily, RscDatabase, StorageBatch};

/// Claves físicas del subestado AMM (CF Default).
pub struct AmmStoreKeys;

impl AmmStoreKeys {
    /// Registro de pools.
    pub const POOLS: &'static [u8] = b"RSC/AMM/V1/POOLS";
    /// LP shares + ops anti-replay.
    pub const ACCOUNTING: &'static [u8] = b"RSC/AMM/V1/ACCOUNTING";
    /// Métricas de fee.
    pub const FEES: &'static [u8] = b"RSC/AMM/V1/FEES";
    /// Autoridades de gobernanza (lista canónica de AccountId).
    pub const GOVERNANCE: &'static [u8] = b"RSC/AMM/V1/GOVERNANCE";
}

/// Alias documentado.
pub const AMM_STORE_KEYS: AmmStoreKeys = AmmStoreKeys;

/// Prefijo de eventos AMM en CF Receipts: `RSC/AMM/V1/EVENTS/{tx_id}`.
pub const AMM_EVENTS_KEY_PREFIX: &[u8] = b"RSC/AMM/V1/EVENTS/";

/// Vista cargada del subestado AMM.
#[derive(Clone, Debug)]
pub struct AmmStore {
    /// Pools.
    pub pools: PoolRegistry,
    /// LP + ops (saldos de activo viven en `rsc-assets`).
    pub accounting: AmmAccounting,
    /// Fees acumulados.
    pub fees: FeeTracker,
    /// Cuentas autorizadas a pause/activate.
    pub governors: Vec<rsc_core::AccountId>,
}

impl Default for AmmStore {
    fn default() -> Self {
        Self {
            pools: PoolRegistry::new(PoolPolicy::default()),
            accounting: AmmAccounting::new(),
            fees: FeeTracker::new(),
            governors: Vec::new(),
        }
    }
}

impl AmmStore {
    /// Carga desde la base. Claves ausentes → subestado vacío.
    pub fn load(db: &RscDatabase) -> Result<Self, AmmError> {
        let pools = match db_get(db, AmmStoreKeys::POOLS)? {
            None => PoolRegistry::new(PoolPolicy::default()),
            Some(bytes) => PoolRegistry::from_canonical_bytes(&bytes)?,
        };
        let accounting = match db_get(db, AmmStoreKeys::ACCOUNTING)? {
            None => AmmAccounting::new(),
            Some(bytes) => AmmAccounting::from_canonical_bytes(&bytes)?,
        };
        let fees = match db_get(db, AmmStoreKeys::FEES)? {
            None => FeeTracker::new(),
            Some(bytes) => FeeTracker::from_canonical_bytes(&bytes)?,
        };
        let governors = match db_get(db, AmmStoreKeys::GOVERNANCE)? {
            None => Vec::new(),
            Some(bytes) => decode_governors(&bytes)?,
        };
        Ok(Self {
            pools,
            accounting,
            fees,
            governors,
        })
    }

    /// Programa escrituras AMM en el lote (sin `write`).
    pub fn stage(&self, batch: &mut StorageBatch) {
        batch.put(
            ColumnFamily::Default,
            AmmStoreKeys::POOLS,
            &self.pools.to_canonical_bytes(),
        );
        batch.put(
            ColumnFamily::Default,
            AmmStoreKeys::ACCOUNTING,
            &self.accounting.to_canonical_bytes(),
        );
        batch.put(
            ColumnFamily::Default,
            AmmStoreKeys::FEES,
            &self.fees.to_canonical_bytes(),
        );
        batch.put(
            ColumnFamily::Default,
            AmmStoreKeys::GOVERNANCE,
            &encode_governors(&self.governors),
        );
    }

    /// Emite eventos de una tx confirmada a CF Receipts.
    pub fn stage_events(
        batch: &mut StorageBatch,
        tx_id: &[u8; 32],
        events: &[AmmEvent],
    ) -> Result<(), AmmError> {
        let mut key = AMM_EVENTS_KEY_PREFIX.to_vec();
        key.extend_from_slice(tx_id);
        let mut payload = Vec::new();
        payload.extend_from_slice(&(events.len() as u32).to_be_bytes());
        for event in events {
            let json = event.to_json()?;
            let bytes = json.as_bytes();
            payload.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
            payload.extend_from_slice(bytes);
        }
        batch.put(ColumnFamily::Receipts, &key, &payload);
        Ok(())
    }
}

fn db_get(db: &RscDatabase, key: &[u8]) -> Result<Option<Vec<u8>>, AmmError> {
    db.get(ColumnFamily::Default, key)
        .map(|opt| opt.map(|v| v.as_bytes().to_vec()))
        .map_err(|_| AmmError::InvalidEncoding)
}

fn encode_governors(governors: &[rsc_core::AccountId]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(governors.len() as u32).to_be_bytes());
    for g in governors {
        out.extend_from_slice(&g.0);
    }
    out
}

fn decode_governors(bytes: &[u8]) -> Result<Vec<rsc_core::AccountId>, AmmError> {
    if bytes.len() < 4 {
        return Err(AmmError::InvalidEncoding);
    }
    let n = u32::from_be_bytes(bytes[0..4].try_into().unwrap()) as usize;
    if bytes.len() != 4 + n * 32 {
        return Err(AmmError::InvalidEncoding);
    }
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let start = 4 + i * 32;
        out.push(rsc_core::AccountId(
            bytes[start..start + 32].try_into().unwrap(),
        ));
    }
    Ok(out)
}
