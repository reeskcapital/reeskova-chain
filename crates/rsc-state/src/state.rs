//! Motor de estado: lecturas, transiciones atómicas y Sparse Merkle Tree.
//!
//! El árbol vive en este archivo. No se expone una API para escribir nodos
//! ni para asignar una raíz a mano. La raíz se deriva del estado lógico.

use crate::account::{derive_account_id, Account, AccountId};
use crate::{
    domain_hash, StateError, ACCOUNT_LEAF_DOMAIN, ACCOUNT_NAMESPACE, META_NAMESPACE,
    SMT_EMPTY_LEAF_DOMAIN, SMT_NAMESPACE, SMT_NODE_DOMAIN, STATE_SCHEMA_V1, TREE_DEPTH,
};
use core::fmt;
use rsc_crypto::KeyId;
use rsc_storage::{ColumnFamily, RscDatabase, StorageBatch};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

/// Raíz del estado lógico. 32 bytes, distinta de un hash de bloque o de transacción.
///
/// Depende solo de las cuentas canónicas. No incluye la versión, el reloj ni
/// la disposición física de RocksDB.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StateRoot([u8; 32]);

impl StateRoot {
    /// Raíz del árbol vacío. Constante de protocolo.
    pub fn empty() -> Self {
        Self(empty_hashes()[0])
    }

    /// Rechaza longitudes distintas de 32.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, StateError> {
        let array: [u8; 32] = bytes.try_into().map_err(|_| StateError::Serialization)?;
        Ok(Self(array))
    }

    /// Vista de los 32 bytes.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Copia de los 32 bytes.
    pub const fn to_bytes(self) -> [u8; 32] {
        self.0
    }
}

impl fmt::Debug for StateRoot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "StateRoot({:02x}{:02x}..)", self.0[0], self.0[1])
    }
}

impl From<StateRoot> for rsc_core::StateRoot {
    fn from(value: StateRoot) -> Self {
        rsc_core::StateRoot(rsc_core::Hash(value.0))
    }
}

impl From<rsc_core::StateRoot> for StateRoot {
    fn from(value: rsc_core::StateRoot) -> Self {
        Self(value.0 .0)
    }
}

/// Cuenta que el génesis puede crear con balance inicial.
///
/// El identificador se deriva del propietario. No se acepta un id arbitrario.
/// Esta es la única vía v1 para un balance distinto de cero en una cuenta nueva.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GenesisAccount {
    owner_key_id: KeyId,
    initial_balance: u128,
}

impl GenesisAccount {
    /// Construye una entrada de génesis. No escribe en la base.
    pub fn new(owner_key_id: KeyId, initial_balance: u128) -> Self {
        Self {
            owner_key_id,
            initial_balance,
        }
    }

    /// Propietario del que se deriva la cuenta.
    pub fn owner_key_id(self) -> KeyId {
        self.owner_key_id
    }

    /// Balance inicial exacto.
    pub fn initial_balance(self) -> u128 {
        self.initial_balance
    }

    /// Identificador derivado. No es un campo independiente.
    pub fn account_id(self) -> AccountId {
        derive_account_id(&self.owner_key_id)
    }
}

/// Operación de protocolo admitida en v1.
///
/// No hay crédito, débito ni asignación libre de balance o nonce.
/// Crear valor fuera del génesis no es una operación de este módulo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StateOperation {
    /// Crea una cuenta con balance 0 y nonce 0.
    CreateAccount {
        /// Propietario. El id se deriva de esta clave.
        owner_key_id: KeyId,
    },
    /// Mueve valor nativo y consume el nonce del emisor.
    Transfer {
        /// Cuenta que envía. Debe existir.
        from: AccountId,
        /// Cuenta que recibe. Debe existir. No puede ser `from`.
        to: AccountId,
        /// Importe estrictamente positivo.
        amount: u128,
        /// Nonce que debe coincidir con el del emisor en ese punto de la transición.
        expected_nonce: u64,
    },
    /// Consume el nonce del emisor sin mover balance nativo.
    ///
    /// Usado por instrucciones AMM (y otras cargas no-transfer) para
    /// protección anti-replay alineada con el nonce de cuenta de `rsc-state`.
    ConsumeNonce {
        /// Cuenta firmante. Debe existir.
        account: AccountId,
        /// Nonce que debe coincidir con el actual.
        expected_nonce: u64,
    },
    /// Transferencia nativa con comisión de protocolo ya partida.
    ///
    /// El emisor paga `amount + validator_fee + burn_fee`. El receptor recibe
    /// `amount`. `validator_fee` se acredita a `fee_pool`. `burn_fee` se
    /// destruye (no tiene receptor). La proporción 30/70 la calcula la capa
    /// monetaria; este enum solo mueve enteros.
    TransferWithFee {
        /// Cuenta que envía. Debe existir.
        from: AccountId,
        /// Cuenta que recibe el importe. Debe existir. No puede ser `from`.
        to: AccountId,
        /// Importe de transferencia estrictamente positivo.
        amount: u128,
        /// Parte de comisión para validadores (puede ser cero).
        validator_fee: u128,
        /// Parte de comisión quemada (puede ser cero).
        burn_fee: u128,
        /// Reserva de fees de protocolo. Debe existir si hay comisión.
        fee_pool: AccountId,
        /// Nonce que debe coincidir con el del emisor.
        expected_nonce: u64,
    },
}

/// Conjunto ordenado de operaciones contra una raíz y una versión conocidas.
///
/// Si la raíz o la versión ya no son las actuales, la transición se rechaza
/// sin escribir. Las operaciones ven los efectos de las anteriores del mismo
/// lote. El commit es único.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateTransition {
    expected_previous_root: StateRoot,
    expected_previous_version: u64,
    operations: Vec<StateOperation>,
}

impl StateTransition {
    /// Transición vacía anclada al estado que el llamador cree actual.
    pub fn new(expected_previous_root: StateRoot, expected_previous_version: u64) -> Self {
        Self {
            expected_previous_root,
            expected_previous_version,
            operations: Vec::new(),
        }
    }

    /// Añade una operación al final. El orden es significativo.
    pub fn push(&mut self, operation: StateOperation) {
        self.operations.push(operation);
    }

    /// Raíz contra la que se valida la transición.
    pub fn expected_previous_root(&self) -> StateRoot {
        self.expected_previous_root
    }

    /// Versión contra la que se valida la transición.
    pub fn expected_previous_version(&self) -> u64 {
        self.expected_previous_version
    }

    /// Operaciones en el orden de aplicación.
    pub fn operations(&self) -> &[StateOperation] {
        &self.operations
    }
}

/// Resultado de un commit: raíces y versiones anterior y nueva.
///
/// La capa superior puede comprobar que la raíz obtenida es la que esperaba.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StateCommitment {
    previous_root: StateRoot,
    new_root: StateRoot,
    previous_version: u64,
    new_version: u64,
}

impl StateCommitment {
    /// Raíz antes del commit.
    pub fn previous_root(self) -> StateRoot {
        self.previous_root
    }

    /// Raíz derivada del estado resultante.
    pub fn new_root(self) -> StateRoot {
        self.new_root
    }

    /// Versión antes del commit.
    pub fn previous_version(self) -> u64 {
        self.previous_version
    }

    /// Versión escrita en el mismo lote que la raíz nueva.
    pub fn new_version(self) -> u64 {
        self.new_version
    }
}

/// Vista lógica de solo lectura.
///
/// Copia la raíz y la versión, y retiene un snapshot físico. No aplica
/// transiciones ni modifica balances.
pub struct StateSnapshot<'a> {
    root: StateRoot,
    version: u64,
    storage: &'a RscDatabase,
}

impl<'a> StateSnapshot<'a> {
    /// Raíz en el instante de la vista.
    pub fn state_root(&self) -> StateRoot {
        self.root
    }

    /// Versión en el instante de la vista. No forma parte de la raíz.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Lee una cuenta sin crearla y sin alterar el estado vivo.
    pub fn get_account(&self, account_id: &AccountId) -> Result<Option<Account>, StateError> {
        match db_get(self.storage, &account_storage_key(account_id))? {
            None => Ok(None),
            Some(bytes) => Ok(Some(Account::from_bytes(&bytes)?)),
        }
    }
}

impl fmt::Debug for StateSnapshot<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StateSnapshot")
            .field("root", &self.root)
            .field("version", &self.version)
            .finish()
    }
}

/// Estado lógico respaldado por `rsc-storage`.
///
/// Las mutaciones pasan por [`Self::initialize_genesis`] o
/// [`Self::apply_transition`]. Ambas validan el cambio completo y lo
/// escriben en un solo lote. Una transición fallida no publica nada.
pub struct State {
    db: Arc<RscDatabase>,
    write_lock: Mutex<()>,
}

impl State {
    /// Abre el estado sobre una base ya abierta.
    ///
    /// Si no hay metadatos, escribe esquema 1, raíz vacía, versión 0 y
    /// `initialized = false` en un solo lote. Unos metadatos parciales o de
    /// otro esquema producen error y no se rellenan con valores inventados.
    pub fn open(database: Arc<RscDatabase>) -> Result<Self, StateError> {
        let state = Self {
            db: database,
            write_lock: Mutex::new(()),
        };
        state.ensure_meta()?;
        Ok(state)
    }

    /// Raíz persistida. No recorre el árbol.
    pub fn state_root(&self) -> Result<StateRoot, StateError> {
        Ok(self.require_meta()?.root)
    }

    /// Versión lógica persistida. No entra en el cálculo de la raíz.
    pub fn version(&self) -> Result<u64, StateError> {
        Ok(self.require_meta()?.version)
    }

    /// Decodifica una cuenta. `Ok(None)` si la clave no existe.
    ///
    /// No crea la cuenta y no modifica nonce, balance, raíz ni versión.
    /// Un valor corrupto es un error, no una ausencia.
    pub fn get_account(&self, account_id: &AccountId) -> Result<Option<Account>, StateError> {
        match db_get(&self.db, &account_storage_key(account_id))? {
            None => Ok(None),
            Some(bytes) => Ok(Some(Account::from_bytes(&bytes)?)),
        }
    }

    /// `true` si el registro existe. No decodifica el valor y no escribe.
    pub fn account_exists(&self, account_id: &AccountId) -> Result<bool, StateError> {
        Ok(db_get(&self.db, &account_storage_key(account_id))?.is_some())
    }

    /// Inicializa el génesis una sola vez.
    ///
    /// Inserta las cuentas en un árbol vacío. El orden del slice no cambia
    /// la raíz. `version` la elige la capa superior; este módulo no inventa
    /// una altura de bloque.
    pub fn initialize_genesis(
        &self,
        accounts: &[GenesisAccount],
        version: u64,
    ) -> Result<StateCommitment, StateError> {
        let _guard = self.lock()?;
        let meta = self.require_meta()?;
        if meta.initialized {
            return Err(StateError::StateAlreadyInitialized);
        }
        if meta.root != StateRoot::empty() {
            return Err(StateError::CorruptedMetadata);
        }
        if !prefix_empty(&self.db, ACCOUNT_NAMESPACE)? || !prefix_empty(&self.db, SMT_NAMESPACE)? {
            return Err(StateError::CorruptedMetadata);
        }

        let staged = stage_genesis(accounts)?;
        let (nodes, root) = build_tree(&staged)?;
        let mut batch = StorageBatch::new();
        fill_state_batch(&mut batch, &staged, &nodes);
        put_meta(&mut batch, &root, version, true);
        let previous_root = meta.root;
        let previous_version = meta.version;
        self.db.write(batch)?;
        Ok(StateCommitment {
            previous_root,
            new_root: StateRoot(root),
            previous_version,
            new_version: version,
        })
    }

    /// Valida y aplica la transición en un único lote.
    ///
    /// `new_version` debe ser mayor que la versión actual. La raíz y la
    /// versión nuevas se escriben juntas. Si cualquier operación falla, el
    /// estado persistido queda como estaba.
    pub fn apply_transition(
        &self,
        transition: &StateTransition,
        new_version: u64,
    ) -> Result<StateCommitment, StateError> {
        let _guard = self.lock()?;
        let mut batch = StorageBatch::new();
        let commitment = self.stage_unlocked(&mut batch, transition, new_version)?;
        self.db.write(batch)?;
        Ok(commitment)
    }

    /// Valida la transición y añade sus escrituras a `batch`. No llama a `write`.
    ///
    /// El llamador publica el lote. Si esta función falla, `batch` no recibe
    /// las cuentas ni la raíz de esta transición.
    pub fn stage_transition(
        &self,
        batch: &mut StorageBatch,
        transition: &StateTransition,
        new_version: u64,
    ) -> Result<StateCommitment, StateError> {
        let _guard = self.lock()?;
        self.stage_unlocked(batch, transition, new_version)
    }

    fn stage_unlocked(
        &self,
        batch: &mut StorageBatch,
        transition: &StateTransition,
        new_version: u64,
    ) -> Result<StateCommitment, StateError> {
        let meta = self.require_meta()?;
        if !meta.initialized {
            return Err(StateError::StateNotInitialized);
        }
        if transition.expected_previous_root != meta.root {
            return Err(StateError::StateRootMismatch);
        }
        if transition.expected_previous_version != meta.version {
            return Err(StateError::StateVersionMismatch);
        }
        if new_version <= meta.version {
            return Err(StateError::VersionNotIncreasing);
        }
        if transition.operations.is_empty() {
            return Err(StateError::InvalidTransition);
        }
        for operation in &transition.operations {
            validate_operation_shape(operation)?;
        }

        let mut accounts = BTreeMap::new();
        let mut nodes = BTreeMap::new();
        let mut computed_root = None;
        for operation in &transition.operations {
            computed_root = Some(apply_operation(
                &self.db,
                operation,
                &mut accounts,
                &mut nodes,
            )?);
        }
        let root = computed_root.ok_or(StateError::InvalidTransition)?;

        fill_state_batch(batch, &accounts, &nodes);
        batch.put(STATE_CF, &meta_key(b"ROOT"), &root);
        batch.put(STATE_CF, &meta_key(b"VERSION"), &new_version.to_be_bytes());
        Ok(StateCommitment {
            previous_root: meta.root,
            new_root: StateRoot(root),
            previous_version: meta.version,
            new_version,
        })
    }

    /// Reconstruye la raíz desde las cuentas canónicas y la compara con la persistida.
    ///
    /// No usa los nodos guardados para calcular esa raíz. Después comprueba
    /// que cada nodo persistido coincide con el árbol reconstruido. Un nodo
    /// de longitud distinta de 32 bytes es un error de serialización.
    pub fn verify_integrity(&self) -> Result<(), StateError> {
        let _guard = self.lock()?;
        let meta = present_meta(read_meta_values(&self.db)?)?;
        let records = db_scan(&self.db, ACCOUNT_NAMESPACE)?;
        let mut staged = BTreeMap::new();
        for (key, value) in records {
            if key.len() != ACCOUNT_NAMESPACE.len() + 32 {
                return Err(StateError::Serialization);
            }
            let id = AccountId::from_bytes(&key[ACCOUNT_NAMESPACE.len()..])?;
            let account = Account::from_bytes(&value)?;
            if account.account_id() != id {
                return Err(StateError::InvalidAccount);
            }
            staged.insert(id, account);
        }
        let (nodes, root) = build_tree(&staged)?;
        if root != meta.root.to_bytes() {
            return Err(StateError::StateRootMismatch);
        }
        let stored = db_scan(&self.db, SMT_NAMESPACE)?;
        let mut seen = 0usize;
        for (key, value) in stored {
            if value.len() != 32 {
                return Err(StateError::Serialization);
            }
            let node_id = parse_node_key(&key)?;
            let hash = decode_node_hash(&value)?;
            match nodes.get(&node_id) {
                Some(Some(expected)) if *expected == hash => {}
                _ => return Err(StateError::StateRootMismatch),
            }
            seen += 1;
        }
        let expected = nodes.values().filter(|slot| slot.is_some()).count();
        if seen != expected {
            return Err(StateError::StateRootMismatch);
        }
        Ok(())
    }

    /// Captura raíz, versión y bytes de cuenta de forma coherente.
    pub fn snapshot(&self) -> Result<StateSnapshot<'_>, StateError> {
        let meta = present_meta(read_meta_values(&self.db)?)?;
        let storage = &self.db;
        Ok(StateSnapshot {
            root: meta.root,
            version: meta.version,
            storage,
        })
    }

    fn ensure_meta(&self) -> Result<(), StateError> {
        let _guard = self.lock()?;
        if matches!(load_meta(&self.db)?, MetaPresence::Missing) {
            let mut batch = StorageBatch::new();
            put_meta(&mut batch, &StateRoot::empty().to_bytes(), 0, false);
            self.db.write(batch)?;
        }
        Ok(())
    }

    fn require_meta(&self) -> Result<StateMeta, StateError> {
        present_meta(read_meta_values(&self.db)?)
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, ()>, StateError> {
        self.write_lock.lock().map_err(|_| StateError::Concurrency)
    }
}

impl fmt::Debug for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("State")
    }
}

struct StateMeta {
    root: StateRoot,
    version: u64,
    initialized: bool,
}

enum MetaPresence {
    Missing,
    Present(StateMeta),
}

fn stage_genesis(accounts: &[GenesisAccount]) -> Result<BTreeMap<AccountId, Account>, StateError> {
    let mut staged = BTreeMap::new();
    for genesis in accounts {
        let account =
            Account::with_initial_balance(genesis.owner_key_id(), genesis.initial_balance());
        account.validate()?;
        if staged.insert(account.account_id(), account).is_some() {
            return Err(StateError::AccountAlreadyExists);
        }
    }
    Ok(staged)
}

type BuiltTree = (BTreeMap<NodeId, Option<[u8; 32]>>, [u8; 32]);
type ScanRows = Vec<(Vec<u8>, Vec<u8>)>;
type MetaRaw = (
    Option<Vec<u8>>,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
    Option<Vec<u8>>,
);

fn build_tree(accounts: &BTreeMap<AccountId, Account>) -> Result<BuiltTree, StateError> {
    let mut nodes = BTreeMap::new();
    let mut root = StateRoot::empty().to_bytes();
    for (id, account) in accounts {
        let leaf = account_leaf_hash(id, &account.to_bytes());
        root = update_leaf(None, &mut nodes, id.as_bytes(), leaf)?;
    }
    Ok((nodes, root))
}

fn apply_operation(
    db: &RscDatabase,
    operation: &StateOperation,
    accounts: &mut BTreeMap<AccountId, Account>,
    nodes: &mut BTreeMap<NodeId, Option<[u8; 32]>>,
) -> Result<[u8; 32], StateError> {
    match operation {
        StateOperation::CreateAccount { owner_key_id } => {
            let account = Account::new(*owner_key_id);
            account.validate()?;
            let id = account.account_id();
            if read_account(db, accounts, &id)?.is_some() {
                return Err(StateError::AccountAlreadyExists);
            }
            let root = write_account_leaf(db, nodes, &account)?;
            accounts.insert(id, account);
            Ok(root)
        }
        StateOperation::Transfer {
            from,
            to,
            amount,
            expected_nonce,
        } => {
            if *amount == 0 {
                return Err(StateError::InvalidAmount);
            }
            if from == to {
                return Err(StateError::InvalidTransfer);
            }
            let sender = read_account(db, accounts, from)?.ok_or(StateError::AccountNotFound)?;
            let receiver = read_account(db, accounts, to)?.ok_or(StateError::AccountNotFound)?;
            if sender.nonce() != *expected_nonce {
                return Err(StateError::NonceMismatch);
            }
            let sender_balance = sender
                .native_balance()
                .checked_sub(*amount)
                .ok_or(StateError::InsufficientBalance)?;
            let receiver_balance = receiver
                .native_balance()
                .checked_add(*amount)
                .ok_or(StateError::BalanceOverflow)?;
            let sender_nonce = sender
                .nonce()
                .checked_add(1)
                .ok_or(StateError::NonceOverflow)?;
            let sender = sender.with_balance_and_nonce(sender_balance, sender_nonce);
            let receiver = receiver.with_balance_and_nonce(receiver_balance, receiver.nonce());
            sender.validate()?;
            receiver.validate()?;
            write_account_leaf(db, nodes, &sender)?;
            let root = write_account_leaf(db, nodes, &receiver)?;
            accounts.insert(*from, sender);
            accounts.insert(*to, receiver);
            Ok(root)
        }
        StateOperation::ConsumeNonce {
            account,
            expected_nonce,
        } => {
            let current = read_account(db, accounts, account)?.ok_or(StateError::AccountNotFound)?;
            if current.nonce() != *expected_nonce {
                return Err(StateError::NonceMismatch);
            }
            let next_nonce = current
                .nonce()
                .checked_add(1)
                .ok_or(StateError::NonceOverflow)?;
            let updated = current.with_balance_and_nonce(current.native_balance(), next_nonce);
            updated.validate()?;
            let root = write_account_leaf(db, nodes, &updated)?;
            accounts.insert(*account, updated);
            Ok(root)
        }
        StateOperation::TransferWithFee {
            from,
            to,
            amount,
            validator_fee,
            burn_fee,
            fee_pool,
            expected_nonce,
        } => {
            if *amount == 0 {
                return Err(StateError::InvalidAmount);
            }
            if from == to {
                return Err(StateError::InvalidTransfer);
            }
            let fee_total = validator_fee
                .checked_add(*burn_fee)
                .ok_or(StateError::BalanceOverflow)?;
            if fee_total > 0 && (fee_pool == from || fee_pool == to) {
                return Err(StateError::InvalidTransfer);
            }
            let sender = read_account(db, accounts, from)?.ok_or(StateError::AccountNotFound)?;
            let receiver = read_account(db, accounts, to)?.ok_or(StateError::AccountNotFound)?;
            if sender.nonce() != *expected_nonce {
                return Err(StateError::NonceMismatch);
            }
            let debit = amount
                .checked_add(fee_total)
                .ok_or(StateError::BalanceOverflow)?;
            let sender_balance = sender
                .native_balance()
                .checked_sub(debit)
                .ok_or(StateError::InsufficientBalance)?;
            let receiver_balance = receiver
                .native_balance()
                .checked_add(*amount)
                .ok_or(StateError::BalanceOverflow)?;
            let sender_nonce = sender
                .nonce()
                .checked_add(1)
                .ok_or(StateError::NonceOverflow)?;
            let sender = sender.with_balance_and_nonce(sender_balance, sender_nonce);
            let receiver = receiver.with_balance_and_nonce(receiver_balance, receiver.nonce());
            sender.validate()?;
            receiver.validate()?;
            write_account_leaf(db, nodes, &sender)?;
            let mut root = write_account_leaf(db, nodes, &receiver)?;
            accounts.insert(*from, sender);
            accounts.insert(*to, receiver);
            if *validator_fee > 0 {
                let pool =
                    read_account(db, accounts, fee_pool)?.ok_or(StateError::AccountNotFound)?;
                let pool_balance = pool
                    .native_balance()
                    .checked_add(*validator_fee)
                    .ok_or(StateError::BalanceOverflow)?;
                let pool = pool.with_balance_and_nonce(pool_balance, pool.nonce());
                pool.validate()?;
                root = write_account_leaf(db, nodes, &pool)?;
                accounts.insert(*fee_pool, pool);
            }
            // burn_fee (si > 0) ya salió del emisor sin receptor.
            Ok(root)
        }
    }
}

fn write_account_leaf(
    db: &RscDatabase,
    nodes: &mut BTreeMap<NodeId, Option<[u8; 32]>>,
    account: &Account,
) -> Result<[u8; 32], StateError> {
    let leaf = account_leaf_hash(&account.account_id(), &account.to_bytes());
    update_leaf(Some(db), nodes, account.account_id().as_bytes(), leaf)
}

fn validate_operation_shape(operation: &StateOperation) -> Result<(), StateError> {
    match operation {
        StateOperation::CreateAccount { .. } => Ok(()),
        StateOperation::Transfer {
            from, to, amount, ..
        } => {
            if *amount == 0 {
                return Err(StateError::InvalidAmount);
            }
            if from == to {
                return Err(StateError::InvalidTransfer);
            }
            Ok(())
        }
        StateOperation::ConsumeNonce { .. } => Ok(()),
        StateOperation::TransferWithFee {
            from,
            to,
            amount,
            validator_fee,
            burn_fee,
            fee_pool,
            ..
        } => {
            if *amount == 0 {
                return Err(StateError::InvalidAmount);
            }
            if from == to {
                return Err(StateError::InvalidTransfer);
            }
            let fee_total = validator_fee
                .checked_add(*burn_fee)
                .ok_or(StateError::BalanceOverflow)?;
            if fee_total > 0 && (fee_pool == from || fee_pool == to) {
                return Err(StateError::InvalidTransfer);
            }
            Ok(())
        }
    }
}

fn read_account(
    db: &RscDatabase,
    pending: &BTreeMap<AccountId, Account>,
    id: &AccountId,
) -> Result<Option<Account>, StateError> {
    if let Some(account) = pending.get(id) {
        return Ok(Some(*account));
    }
    match db_get(db, &account_storage_key(id))? {
        None => Ok(None),
        Some(bytes) => Ok(Some(Account::from_bytes(&bytes)?)),
    }
}

fn fill_state_batch(
    batch: &mut StorageBatch,
    accounts: &BTreeMap<AccountId, Account>,
    nodes: &BTreeMap<NodeId, Option<[u8; 32]>>,
) {
    for (id, account) in accounts {
        batch.put(STATE_CF, &account_storage_key(id), &account.to_bytes());
    }
    for (node, slot) in nodes {
        let key = node_storage_key(node.depth, &node.prefix);
        match slot {
            Some(hash) => batch.put(STATE_CF, &key, hash),
            None => batch.delete(STATE_CF, &key),
        }
    }
}

fn put_meta(batch: &mut StorageBatch, root: &[u8; 32], version: u64, initialized: bool) {
    batch.put(
        STATE_CF,
        &meta_key(b"SCHEMA"),
        &STATE_SCHEMA_V1.to_be_bytes(),
    );
    batch.put(STATE_CF, &meta_key(b"ROOT"), root);
    batch.put(STATE_CF, &meta_key(b"VERSION"), &version.to_be_bytes());
    batch.put(
        STATE_CF,
        &meta_key(b"INITIALIZED"),
        &[u8::from(initialized)],
    );
}

const STATE_CF: ColumnFamily = ColumnFamily::Default;

fn db_get(db: &RscDatabase, key: &[u8]) -> Result<Option<Vec<u8>>, StateError> {
    Ok(db
        .get(STATE_CF, key)?
        .map(|value| value.as_bytes().to_vec()))
}

fn db_scan(db: &RscDatabase, prefix: &[u8]) -> Result<ScanRows, StateError> {
    Ok(db
        .scan(STATE_CF, prefix)?
        .into_iter()
        .map(|(key, value)| (key.as_bytes().to_vec(), value.as_bytes().to_vec()))
        .collect())
}

fn prefix_empty(db: &RscDatabase, prefix: &[u8]) -> Result<bool, StateError> {
    Ok(db_scan(db, prefix)?.is_empty())
}

fn load_meta(db: &RscDatabase) -> Result<MetaPresence, StateError> {
    interpret_meta(
        db_get(db, &meta_key(b"SCHEMA"))?,
        db_get(db, &meta_key(b"ROOT"))?,
        db_get(db, &meta_key(b"VERSION"))?,
        db_get(db, &meta_key(b"INITIALIZED"))?,
    )
}

fn read_meta_values(snap: &RscDatabase) -> Result<MetaRaw, StateError> {
    Ok((
        db_get(snap, &meta_key(b"SCHEMA"))?,
        db_get(snap, &meta_key(b"ROOT"))?,
        db_get(snap, &meta_key(b"VERSION"))?,
        db_get(snap, &meta_key(b"INITIALIZED"))?,
    ))
}

fn present_meta(values: MetaRaw) -> Result<StateMeta, StateError> {
    match interpret_meta(values.0, values.1, values.2, values.3)? {
        MetaPresence::Missing => Err(StateError::CorruptedMetadata),
        MetaPresence::Present(meta) => Ok(meta),
    }
}

fn interpret_meta(
    schema: Option<Vec<u8>>,
    root: Option<Vec<u8>>,
    version: Option<Vec<u8>>,
    initialized: Option<Vec<u8>>,
) -> Result<MetaPresence, StateError> {
    if schema.is_none() && root.is_none() && version.is_none() && initialized.is_none() {
        return Ok(MetaPresence::Missing);
    }
    let (Some(schema), Some(root), Some(version), Some(initialized)) =
        (schema, root, version, initialized)
    else {
        return Err(StateError::CorruptedMetadata);
    };
    if schema.len() != 4 {
        return Err(StateError::CorruptedMetadata);
    }
    let mut schema_bytes = [0u8; 4];
    schema_bytes.copy_from_slice(&schema);
    if u32::from_be_bytes(schema_bytes) != STATE_SCHEMA_V1 {
        return Err(StateError::StateSchemaMismatch);
    }
    if root.len() != 32 {
        return Err(StateError::CorruptedMetadata);
    }
    let root = StateRoot::from_bytes(&root)?;
    if version.len() != 8 {
        return Err(StateError::CorruptedMetadata);
    }
    let mut version_bytes = [0u8; 8];
    version_bytes.copy_from_slice(&version);
    let version = u64::from_be_bytes(version_bytes);
    if initialized.len() != 1 {
        return Err(StateError::CorruptedMetadata);
    }
    let initialized = match initialized[0] {
        0x00 => false,
        0x01 => true,
        _ => return Err(StateError::CorruptedMetadata),
    };
    Ok(MetaPresence::Present(StateMeta {
        root,
        version,
        initialized,
    }))
}

fn account_storage_key(id: &AccountId) -> Vec<u8> {
    let mut key = Vec::with_capacity(ACCOUNT_NAMESPACE.len() + 32);
    key.extend_from_slice(ACCOUNT_NAMESPACE);
    key.extend_from_slice(id.as_bytes());
    key
}

fn meta_key(suffix: &[u8]) -> Vec<u8> {
    let mut key = Vec::with_capacity(META_NAMESPACE.len() + suffix.len());
    key.extend_from_slice(META_NAMESPACE);
    key.extend_from_slice(suffix);
    key
}

fn account_leaf_hash(id: &AccountId, canonical: &[u8]) -> [u8; 32] {
    domain_hash(ACCOUNT_LEAF_DOMAIN, &[id.as_bytes(), canonical])
}

// ----------------------------
// Sparse Merkle implementation
// ----------------------------

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct NodeId {
    depth: u16,
    prefix: [u8; 32],
}

fn empty_leaf_hash() -> [u8; 32] {
    domain_hash(SMT_EMPTY_LEAF_DOMAIN, &[])
}

fn hash_node(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    domain_hash(SMT_NODE_DOMAIN, &[left, right])
}

fn empty_hashes() -> &'static [[u8; 32]; 257] {
    static TABLE: OnceLock<[[u8; 32]; 257]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = [[0u8; 32]; 257];
        table[256] = empty_leaf_hash();
        for depth in (0..256).rev() {
            table[depth] = hash_node(&table[depth + 1], &table[depth + 1]);
        }
        table
    })
}

/// Primeros `depth` bits, MSB primero. El resto queda a cero.
fn normalize_path(id: &[u8; 32], depth: u16) -> [u8; 32] {
    if depth >= TREE_DEPTH {
        return *id;
    }
    if depth == 0 {
        return [0u8; 32];
    }
    let depth = usize::from(depth);
    let mut out = [0u8; 32];
    let full_bytes = depth / 8;
    let rem_bits = depth % 8;
    out[..full_bytes].copy_from_slice(&id[..full_bytes]);
    if rem_bits > 0 {
        let mask = 0xffu8 << (8 - rem_bits);
        out[full_bytes] = id[full_bytes] & mask;
    }
    out
}

/// Bit `bit_index` de `id`, con el bit 7 del byte 0 como índice 0.
fn path_bit(id: &[u8; 32], bit_index: usize) -> bool {
    let byte = id[bit_index / 8];
    let shift = 7 - (bit_index % 8);
    ((byte >> shift) & 1) == 1
}

/// Prefijo del hermano: mismo prefijo, con el bit `depth - 1` invertido.
fn sibling_prefix(id: &[u8; 32], depth: u16) -> [u8; 32] {
    let mut prefix = normalize_path(id, depth);
    let bit_index = usize::from(depth - 1);
    let mask = 1u8 << (7 - (bit_index % 8));
    prefix[bit_index / 8] ^= mask;
    prefix
}

fn node_storage_key(depth: u16, prefix: &[u8; 32]) -> Vec<u8> {
    let mut key = Vec::with_capacity(SMT_NAMESPACE.len() + 2 + 32);
    key.extend_from_slice(SMT_NAMESPACE);
    key.extend_from_slice(&depth.to_be_bytes());
    key.extend_from_slice(prefix);
    key
}

fn parse_node_key(key: &[u8]) -> Result<NodeId, StateError> {
    if key.len() != SMT_NAMESPACE.len() + 2 + 32 || !key.starts_with(SMT_NAMESPACE) {
        return Err(StateError::Serialization);
    }
    let rest = &key[SMT_NAMESPACE.len()..];
    let mut depth_bytes = [0u8; 2];
    depth_bytes.copy_from_slice(&rest[..2]);
    let depth = u16::from_be_bytes(depth_bytes);
    if depth > TREE_DEPTH {
        return Err(StateError::Serialization);
    }
    let mut prefix = [0u8; 32];
    prefix.copy_from_slice(&rest[2..]);
    if normalize_path(&prefix, depth) != prefix {
        return Err(StateError::Serialization);
    }
    Ok(NodeId { depth, prefix })
}

fn decode_node_hash(bytes: &[u8]) -> Result<[u8; 32], StateError> {
    if bytes.len() != 32 {
        return Err(StateError::Serialization);
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(bytes);
    Ok(out)
}

fn lookup_node(
    db: Option<&RscDatabase>,
    pending: &BTreeMap<NodeId, Option<[u8; 32]>>,
    depth: u16,
    prefix: [u8; 32],
) -> Result<[u8; 32], StateError> {
    let id = NodeId { depth, prefix };
    if let Some(slot) = pending.get(&id) {
        return match slot {
            Some(hash) => Ok(*hash),
            None => Ok(empty_hashes()[usize::from(depth)]),
        };
    }
    let Some(db) = db else {
        return Ok(empty_hashes()[usize::from(depth)]);
    };
    match db_get(db, &node_storage_key(depth, &prefix))? {
        None => Ok(empty_hashes()[usize::from(depth)]),
        Some(bytes) => decode_node_hash(&bytes),
    }
}

/// Actualiza la hoja y sus ancestros solo en `pending`. No escribe en disco.
fn update_leaf(
    db: Option<&RscDatabase>,
    pending: &mut BTreeMap<NodeId, Option<[u8; 32]>>,
    account_id: &[u8; 32],
    new_leaf: [u8; 32],
) -> Result<[u8; 32], StateError> {
    let empty = empty_hashes();
    let mut current_hash = new_leaf;
    let mut depth = TREE_DEPTH;
    loop {
        let prefix = normalize_path(account_id, depth);
        let id = NodeId { depth, prefix };
        if current_hash == empty[usize::from(depth)] {
            pending.insert(id, None);
        } else {
            pending.insert(id, Some(current_hash));
        }
        if depth == 0 {
            return Ok(current_hash);
        }
        let branch = path_bit(account_id, usize::from(depth - 1));
        let sibling = sibling_prefix(account_id, depth);
        let sibling_hash = lookup_node(db, pending, depth, sibling)?;
        let (left, right) = if branch {
            (sibling_hash, current_hash)
        } else {
            (current_hash, sibling_hash)
        };
        current_hash = hash_node(&left, &right);
        depth -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::derive_account_id;
    use std::sync::Arc;

    fn owner(byte: u8) -> KeyId {
        KeyId::from_bytes(&[byte; 32]).unwrap()
    }

    fn test_db_config() -> rsc_storage::DatabaseConfig {
        rsc_storage::DatabaseConfig {
            max_value_size: 8 * 1024 * 1024,
            network_id: "devnet".into(),
            chain_id: "3".into(),
        }
    }

    struct Env {
        _dir: tempfile::TempDir,
        db: Arc<RscDatabase>,
        state: State,
    }

    fn open_env() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(RscDatabase::open(dir.path(), test_db_config()).unwrap());
        let state = State::open(Arc::clone(&db)).unwrap();
        Env {
            _dir: dir,
            db,
            state,
        }
    }

    fn fingerprint(env: &Env) -> (StateRoot, u64, u64) {
        (
            env.state.state_root().unwrap(),
            env.state.version().unwrap(),
            env.db.commit_count(),
        )
    }

    fn genesis(state: &State, accounts: &[GenesisAccount], version: u64) -> StateCommitment {
        state.initialize_genesis(accounts, version).unwrap()
    }

    fn apply(
        state: &State,
        ops: Vec<StateOperation>,
        version: u64,
    ) -> Result<StateCommitment, StateError> {
        let mut transition = StateTransition::new(state.state_root()?, state.version()?);
        for op in ops {
            transition.push(op);
        }
        state.apply_transition(&transition, version)
    }

    fn shared_prefix_owners() -> (KeyId, KeyId) {
        let mut seen = std::collections::HashMap::<u8, KeyId>::new();
        for i in 0..100_000u32 {
            let mut raw = [0u8; 32];
            raw[..4].copy_from_slice(&i.to_le_bytes());
            let key = KeyId::from_bytes(&raw).unwrap();
            let first = derive_account_id(&key).as_bytes()[0];
            if let Some(previous) = seen.insert(first, key) {
                return (previous, key);
            }
        }
        panic!("no shared prefix");
    }

    #[test]
    fn smt_path_is_msb_first() {
        let mut id = [0u8; 32];
        id[0] = 0b1011_0001;
        assert!(path_bit(&id, 0));
        assert!(!path_bit(&id, 1));
        assert!(path_bit(&id, 2));
        assert!(path_bit(&id, 3));
        assert!(!path_bit(&id, 4));
        assert!(path_bit(&id, 7));
        id[0] = 0b0000_0001;
        assert!(!path_bit(&id, 0));
        assert!(path_bit(&id, 7));

        id[0] = 0xff;
        id[1] = 0xff;
        let one = normalize_path(&id, 1);
        assert_eq!(one[0], 0x80);
        assert_eq!(one[1], 0);
        assert_eq!(normalize_path(&id, 4)[0], 0xf0);
        assert_eq!(normalize_path(&id, 0), [0u8; 32]);
        assert_eq!(normalize_path(&id, 256), id);

        id[0] = 0x80;
        assert_eq!(sibling_prefix(&id, 1)[0], 0x00);
        id[0] = 0x01;
        assert_eq!(sibling_prefix(&id, 8)[0], 0x00);
    }

    #[test]
    fn empty_root_and_node_orientation_are_deterministic() {
        let table = empty_hashes();
        assert_eq!(table[256], empty_leaf_hash());
        for depth in 0..256 {
            assert_eq!(
                table[depth],
                hash_node(&table[depth + 1], &table[depth + 1])
            );
        }
        assert_eq!(StateRoot::empty(), StateRoot::empty());
        assert_eq!(StateRoot::empty().to_bytes(), table[0]);
        let left = [1u8; 32];
        let right = [2u8; 32];
        assert_ne!(hash_node(&left, &right), hash_node(&right, &left));
        let env_a = open_env();
        let env_b = open_env();
        assert_eq!(env_a.state.state_root().unwrap(), StateRoot::empty());
        assert_eq!(
            env_a.state.state_root().unwrap(),
            env_b.state.state_root().unwrap()
        );
        assert_eq!(env_a.state.version().unwrap(), 0);
    }

    #[test]
    fn create_account_and_reject_duplicate() {
        let env = open_env();
        genesis(&env.state, &[], 1);
        let before_missing = env
            .state
            .get_account(&derive_account_id(&owner(1)))
            .unwrap();
        assert!(before_missing.is_none());
        let commitment = apply(
            &env.state,
            vec![StateOperation::CreateAccount {
                owner_key_id: owner(1),
            }],
            2,
        )
        .unwrap();
        assert_ne!(commitment.new_root(), commitment.previous_root());
        let account = env
            .state
            .get_account(&derive_account_id(&owner(1)))
            .unwrap()
            .unwrap();
        assert_eq!(account.native_balance(), 0);
        assert_eq!(account.nonce(), 0);
        assert_eq!(account.version(), 1);
        assert!(env.state.account_exists(&account.account_id()).unwrap());
        let stuck = fingerprint(&env);
        let err = apply(
            &env.state,
            vec![StateOperation::CreateAccount {
                owner_key_id: owner(1),
            }],
            3,
        )
        .unwrap_err();
        assert!(matches!(err, StateError::AccountAlreadyExists));
        assert_eq!(fingerprint(&env), stuck);
    }

    #[test]
    fn transfer_rules_and_exact_balance() {
        let env = open_env();
        genesis(
            &env.state,
            &[
                GenesisAccount::new(owner(1), 1000),
                GenesisAccount::new(owner(2), 500),
            ],
            1,
        );
        let alice = derive_account_id(&owner(1));
        let bob = derive_account_id(&owner(2));
        apply(
            &env.state,
            vec![StateOperation::Transfer {
                from: alice,
                to: bob,
                amount: 300,
                expected_nonce: 0,
            }],
            2,
        )
        .unwrap();
        assert_eq!(
            env.state
                .get_account(&alice)
                .unwrap()
                .unwrap()
                .native_balance(),
            700
        );
        assert_eq!(
            env.state
                .get_account(&bob)
                .unwrap()
                .unwrap()
                .native_balance(),
            800
        );
        assert_eq!(env.state.get_account(&alice).unwrap().unwrap().nonce(), 1);

        apply(
            &env.state,
            vec![StateOperation::Transfer {
                from: alice,
                to: bob,
                amount: 700,
                expected_nonce: 1,
            }],
            3,
        )
        .unwrap();
        let sender = env.state.get_account(&alice).unwrap().unwrap();
        assert_eq!(sender.native_balance(), 0);
        assert!(env.state.account_exists(&alice).unwrap());
        assert_eq!(
            env.state
                .get_account(&bob)
                .unwrap()
                .unwrap()
                .native_balance(),
            1500
        );
    }

    #[test]
    fn transfer_with_fee_burns_and_credits_pool() {
        let env = open_env();
        genesis(
            &env.state,
            &[
                GenesisAccount::new(owner(1), 1_000),
                GenesisAccount::new(owner(2), 100),
                GenesisAccount::new(owner(3), 1),
            ],
            1,
        );
        let alice = derive_account_id(&owner(1));
        let bob = derive_account_id(&owner(2));
        let pool = derive_account_id(&owner(3));
        apply(
            &env.state,
            vec![StateOperation::TransferWithFee {
                from: alice,
                to: bob,
                amount: 100,
                validator_fee: 70,
                burn_fee: 30,
                fee_pool: pool,
                expected_nonce: 0,
            }],
            2,
        )
        .unwrap();
        assert_eq!(
            env.state
                .get_account(&alice)
                .unwrap()
                .unwrap()
                .native_balance(),
            800
        );
        assert_eq!(
            env.state
                .get_account(&bob)
                .unwrap()
                .unwrap()
                .native_balance(),
            200
        );
        assert_eq!(
            env.state
                .get_account(&pool)
                .unwrap()
                .unwrap()
                .native_balance(),
            71
        );
    }

    #[test]
    fn transfer_failures_do_not_commit() {
        let env = open_env();
        genesis(
            &env.state,
            &[
                GenesisAccount::new(owner(1), 40),
                GenesisAccount::new(owner(2), 5),
            ],
            1,
        );
        let alice = derive_account_id(&owner(1));
        let bob = derive_account_id(&owner(2));
        let missing = derive_account_id(&owner(9));
        let cases = [
            StateOperation::Transfer {
                from: alice,
                to: bob,
                amount: 41,
                expected_nonce: 0,
            },
            StateOperation::Transfer {
                from: alice,
                to: bob,
                amount: 0,
                expected_nonce: 0,
            },
            StateOperation::Transfer {
                from: alice,
                to: alice,
                amount: 1,
                expected_nonce: 0,
            },
            StateOperation::Transfer {
                from: alice,
                to: bob,
                amount: 1,
                expected_nonce: 4,
            },
            StateOperation::Transfer {
                from: alice,
                to: missing,
                amount: 1,
                expected_nonce: 0,
            },
        ];
        let expected = [
            StateError::InsufficientBalance.to_string(),
            StateError::InvalidAmount.to_string(),
            StateError::InvalidTransfer.to_string(),
            StateError::NonceMismatch.to_string(),
            StateError::AccountNotFound.to_string(),
        ];
        for (op, label) in cases.into_iter().zip(expected) {
            let stuck = fingerprint(&env);
            let err = apply(&env.state, vec![op], 2).unwrap_err();
            assert_eq!(err.to_string(), label);
            assert_eq!(fingerprint(&env), stuck);
            assert!(env.state.get_account(&missing).unwrap().is_none());
        }
    }

    #[test]
    fn nonce_overflow_and_balance_overflow_do_not_commit() {
        let env = open_env();
        genesis(
            &env.state,
            &[
                GenesisAccount::new(owner(1), 10),
                GenesisAccount::new(owner(2), u128::MAX),
            ],
            1,
        );
        let alice = derive_account_id(&owner(1));
        let bob = derive_account_id(&owner(2));
        let stuck = fingerprint(&env);
        let err = apply(
            &env.state,
            vec![StateOperation::Transfer {
                from: alice,
                to: bob,
                amount: 1,
                expected_nonce: 0,
            }],
            2,
        )
        .unwrap_err();
        assert!(matches!(err, StateError::BalanceOverflow));
        assert_eq!(fingerprint(&env), stuck);

        let nonce_env = open_env();
        genesis(
            &nonce_env.state,
            &[
                GenesisAccount::new(owner(1), 10),
                GenesisAccount::new(owner(2), 0),
            ],
            1,
        );
        let alice = derive_account_id(&owner(1));
        let bob = derive_account_id(&owner(2));
        let mut bytes = nonce_env
            .state
            .get_account(&alice)
            .unwrap()
            .unwrap()
            .to_bytes();
        bytes[68..76].copy_from_slice(&u64::MAX.to_be_bytes());
        nonce_env
            .db
            .put(ColumnFamily::Default, &account_storage_key(&alice), &bytes)
            .unwrap();
        let root = nonce_env.state.state_root().unwrap();
        let version = nonce_env.state.version().unwrap();
        let mut transition = StateTransition::new(root, version);
        transition.push(StateOperation::Transfer {
            from: alice,
            to: bob,
            amount: 1,
            expected_nonce: u64::MAX,
        });
        let err = nonce_env
            .state
            .apply_transition(&transition, version + 1)
            .unwrap_err();
        assert!(matches!(err, StateError::NonceOverflow));
        assert_eq!(nonce_env.state.state_root().unwrap(), root);
        assert_eq!(nonce_env.state.version().unwrap(), version);
        assert_eq!(
            nonce_env
                .state
                .get_account(&alice)
                .unwrap()
                .unwrap()
                .nonce(),
            u64::MAX
        );
    }

    #[test]
    fn roots_match_across_order_version_and_replay() {
        let mut roots = Vec::new();
        for order in [[1u8, 2, 3], [3, 1, 2], [2, 3, 1]] {
            let env = open_env();
            let accounts: Vec<_> = order
                .into_iter()
                .map(|byte| GenesisAccount::new(owner(byte), 5))
                .collect();
            genesis(&env.state, &accounts, u64::from(order[0]) + 10);
            roots.push(env.state.state_root().unwrap());
            env.state.verify_integrity().unwrap();
        }
        assert_eq!(roots[0], roots[1]);
        assert_eq!(roots[1], roots[2]);

        let low = open_env();
        let high = open_env();
        let accounts = [
            GenesisAccount::new(owner(4), 8),
            GenesisAccount::new(owner(5), 13),
        ];
        genesis(&low.state, &accounts, 100);
        genesis(&high.state, &accounts, 101);
        assert_eq!(
            low.state.state_root().unwrap(),
            high.state.state_root().unwrap()
        );
        assert_ne!(low.state.version().unwrap(), high.state.version().unwrap());

        let mut created = Vec::new();
        for order in [[1u8, 2, 3], [3, 2, 1]] {
            let env = open_env();
            genesis(&env.state, &[], 1);
            let mut version = 1u64;
            for byte in order {
                version += 1;
                apply(
                    &env.state,
                    vec![StateOperation::CreateAccount {
                        owner_key_id: owner(byte),
                    }],
                    version,
                )
                .unwrap();
            }
            created.push(env.state.state_root().unwrap());
        }
        assert_eq!(created[0], created[1]);
    }

    #[test]
    fn reopen_preserves_logical_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let root;
        let version;
        let alice_bytes;
        {
            let db = Arc::new(RscDatabase::open(&path, test_db_config()).unwrap());
            let state = State::open(db).unwrap();
            genesis(
                &state,
                &[
                    GenesisAccount::new(owner(1), 80),
                    GenesisAccount::new(owner(2), 20),
                ],
                4,
            );
            let alice = derive_account_id(&owner(1));
            apply(
                &state,
                vec![StateOperation::Transfer {
                    from: alice,
                    to: derive_account_id(&owner(2)),
                    amount: 30,
                    expected_nonce: 0,
                }],
                5,
            )
            .unwrap();
            root = state.state_root().unwrap();
            version = state.version().unwrap();
            alice_bytes = state.get_account(&alice).unwrap().unwrap().to_bytes();
            state.verify_integrity().unwrap();
        }
        let db = Arc::new(RscDatabase::open(&path, test_db_config()).unwrap());
        let state = State::open(db).unwrap();
        assert_eq!(state.state_root().unwrap(), root);
        assert_eq!(state.version().unwrap(), version);
        assert_eq!(
            state
                .get_account(&derive_account_id(&owner(1)))
                .unwrap()
                .unwrap()
                .to_bytes(),
            alice_bytes
        );
        state.verify_integrity().unwrap();
    }

    #[test]
    fn genesis_is_once_and_apply_requires_it() {
        let env = open_env();
        let err = apply(
            &env.state,
            vec![StateOperation::CreateAccount {
                owner_key_id: owner(1),
            }],
            1,
        )
        .unwrap_err();
        assert!(matches!(err, StateError::StateNotInitialized));
        genesis(&env.state, &[], 7);
        assert_eq!(env.state.state_root().unwrap(), StateRoot::empty());
        assert_eq!(env.state.version().unwrap(), 7);
        let err = env
            .state
            .initialize_genesis(&[GenesisAccount::new(owner(1), 1)], 8)
            .unwrap_err();
        assert!(matches!(err, StateError::StateAlreadyInitialized));
        assert_eq!(env.state.version().unwrap(), 7);

        let fresh = open_env();
        let duplicate = [
            GenesisAccount::new(owner(1), 3),
            GenesisAccount::new(owner(1), 4),
        ];
        let err = fresh.state.initialize_genesis(&duplicate, 1).unwrap_err();
        assert!(matches!(err, StateError::AccountAlreadyExists));
        assert_eq!(fresh.state.version().unwrap(), 0);
        assert!(fresh
            .state
            .get_account(&derive_account_id(&owner(1)))
            .unwrap()
            .is_none());
    }

    #[test]
    fn schema_and_metadata_corruption() {
        let env = open_env();
        env.db
            .put(
                ColumnFamily::Default,
                &meta_key(b"SCHEMA"),
                &2u32.to_be_bytes(),
            )
            .unwrap();
        let err = State::open(Arc::clone(&env.db)).unwrap_err();
        assert!(matches!(err, StateError::StateSchemaMismatch));

        let checks: Vec<(&str, Box<dyn Fn(&RscDatabase)>)> = vec![
            (
                "schema-missing",
                Box::new(|db| {
                    db.delete(ColumnFamily::Default, &meta_key(b"SCHEMA"))
                        .unwrap()
                }),
            ),
            (
                "schema-short",
                Box::new(|db| {
                    db.put(ColumnFamily::Default, &meta_key(b"SCHEMA"), &[1])
                        .unwrap()
                }),
            ),
            (
                "root-short",
                Box::new(|db| {
                    db.put(ColumnFamily::Default, &meta_key(b"ROOT"), &[0; 31])
                        .unwrap()
                }),
            ),
            (
                "root-missing",
                Box::new(|db| {
                    db.delete(ColumnFamily::Default, &meta_key(b"ROOT"))
                        .unwrap()
                }),
            ),
            (
                "version-short",
                Box::new(|db| {
                    db.put(ColumnFamily::Default, &meta_key(b"VERSION"), &[0; 4])
                        .unwrap()
                }),
            ),
            (
                "version-missing",
                Box::new(|db| {
                    db.delete(ColumnFamily::Default, &meta_key(b"VERSION"))
                        .unwrap()
                }),
            ),
            (
                "init-bad",
                Box::new(|db| {
                    db.put(ColumnFamily::Default, &meta_key(b"INITIALIZED"), &[0x02])
                        .unwrap()
                }),
            ),
            (
                "init-missing",
                Box::new(|db| {
                    db.delete(ColumnFamily::Default, &meta_key(b"INITIALIZED"))
                        .unwrap()
                }),
            ),
        ];
        for (label, mutate) in checks {
            let case = open_env();
            mutate(&case.db);
            let err = State::open(Arc::clone(&case.db)).unwrap_err();
            assert!(
                matches!(err, StateError::CorruptedMetadata),
                "{label}: {err}"
            );
        }
    }

    #[test]
    fn version_root_and_expected_guards() {
        let env = open_env();
        genesis(&env.state, &[GenesisAccount::new(owner(1), 20)], 10);
        let root = env.state.state_root().unwrap();
        let mut same_version = StateTransition::new(root, 10);
        same_version.push(StateOperation::CreateAccount {
            owner_key_id: owner(2),
        });
        assert!(matches!(
            env.state.apply_transition(&same_version, 10).unwrap_err(),
            StateError::VersionNotIncreasing
        ));
        assert!(matches!(
            env.state.apply_transition(&same_version, 9).unwrap_err(),
            StateError::VersionNotIncreasing
        ));
        assert_eq!(env.state.version().unwrap(), 10);

        apply(
            &env.state,
            vec![StateOperation::CreateAccount {
                owner_key_id: owner(2),
            }],
            11,
        )
        .unwrap();
        let mut stale_root = StateTransition::new(root, env.state.version().unwrap());
        stale_root.push(StateOperation::CreateAccount {
            owner_key_id: owner(3),
        });
        let stuck = fingerprint(&env);
        assert!(matches!(
            env.state.apply_transition(&stale_root, 12).unwrap_err(),
            StateError::StateRootMismatch
        ));
        let mut stale_version = StateTransition::new(env.state.state_root().unwrap(), 10);
        stale_version.push(StateOperation::CreateAccount {
            owner_key_id: owner(3),
        });
        assert!(matches!(
            env.state.apply_transition(&stale_version, 12).unwrap_err(),
            StateError::StateVersionMismatch
        ));
        assert_eq!(fingerprint(&env), stuck);
        assert!(env
            .state
            .get_account(&derive_account_id(&owner(3)))
            .unwrap()
            .is_none());
    }

    #[test]
    fn multi_operation_atomicity_and_shared_ancestors() {
        let env = open_env();
        genesis(&env.state, &[GenesisAccount::new(owner(1), 100)], 1);
        let alice = derive_account_id(&owner(1));
        let carol = derive_account_id(&owner(3));
        let stuck = fingerprint(&env);
        let err = apply(
            &env.state,
            vec![
                StateOperation::CreateAccount {
                    owner_key_id: owner(3),
                },
                StateOperation::Transfer {
                    from: alice,
                    to: carol,
                    amount: 1000,
                    expected_nonce: 0,
                },
            ],
            2,
        )
        .unwrap_err();
        assert!(matches!(err, StateError::InsufficientBalance));
        assert_eq!(fingerprint(&env), stuck);
        assert!(!env.state.account_exists(&carol).unwrap());

        let commitment = apply(
            &env.state,
            vec![
                StateOperation::CreateAccount {
                    owner_key_id: owner(3),
                },
                StateOperation::Transfer {
                    from: alice,
                    to: carol,
                    amount: 25,
                    expected_nonce: 0,
                },
                StateOperation::Transfer {
                    from: alice,
                    to: carol,
                    amount: 10,
                    expected_nonce: 1,
                },
            ],
            2,
        )
        .unwrap();
        assert_eq!(commitment.previous_version(), 1);
        assert_eq!(commitment.new_version(), 2);
        assert_eq!(
            env.state
                .get_account(&alice)
                .unwrap()
                .unwrap()
                .native_balance(),
            65
        );
        assert_eq!(env.state.get_account(&alice).unwrap().unwrap().nonce(), 2);
        assert_eq!(
            env.state
                .get_account(&carol)
                .unwrap()
                .unwrap()
                .native_balance(),
            35
        );
        assert_eq!(env.state.get_account(&carol).unwrap().unwrap().nonce(), 0);
        env.state.verify_integrity().unwrap();
        assert_eq!(env.db.commit_count(), stuck.2 + 1);

        let (left, right) = shared_prefix_owners();
        let one = open_env();
        let two = open_env();
        let genesis_accounts = [
            GenesisAccount::new(owner(1), 1000),
            GenesisAccount::new(left, 0),
            GenesisAccount::new(right, 0),
        ];
        genesis(&one.state, &genesis_accounts, 1);
        genesis(&two.state, &genesis_accounts, 1);
        let funder = derive_account_id(&owner(1));
        let a = derive_account_id(&left);
        let b = derive_account_id(&right);
        apply(
            &one.state,
            vec![
                StateOperation::Transfer {
                    from: funder,
                    to: a,
                    amount: 10,
                    expected_nonce: 0,
                },
                StateOperation::Transfer {
                    from: funder,
                    to: b,
                    amount: 20,
                    expected_nonce: 1,
                },
            ],
            2,
        )
        .unwrap();
        apply(
            &two.state,
            vec![StateOperation::Transfer {
                from: funder,
                to: a,
                amount: 10,
                expected_nonce: 0,
            }],
            2,
        )
        .unwrap();
        apply(
            &two.state,
            vec![StateOperation::Transfer {
                from: funder,
                to: b,
                amount: 20,
                expected_nonce: 1,
            }],
            3,
        )
        .unwrap();
        assert_eq!(
            one.state.state_root().unwrap(),
            two.state.state_root().unwrap()
        );
        assert_ne!(one.state.version().unwrap(), two.state.version().unwrap());
        one.state.verify_integrity().unwrap();
        two.state.verify_integrity().unwrap();
    }

    #[test]
    fn corrupted_and_malformed_smt_nodes() {
        let env = open_env();
        genesis(&env.state, &[GenesisAccount::new(owner(1), 15)], 1);
        let alice = derive_account_id(&owner(1));
        let leaf_key = node_storage_key(256, alice.as_bytes());
        env.db
            .put(ColumnFamily::Default, &leaf_key, &[0xab; 32])
            .unwrap();
        assert!(matches!(
            env.state.verify_integrity().unwrap_err(),
            StateError::StateRootMismatch
        ));

        let fresh = open_env();
        genesis(
            &fresh.state,
            &[
                GenesisAccount::new(owner(1), 15),
                GenesisAccount::new(owner(2), 0),
            ],
            1,
        );
        let alice = derive_account_id(&owner(1));
        let sibling = sibling_prefix(alice.as_bytes(), 256);
        fresh
            .db
            .put(
                ColumnFamily::Default,
                &node_storage_key(256, &sibling),
                &[1, 2, 3],
            )
            .unwrap();
        assert!(matches!(
            fresh.state.verify_integrity().unwrap_err(),
            StateError::Serialization
        ));
        let root = fresh.state.state_root().unwrap();
        let version = fresh.state.version().unwrap();
        let err = apply(
            &fresh.state,
            vec![StateOperation::Transfer {
                from: alice,
                to: derive_account_id(&owner(2)),
                amount: 1,
                expected_nonce: 0,
            }],
            2,
        )
        .unwrap_err();
        assert!(matches!(err, StateError::Serialization));
        assert_eq!(fresh.state.state_root().unwrap(), root);
        assert_eq!(fresh.state.version().unwrap(), version);
        assert!(decode_node_hash(&[0; 31]).is_err());
        assert!(decode_node_hash(&[0; 33]).is_err());
        assert!(decode_node_hash(&[0; 32]).is_ok());
    }

    #[test]
    fn snapshot_and_reads_are_stable() {
        let env = open_env();
        genesis(
            &env.state,
            &[
                GenesisAccount::new(owner(1), 100),
                GenesisAccount::new(owner(2), 0),
            ],
            1,
        );
        let alice = derive_account_id(&owner(1));
        let snapshot = env.state.snapshot().unwrap();
        assert_eq!(
            snapshot
                .get_account(&alice)
                .unwrap()
                .unwrap()
                .native_balance(),
            100
        );
        let before = fingerprint(&env);
        for _ in 0..10 {
            assert!(env.state.get_account(&alice).unwrap().is_some());
            assert!(env.state.account_exists(&alice).unwrap());
            let _ = env.state.state_root().unwrap();
            let _ = env.state.version().unwrap();
            let missing = derive_account_id(&owner(8));
            assert!(env.state.get_account(&missing).unwrap().is_none());
            assert!(!env.state.account_exists(&missing).unwrap());
        }
        assert_eq!(fingerprint(&env), before);

        apply(
            &env.state,
            vec![StateOperation::Transfer {
                from: alice,
                to: derive_account_id(&owner(2)),
                amount: 50,
                expected_nonce: 0,
            }],
            2,
        )
        .unwrap();
        assert_eq!(
            env.state
                .get_account(&alice)
                .unwrap()
                .unwrap()
                .native_balance(),
            50
        );
        assert_eq!(
            snapshot
                .get_account(&alice)
                .unwrap()
                .unwrap()
                .native_balance(),
            100
        );
        assert_eq!(snapshot.version(), 1);
        assert_eq!(snapshot.state_root(), before.0);
        assert_ne!(snapshot.state_root(), env.state.state_root().unwrap());
    }

    #[test]
    fn crate_manifest_does_not_depend_on_rocksdb() {
        let manifest = include_str!("../Cargo.toml");
        assert!(manifest.contains("rsc-storage"));
        assert!(manifest.contains("rsc-crypto"));
        assert!(manifest.contains("rsc-core"));
        assert!(!manifest.contains("rocksdb"));
        assert!(!manifest.contains("sha2"));
        assert!(!manifest.contains("ed25519"));
        assert!(!manifest.contains("rsc-consensus"));
        assert!(!manifest.contains("rsc-p2p"));
        assert!(!manifest.contains("rsc-rpc"));
        assert!(!manifest.contains("rsc-execution"));
        let balance = Account::new(owner(1)).native_balance();
        assert_eq!(balance, 0u128);
        let nonce = Account::new(owner(1)).nonce();
        assert_eq!(nonce, 0u64);
    }
}
