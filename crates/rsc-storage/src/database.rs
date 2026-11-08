//! Base de bytes, column families y lotes atómicos. RocksDB queda encapsulado aquí.

use rocksdb::{
    ColumnFamilyDescriptor, DBWithThreadMode, IteratorMode, MultiThreaded, Options, WriteBatch,
    WriteOptions,
};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Versión de esquema físico V1. No se sube para hacer compilar el crate.
pub const STORAGE_SCHEMA_VERSION: u32 = 1;

const KEY_SCHEMA_VERSION: &[u8] = b"schema/version";
const KEY_NETWORK_ID: &[u8] = b"network/id";
const KEY_CHAIN_ID: &[u8] = b"chain/id";

/// Column families fijas de V1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColumnFamily {
    /// Familia por defecto de RocksDB.
    Default,
    /// Identidad de esquema, red y cadena.
    Metadata,
    /// Bloques por hash.
    Blocks,
    /// Hash canónico por altura.
    BlockByHeight,
    /// Transacciones por id.
    Transactions,
    /// Ubicación de una transacción.
    TransactionLocations,
    /// Recibos de ejecución.
    Receipts,
}

impl ColumnFamily {
    /// Nombre estable en disco.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Metadata => "metadata",
            Self::Blocks => "blocks",
            Self::BlockByHeight => "block_by_height",
            Self::Transactions => "transactions",
            Self::TransactionLocations => "transaction_locations",
            Self::Receipts => "receipts",
        }
    }

    /// Las siete familias, en orden fijo.
    pub const fn all() -> [Self; 7] {
        [
            Self::Default,
            Self::Metadata,
            Self::Blocks,
            Self::BlockByHeight,
            Self::Transactions,
            Self::TransactionLocations,
            Self::Receipts,
        ]
    }
}

/// Límites e identidad con los que se abre una base.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatabaseConfig {
    /// Tamaño máximo de un valor, en bytes. No se trunca.
    pub max_value_size: usize,
    /// Identidad de red persistida en `network/id`.
    pub network_id: String,
    /// Identidad de cadena persistida en `chain/id`.
    pub chain_id: String,
}

/// Clave de almacenamiento. Nunca vacía.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct StorageKey(Vec<u8>);

impl StorageKey {
    /// Bytes de la clave.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl AsRef<[u8]> for StorageKey {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

/// Valor almacenado.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageValue(Vec<u8>);

impl StorageValue {
    /// Bytes del valor.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl AsRef<[u8]> for StorageValue {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

/// Fallo de la capa física. El `Display` no incluye rutas ni valores.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// El motor rechazó la operación.
    #[error("storage backend error")]
    Backend(#[source] Box<rocksdb::Error>),
    /// La base no tiene la versión de esquema esperada.
    #[error("storage schema mismatch")]
    SchemaMismatch,
    /// La base pertenece a otra red.
    #[error("storage network mismatch")]
    NetworkMismatch,
    /// La base pertenece a otra cadena.
    #[error("storage chain mismatch")]
    ChainMismatch,
    /// Falta metadata de identidad.
    #[error("storage metadata missing")]
    MetadataMissing,
    /// Falta una column family de V1.
    #[error("storage column family missing")]
    MissingColumnFamily,
    /// El valor supera [`DatabaseConfig::max_value_size`].
    #[error("storage value too large")]
    ValueTooLarge,
    /// La clave está vacía.
    #[error("storage key empty")]
    EmptyKey,
    /// La ruta contiene `..` u otro componente rechazado.
    #[error("storage path rejected")]
    InvalidPath,
}

impl StorageError {
    pub(crate) fn backend(error: rocksdb::Error) -> Self {
        Self::Backend(Box::new(error))
    }
}

enum BatchOp {
    Put {
        cf: ColumnFamily,
        key: Vec<u8>,
        value: Vec<u8>,
    },
    Delete {
        cf: ColumnFamily,
        key: Vec<u8>,
    },
}

/// Lote de `put` y `delete` que se aplica en un único commit.
#[derive(Default)]
pub struct StorageBatch {
    ops: Vec<BatchOp>,
}

impl StorageBatch {
    /// Lote vacío.
    pub fn new() -> Self {
        Self { ops: Vec::new() }
    }

    /// Programa una escritura. No toca la base hasta [`RscDatabase::write`].
    pub fn put(&mut self, cf: ColumnFamily, key: &[u8], value: &[u8]) {
        self.ops.push(BatchOp::Put {
            cf,
            key: key.to_vec(),
            value: value.to_vec(),
        });
    }

    /// Programa un borrado. No toca la base hasta [`RscDatabase::write`].
    pub fn delete(&mut self, cf: ColumnFamily, key: &[u8]) {
        self.ops.push(BatchOp::Delete {
            cf,
            key: key.to_vec(),
        });
    }

    /// Operaciones programadas.
    pub fn len(&self) -> usize {
        self.ops.len()
    }

    /// No hay operaciones programadas.
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }
}

type Db = DBWithThreadMode<MultiThreaded>;

/// Almacén físico de pares clave/valor.
///
/// No conoce cuentas ni raíces. Un lote se publica entero o no se publica.
pub struct RscDatabase {
    db: Db,
    commits: AtomicU64,
    config: DatabaseConfig,
    path: PathBuf,
}

impl RscDatabase {
    /// Abre o crea la base en `path`.
    ///
    /// Una base nueva recibe metadata V1. Una base existente se valida y no se repara.
    pub fn open(path: impl AsRef<Path>, config: DatabaseConfig) -> Result<Self, StorageError> {
        let path = path.as_ref();
        reject_parent_components(path)?;
        let exists = rocksdb_exists(path);
        if exists {
            require_column_families(path)?;
        }
        let db = open_db(path)?;
        let database = Self {
            db,
            commits: AtomicU64::new(0),
            config,
            path: path.to_path_buf(),
        };
        if exists {
            database.validate_metadata()?;
        } else {
            database.initialize_metadata()?;
        }
        Ok(database)
    }

    /// Lee una clave. `Ok(None)` si no existe.
    pub fn get(&self, cf: ColumnFamily, key: &[u8]) -> Result<Option<StorageValue>, StorageError> {
        reject_empty_key(key)?;
        let handle = self.cf(cf)?;
        let value = self
            .db
            .get_cf(&handle, key)
            .map_err(StorageError::backend)?;
        Ok(value.map(StorageValue))
    }

    /// Escribe una clave fuera de un lote. Incrementa [`Self::commit_count`].
    pub fn put(&self, cf: ColumnFamily, key: &[u8], value: &[u8]) -> Result<(), StorageError> {
        reject_empty_key(key)?;
        self.reject_value(value)?;
        let handle = self.cf(cf)?;
        self.db
            .put_cf_opt(&handle, key, value, &sync_options())
            .map_err(StorageError::backend)?;
        self.commits.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    /// Borra una clave fuera de un lote. Incrementa [`Self::commit_count`].
    pub fn delete(&self, cf: ColumnFamily, key: &[u8]) -> Result<(), StorageError> {
        reject_empty_key(key)?;
        let handle = self.cf(cf)?;
        self.db
            .delete_cf_opt(&handle, key, &sync_options())
            .map_err(StorageError::backend)?;
        self.commits.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    /// Aplica el lote de forma atómica. Un fallo no publica ninguna operación.
    pub fn write(&self, batch: StorageBatch) -> Result<(), StorageError> {
        let mut native = WriteBatch::default();
        for op in &batch.ops {
            match op {
                BatchOp::Put { cf, key, value } => {
                    reject_empty_key(key)?;
                    self.reject_value(value)?;
                    let handle = self.cf(*cf)?;
                    native.put_cf(&handle, key, value);
                }
                BatchOp::Delete { cf, key } => {
                    reject_empty_key(key)?;
                    let handle = self.cf(*cf)?;
                    native.delete_cf(&handle, key);
                }
            }
        }
        self.db
            .write_opt(native, &sync_options())
            .map_err(StorageError::backend)?;
        self.commits.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    /// Pares bajo `prefix`, en orden lexicográfico de clave.
    pub fn scan(
        &self,
        cf: ColumnFamily,
        prefix: &[u8],
    ) -> Result<Vec<(StorageKey, StorageValue)>, StorageError> {
        let handle = self.cf(cf)?;
        let mode = IteratorMode::From(prefix, rocksdb::Direction::Forward);
        let mut out = Vec::new();
        for item in self.db.iterator_cf(&handle, mode) {
            let (key, value) = item.map_err(StorageError::backend)?;
            if !key.starts_with(prefix) {
                break;
            }
            out.push((StorageKey(key.into_vec()), StorageValue(value.into_vec())));
        }
        Ok(out)
    }

    /// Número de `put`, `delete` y `write` confirmados por esta instancia.
    pub fn commit_count(&self) -> u64 {
        self.commits.load(Ordering::SeqCst)
    }

    pub(crate) fn raw_db(&self) -> &Db {
        &self.db
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn config(&self) -> &DatabaseConfig {
        &self.config
    }

    fn cf(
        &self,
        cf: ColumnFamily,
    ) -> Result<std::sync::Arc<rocksdb::BoundColumnFamily<'_>>, StorageError> {
        self.db
            .cf_handle(cf.name())
            .ok_or(StorageError::MissingColumnFamily)
    }

    fn reject_value(&self, value: &[u8]) -> Result<(), StorageError> {
        if value.len() > self.config.max_value_size {
            Err(StorageError::ValueTooLarge)
        } else {
            Ok(())
        }
    }

    fn initialize_metadata(&self) -> Result<(), StorageError> {
        let mut batch = StorageBatch::new();
        batch.put(
            ColumnFamily::Metadata,
            KEY_SCHEMA_VERSION,
            STORAGE_SCHEMA_VERSION.to_string().as_bytes(),
        );
        batch.put(
            ColumnFamily::Metadata,
            KEY_NETWORK_ID,
            self.config.network_id.as_bytes(),
        );
        batch.put(
            ColumnFamily::Metadata,
            KEY_CHAIN_ID,
            self.config.chain_id.as_bytes(),
        );
        self.write(batch)
    }

    fn validate_metadata(&self) -> Result<(), StorageError> {
        let version = self.metadata_bytes(KEY_SCHEMA_VERSION)?;
        let network = self.metadata_bytes(KEY_NETWORK_ID)?;
        let chain = self.metadata_bytes(KEY_CHAIN_ID)?;
        let version = std::str::from_utf8(&version).map_err(|_| StorageError::SchemaMismatch)?;
        let parsed: u32 = version.parse().map_err(|_| StorageError::SchemaMismatch)?;
        if parsed != STORAGE_SCHEMA_VERSION {
            return Err(StorageError::SchemaMismatch);
        }
        if network != self.config.network_id.as_bytes() {
            return Err(StorageError::NetworkMismatch);
        }
        if chain != self.config.chain_id.as_bytes() {
            return Err(StorageError::ChainMismatch);
        }
        Ok(())
    }

    fn metadata_bytes(&self, key: &[u8]) -> Result<Vec<u8>, StorageError> {
        self.get(ColumnFamily::Metadata, key)?
            .map(|value| value.0)
            .ok_or(StorageError::MetadataMissing)
    }
}

fn sync_options() -> WriteOptions {
    let mut options = WriteOptions::default();
    options.set_sync(true);
    options
}

fn db_options() -> Options {
    let mut options = Options::default();
    options.create_if_missing(true);
    options.create_missing_column_families(true);
    options
}

fn descriptors() -> Vec<ColumnFamilyDescriptor> {
    ColumnFamily::all()
        .into_iter()
        .map(|cf| ColumnFamilyDescriptor::new(cf.name(), Options::default()))
        .collect()
}

fn open_db(path: &Path) -> Result<Db, StorageError> {
    let options = db_options();
    Db::open_cf_descriptors(&options, path, descriptors()).map_err(StorageError::backend)
}

fn rocksdb_exists(path: &Path) -> bool {
    path.join("CURRENT").is_file()
}

fn require_column_families(path: &Path) -> Result<(), StorageError> {
    let options = Options::default();
    let present = Db::list_cf(&options, path).map_err(StorageError::backend)?;
    for cf in ColumnFamily::all() {
        if !present.iter().any(|name| name == cf.name()) {
            return Err(StorageError::MissingColumnFamily);
        }
    }
    Ok(())
}

pub(crate) fn reject_parent_components(path: &Path) -> Result<(), StorageError> {
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        Err(StorageError::InvalidPath)
    } else {
        Ok(())
    }
}

fn reject_empty_key(key: &[u8]) -> Result<(), StorageError> {
    if key.is_empty() {
        Err(StorageError::EmptyKey)
    } else {
        Ok(())
    }
}

pub(crate) fn read_identity(db: &Db) -> Result<(u32, String, String), StorageError> {
    let metadata = db
        .cf_handle(ColumnFamily::Metadata.name())
        .ok_or(StorageError::MissingColumnFamily)?;
    let version = required_cf(db, &metadata, KEY_SCHEMA_VERSION)?;
    let network = required_cf(db, &metadata, KEY_NETWORK_ID)?;
    let chain = required_cf(db, &metadata, KEY_CHAIN_ID)?;
    let version = std::str::from_utf8(&version).map_err(|_| StorageError::SchemaMismatch)?;
    let parsed: u32 = version.parse().map_err(|_| StorageError::SchemaMismatch)?;
    let network = String::from_utf8(network).map_err(|_| StorageError::NetworkMismatch)?;
    let chain = String::from_utf8(chain).map_err(|_| StorageError::ChainMismatch)?;
    Ok((parsed, network, chain))
}

fn required_cf(
    db: &Db,
    handle: &impl rocksdb::AsColumnFamilyRef,
    key: &[u8],
) -> Result<Vec<u8>, StorageError> {
    db.get_cf(handle, key)
        .map_err(StorageError::backend)?
        .ok_or(StorageError::MetadataMissing)
}

pub(crate) fn open_read_only(path: &Path) -> Result<Db, StorageError> {
    require_column_families(path)?;
    let options = Options::default();
    Db::open_cf_descriptors_read_only(&options, path, descriptors(), false)
        .map_err(StorageError::backend)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> DatabaseConfig {
        DatabaseConfig {
            max_value_size: 32,
            network_id: "devnet".to_string(),
            chain_id: "rsc-dev".to_string(),
        }
    }

    fn expect_open_err(result: Result<RscDatabase, StorageError>) -> StorageError {
        match result {
            Ok(_db) => panic!("expected open to fail"),
            Err(error) => error,
        }
    }

    fn open_tmp() -> (tempfile::TempDir, RscDatabase) {
        let dir = tempfile::tempdir().unwrap();
        let db = RscDatabase::open(dir.path(), config()).unwrap();
        (dir, db)
    }

    #[test]
    fn database_opening_initializes_metadata() {
        let (_dir, db) = open_tmp();
        let version = db
            .get(ColumnFamily::Metadata, KEY_SCHEMA_VERSION)
            .unwrap()
            .unwrap();
        assert_eq!(version.as_bytes(), b"1");
        assert_eq!(
            db.get(ColumnFamily::Metadata, KEY_NETWORK_ID)
                .unwrap()
                .unwrap()
                .as_bytes(),
            b"devnet"
        );
    }

    #[test]
    fn path_traversal_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("..").join("escape");
        let error = expect_open_err(RscDatabase::open(&path, config()));
        assert!(matches!(error, StorageError::InvalidPath));
    }

    #[test]
    fn empty_key_rejected() {
        let (_dir, db) = open_tmp();
        let error = db.put(ColumnFamily::Blocks, b"", b"x").unwrap_err();
        assert!(matches!(error, StorageError::EmptyKey));
    }

    #[test]
    fn oversized_value_rejected() {
        let (_dir, db) = open_tmp();
        let error = db
            .put(ColumnFamily::Blocks, b"k", &[1; 33])
            .unwrap_err();
        assert!(matches!(error, StorageError::ValueTooLarge));
        assert!(db.get(ColumnFamily::Blocks, b"k").unwrap().is_none());
    }

    #[test]
    fn batch_is_atomic() {
        let (_dir, db) = open_tmp();
        let mut batch = StorageBatch::new();
        batch.put(ColumnFamily::Blocks, b"ok", b"1");
        batch.put(ColumnFamily::Blocks, b"big", &[9; 64]);
        assert_eq!(batch.len(), 2);
        assert!(!batch.is_empty());
        let error = db.write(batch).unwrap_err();
        assert!(matches!(error, StorageError::ValueTooLarge));
        assert!(db.get(ColumnFamily::Blocks, b"ok").unwrap().is_none());
        assert_eq!(db.commit_count(), 1);
    }

    #[test]
    fn reopen_preserves_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let db = RscDatabase::open(&path, config()).unwrap();
        db.put(ColumnFamily::Blocks, b"alpha", b"1").unwrap();
        drop(db);
        let db = RscDatabase::open(&path, config()).unwrap();
        assert_eq!(
            db.get(ColumnFamily::Blocks, b"alpha")
                .unwrap()
                .unwrap()
                .as_bytes(),
            b"1"
        );
    }

    #[test]
    fn missing_column_family_detected() {
        let dir = tempfile::tempdir().unwrap();
        {
            let mut options = Options::default();
            options.create_if_missing(true);
            let _db = rocksdb::DB::open(&options, dir.path()).unwrap();
        }
        let error = expect_open_err(RscDatabase::open(dir.path(), config()));
        assert!(matches!(error, StorageError::MissingColumnFamily));
    }

    #[test]
    fn schema_metadata_missing_detected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        {
            let db = RscDatabase::open(&path, config()).unwrap();
            db.delete(ColumnFamily::Metadata, KEY_SCHEMA_VERSION)
                .unwrap();
        }
        let error = expect_open_err(RscDatabase::open(&path, config()));
        assert!(matches!(error, StorageError::MetadataMissing));
    }

    #[test]
    fn schema_version_mismatch_detected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        {
            let db = RscDatabase::open(&path, config()).unwrap();
            db.put(ColumnFamily::Metadata, KEY_SCHEMA_VERSION, b"2")
                .unwrap();
        }
        let error = expect_open_err(RscDatabase::open(&path, config()));
        assert!(matches!(error, StorageError::SchemaMismatch));
    }

    #[test]
    fn network_mismatch_detected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        {
            let db = RscDatabase::open(&path, config()).unwrap();
            drop(db);
        }
        let mut other = config();
        other.network_id = "other".to_string();
        let error = expect_open_err(RscDatabase::open(&path, other));
        assert!(matches!(error, StorageError::NetworkMismatch));
    }

    #[test]
    fn chain_mismatch_detected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        {
            let _db = RscDatabase::open(&path, config()).unwrap();
        }
        let mut other = config();
        other.chain_id = "other-chain".to_string();
        let error = expect_open_err(RscDatabase::open(&path, other));
        assert!(matches!(error, StorageError::ChainMismatch));
    }

    #[test]
    fn scan_is_key_ordered() {
        let (_dir, db) = open_tmp();
        let mut batch = StorageBatch::new();
        batch.put(ColumnFamily::Blocks, b"m", b"2");
        batch.put(ColumnFamily::Blocks, b"a", b"1");
        batch.put(ColumnFamily::Blocks, b"z", b"3");
        db.write(batch).unwrap();
        let scanned = db.scan(ColumnFamily::Blocks, b"").unwrap();
        let keys: Vec<&[u8]> = scanned.iter().map(|(key, _)| key.as_bytes()).collect();
        assert_eq!(
            keys,
            vec![b"a".as_slice(), b"m".as_slice(), b"z".as_slice()]
        );
    }
}
