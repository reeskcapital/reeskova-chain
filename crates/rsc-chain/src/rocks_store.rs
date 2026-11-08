//! Adaptador de [`ChainStore`] sobre `rsc-storage`.
//!
//! Persiste bloques y el índice de hijos. La altura canónica se recorre
//! desde el tip, igual que el store en memoria. No es un segundo índice.

use crate::block::Block;
use crate::codec::{decode_block, encode_block};
use crate::errors::ChainError;
use crate::store::ChainStore;
use rsc_core::{BlockHash, BlockHeight, Hash};
use rsc_storage::{ColumnFamily, RscDatabase, StorageBatch};
use std::sync::Arc;

const TIP_KEY: &[u8] = b"canonical-tip";
const CHILD_PREFIX: &[u8] = b"ch/";

/// `ChainStore` sobre una [`RscDatabase`] ya abierta y validada.
pub struct RocksDbChainStore {
    database: Arc<RscDatabase>,
}

impl RocksDbChainStore {
    /// Toma la base ya validada. No vuelve a abrir ni a comprobar la identidad.
    pub fn new(database: RscDatabase) -> Self {
        Self::from_shared(Arc::new(database))
    }

    /// Comparte la base con otro dueño, por ejemplo el estado.
    pub fn from_shared(database: Arc<RscDatabase>) -> Self {
        Self { database }
    }

    /// La misma base que usa el store.
    pub fn database(&self) -> Arc<RscDatabase> {
        Arc::clone(&self.database)
    }

    fn hash_bytes(hash: &BlockHash) -> &[u8; 32] {
        &hash.0 .0
    }

    fn hash_from_bytes(bytes: &[u8]) -> Result<BlockHash, ChainError> {
        let raw: [u8; 32] = bytes.try_into().map_err(|_| ChainError::InvalidEncoding)?;
        Ok(BlockHash(Hash(raw)))
    }

    fn child_key(parent: &BlockHash) -> Vec<u8> {
        let mut key = Vec::with_capacity(CHILD_PREFIX.len() + 32);
        key.extend_from_slice(CHILD_PREFIX);
        key.extend_from_slice(Self::hash_bytes(parent));
        key
    }

    /// Añade el bloque, el índice de hijos y el tip canónico a `batch`. No escribe.
    ///
    /// El llamador publica el lote. El bloque y el tip salen juntos.
    pub fn stage_canonical(
        &self,
        batch: &mut StorageBatch,
        block: &Block,
    ) -> Result<BlockHash, ChainError> {
        let hash = block.block_hash()?;
        let bytes = encode_block(block)?;
        let parent = block.header.previous_block_hash;
        let mut children = self.read_children(&parent)?;
        if !children.contains(&hash) {
            children.push(hash);
        }
        batch.put(ColumnFamily::Blocks, Self::hash_bytes(&hash), &bytes);
        batch.put(
            ColumnFamily::Default,
            &Self::child_key(&parent),
            &encode_hash_list(&children),
        );
        batch.put(ColumnFamily::Default, TIP_KEY, Self::hash_bytes(&hash));
        Ok(hash)
    }

    fn read_children(&self, parent: &BlockHash) -> Result<Vec<BlockHash>, ChainError> {
        let raw = self
            .database
            .get(ColumnFamily::Default, &Self::child_key(parent))
            .map_err(|_| ChainError::Store)?;
        let Some(raw) = raw else {
            return Ok(Vec::new());
        };
        decode_hash_list(raw.as_bytes())
    }
}

impl ChainStore for RocksDbChainStore {
    type Error = ChainError;

    fn get_block(&self, hash: &BlockHash) -> Result<Option<Block>, ChainError> {
        let value = self
            .database
            .get(ColumnFamily::Blocks, Self::hash_bytes(hash))
            .map_err(|_| ChainError::Store)?;
        match value {
            None => Ok(None),
            Some(value) => decode_block(value.as_bytes()).map(Some),
        }
    }

    fn put_block(&self, block: &Block) -> Result<(), ChainError> {
        let hash = block.block_hash()?;
        let bytes = encode_block(block)?;
        let parent = block.header.previous_block_hash;
        let mut children = self.read_children(&parent)?;
        if !children.contains(&hash) {
            children.push(hash);
        }
        let mut batch = StorageBatch::new();
        batch.put(ColumnFamily::Blocks, Self::hash_bytes(&hash), &bytes);
        batch.put(
            ColumnFamily::Default,
            &Self::child_key(&parent),
            &encode_hash_list(&children),
        );
        self.database.write(batch).map_err(|_| ChainError::Store)
    }

    fn get_block_by_height(&self, height: BlockHeight) -> Result<Option<BlockHash>, ChainError> {
        let Some(mut current) = self.get_canonical_tip()? else {
            return Ok(None);
        };
        loop {
            let Some(block) = self.get_block(&current)? else {
                return Ok(None);
            };
            if block.header.height == height {
                return Ok(Some(current));
            }
            if block.header.height == BlockHeight::GENESIS {
                return Ok(None);
            }
            current = block.header.previous_block_hash;
        }
    }

    fn get_canonical_tip(&self) -> Result<Option<BlockHash>, ChainError> {
        let value = self
            .database
            .get(ColumnFamily::Default, TIP_KEY)
            .map_err(|_| ChainError::Store)?;
        match value {
            None => Ok(None),
            Some(value) => Self::hash_from_bytes(value.as_bytes()).map(Some),
        }
    }

    fn set_canonical_tip(&self, hash: &BlockHash) -> Result<(), ChainError> {
        self.database
            .put(ColumnFamily::Default, TIP_KEY, Self::hash_bytes(hash))
            .map_err(|_| ChainError::Store)
    }

    fn get_children(&self, hash: &BlockHash) -> Result<Vec<BlockHash>, ChainError> {
        self.read_children(hash)
    }
}

fn encode_hash_list(hashes: &[BlockHash]) -> Vec<u8> {
    let mut out = Vec::with_capacity(hashes.len() * 32);
    for hash in hashes {
        out.extend_from_slice(&hash.0 .0);
    }
    out
}

fn decode_hash_list(bytes: &[u8]) -> Result<Vec<BlockHash>, ChainError> {
    if !bytes.len().is_multiple_of(32) {
        return Err(ChainError::InvalidEncoding);
    }
    bytes
        .chunks(32)
        .map(|chunk| {
            let raw: [u8; 32] = chunk.try_into().map_err(|_| ChainError::InvalidEncoding)?;
            Ok(BlockHash(Hash(raw)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::RawTransaction;
    use crate::header::BlockHeader;
    use rsc_core::{AccountId, ChainId, Timestamp};
    use rsc_storage::{ColumnFamily, DatabaseConfig, RscDatabase, StorageError};

    fn config() -> DatabaseConfig {
        DatabaseConfig {
            max_value_size: 1024 * 1024,
            network_id: "devnet".to_string(),
            chain_id: "rsc-dev".to_string(),
        }
    }

    fn tiny_config() -> DatabaseConfig {
        let mut config = config();
        config.max_value_size = 8;
        config
    }

    fn sample(height: u64, parent: BlockHash) -> Block {
        let mut block = Block {
            header: BlockHeader {
                version: 1,
                chain_id: ChainId(1),
                height: BlockHeight(height),
                previous_block_hash: parent,
                timestamp: Timestamp::from_secs(1),
                transactions_root: Hash::ZERO,
                state_root: Hash::ZERO,
                proposer: AccountId::ZERO,
            },
            transactions: vec![RawTransaction(b"tx".to_vec())],
        };
        block.header.transactions_root = block.compute_transaction_root();
        block
    }

    fn open_store(dir: &std::path::Path) -> RocksDbChainStore {
        RocksDbChainStore::new(RscDatabase::open(dir, config()).unwrap())
    }

    #[test]
    fn open_store_and_missing_records() {
        let dir = tempfile::tempdir().unwrap();
        let store = open_store(dir.path());
        let hash = BlockHash::ZERO;
        assert_eq!(store.get_block(&hash).unwrap(), None);
        assert_eq!(store.get_canonical_tip().unwrap(), None);
        assert_eq!(store.get_block_by_height(BlockHeight(0)).unwrap(), None);
        assert!(store.get_children(&hash).unwrap().is_empty());
    }

    #[test]
    fn store_retrieve_and_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let genesis = sample(0, BlockHash::ZERO);
        let hash = genesis.block_hash().unwrap();
        {
            let store = open_store(&path);
            store.put_block(&genesis).unwrap();
            store.set_canonical_tip(&hash).unwrap();
        }
        let store = open_store(&path);
        assert_eq!(store.get_block(&hash).unwrap().as_ref(), Some(&genesis));
        assert_eq!(store.get_canonical_tip().unwrap(), Some(hash));
        assert_eq!(
            store.get_block_by_height(BlockHeight(0)).unwrap(),
            Some(hash)
        );
    }

    #[test]
    fn height_follows_canonical_tip() {
        let dir = tempfile::tempdir().unwrap();
        let store = open_store(dir.path());
        let genesis = sample(0, BlockHash::ZERO);
        let genesis_hash = genesis.block_hash().unwrap();
        let child = sample(1, genesis_hash);
        let child_hash = child.block_hash().unwrap();
        store.put_block(&genesis).unwrap();
        store.put_block(&child).unwrap();
        assert_eq!(store.get_block_by_height(BlockHeight(1)).unwrap(), None);
        store.set_canonical_tip(&child_hash).unwrap();
        assert_eq!(
            store.get_block_by_height(BlockHeight(1)).unwrap(),
            Some(child_hash)
        );
        assert_eq!(
            store.get_block_by_height(BlockHeight(0)).unwrap(),
            Some(genesis_hash)
        );
        assert_eq!(store.get_children(&genesis_hash).unwrap(), vec![child_hash]);
    }

    #[test]
    fn corrupt_block_bytes_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let hash = BlockHash(Hash([9; 32]));
        {
            let db = RscDatabase::open(&path, config()).unwrap();
            db.put(ColumnFamily::Blocks, &hash.0 .0, b"not-a-block")
                .unwrap();
        }
        let store = open_store(&path);
        let error = store.get_block(&hash).unwrap_err();
        assert_eq!(error, ChainError::InvalidEncoding);
    }

    #[test]
    fn corrupt_children_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let parent = BlockHash(Hash([3; 32]));
        {
            let db = RscDatabase::open(&path, config()).unwrap();
            let mut key = b"ch/".to_vec();
            key.extend_from_slice(&parent.0 .0);
            db.put(ColumnFamily::Default, &key, b"short").unwrap();
        }
        let store = open_store(&path);
        assert_eq!(
            store.get_children(&parent).unwrap_err(),
            ChainError::InvalidEncoding
        );
    }

    #[test]
    fn failed_batch_leaves_no_partial_block() {
        let dir = tempfile::tempdir().unwrap();
        let db = RscDatabase::open(dir.path(), tiny_config()).unwrap();
        let store = RocksDbChainStore::new(db);
        let block = sample(0, BlockHash::ZERO);
        let hash = block.block_hash().unwrap();
        assert_eq!(store.put_block(&block).unwrap_err(), ChainError::Store);
        assert_eq!(store.get_block(&hash).unwrap(), None);
        assert!(store
            .get_children(&block.header.previous_block_hash)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn duplicate_put_does_not_duplicate_child() {
        let dir = tempfile::tempdir().unwrap();
        let store = open_store(dir.path());
        let block = sample(1, BlockHash::ZERO);
        let hash = block.block_hash().unwrap();
        store.put_block(&block).unwrap();
        store.put_block(&block).unwrap();
        assert_eq!(store.get_children(&BlockHash::ZERO).unwrap(), vec![hash]);
    }

    #[test]
    fn storage_key_is_the_block_hash() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let block = sample(0, BlockHash::ZERO);
        let hash = block.block_hash().unwrap();
        {
            let store = open_store(&path);
            store.put_block(&block).unwrap();
        }
        let db = RscDatabase::open(&path, config()).unwrap();
        let stored = db.get(ColumnFamily::Blocks, &hash.0 .0).unwrap().unwrap();
        assert_eq!(stored.as_bytes(), encode_block(&block).unwrap());
        let again = sample(0, BlockHash::ZERO);
        assert_eq!(again.block_hash().unwrap().0 .0, hash.0 .0);
    }

    #[test]
    fn identity_mismatch_is_rejected_by_storage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        {
            let _db = RscDatabase::open(&path, config()).unwrap();
        }
        let mut other = config();
        other.chain_id = "other".to_string();
        let opened = RscDatabase::open(&path, other);
        match opened {
            Ok(_db) => panic!("expected chain mismatch"),
            Err(error) => assert!(matches!(error, StorageError::ChainMismatch)),
        }
    }
}
