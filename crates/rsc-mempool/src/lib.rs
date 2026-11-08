//! Mempool en memoria de transacciones pendientes.
//!
//! Guarda transacciones recibidas y admitidas que todavía no están en un
//! bloque. No ejecuta, no confirma, no crea bloques y no participa en
//! consenso.
//!
//! ```text
//! aceptada  ≠ confirmada
//! ready     ≠ finalizada
//! orden (sender, nonce, hash) ≠ orden canónico de bloque
//! TTL operacional ≠ expiración protocolaria
//! ```
//!
//! Un reinicio del nodo vacía el pool. Eso no borra estado de cadena: solo
//! las transacciones incluidas en bloques son canónicas. El mismo payload
//! puede volver a entrar por RPC o P2P si el validador sigue aceptándolo.
//!
//! ```
//! use rsc_mempool::MempoolConfig;
//! let config = MempoolConfig::default();
//! assert!(config.max_transactions > 0);
//! assert!(config.max_total_bytes >= config.max_transaction_size);
//! ```

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, Instant};

use rsc_core::{AccountId, Nonce};

/// Tope operacional por defecto del número de transacciones. No es consenso.
pub const DEFAULT_MAX_TRANSACTIONS: usize = 4_096;
/// Tope operacional por defecto de transacciones de una misma cuenta.
pub const DEFAULT_MAX_TRANSACTIONS_PER_SENDER: usize = 64;
/// Tope operacional por defecto del tamaño de una transacción, en bytes.
pub const DEFAULT_MAX_TRANSACTION_SIZE: usize = 128 * 1024;
/// Tope operacional por defecto de la suma de payloads, en bytes.
pub const DEFAULT_MAX_TOTAL_BYTES: usize = 16 * 1024 * 1024;
/// TTL operacional por defecto, en segundos. No viaja en el hash ni en el estado.
pub const DEFAULT_TRANSACTION_TTL_SECS: u64 = 15 * 60;

/// Identidad primaria de una transacción pendiente.
///
/// Es [`rsc_core::TransactionId`]. El digest lo produce el validador (con
/// `rsc-crypto` si corresponde). Este crate no calcula SHA-256.
pub type TransactionHash = rsc_core::TransactionId;

/// Error de admisión o de contabilidad del pool.
///
/// Los mensajes no incluyen rutas, claves ni trazas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MempoolError {
    /// El mismo [`TransactionHash`] ya está almacenado.
    #[error("transaction is already in the mempool")]
    AlreadyKnown,
    /// Ya hay otra transacción pendiente con el mismo sender y nonce.
    ///
    /// V1 no reemplaza. No hay fee bump.
    #[error("sender nonce is already pending")]
    NonceConflict,
    /// `max_transactions` está alcanzado.
    #[error("mempool transaction limit reached")]
    PoolFull,
    /// `max_transactions_per_sender` está alcanzado.
    #[error("sender transaction limit reached")]
    SenderPoolFull,
    /// El payload supera `max_transaction_size`.
    #[error("transaction exceeds max size")]
    TransactionTooLarge,
    /// La suma de payloads superaría `max_total_bytes`.
    #[error("mempool byte limit reached")]
    PoolBytesExceeded,
    /// El validador rechazó la transacción.
    #[error("transaction validation failed")]
    ValidationFailed,
    /// El validador no pudo ejecutarse. La transacción no se admite.
    #[error("transaction validator is unavailable")]
    ValidationUnavailable,
    /// Bytes mal formados o identidad incoherente con el payload recibido.
    #[error("transaction is invalid")]
    InvalidTransaction,
    /// El TTL operacional venció.
    ///
    /// La expulsión normal informa un conteo, no este error por transacción.
    #[error("transaction expired")]
    TransactionExpired,
    /// No hay una entrada con ese hash.
    #[error("transaction was not found")]
    NotFound,
    /// Índices o aritmética interna inconsistentes.
    #[error("mempool internal inconsistency")]
    Internal,
}

/// Error producido por el adaptador de transacciones, no por los límites del pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MempoolValidationError {
    /// La transacción no decodifica o está mal formada.
    #[error("transaction is invalid")]
    InvalidTransaction,
    /// La transacción decodifica pero no es válida frente al protocolo o al estado.
    #[error("transaction validation failed")]
    ValidationFailed,
    /// El validador no está disponible. No implica que la transacción sea válida.
    #[error("transaction validator is unavailable")]
    ValidationUnavailable,
}

/// Fallo al leer el nonce canónico de una cuenta.
///
/// El nonce canónico pertenece al estado, no al mempool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum NonceLookupError {
    /// El estado no pudo responder. Esa cuenta no aporta transacciones ready.
    #[error("account nonce is unavailable")]
    Unavailable,
}

/// Adapta bytes crudos a la identidad que el pool necesita.
///
/// El validador decodifica, calcula el hash, extrae sender y nonce y aplica
/// la validación de protocolo (firma incluida). El pool no firma, no ejecuta
/// WASM y no lee la base de estado.
///
/// Durante `validate` el pool no sostiene su lock. El validador puede
/// reentrar en lecturas del pool. No debe asumir que la transacción ya fue
/// admitida.
pub trait MempoolValidator: Send + Sync {
    /// Convierte `raw_transaction` en una transacción de mempool.
    ///
    /// `raw_transaction` del resultado tiene que ser idéntico a la entrada.
    /// El pool descarta el `received_at` devuelto y graba su propio reloj.
    fn validate(
        &self,
        raw_transaction: &[u8],
    ) -> Result<MempoolTransaction, MempoolValidationError>;
}

/// Nonce ejecutable actual de una cuenta, según el estado canónico.
///
/// El valor es el menor nonce que esa cuenta puede ejecutar ahora. No es un
/// contador de transacciones pendientes.
pub trait PendingNonceProvider: Send + Sync {
    /// Nonce de estado de `sender`.
    fn current_nonce(&self, sender: &AccountId) -> Result<Nonce, NonceLookupError>;
}

/// Reloj monótono para el TTL operacional.
///
/// No es `SystemTime` y no entra en la identidad de la transacción.
pub trait MonotonicClock: Send + Sync {
    /// Instante operacional actual.
    fn now(&self) -> Instant;
}

/// Reloj de [`Instant::now`].
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl MonotonicClock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// Límites operacionales del nodo.
///
/// Dos nodos pueden usar topes distintos sin ejecutar consensos distintos.
/// Estos números no entran en `StateRoot`, `BlockHash` ni en las reglas de
/// consenso. No expresan fee, gas ni prioridad económica.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MempoolConfig {
    /// Máximo de transacciones en el pool.
    pub max_transactions: usize,
    /// Máximo de transacciones pendientes de un mismo sender.
    pub max_transactions_per_sender: usize,
    /// Máximo de bytes de una transacción cruda.
    pub max_transaction_size: usize,
    /// Máximo de la suma de `raw_transaction.len()`.
    pub max_total_bytes: usize,
    /// Tiempo máximo en el pool antes de `evict_expired`.
    pub transaction_ttl: Duration,
}

impl Default for MempoolConfig {
    fn default() -> Self {
        Self {
            max_transactions: DEFAULT_MAX_TRANSACTIONS,
            max_transactions_per_sender: DEFAULT_MAX_TRANSACTIONS_PER_SENDER,
            max_transaction_size: DEFAULT_MAX_TRANSACTION_SIZE,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
            transaction_ttl: Duration::from_secs(DEFAULT_TRANSACTION_TTL_SECS),
        }
    }
}

/// Ocupación operacional. `Full` no significa consenso detenido.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MempoolStatus {
    /// No hay transacciones pendientes.
    Empty,
    /// Hay transacciones y todavía cabe al menos el criterio global de conteo y bytes.
    NonEmpty,
    /// El conteo global o el tope de bytes está alcanzado.
    Full,
}

/// Vista de solo lectura para RPC y monitoreo.
///
/// No incluye payloads ni claves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MempoolInfo {
    /// Transacciones almacenadas.
    pub transaction_count: usize,
    /// Suma de `raw_transaction.len()`.
    pub total_bytes: usize,
    /// Transacciones ready respecto del nonce de estado actual.
    pub ready_count: usize,
    /// Copia de [`MempoolConfig::max_transactions`].
    pub max_transactions: usize,
    /// Copia de [`MempoolConfig::max_total_bytes`].
    pub max_total_bytes: usize,
    /// Copia de [`MempoolConfig::max_transactions_per_sender`].
    pub max_transactions_per_sender: usize,
}

/// Transacción admitida. El payload crudo no se reescribe.
///
/// `received_at` es metadata local para el TTL. No es parte del hash, del
/// estado ni del consenso. [`PartialEq`] lo incluye: igualdad de struct no es
/// igualdad protocolaria. La identidad protocolaria es `hash`.
#[derive(Clone, PartialEq, Eq)]
pub struct MempoolTransaction {
    /// Identidad primaria.
    pub hash: TransactionHash,
    /// Cuenta que origina la transacción. No es un `PeerId`.
    pub sender: AccountId,
    /// Nonce declarado por la transacción. El pool no lo asigna.
    pub nonce: Nonce,
    /// Bytes exactos recibidos.
    pub raw_transaction: Vec<u8>,
    /// Momento local de admisión.
    pub received_at: Instant,
}

impl std::fmt::Debug for MempoolTransaction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MempoolTransaction")
            .field("hash", &self.hash)
            .field("sender", &self.sender)
            .field("nonce", &self.nonce)
            .field("raw_len", &self.raw_transaction.len())
            .field("received_at", &self.received_at)
            .finish()
    }
}

/// Clasificación local. Nunca significa confirmada ni finalizada.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionQueueState {
    /// El nonce forma una cadena continua desde el nonce de estado.
    Ready,
    /// Está almacenada, pero un hueco de nonce impide ejecutarla ahora.
    Pending,
}

/// Resultado local de [`Mempool::revalidate`].
///
/// No es un resultado de consenso ni una transición de estado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevalidationResult {
    /// Hashes eliminados por dejar de ser válidos, ordenados por hash.
    pub removed: Vec<TransactionHash>,
    /// Entradas del snapshot que siguen en el pool.
    pub retained: usize,
    /// Transacciones que el validador revisó.
    pub checked: usize,
}

/// Pool concurrente de transacciones pendientes.
///
/// Varios hilos (RPC, P2P, armado de bloque, revalidación) pueden usarlo.
/// La validación externa ocurre sin el lock. Bajo el lock se vuelven a
/// comprobar duplicado, nonce y capacidad antes de insertar.
///
/// El orden de admisión no es el orden de selección. La selección ready usa
/// `(sender, nonce, hash)` sobre los bytes de [`AccountId`] y del hash.
/// No usa llegada, aleatoriedad ni fee.
pub struct Mempool {
    config: MempoolConfig,
    validator: Arc<dyn MempoolValidator>,
    nonces: Arc<dyn PendingNonceProvider>,
    clock: Arc<dyn MonotonicClock>,
    inner: RwLock<MempoolInner>,
}

impl Mempool {
    /// Pool con reloj de sistema.
    ///
    /// `nonces` aporta el nonce canónico. El pool no lo persiste.
    pub fn new(
        config: MempoolConfig,
        validator: Arc<dyn MempoolValidator>,
        nonces: Arc<dyn PendingNonceProvider>,
    ) -> Self {
        Self::with_clock(config, validator, nonces, Arc::new(SystemClock))
    }

    /// Igual que [`Self::new`] con un reloj inyectable.
    ///
    /// El reloj solo afecta al TTL. No ordena transacciones.
    pub fn with_clock(
        config: MempoolConfig,
        validator: Arc<dyn MempoolValidator>,
        nonces: Arc<dyn PendingNonceProvider>,
        clock: Arc<dyn MonotonicClock>,
    ) -> Self {
        Self {
            config,
            validator,
            nonces,
            clock,
            inner: RwLock::new(MempoolInner::default()),
        }
    }

    /// Admite `raw_transaction`.
    ///
    /// Orden: tamaño, validador, y ya con el lock: duplicado, conflicto de
    /// nonce, capacidad, inserción. Aceptar significa pendiente local, no
    /// confirmada. V1 no reemplaza un sender+nonce existente.
    pub fn insert(&self, raw_transaction: Vec<u8>) -> Result<TransactionHash, MempoolError> {
        if raw_transaction.len() > self.config.max_transaction_size {
            return Err(MempoolError::TransactionTooLarge);
        }
        let mut validated = self
            .validator
            .validate(&raw_transaction)
            .map_err(map_validation)?;
        if validated.raw_transaction.as_slice() != raw_transaction.as_slice() {
            return Err(MempoolError::InvalidTransaction);
        }
        validated.raw_transaction = raw_transaction;
        validated.received_at = self.clock.now();
        let hash = validated.hash;
        admit(&mut self.write(), &self.config, validated)?;
        Ok(hash)
    }

    /// Copia de la transacción, si sigue pendiente.
    pub fn get(&self, hash: &TransactionHash) -> Option<MempoolTransaction> {
        self.read()
            .by_hash
            .get(hash)
            .map(|entry| entry.transaction.clone())
    }

    /// `true` si el hash está almacenado.
    pub fn contains(&self, hash: &TransactionHash) -> bool {
        self.read().by_hash.contains_key(hash)
    }

    /// Quita una transacción y limpia todos los índices.
    ///
    /// `None` si no estaba. No revierte un bloque.
    pub fn remove(&self, hash: &TransactionHash) -> Option<MempoolTransaction> {
        match remove_locked(&mut self.write(), hash) {
            Ok(transaction) => transaction,
            Err(MempoolError::Internal) => None,
            Err(_) => None,
        }
    }

    /// Quita las transacciones incluidas en un bloque ya aceptado por la capa de cadena.
    ///
    /// Devuelve cuántas entradas existían y fueron eliminadas. Los hashes
    /// desconocidos se ignoran. No elimina por orden de llegada.
    pub fn remove_committed(&self, hashes: &[TransactionHash]) -> usize {
        let mut guard = self.write();
        let mut removed = 0usize;
        for hash in hashes {
            if let Ok(Some(_)) = remove_locked(&mut guard, hash) {
                removed = saturating_inc(removed);
            }
        }
        removed
    }

    /// Transacciones del sender, ordenadas por nonce y luego por hash.
    ///
    /// No modifica el pool. Incluye pending y ready.
    pub fn get_sender_transactions(&self, sender: &AccountId) -> Vec<MempoolTransaction> {
        let guard = self.read();
        let Some(nonces) = guard.by_sender.get(sender) else {
            return Vec::new();
        };
        let mut txs = Vec::new();
        for hash in nonces.values() {
            if let Some(entry) = guard.by_hash.get(hash) {
                txs.push(entry.transaction.clone());
            }
        }
        txs.sort_by_key(ordering_key);
        txs
    }

    /// Cuántas transacciones pendientes tiene `sender`.
    pub fn sender_transaction_count(&self, sender: &AccountId) -> usize {
        match self.read().by_sender.get(sender) {
            Some(nonces) => nonces.len(),
            None => 0,
        }
    }

    /// Hasta `limit` transacciones ready, en orden determinista.
    ///
    /// Solo lectura. Un hueco de nonce excluye el resto de esa cuenta.
    /// `limit == 0` devuelve un vector vacío.
    pub fn ready_transactions(&self, limit: usize) -> Vec<MempoolTransaction> {
        let mut ready = self.ready_all();
        ready.truncate(limit);
        ready
    }

    /// Candidatas para un bloque. No las elimina.
    ///
    /// Recorre solo transacciones ready en orden `(sender, nonce, hash)` y
    /// se detiene al alcanzar `max_count` o cuando la siguiente no cabe en
    /// `max_bytes`. No reordena por tamaño, llegada ni fee. El armado del
    /// bloque decide la inclusión real.
    pub fn select_for_block(&self, max_count: usize, max_bytes: usize) -> Vec<MempoolTransaction> {
        let mut selected = Vec::new();
        let mut used = 0usize;
        for tx in self.ready_all() {
            if selected.len() >= max_count {
                break;
            }
            let Some(next) = used.checked_add(tx.raw_transaction.len()) else {
                break;
            };
            if next > max_bytes {
                break;
            }
            used = next;
            selected.push(tx);
        }
        selected
    }

    /// Elimina transacciones cuyo TTL operacional venció.
    ///
    /// Devuelve cuántas eliminó. Una transacción expirada puede volver a
    /// admitirse si llega de nuevo y el validador la acepta. No hay lista
    /// negra por expiración.
    pub fn evict_expired(&self) -> usize {
        let now = self.clock.now();
        let ttl = self.config.transaction_ttl;
        let mut guard = self.write();
        let expired = expired_hashes(&guard, now, ttl);
        let mut removed = 0usize;
        for hash in expired {
            if let Ok(Some(_)) = remove_locked(&mut guard, &hash) {
                removed = saturating_inc(removed);
            }
        }
        removed
    }

    /// Vuelve a validar las pendientes y elimina las que ya no son válidas.
    ///
    /// No modifica hashes, senders, nonces ni bytes. No modifica estado de
    /// cadena. Si el validador responde [`MempoolValidationError::ValidationUnavailable`],
    /// la transacción se conserva.
    pub fn revalidate(&self) -> Result<RevalidationResult, MempoolError> {
        let snapshot = self.snapshot_txs();
        let checked = snapshot.len();
        let mut decisions = Vec::with_capacity(checked);
        for tx in snapshot {
            let drop = match self.validator.validate(&tx.raw_transaction) {
                Ok(checked_tx) => {
                    checked_tx.hash != tx.hash
                        || checked_tx.sender != tx.sender
                        || checked_tx.nonce != tx.nonce
                        || checked_tx.raw_transaction != tx.raw_transaction
                }
                Err(MempoolValidationError::ValidationUnavailable) => false,
                Err(
                    MempoolValidationError::InvalidTransaction
                    | MempoolValidationError::ValidationFailed,
                ) => true,
            };
            decisions.push(RevalidationItem {
                hash: tx.hash,
                received_at: tx.received_at,
                drop,
            });
        }

        let mut removed = Vec::new();
        let mut retained = 0usize;
        {
            let mut guard = self.write();
            for item in &decisions {
                let still = matches!(
                    guard.by_hash.get(&item.hash),
                    Some(entry) if entry.transaction.received_at == item.received_at
                );
                if !still {
                    continue;
                }
                if item.drop {
                    match remove_locked(&mut guard, &item.hash) {
                        Ok(Some(_)) => removed.push(item.hash),
                        Ok(None) => {}
                        Err(err) => return Err(err),
                    }
                } else {
                    retained = saturating_inc(retained);
                }
            }
        }
        removed.sort();
        Ok(RevalidationResult {
            removed,
            retained,
            checked,
        })
    }

    /// Número de transacciones almacenadas.
    pub fn len(&self) -> usize {
        self.read().by_hash.len()
    }

    /// `true` si no hay transacciones.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Suma de bytes lógicos de los payloads (`len`, no `capacity`).
    pub fn total_bytes(&self) -> usize {
        self.read().total_bytes
    }

    /// Conteos operacionales, incluida la clasificación ready actual.
    pub fn info(&self) -> MempoolInfo {
        let (txs, total_bytes) = self.snapshot_with_bytes();
        let transaction_count = txs.len();
        let ready_count = collect_ready(txs, self.nonces.as_ref()).len();
        MempoolInfo {
            transaction_count,
            total_bytes,
            ready_count,
            max_transactions: self.config.max_transactions,
            max_total_bytes: self.config.max_total_bytes,
            max_transactions_per_sender: self.config.max_transactions_per_sender,
        }
    }

    /// `Empty`, `NonEmpty` o `Full` según conteo y bytes globales.
    ///
    /// El tope por sender no marca el pool entero como `Full`.
    pub fn status(&self) -> MempoolStatus {
        let guard = self.read();
        if guard.by_hash.len() >= self.config.max_transactions
            || guard.total_bytes >= self.config.max_total_bytes
        {
            MempoolStatus::Full
        } else if guard.by_hash.is_empty() {
            MempoolStatus::Empty
        } else {
            MempoolStatus::NonEmpty
        }
    }

    /// `Ready` o `Pending` para un hash presente. `None` si no está.
    pub fn queue_state(&self, hash: &TransactionHash) -> Option<TransactionQueueState> {
        let txs = self.snapshot_txs();
        let present = txs.iter().any(|tx| tx.hash == *hash);
        if !present {
            return None;
        }
        let ready = collect_ready(txs, self.nonces.as_ref());
        if ready.iter().any(|tx| tx.hash == *hash) {
            Some(TransactionQueueState::Ready)
        } else {
            Some(TransactionQueueState::Pending)
        }
    }

    fn ready_all(&self) -> Vec<MempoolTransaction> {
        collect_ready(self.snapshot_txs(), self.nonces.as_ref())
    }

    fn snapshot_txs(&self) -> Vec<MempoolTransaction> {
        self.snapshot_with_bytes().0
    }

    fn snapshot_with_bytes(&self) -> (Vec<MempoolTransaction>, usize) {
        let guard = self.read();
        let txs = guard
            .by_hash
            .values()
            .map(|entry| entry.transaction.clone())
            .collect();
        (txs, guard.total_bytes)
    }

    fn read(&self) -> RwLockReadGuard<'_, MempoolInner> {
        match self.inner.read() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    fn write(&self) -> RwLockWriteGuard<'_, MempoolInner> {
        match self.inner.write() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    #[cfg(test)]
    fn check(&self) -> Result<(), &'static str> {
        check_invariants(&self.read(), &self.config)
    }
}

/// Entrada local. V2 puede colgar aquí metadata de prioridad sin reescribir el payload.
struct PendingEntry {
    transaction: MempoolTransaction,
}

#[derive(Default)]
struct MempoolInner {
    by_hash: HashMap<TransactionHash, PendingEntry>,
    by_sender_nonce: BTreeMap<(AccountId, u64), TransactionHash>,
    by_sender: BTreeMap<AccountId, BTreeMap<u64, TransactionHash>>,
    total_bytes: usize,
}

struct RevalidationItem {
    hash: TransactionHash,
    received_at: Instant,
    drop: bool,
}

fn map_validation(err: MempoolValidationError) -> MempoolError {
    match err {
        MempoolValidationError::InvalidTransaction => MempoolError::InvalidTransaction,
        MempoolValidationError::ValidationFailed => MempoolError::ValidationFailed,
        MempoolValidationError::ValidationUnavailable => MempoolError::ValidationUnavailable,
    }
}

fn ordering_key(tx: &MempoolTransaction) -> (AccountId, u64, TransactionHash) {
    (tx.sender, tx.nonce.0, tx.hash)
}

fn saturating_inc(value: usize) -> usize {
    match value.checked_add(1) {
        Some(next) => next,
        None => value,
    }
}

fn admit(
    inner: &mut MempoolInner,
    config: &MempoolConfig,
    transaction: MempoolTransaction,
) -> Result<(), MempoolError> {
    let hash = transaction.hash;
    let sender = transaction.sender;
    let nonce = transaction.nonce.0;
    let size = transaction.raw_transaction.len();

    if inner.by_hash.contains_key(&hash) {
        return Err(MempoolError::AlreadyKnown);
    }
    if inner.by_sender_nonce.contains_key(&(sender, nonce)) {
        return Err(MempoolError::NonceConflict);
    }
    if inner.by_hash.len() >= config.max_transactions {
        return Err(MempoolError::PoolFull);
    }
    let sender_len = match inner.by_sender.get(&sender) {
        Some(nonces) => nonces.len(),
        None => 0,
    };
    if sender_len >= config.max_transactions_per_sender {
        return Err(MempoolError::SenderPoolFull);
    }
    if size > config.max_transaction_size {
        return Err(MempoolError::TransactionTooLarge);
    }
    let new_total = match inner.total_bytes.checked_add(size) {
        Some(total) => total,
        None => return Err(MempoolError::Internal),
    };
    if new_total > config.max_total_bytes {
        return Err(MempoolError::PoolBytesExceeded);
    }

    inner.by_hash.insert(hash, PendingEntry { transaction });
    inner.by_sender_nonce.insert((sender, nonce), hash);
    inner
        .by_sender
        .entry(sender)
        .or_default()
        .insert(nonce, hash);
    inner.total_bytes = new_total;
    Ok(())
}

fn remove_locked(
    inner: &mut MempoolInner,
    hash: &TransactionHash,
) -> Result<Option<MempoolTransaction>, MempoolError> {
    let Some(entry) = inner.by_hash.get(hash) else {
        return Ok(None);
    };
    let sender = entry.transaction.sender;
    let nonce = entry.transaction.nonce.0;
    let size = entry.transaction.raw_transaction.len();
    let nonce_ok = inner.by_sender_nonce.get(&(sender, nonce)) == Some(hash);
    let sender_ok = match inner.by_sender.get(&sender) {
        Some(nonces) => nonces.get(&nonce) == Some(hash),
        None => false,
    };
    if !nonce_ok || !sender_ok {
        return Err(MempoolError::Internal);
    }
    let new_total = match inner.total_bytes.checked_sub(size) {
        Some(total) => total,
        None => return Err(MempoolError::Internal),
    };

    let Some(entry) = inner.by_hash.remove(hash) else {
        return Err(MempoolError::Internal);
    };
    inner.by_sender_nonce.remove(&(sender, nonce));
    let empty = match inner.by_sender.get_mut(&sender) {
        Some(nonces) => {
            nonces.remove(&nonce);
            nonces.is_empty()
        }
        None => false,
    };
    if empty {
        inner.by_sender.remove(&sender);
    }
    inner.total_bytes = new_total;
    Ok(Some(entry.transaction))
}

fn expired_hashes(inner: &MempoolInner, now: Instant, ttl: Duration) -> BTreeSet<TransactionHash> {
    let mut expired = BTreeSet::new();
    for (hash, entry) in &inner.by_hash {
        if now.saturating_duration_since(entry.transaction.received_at) >= ttl {
            expired.insert(*hash);
        }
    }
    expired
}

fn collect_ready(
    txs: Vec<MempoolTransaction>,
    nonces: &dyn PendingNonceProvider,
) -> Vec<MempoolTransaction> {
    let mut grouped: BTreeMap<AccountId, BTreeMap<u64, MempoolTransaction>> = BTreeMap::new();
    for tx in txs {
        grouped.entry(tx.sender).or_default().insert(tx.nonce.0, tx);
    }
    let mut ready = Vec::new();
    for (sender, mut pending) in grouped {
        let Ok(mut expected) = nonces.current_nonce(&sender) else {
            continue;
        };
        while let Some(tx) = pending.remove(&expected.0) {
            ready.push(tx);
            match expected.checked_next() {
                Ok(next) => expected = next,
                Err(_) => break,
            }
        }
    }
    ready.sort_by_key(ordering_key);
    ready
}

#[cfg(test)]
fn check_invariants(inner: &MempoolInner, config: &MempoolConfig) -> Result<(), &'static str> {
    if inner.by_hash.len() != inner.by_sender_nonce.len() {
        return Err("hash and sender-nonce lengths differ");
    }
    if inner.by_hash.len() > config.max_transactions {
        return Err("transaction count over limit");
    }
    if inner.total_bytes > config.max_total_bytes {
        return Err("total bytes over limit");
    }
    let mut computed = 0usize;
    let mut sender_counts: BTreeMap<AccountId, usize> = BTreeMap::new();
    for (hash, entry) in &inner.by_hash {
        let tx = &entry.transaction;
        if tx.hash != *hash {
            return Err("hash key mismatch");
        }
        if tx.raw_transaction.len() > config.max_transaction_size {
            return Err("stored transaction over size");
        }
        computed = match computed.checked_add(tx.raw_transaction.len()) {
            Some(total) => total,
            None => return Err("byte sum overflow"),
        };
        let indexed = inner.by_sender_nonce.get(&(tx.sender, tx.nonce.0));
        if indexed != Some(hash) {
            return Err("sender-nonce index missing");
        }
        let sender_hit = match inner.by_sender.get(&tx.sender) {
            Some(nonces) => nonces.get(&tx.nonce.0) == Some(hash),
            None => false,
        };
        if !sender_hit {
            return Err("sender index missing");
        }
        let count = sender_counts.entry(tx.sender).or_insert(0);
        *count = saturating_inc(*count);
    }
    if computed != inner.total_bytes {
        return Err("byte accounting mismatch");
    }
    for (sender, count) in &sender_counts {
        if *count > config.max_transactions_per_sender {
            return Err("sender over limit");
        }
        let indexed = match inner.by_sender.get(sender) {
            Some(nonces) => nonces.len(),
            None => 0,
        };
        if indexed != *count {
            return Err("sender count mismatch");
        }
    }
    for ((sender, nonce), hash) in &inner.by_sender_nonce {
        let Some(entry) = inner.by_hash.get(hash) else {
            return Err("sender-nonce index dangling");
        };
        if entry.transaction.sender != *sender || entry.transaction.nonce.0 != *nonce {
            return Err("sender-nonce target mismatch");
        }
    }
    let mut indexed_senders = 0usize;
    for (sender, nonces) in &inner.by_sender {
        if nonces.is_empty() {
            return Err("empty sender bucket");
        }
        indexed_senders = match indexed_senders.checked_add(nonces.len()) {
            Some(total) => total,
            None => return Err("sender index overflow"),
        };
        for (nonce, hash) in nonces {
            let Some(entry) = inner.by_hash.get(hash) else {
                return Err("sender index dangling");
            };
            if entry.transaction.sender != *sender || entry.transaction.nonce.0 != *nonce {
                return Err("sender index target mismatch");
            }
            if inner.by_sender_nonce.get(&(*sender, *nonce)) != Some(hash) {
                return Err("sender index not in nonce index");
            }
        }
    }
    if indexed_senders != inner.by_hash.len() {
        return Err("sender index size");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use rsc_core::Hash;
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    use std::sync::Mutex;
    use std::thread;

    const HEADER_LEN: usize = 73;
    const KIND_OK: u8 = 0;
    const KIND_FAIL: u8 = 1;
    const KIND_UNAVAILABLE: u8 = 2;
    const KIND_BAD: u8 = 3;

    struct Gate {
        deny: Mutex<BTreeSet<TransactionHash>>,
        min_nonce: Mutex<BTreeMap<AccountId, u64>>,
        calls: AtomicUsize,
        mutate_raw: AtomicBool,
        force_unavailable: AtomicBool,
    }

    struct MapNonces {
        values: Mutex<BTreeMap<AccountId, Nonce>>,
        default_nonce: Nonce,
    }

    struct ManualClock {
        origin: Instant,
        offset_ms: AtomicU64,
    }

    struct Env {
        pool: Mempool,
        nonces: Arc<MapNonces>,
        clock: Arc<ManualClock>,
        validator: Arc<Gate>,
    }

    fn account(byte: u8) -> AccountId {
        AccountId([byte; 32])
    }

    fn txid(byte: u8) -> TransactionHash {
        TransactionHash::from(Hash([byte; 32]))
    }

    fn encode(
        kind: u8,
        sender: AccountId,
        nonce: u64,
        hash: TransactionHash,
        extra: usize,
    ) -> Vec<u8> {
        let mut raw = Vec::with_capacity(HEADER_LEN + extra);
        raw.push(kind);
        raw.extend_from_slice(&sender.0);
        raw.extend_from_slice(&nonce.to_le_bytes());
        raw.extend_from_slice(&hash.0 .0);
        raw.resize(HEADER_LEN + extra, 0x11);
        raw
    }

    fn raw(sender: u8, nonce: u64, hash: u8, extra: usize) -> Vec<u8> {
        encode(KIND_OK, account(sender), nonce, txid(hash), extra)
    }

    fn small_config() -> MempoolConfig {
        MempoolConfig {
            max_transactions: 32,
            max_transactions_per_sender: 8,
            max_transaction_size: 1024,
            max_total_bytes: 16 * 1024,
            transaction_ttl: Duration::from_secs(60),
        }
    }

    fn inserted(pool: &Mempool, bytes: Vec<u8>) -> TransactionHash {
        let outcome = pool.insert(bytes);
        assert!(outcome.is_ok());
        match outcome {
            Ok(hash) => hash,
            Err(_) => txid(0),
        }
    }

    fn consistent(pool: &Mempool) {
        assert_eq!(pool.check(), Ok(()));
    }

    fn keys(txs: &[MempoolTransaction]) -> Vec<(AccountId, u64, TransactionHash)> {
        txs.iter().map(ordering_key).collect()
    }

    impl Gate {
        fn new() -> Self {
            Self {
                deny: Mutex::new(BTreeSet::new()),
                min_nonce: Mutex::new(BTreeMap::new()),
                calls: AtomicUsize::new(0),
                mutate_raw: AtomicBool::new(false),
                force_unavailable: AtomicBool::new(false),
            }
        }

        fn deny(&self, hash: TransactionHash) {
            if let Ok(mut guard) = self.deny.lock() {
                guard.insert(hash);
            }
        }

        fn set_min_nonce(&self, sender: AccountId, nonce: u64) {
            if let Ok(mut guard) = self.min_nonce.lock() {
                guard.insert(sender, nonce);
            }
        }
    }

    impl MempoolValidator for Gate {
        fn validate(
            &self,
            raw_transaction: &[u8],
        ) -> Result<MempoolTransaction, MempoolValidationError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            if self.force_unavailable.load(Ordering::Relaxed) {
                return Err(MempoolValidationError::ValidationUnavailable);
            }
            if raw_transaction.len() < HEADER_LEN {
                return Err(MempoolValidationError::InvalidTransaction);
            }
            let kind = raw_transaction[0];
            let sender = match AccountId::from_slice(&raw_transaction[1..33]) {
                Ok(sender) => sender,
                Err(_) => return Err(MempoolValidationError::InvalidTransaction),
            };
            let nonce_bytes: [u8; 8] = match raw_transaction[33..41].try_into() {
                Ok(bytes) => bytes,
                Err(_) => return Err(MempoolValidationError::InvalidTransaction),
            };
            let nonce = Nonce(u64::from_le_bytes(nonce_bytes));
            let hash = match TransactionHash::from_slice(&raw_transaction[41..73]) {
                Ok(hash) => hash,
                Err(_) => return Err(MempoolValidationError::InvalidTransaction),
            };
            match kind {
                KIND_OK => {}
                KIND_FAIL => return Err(MempoolValidationError::ValidationFailed),
                KIND_UNAVAILABLE => return Err(MempoolValidationError::ValidationUnavailable),
                _ => return Err(MempoolValidationError::InvalidTransaction),
            }
            if self.mutate_raw.load(Ordering::Relaxed) {
                let mut owned = raw_transaction.to_vec();
                owned.push(0x5A);
                return Ok(MempoolTransaction {
                    hash,
                    sender,
                    nonce,
                    raw_transaction: owned,
                    received_at: Instant::now(),
                });
            }
            if let Ok(mins) = self.min_nonce.lock() {
                if let Some(min) = mins.get(&sender) {
                    if nonce.0 < *min {
                        return Err(MempoolValidationError::ValidationFailed);
                    }
                }
            }
            if let Ok(denied) = self.deny.lock() {
                if denied.contains(&hash) {
                    return Err(MempoolValidationError::ValidationFailed);
                }
            }
            Ok(MempoolTransaction {
                hash,
                sender,
                nonce,
                raw_transaction: raw_transaction.to_vec(),
                received_at: Instant::now(),
            })
        }
    }

    impl MapNonces {
        fn new(default_nonce: Nonce) -> Self {
            Self {
                values: Mutex::new(BTreeMap::new()),
                default_nonce,
            }
        }

        fn set(&self, sender: AccountId, nonce: Nonce) -> bool {
            match self.values.lock() {
                Ok(mut guard) => {
                    guard.insert(sender, nonce);
                    true
                }
                Err(_) => false,
            }
        }
    }

    impl PendingNonceProvider for MapNonces {
        fn current_nonce(&self, sender: &AccountId) -> Result<Nonce, NonceLookupError> {
            let guard = match self.values.lock() {
                Ok(guard) => guard,
                Err(_) => return Err(NonceLookupError::Unavailable),
            };
            match guard.get(sender) {
                Some(nonce) => Ok(*nonce),
                None => Ok(self.default_nonce),
            }
        }
    }

    impl ManualClock {
        fn new() -> Self {
            Self {
                origin: Instant::now(),
                offset_ms: AtomicU64::new(0),
            }
        }

        fn advance(&self, duration: Duration) {
            let millis = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
            self.offset_ms.fetch_add(millis, Ordering::Relaxed);
        }
    }

    impl MonotonicClock for ManualClock {
        fn now(&self) -> Instant {
            self.origin + Duration::from_millis(self.offset_ms.load(Ordering::Relaxed))
        }
    }

    impl Env {
        fn new(config: MempoolConfig) -> Self {
            let validator = Arc::new(Gate::new());
            let nonces = Arc::new(MapNonces::new(Nonce::ZERO));
            let clock = Arc::new(ManualClock::new());
            let pool = Mempool::with_clock(
                config,
                Arc::clone(&validator) as Arc<dyn MempoolValidator>,
                Arc::clone(&nonces) as Arc<dyn PendingNonceProvider>,
                Arc::clone(&clock) as Arc<dyn MonotonicClock>,
            );
            Self {
                pool,
                nonces,
                clock,
                validator,
            }
        }
    }

    #[test]
    fn test_mempool_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Mempool>();
    }

    #[test]
    fn test_new_mempool() {
        let validator = Arc::new(Gate::new());
        let nonces = Arc::new(MapNonces::new(Nonce::ZERO));
        let pool = Mempool::new(
            MempoolConfig::default(),
            validator,
            nonces as Arc<dyn PendingNonceProvider>,
        );
        let config = MempoolConfig::default();
        assert!(config.max_transactions > 0);
        assert!(config.max_transactions_per_sender > 0);
        assert!(config.max_transaction_size > 0);
        assert!(config.max_total_bytes >= config.max_transaction_size);
        assert!(config.transaction_ttl > Duration::ZERO);
        assert!(pool.is_empty());
        assert_eq!(pool.len(), 0);
        assert_eq!(pool.total_bytes(), 0);
        assert_eq!(pool.status(), MempoolStatus::Empty);
        assert!(pool.ready_transactions(10).is_empty());
        consistent(&pool);
    }

    #[test]
    fn test_empty_mempool() {
        let env = Env::new(small_config());
        assert!(env.pool.is_empty());
        assert_eq!(env.pool.len(), 0);
        assert_eq!(env.pool.total_bytes(), 0);
        assert!(env.pool.get(&txid(1)).is_none());
        assert!(!env.pool.contains(&txid(1)));
        assert!(env.pool.remove(&txid(1)).is_none());
        assert!(env.pool.ready_transactions(4).is_empty());
        assert!(env.pool.select_for_block(4, 10_000).is_empty());
        assert_eq!(env.pool.evict_expired(), 0);
        assert_eq!(env.pool.info().transaction_count, 0);
        assert_eq!(env.pool.status(), MempoolStatus::Empty);
        consistent(&env.pool);
    }

    #[test]
    fn test_insert() {
        let env = Env::new(small_config());
        let bytes = raw(1, 0, 1, 4);
        let hash = inserted(&env.pool, bytes.clone());
        assert_eq!(hash, txid(1));
        assert_eq!(env.pool.len(), 1);
        assert_eq!(env.pool.status(), MempoolStatus::NonEmpty);
        let stored = env.pool.get(&hash);
        assert!(stored.is_some());
        let Some(stored) = stored else {
            return;
        };
        assert_eq!(stored.raw_transaction, bytes);
        assert_eq!(stored.sender, account(1));
        assert_eq!(stored.nonce, Nonce(0));
        consistent(&env.pool);
    }

    #[test]
    fn test_get() {
        let env = Env::new(small_config());
        let bytes = raw(2, 0, 2, 0);
        let hash = inserted(&env.pool, bytes.clone());
        assert_eq!(env.pool.get(&hash), env.pool.get(&hash));
        let Some(stored) = env.pool.get(&hash) else {
            assert!(env.pool.get(&hash).is_some());
            return;
        };
        assert_eq!(stored.raw_transaction, bytes);
        assert!(env.pool.get(&txid(9)).is_none());
    }

    #[test]
    fn test_contains() {
        let env = Env::new(small_config());
        let hash = inserted(&env.pool, raw(1, 0, 3, 0));
        assert!(env.pool.contains(&hash));
        assert!(!env.pool.contains(&txid(4)));
    }

    #[test]
    fn test_remove() {
        let env = Env::new(small_config());
        let bytes = raw(1, 0, 5, 2);
        let hash = inserted(&env.pool, bytes.clone());
        let removed = env.pool.remove(&hash);
        assert!(removed.is_some());
        let Some(removed) = removed else {
            return;
        };
        assert_eq!(removed.raw_transaction, bytes);
        assert!(env.pool.remove(&hash).is_none());
        assert!(!env.pool.contains(&hash));
        assert_eq!(env.pool.len(), 0);
        assert_eq!(env.pool.total_bytes(), 0);
        assert_eq!(env.pool.sender_transaction_count(&account(1)), 0);
        consistent(&env.pool);
    }

    #[test]
    fn test_len() {
        let env = Env::new(small_config());
        assert_eq!(env.pool.len(), 0);
        inserted(&env.pool, raw(1, 0, 1, 0));
        inserted(&env.pool, raw(1, 1, 2, 0));
        assert_eq!(env.pool.len(), 2);
    }

    #[test]
    fn test_is_empty() {
        let env = Env::new(small_config());
        assert!(env.pool.is_empty());
        inserted(&env.pool, raw(1, 0, 1, 0));
        assert!(!env.pool.is_empty());
    }

    #[test]
    fn test_total_bytes() {
        let env = Env::new(small_config());
        assert_eq!(env.pool.total_bytes(), 0);
        let first = raw(1, 0, 1, 3);
        let second = raw(1, 1, 2, 5);
        let expected = first.len() + second.len();
        inserted(&env.pool, first);
        inserted(&env.pool, second);
        assert_eq!(env.pool.total_bytes(), expected);
        assert_eq!(env.pool.info().total_bytes, expected);
    }

    #[test]
    fn test_duplicate_hash() {
        let env = Env::new(small_config());
        let bytes = raw(1, 0, 1, 0);
        inserted(&env.pool, bytes.clone());
        assert_eq!(env.pool.insert(bytes), Err(MempoolError::AlreadyKnown));
    }

    #[test]
    fn test_duplicate_hash_rejected() {
        let env = Env::new(small_config());
        let bytes = raw(1, 0, 7, 1);
        inserted(&env.pool, bytes.clone());
        assert_eq!(env.pool.insert(bytes), Err(MempoolError::AlreadyKnown));
        assert_eq!(env.pool.len(), 1);
        consistent(&env.pool);
    }

    #[test]
    fn test_duplicate_does_not_change_capacity() {
        let env = Env::new(small_config());
        let bytes = raw(1, 0, 1, 0);
        inserted(&env.pool, bytes.clone());
        let before = env.pool.len();
        assert_eq!(env.pool.insert(bytes), Err(MempoolError::AlreadyKnown));
        assert_eq!(env.pool.len(), before);
        assert_eq!(env.pool.sender_transaction_count(&account(1)), 1);
    }

    #[test]
    fn test_duplicate_does_not_change_bytes() {
        let env = Env::new(small_config());
        let bytes = raw(1, 0, 1, 6);
        inserted(&env.pool, bytes.clone());
        let before = env.pool.total_bytes();
        assert_eq!(env.pool.insert(bytes), Err(MempoolError::AlreadyKnown));
        assert_eq!(env.pool.total_bytes(), before);
        assert_eq!(before, HEADER_LEN + 6);
    }

    #[test]
    fn test_same_sender_same_nonce_rejected() {
        let env = Env::new(small_config());
        inserted(&env.pool, raw(1, 10, 1, 0));
        assert_eq!(
            env.pool.insert(raw(1, 10, 2, 0)),
            Err(MempoolError::NonceConflict)
        );
        assert_eq!(env.pool.len(), 1);
        assert!(env.pool.contains(&txid(1)));
        assert!(!env.pool.contains(&txid(2)));
        consistent(&env.pool);
    }

    #[test]
    fn test_same_sender_different_nonce() {
        let env = Env::new(small_config());
        inserted(&env.pool, raw(1, 0, 1, 0));
        inserted(&env.pool, raw(1, 1, 2, 0));
        assert_eq!(env.pool.len(), 2);
        assert_eq!(env.pool.sender_transaction_count(&account(1)), 2);
        consistent(&env.pool);
    }

    #[test]
    fn test_different_senders_same_nonce() {
        let env = Env::new(small_config());
        inserted(&env.pool, raw(1, 10, 1, 0));
        inserted(&env.pool, raw(2, 10, 2, 0));
        assert_eq!(env.pool.len(), 2);
        assert!(env.pool.contains(&txid(1)));
        assert!(env.pool.contains(&txid(2)));
    }

    #[test]
    fn test_multiple_pending_nonces() {
        let env = Env::new(small_config());
        inserted(&env.pool, raw(1, 2, 3, 0));
        inserted(&env.pool, raw(1, 0, 1, 0));
        inserted(&env.pool, raw(1, 1, 2, 0));
        assert_eq!(env.pool.sender_transaction_count(&account(1)), 3);
        let txs = env.pool.get_sender_transactions(&account(1));
        assert_eq!(
            txs.iter().map(|tx| tx.nonce.0).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
    }

    #[test]
    fn test_max_transactions() {
        let mut config = small_config();
        config.max_transactions = 3;
        config.max_transactions_per_sender = 10;
        let env = Env::new(config);
        for index in 0..3u64 {
            inserted(&env.pool, raw(1, index, (index + 1) as u8, 0));
        }
        assert_eq!(env.pool.status(), MempoolStatus::Full);
        assert_eq!(
            env.pool.insert(raw(1, 3, 4, 0)),
            Err(MempoolError::PoolFull)
        );
        assert_eq!(env.pool.len(), 3);
        consistent(&env.pool);
    }

    #[test]
    fn test_sender_limit() {
        let mut config = small_config();
        config.max_transactions = 20;
        config.max_transactions_per_sender = 2;
        let env = Env::new(config);
        inserted(&env.pool, raw(1, 0, 1, 0));
        inserted(&env.pool, raw(1, 1, 2, 0));
        assert_eq!(
            env.pool.insert(raw(1, 2, 3, 0)),
            Err(MempoolError::SenderPoolFull)
        );
        assert_eq!(env.pool.sender_transaction_count(&account(1)), 2);
        inserted(&env.pool, raw(2, 0, 4, 0));
        assert_eq!(env.pool.len(), 3);
        consistent(&env.pool);
    }

    #[test]
    fn test_max_transaction_size() {
        let mut config = small_config();
        config.max_transaction_size = 10;
        let env = Env::new(config);
        let before = env.validator.calls.load(Ordering::Relaxed);
        assert_eq!(
            env.pool.insert(raw(1, 0, 1, 0)),
            Err(MempoolError::TransactionTooLarge)
        );
        assert_eq!(env.validator.calls.load(Ordering::Relaxed), before);
        assert!(env.pool.is_empty());
        assert_eq!(env.pool.total_bytes(), 0);
    }

    #[test]
    fn test_total_bytes_limit() {
        let sample = raw(1, 0, 1, 0);
        let mut config = small_config();
        config.max_transactions = 10;
        config.max_total_bytes = sample.len() * 2;
        let env = Env::new(config);
        inserted(&env.pool, raw(1, 0, 1, 0));
        inserted(&env.pool, raw(1, 1, 2, 0));
        assert_eq!(
            env.pool.insert(raw(1, 2, 3, 0)),
            Err(MempoolError::PoolBytesExceeded)
        );
        assert_eq!(env.pool.len(), 2);
        assert_eq!(env.pool.total_bytes(), sample.len() * 2);
        assert_eq!(env.pool.status(), MempoolStatus::Full);
        consistent(&env.pool);
    }

    #[test]
    fn test_deterministic_order() {
        let tx_a = raw(1, 0, 1, 0);
        let tx_b = raw(1, 1, 2, 0);
        let tx_c = raw(2, 0, 3, 0);
        let pool_a = Env::new(small_config());
        inserted(&pool_a.pool, tx_c.clone());
        inserted(&pool_a.pool, tx_a.clone());
        inserted(&pool_a.pool, tx_b.clone());
        let pool_b = Env::new(small_config());
        inserted(&pool_b.pool, tx_b);
        inserted(&pool_b.pool, tx_c);
        inserted(&pool_b.pool, tx_a);
        let ready_a = pool_a.pool.ready_transactions(10);
        let ready_b = pool_b.pool.ready_transactions(10);
        assert_eq!(keys(&ready_a), keys(&ready_b));
        assert_eq!(
            ready_a.iter().map(|tx| tx.hash).collect::<Vec<_>>(),
            vec![txid(1), txid(2), txid(3)]
        );
        assert_eq!(
            ready_a
                .iter()
                .map(|tx| tx.raw_transaction.clone())
                .collect::<Vec<_>>(),
            ready_b
                .iter()
                .map(|tx| tx.raw_transaction.clone())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_sender_order() {
        let env = Env::new(small_config());
        inserted(&env.pool, raw(2, 0, 2, 0));
        inserted(&env.pool, raw(1, 0, 1, 0));
        let ready = env.pool.ready_transactions(10);
        assert_eq!(
            ready.iter().map(|tx| tx.sender).collect::<Vec<_>>(),
            vec![account(1), account(2)]
        );
    }

    #[test]
    fn test_nonce_order() {
        let env = Env::new(small_config());
        inserted(&env.pool, raw(1, 2, 3, 0));
        inserted(&env.pool, raw(1, 0, 1, 0));
        inserted(&env.pool, raw(1, 1, 2, 0));
        let ready = env.pool.ready_transactions(10);
        assert_eq!(
            ready.iter().map(|tx| tx.nonce.0).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
    }

    #[test]
    fn test_hash_order() {
        let low = (account(1), 1u64, txid(1));
        let high = (account(1), 1u64, txid(2));
        let later_nonce = (account(1), 2u64, txid(0));
        let later_sender = (account(2), 0u64, txid(0));
        assert!(low < high);
        assert!(high < later_nonce);
        assert!(later_nonce < later_sender);
        let tx = MempoolTransaction {
            hash: txid(2),
            sender: account(1),
            nonce: Nonce(1),
            raw_transaction: Vec::new(),
            received_at: Instant::now(),
        };
        assert_eq!(ordering_key(&tx), high);
        let env = Env::new(small_config());
        inserted(&env.pool, raw(1, 1, 1, 0));
        assert_eq!(
            env.pool.insert(raw(1, 1, 2, 0)),
            Err(MempoolError::NonceConflict)
        );
    }

    #[test]
    fn test_ready_nonce() {
        let env = Env::new(small_config());
        assert!(env.nonces.set(account(1), Nonce(5)));
        inserted(&env.pool, raw(1, 5, 1, 0));
        let ready = env.pool.ready_transactions(10);
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].nonce, Nonce(5));
        assert_eq!(
            env.pool.queue_state(&txid(1)),
            Some(TransactionQueueState::Ready)
        );
    }

    #[test]
    fn test_nonce_gap() {
        let env = Env::new(small_config());
        assert!(env.nonces.set(account(1), Nonce(5)));
        inserted(&env.pool, raw(1, 7, 7, 0));
        assert!(env.pool.ready_transactions(10).is_empty());
        assert_eq!(
            env.pool.queue_state(&txid(7)),
            Some(TransactionQueueState::Pending)
        );
        inserted(&env.pool, raw(1, 5, 5, 0));
        let ready = env.pool.ready_transactions(10);
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].nonce, Nonce(5));
        assert_eq!(
            env.pool.queue_state(&txid(7)),
            Some(TransactionQueueState::Pending)
        );
    }

    #[test]
    fn test_gap_filled() {
        let env = Env::new(small_config());
        assert!(env.nonces.set(account(1), Nonce(5)));
        inserted(&env.pool, raw(1, 5, 5, 0));
        inserted(&env.pool, raw(1, 7, 7, 0));
        inserted(&env.pool, raw(1, 6, 6, 0));
        let ready = env.pool.ready_transactions(10);
        assert_eq!(
            ready.iter().map(|tx| tx.nonce.0).collect::<Vec<_>>(),
            vec![5, 6, 7]
        );
    }

    #[test]
    fn test_multiple_ready_transactions() {
        let env = Env::new(small_config());
        assert!(env.nonces.set(account(1), Nonce(5)));
        inserted(&env.pool, raw(1, 7, 3, 0));
        inserted(&env.pool, raw(1, 5, 1, 0));
        inserted(&env.pool, raw(1, 6, 2, 0));
        assert_eq!(env.pool.ready_transactions(10).len(), 3);
        assert_eq!(env.pool.info().ready_count, 3);
    }

    #[test]
    fn test_pending_after_gap() {
        let env = Env::new(small_config());
        assert!(env.nonces.set(account(1), Nonce(5)));
        inserted(&env.pool, raw(1, 5, 1, 0));
        inserted(&env.pool, raw(1, 8, 2, 0));
        let ready = env.pool.ready_transactions(10);
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].hash, txid(1));
        assert_eq!(
            env.pool.queue_state(&txid(2)),
            Some(TransactionQueueState::Pending)
        );
    }

    #[test]
    fn test_contiguous_sender_nonce_ready() {
        let env = Env::new(small_config());
        assert!(env.nonces.set(account(1), Nonce(5)));
        inserted(&env.pool, raw(1, 5, 1, 0));
        inserted(&env.pool, raw(1, 6, 2, 0));
        inserted(&env.pool, raw(1, 7, 3, 0));
        assert_eq!(
            env.pool
                .ready_transactions(10)
                .iter()
                .map(|tx| tx.nonce.0)
                .collect::<Vec<_>>(),
            vec![5, 6, 7]
        );
    }

    #[test]
    fn test_nonce_gap_not_ready() {
        let env = Env::new(small_config());
        assert!(env.nonces.set(account(1), Nonce(5)));
        inserted(&env.pool, raw(1, 5, 1, 0));
        inserted(&env.pool, raw(1, 7, 2, 0));
        let ready = env.pool.ready_transactions(10);
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].nonce, Nonce(5));
    }

    #[test]
    fn test_ready_after_gap_filled() {
        let env = Env::new(small_config());
        assert!(env.nonces.set(account(1), Nonce(5)));
        inserted(&env.pool, raw(1, 5, 1, 0));
        inserted(&env.pool, raw(1, 7, 3, 0));
        assert_eq!(env.pool.ready_transactions(10).len(), 1);
        inserted(&env.pool, raw(1, 6, 2, 0));
        assert_eq!(env.pool.ready_transactions(10).len(), 3);
    }

    #[test]
    fn test_select_for_block() {
        let env = Env::new(small_config());
        inserted(&env.pool, raw(2, 0, 2, 0));
        inserted(&env.pool, raw(1, 0, 1, 0));
        inserted(&env.pool, raw(1, 1, 3, 0));
        let selected = env.pool.select_for_block(10, 10_000);
        assert_eq!(
            selected.iter().map(|tx| tx.hash).collect::<Vec<_>>(),
            vec![txid(1), txid(3), txid(2)]
        );
    }

    #[test]
    fn test_selection_max_count() {
        let env = Env::new(small_config());
        inserted(&env.pool, raw(1, 0, 1, 0));
        inserted(&env.pool, raw(1, 1, 2, 0));
        inserted(&env.pool, raw(1, 2, 3, 0));
        let selected = env.pool.select_for_block(2, 10_000);
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].hash, txid(1));
        assert_eq!(selected[1].hash, txid(2));
    }

    #[test]
    fn test_selection_max_bytes() {
        let env = Env::new(small_config());
        let first = raw(1, 0, 1, 10);
        let second = raw(1, 1, 2, 10);
        let third = raw(1, 2, 3, 0);
        let first_len = first.len();
        inserted(&env.pool, first);
        inserted(&env.pool, second);
        inserted(&env.pool, third);
        let selected = env.pool.select_for_block(10, first_len);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].hash, txid(1));
    }

    #[test]
    fn test_selection_does_not_remove() {
        let env = Env::new(small_config());
        inserted(&env.pool, raw(1, 0, 1, 0));
        inserted(&env.pool, raw(1, 1, 2, 0));
        let before = env.pool.len();
        let bytes_before = env.pool.total_bytes();
        let _ = env.pool.select_for_block(1, 10_000);
        assert_eq!(env.pool.len(), before);
        assert_eq!(env.pool.total_bytes(), bytes_before);
        assert!(env.pool.contains(&txid(1)));
        assert!(env.pool.contains(&txid(2)));
    }

    #[test]
    fn test_selection_only_ready_transactions() {
        let env = Env::new(small_config());
        assert!(env.nonces.set(account(1), Nonce(5)));
        inserted(&env.pool, raw(1, 5, 1, 0));
        inserted(&env.pool, raw(1, 7, 2, 0));
        let selected = env.pool.select_for_block(10, 10_000);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].hash, txid(1));
    }

    #[test]
    fn test_remove_committed() {
        let env = Env::new(small_config());
        let bytes = raw(1, 0, 1, 0);
        let hash = inserted(&env.pool, bytes.clone());
        assert_eq!(env.pool.remove_committed(&[hash]), 1);
        assert!(env.pool.is_empty());
        let again = inserted(&env.pool, bytes);
        assert_eq!(again, hash);
        consistent(&env.pool);
    }

    #[test]
    fn test_remove_multiple_committed() {
        let env = Env::new(small_config());
        let a = inserted(&env.pool, raw(1, 0, 1, 0));
        let b = inserted(&env.pool, raw(1, 1, 2, 0));
        let c = inserted(&env.pool, raw(2, 0, 3, 0));
        assert_eq!(env.pool.remove_committed(&[a, b, txid(9)]), 2);
        assert_eq!(env.pool.len(), 1);
        assert!(env.pool.contains(&c));
        consistent(&env.pool);
    }

    #[test]
    fn test_remove_unknown() {
        let env = Env::new(small_config());
        inserted(&env.pool, raw(1, 0, 1, 0));
        assert!(env.pool.remove(&txid(9)).is_none());
        assert_eq!(env.pool.remove_committed(&[txid(8)]), 0);
        assert_eq!(env.pool.len(), 1);
        consistent(&env.pool);
    }

    #[test]
    fn test_indexes_clean_after_remove() {
        let env = Env::new(small_config());
        inserted(&env.pool, raw(1, 0, 1, 0));
        inserted(&env.pool, raw(1, 1, 2, 0));
        inserted(&env.pool, raw(2, 0, 3, 0));
        assert!(env.pool.remove(&txid(2)).is_some());
        assert_eq!(env.pool.sender_transaction_count(&account(1)), 1);
        assert_eq!(
            env.pool
                .get_sender_transactions(&account(1))
                .iter()
                .map(|tx| tx.nonce.0)
                .collect::<Vec<_>>(),
            vec![0]
        );
        consistent(&env.pool);
    }

    #[test]
    fn test_unexpired_transaction() {
        let mut config = small_config();
        config.transaction_ttl = Duration::from_millis(1_000);
        let env = Env::new(config);
        inserted(&env.pool, raw(1, 0, 1, 0));
        env.clock.advance(Duration::from_millis(999));
        assert_eq!(env.pool.evict_expired(), 0);
        assert_eq!(env.pool.len(), 1);
        consistent(&env.pool);
    }

    #[test]
    fn test_expired_transaction() {
        let mut config = small_config();
        config.transaction_ttl = Duration::from_millis(1_000);
        let env = Env::new(config);
        let bytes = raw(1, 0, 1, 0);
        let hash = inserted(&env.pool, bytes.clone());
        env.clock.advance(Duration::from_millis(1_000));
        assert_eq!(env.pool.evict_expired(), 1);
        assert!(env.pool.is_empty());
        let restored = inserted(&env.pool, bytes);
        assert_eq!(restored, hash);
        assert_eq!(env.pool.len(), 1);
    }

    #[test]
    fn test_evict_expired() {
        let mut config = small_config();
        config.transaction_ttl = Duration::from_millis(50);
        let env = Env::new(config);
        inserted(&env.pool, raw(1, 0, 1, 0));
        inserted(&env.pool, raw(1, 1, 2, 0));
        env.clock.advance(Duration::from_millis(49));
        assert_eq!(env.pool.evict_expired(), 0);
        env.clock.advance(Duration::from_millis(1));
        assert_eq!(env.pool.evict_expired(), 2);
        assert!(env.pool.is_empty());
        consistent(&env.pool);
    }

    #[test]
    fn test_expired_removal_updates_bytes() {
        let mut config = small_config();
        config.transaction_ttl = Duration::from_millis(10);
        let env = Env::new(config);
        let bytes = raw(1, 0, 1, 8);
        let len = bytes.len();
        inserted(&env.pool, bytes);
        assert_eq!(env.pool.total_bytes(), len);
        env.clock.advance(Duration::from_millis(10));
        assert_eq!(env.pool.evict_expired(), 1);
        assert_eq!(env.pool.total_bytes(), 0);
    }

    #[test]
    fn test_expired_removal_updates_sender_count() {
        let mut config = small_config();
        config.transaction_ttl = Duration::from_millis(10);
        let env = Env::new(config);
        inserted(&env.pool, raw(1, 0, 1, 0));
        inserted(&env.pool, raw(1, 1, 2, 0));
        assert_eq!(env.pool.sender_transaction_count(&account(1)), 2);
        env.clock.advance(Duration::from_millis(10));
        assert_eq!(env.pool.evict_expired(), 2);
        assert_eq!(env.pool.sender_transaction_count(&account(1)), 0);
        consistent(&env.pool);
    }

    #[test]
    fn test_revalidation_keeps_valid() {
        let env = Env::new(small_config());
        inserted(&env.pool, raw(1, 0, 1, 0));
        inserted(&env.pool, raw(1, 1, 2, 0));
        let result = env.pool.revalidate();
        assert!(result.is_ok());
        let Ok(result) = result else {
            return;
        };
        assert!(result.removed.is_empty());
        assert_eq!(result.retained, 2);
        assert_eq!(result.checked, 2);
        env.validator
            .force_unavailable
            .store(true, Ordering::Relaxed);
        let again = env.pool.revalidate();
        assert!(again.is_ok());
        assert_eq!(env.pool.len(), 2);
        consistent(&env.pool);
    }

    #[test]
    fn test_revalidation_removes_invalid() {
        let env = Env::new(small_config());
        assert!(env.nonces.set(account(1), Nonce(5)));
        inserted(&env.pool, raw(1, 5, 1, 0));
        inserted(&env.pool, raw(1, 6, 2, 0));
        env.validator.set_min_nonce(account(1), 6);
        assert!(env.nonces.set(account(1), Nonce(6)));
        let result = env.pool.revalidate();
        assert!(result.is_ok());
        let Ok(result) = result else {
            return;
        };
        assert_eq!(result.removed, vec![txid(1)]);
        assert!(!env.pool.contains(&txid(1)));
        assert!(env.pool.contains(&txid(2)));
        assert_eq!(
            env.pool
                .ready_transactions(10)
                .iter()
                .map(|tx| tx.hash)
                .collect::<Vec<_>>(),
            vec![txid(2)]
        );
        consistent(&env.pool);
    }

    #[test]
    fn test_revalidation_result() {
        let env = Env::new(small_config());
        inserted(&env.pool, raw(1, 0, 1, 0));
        inserted(&env.pool, raw(1, 1, 2, 0));
        inserted(&env.pool, raw(2, 0, 3, 0));
        env.validator.deny(txid(3));
        env.validator.deny(txid(1));
        let result = env.pool.revalidate();
        assert!(result.is_ok());
        let Ok(result) = result else {
            return;
        };
        assert_eq!(result.checked, 3);
        assert_eq!(result.removed, vec![txid(1), txid(3)]);
        assert_eq!(result.retained, 1);
        assert_eq!(env.pool.len(), 1);
    }

    #[test]
    fn test_revalidation_updates_pool() {
        let env = Env::new(small_config());
        let kept = raw(1, 0, 1, 4);
        let dropped = raw(1, 1, 2, 6);
        let kept_len = kept.len();
        inserted(&env.pool, kept);
        inserted(&env.pool, dropped);
        env.validator.deny(txid(2));
        assert!(env.pool.revalidate().is_ok());
        assert_eq!(env.pool.len(), 1);
        assert_eq!(env.pool.total_bytes(), kept_len);
        assert_eq!(env.pool.sender_transaction_count(&account(1)), 1);
        let stored = env.pool.get(&txid(1));
        assert!(stored.is_some());
        let Some(stored) = stored else {
            return;
        };
        assert_eq!(stored.raw_transaction.len(), kept_len);
        consistent(&env.pool);
    }

    #[test]
    fn test_failed_insert_is_atomic() {
        let mut config = small_config();
        config.max_transactions = 1;
        let env = Env::new(config);
        inserted(&env.pool, raw(1, 0, 1, 3));
        let len_before = env.pool.len();
        let bytes_before = env.pool.total_bytes();
        assert_eq!(
            env.pool.insert(raw(2, 0, 2, 3)),
            Err(MempoolError::PoolFull)
        );
        assert_eq!(
            env.pool.insert(raw(1, 0, 9, 0)),
            Err(MempoolError::NonceConflict)
        );
        assert_eq!(env.pool.len(), len_before);
        assert_eq!(env.pool.total_bytes(), bytes_before);
        assert!(!env.pool.contains(&txid(2)));
        assert!(!env.pool.contains(&txid(9)));
        consistent(&env.pool);
    }

    #[test]
    fn test_malformed_transaction() {
        let env = Env::new(small_config());
        assert_eq!(
            env.pool.insert(vec![0x00, 0x01, 0x02]),
            Err(MempoolError::InvalidTransaction)
        );
        assert_eq!(
            env.pool.insert(encode(KIND_BAD, account(1), 0, txid(1), 0)),
            Err(MempoolError::InvalidTransaction)
        );
        assert_eq!(
            env.pool
                .insert(encode(KIND_FAIL, account(1), 0, txid(1), 0)),
            Err(MempoolError::ValidationFailed)
        );
        assert!(env.pool.is_empty());
        assert_eq!(env.pool.total_bytes(), 0);
        consistent(&env.pool);
    }

    #[test]
    fn test_validation_unavailable() {
        let env = Env::new(small_config());
        assert_eq!(
            env.pool
                .insert(encode(KIND_UNAVAILABLE, account(1), 0, txid(1), 0)),
            Err(MempoolError::ValidationUnavailable)
        );
        env.validator
            .force_unavailable
            .store(true, Ordering::Relaxed);
        assert_eq!(
            env.pool.insert(raw(1, 0, 1, 0)),
            Err(MempoolError::ValidationUnavailable)
        );
        assert!(env.pool.is_empty());
    }

    #[test]
    fn test_validator_raw_mismatch() {
        let env = Env::new(small_config());
        env.validator.mutate_raw.store(true, Ordering::Relaxed);
        assert_eq!(
            env.pool.insert(raw(1, 0, 1, 0)),
            Err(MempoolError::InvalidTransaction)
        );
        assert!(env.pool.is_empty());
        assert_eq!(env.pool.total_bytes(), 0);
    }

    #[test]
    fn test_oversized_transaction() {
        let env = Env::new(small_config());
        let huge = vec![0xABu8; 100_000];
        let calls = env.validator.calls.load(Ordering::Relaxed);
        assert_eq!(
            env.pool.insert(huge),
            Err(MempoolError::TransactionTooLarge)
        );
        assert_eq!(env.validator.calls.load(Ordering::Relaxed), calls);
        assert_eq!(env.pool.total_bytes(), 0);
    }

    #[test]
    fn test_pool_does_not_exceed_limits() {
        let mut config = small_config();
        config.max_transactions = 4;
        config.max_transactions_per_sender = 10;
        config.max_total_bytes = 10_000;
        let env = Env::new(config);
        let mut accepted = 0usize;
        for index in 0..12u64 {
            match env.pool.insert(raw(1, index, (index + 1) as u8, 0)) {
                Ok(_) => accepted = saturating_inc(accepted),
                Err(MempoolError::PoolFull) => break,
                Err(err) => assert_eq!(err, MempoolError::PoolFull),
            }
        }
        assert_eq!(accepted, 4);
        assert_eq!(env.pool.len(), 4);
        assert!(env.pool.total_bytes() <= 10_000);
        assert_eq!(
            env.pool.insert(raw(2, 0, 20, 0)),
            Err(MempoolError::PoolFull)
        );
        consistent(&env.pool);
    }

    #[test]
    fn test_sender_cannot_exhaust_pool() {
        let mut config = small_config();
        config.max_transactions = 10;
        config.max_transactions_per_sender = 2;
        let env = Env::new(config);
        let mut rejected = 0usize;
        for index in 0..6u64 {
            match env.pool.insert(raw(1, index, (index + 1) as u8, 0)) {
                Ok(_) => {}
                Err(MempoolError::SenderPoolFull) => rejected = saturating_inc(rejected),
                Err(err) => assert_eq!(err, MempoolError::SenderPoolFull),
            }
        }
        assert_eq!(env.pool.sender_transaction_count(&account(1)), 2);
        assert!(rejected >= 1);
        inserted(&env.pool, raw(9, 0, 30, 0));
        assert!(env.pool.len() < 10);
        assert_eq!(env.pool.status(), MempoolStatus::NonEmpty);
        consistent(&env.pool);
    }

    #[test]
    fn test_info() {
        let env = Env::new(small_config());
        inserted(&env.pool, raw(1, 0, 1, 2));
        inserted(&env.pool, raw(1, 2, 2, 2));
        let info = env.pool.info();
        assert_eq!(info.transaction_count, 2);
        assert_eq!(info.total_bytes, env.pool.total_bytes());
        assert_eq!(info.ready_count, 1);
        assert_eq!(info.max_transactions, 32);
        assert_eq!(info.max_transactions_per_sender, 8);
        assert_eq!(env.pool.sender_transaction_count(&account(1)), 2);
        assert_eq!(env.pool.len(), info.transaction_count);
        assert!(!env.pool.is_empty());
    }

    #[test]
    fn test_sender_queries() {
        let env = Env::new(small_config());
        inserted(&env.pool, raw(4, 1, 2, 0));
        inserted(&env.pool, raw(4, 0, 1, 0));
        inserted(&env.pool, raw(3, 0, 3, 0));
        let txs = env.pool.get_sender_transactions(&account(4));
        assert_eq!(
            txs.iter()
                .map(|tx| (tx.nonce.0, tx.hash))
                .collect::<Vec<_>>(),
            vec![(0, txid(1)), (1, txid(2))]
        );
        assert!(env.pool.get_sender_transactions(&account(8)).is_empty());
        assert_eq!(env.pool.sender_transaction_count(&account(3)), 1);
        assert_eq!(env.pool.sender_transaction_count(&account(8)), 0);
    }

    #[test]
    fn test_concurrent_duplicate_insert() {
        let env = Env::new(small_config());
        let bytes = raw(1, 0, 1, 4);
        let results = Mutex::new(Vec::new());
        thread::scope(|scope| {
            for _ in 0..8 {
                let bytes = bytes.clone();
                let pool = &env.pool;
                let results = &results;
                scope.spawn(move || {
                    let outcome = pool.insert(bytes);
                    if let Ok(mut slot) = results.lock() {
                        slot.push(outcome);
                    }
                });
            }
        });
        let Ok(results) = results.lock() else {
            assert!(results.lock().is_ok());
            return;
        };
        assert_eq!(results.len(), 8);
        let mut accepted = 0usize;
        let mut known = 0usize;
        for item in results.iter() {
            match item {
                Ok(hash) => {
                    assert_eq!(*hash, txid(1));
                    accepted = saturating_inc(accepted);
                }
                Err(MempoolError::AlreadyKnown) => known = saturating_inc(known),
                Err(err) => assert_eq!(*err, MempoolError::AlreadyKnown),
            }
        }
        assert_eq!(accepted, 1);
        assert_eq!(known, 7);
        assert_eq!(env.pool.len(), 1);
        assert_eq!(env.pool.total_bytes(), bytes.len());
        consistent(&env.pool);
    }

    #[test]
    fn test_concurrent_same_nonce() {
        let env = Env::new(small_config());
        let left = raw(1, 4, 1, 0);
        let right = raw(1, 4, 2, 0);
        let results = Mutex::new(Vec::new());
        thread::scope(|scope| {
            for index in 0..8 {
                let bytes = if index % 2 == 0 {
                    left.clone()
                } else {
                    right.clone()
                };
                let pool = &env.pool;
                let results = &results;
                scope.spawn(move || {
                    let outcome = pool.insert(bytes);
                    if let Ok(mut slot) = results.lock() {
                        slot.push(outcome);
                    }
                });
            }
        });
        let Ok(results) = results.lock() else {
            assert!(results.lock().is_ok());
            return;
        };
        let mut accepted = 0usize;
        let mut conflicts = 0usize;
        for item in results.iter() {
            match item {
                Ok(_) => accepted = saturating_inc(accepted),
                Err(MempoolError::NonceConflict | MempoolError::AlreadyKnown) => {
                    conflicts = saturating_inc(conflicts);
                }
                Err(err) => assert_eq!(*err, MempoolError::NonceConflict),
            }
        }
        assert_eq!(accepted, 1);
        assert_eq!(accepted + conflicts, results.len());
        assert!(conflicts >= 1);
        assert_eq!(env.pool.len(), 1);
        assert_eq!(env.pool.sender_transaction_count(&account(1)), 1);
        consistent(&env.pool);
    }

    #[test]
    fn test_concurrent_reads() {
        let env = Env::new(small_config());
        inserted(&env.pool, raw(1, 0, 1, 0));
        thread::scope(|scope| {
            for _ in 0..4 {
                let pool = &env.pool;
                scope.spawn(move || {
                    for _ in 0..40 {
                        let _ = pool.len();
                        let _ = pool.is_empty();
                        let _ = pool.total_bytes();
                        let _ = pool.contains(&txid(1));
                        let _ = pool.get(&txid(1));
                        let _ = pool.ready_transactions(5);
                        let _ = pool.info();
                        let _ = pool.get_sender_transactions(&account(1));
                    }
                });
            }
            let pool = &env.pool;
            scope.spawn(move || {
                for index in 1..6u64 {
                    let _ = pool.insert(raw(1, index, (index + 1) as u8, 0));
                }
            });
        });
        assert!(!env.pool.is_empty());
        consistent(&env.pool);
    }

    #[test]
    fn test_concurrent_insert_and_remove() {
        let env = Env::new(small_config());
        thread::scope(|scope| {
            for index in 0..6u64 {
                let pool = &env.pool;
                scope.spawn(move || {
                    let _ = pool.insert(raw(1, index, (index + 1) as u8, index as usize));
                });
            }
            let pool = &env.pool;
            scope.spawn(move || {
                for hash in 1..4u8 {
                    let _ = pool.remove(&txid(hash));
                }
            });
        });
        assert!(env.pool.len() <= 6);
        assert!(env.pool.sender_transaction_count(&account(1)) <= 8);
        assert!(env.pool.total_bytes() <= small_config().max_total_bytes);
        consistent(&env.pool);
    }

    #[test]
    fn test_consistency_after_mixed_ops() {
        let mut config = small_config();
        config.max_transactions = 8;
        config.max_transactions_per_sender = 4;
        config.transaction_ttl = Duration::from_millis(1_000);
        config.max_total_bytes = 4_000;
        let env = Env::new(config);
        let mut state = 0x1234_5678u64;
        for _ in 0..80 {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            match state % 5 {
                0 | 1 => {
                    let sender = (state % 3) as u8 + 1;
                    let nonce = (state / 7) % 6;
                    let hash = ((state / 11) % 20) as u8 + 1;
                    let extra = (state % 5) as usize;
                    let _ = env.pool.insert(raw(sender, nonce, hash, extra));
                }
                2 => {
                    let hash = txid(((state % 20) as u8).wrapping_add(1));
                    let _ = env.pool.remove(&hash);
                }
                3 => {
                    let hash = txid(((state % 20) as u8).wrapping_add(1));
                    let _ = env.pool.remove_committed(&[hash]);
                }
                _ => {
                    env.clock.advance(Duration::from_millis(1_000));
                    let _ = env.pool.evict_expired();
                }
            }
            consistent(&env.pool);
            let info = env.pool.info();
            assert!(info.transaction_count <= info.max_transactions);
            assert!(info.total_bytes <= info.max_total_bytes);
            assert!(info.ready_count <= info.transaction_count);
        }
    }

    #[derive(Clone, Debug)]
    enum Op {
        Insert(u8, u64, u8, usize),
        Remove(u8),
        Evict,
    }

    fn op_strategy() -> impl Strategy<Value = Op> {
        prop_oneof![
            (0u8..3, 0u64..5, 1u8..12, 0usize..12)
                .prop_map(|(sender, nonce, hash, extra)| Op::Insert(sender, nonce, hash, extra)),
            (0u8..12).prop_map(Op::Remove),
            Just(Op::Evict),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(32))]
        #[test]
        fn prop_insert_remove_consistency(ops in proptest::collection::vec(op_strategy(), 0..24)) {
            let mut config = small_config();
            config.max_transactions = 10;
            config.max_transactions_per_sender = 3;
            config.max_transaction_size = 80;
            config.max_total_bytes = 500;
            config.transaction_ttl = Duration::from_secs(60);
            let env = Env::new(config);
            for op in ops {
                match op {
                    Op::Insert(sender, nonce, hash, extra) => {
                        let outcome = env.pool.insert(raw(sender + 1, nonce, hash, extra));
                        if let Err(err) = outcome {
                            prop_assert!(matches!(
                                err,
                                MempoolError::AlreadyKnown
                                    | MempoolError::NonceConflict
                                    | MempoolError::PoolFull
                                    | MempoolError::SenderPoolFull
                                    | MempoolError::TransactionTooLarge
                                    | MempoolError::PoolBytesExceeded
                            ));
                        }
                    }
                    Op::Remove(index) => {
                        let txs = env.pool.snapshot_txs();
                        if !txs.is_empty() {
                            let idx = usize::from(index) % txs.len();
                            let _ = env.pool.remove(&txs[idx].hash);
                        }
                    }
                    Op::Evict => {
                        env.clock.advance(Duration::from_secs(61));
                        let _ = env.pool.evict_expired();
                    }
                }
                prop_assert_eq!(env.pool.check(), Ok(()));
                let info = env.pool.info();
                prop_assert!(info.transaction_count <= info.max_transactions);
                prop_assert!(info.total_bytes <= info.max_total_bytes);
                prop_assert!(info.ready_count <= info.transaction_count);
                for sender in 1..4u8 {
                    prop_assert!(
                        env.pool.sender_transaction_count(&account(sender))
                            <= info.max_transactions_per_sender
                    );
                }
            }
        }
    }
}
