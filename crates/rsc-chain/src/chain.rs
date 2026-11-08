//! DAG de bloques. Inserta y navega. No elige la fork canónica.

use crate::block::Block;
use crate::errors::ChainError;
use crate::store::ChainStore;
use crate::validation::{validate_block, ChainValidationContext};
use rsc_core::{BlockHash, BlockHeight};

/// Resultado de insertar un bloque ya validado.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InsertStatus {
    /// Nuevo bloque persistido.
    Inserted,
    /// Ya estaba en el store.
    AlreadyKnown,
}

/// Vista estructural sobre un [`ChainStore`].
pub struct Chain<S> {
    store: S,
}

impl<S: ChainStore> Chain<S> {
    /// Envuelve un store.
    pub fn new(store: S) -> Self {
        Self { store }
    }

    /// Valida y, solo si pasa, persiste. No toca el tip canónico.
    pub fn insert(
        &self,
        block: &Block,
        ctx: &ChainValidationContext,
    ) -> Result<InsertStatus, ChainError> {
        let hash = block.block_hash()?;
        if self
            .store
            .get_block(&hash)
            .map_err(|_| ChainError::Store)?
            .is_some()
        {
            return Ok(InsertStatus::AlreadyKnown);
        }
        validate_block(block, ctx)?;
        self.store.put_block(block).map_err(|_| ChainError::Store)?;
        Ok(InsertStatus::Inserted)
    }

    /// Obtiene un bloque.
    pub fn get_block(&self, hash: &BlockHash) -> Result<Option<Block>, ChainError> {
        self.store.get_block(hash).map_err(|_| ChainError::Store)
    }

    /// ¿Existe el hash?
    pub fn contains(&self, hash: &BlockHash) -> Result<bool, ChainError> {
        Ok(self.get_block(hash)?.is_some())
    }

    /// Padre estructural, o `None` en genesis.
    pub fn parent(&self, hash: &BlockHash) -> Result<Option<BlockHash>, ChainError> {
        let block = self.get_block(hash)?.ok_or(ChainError::BlockNotFound)?;
        if block.header.height == BlockHeight::GENESIS {
            Ok(None)
        } else {
            Ok(Some(block.header.previous_block_hash))
        }
    }

    /// Hijos conocidos (todas las forks).
    pub fn children(&self, hash: &BlockHash) -> Result<Vec<BlockHash>, ChainError> {
        self.store.get_children(hash).map_err(|_| ChainError::Store)
    }

    /// Ancestros hacia genesis, sin incluir `hash`.
    pub fn ancestors(&self, hash: &BlockHash) -> Result<Vec<BlockHash>, ChainError> {
        let mut out = Vec::new();
        let mut current = *hash;
        loop {
            match self.parent(&current)? {
                Some(parent) => {
                    out.push(parent);
                    current = parent;
                }
                None => return Ok(out),
            }
        }
    }

    /// Hash canónico a esa altura, según el tip fijado por consenso.
    pub fn get_block_by_height(
        &self,
        height: BlockHeight,
    ) -> Result<Option<BlockHash>, ChainError> {
        self.store
            .get_block_by_height(height)
            .map_err(|_| ChainError::Store)
    }

    /// Tip canónico.
    pub fn canonical_tip(&self) -> Result<Option<BlockHash>, ChainError> {
        self.store
            .get_canonical_tip()
            .map_err(|_| ChainError::Store)
    }

    /// Fija el tip. El bloque debe existir. No compara stakes.
    pub fn set_canonical_tip(&self, hash: &BlockHash) -> Result<(), ChainError> {
        if !self.contains(hash)? {
            return Err(ChainError::BlockNotFound);
        }
        self.store
            .set_canonical_tip(hash)
            .map_err(|_| ChainError::Store)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::RawTransaction;
    use crate::codec::header_hash;
    use crate::header::BlockHeader;
    use crate::store::ChainStore;
    use rsc_core::{AccountId, ChainId, Hash, Timestamp};
    use std::cell::RefCell;
    use std::collections::HashMap;

    struct MemoryStore {
        blocks: RefCell<HashMap<BlockHash, Block>>,
        children: RefCell<HashMap<BlockHash, Vec<BlockHash>>>,
        tip: RefCell<Option<BlockHash>>,
    }

    impl MemoryStore {
        fn new() -> Self {
            Self {
                blocks: RefCell::new(HashMap::new()),
                children: RefCell::new(HashMap::new()),
                tip: RefCell::new(None),
            }
        }
    }

    impl ChainStore for MemoryStore {
        type Error = ();

        fn get_block(&self, hash: &BlockHash) -> Result<Option<Block>, ()> {
            Ok(self.blocks.borrow().get(hash).cloned())
        }

        fn put_block(&self, block: &Block) -> Result<(), ()> {
            let hash = block.block_hash().map_err(|_| ())?;
            self.blocks.borrow_mut().insert(hash, block.clone());
            let parent = block.header.previous_block_hash;
            let mut children = self.children.borrow_mut();
            let list = children.entry(parent).or_default();
            if !list.contains(&hash) {
                list.push(hash);
            }
            Ok(())
        }

        fn get_block_by_height(&self, height: BlockHeight) -> Result<Option<BlockHash>, ()> {
            let Some(mut current) = *self.tip.borrow() else {
                return Ok(None);
            };
            loop {
                let Some(block) = self.blocks.borrow().get(&current).cloned() else {
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

        fn get_canonical_tip(&self) -> Result<Option<BlockHash>, ()> {
            Ok(*self.tip.borrow())
        }

        fn set_canonical_tip(&self, hash: &BlockHash) -> Result<(), ()> {
            *self.tip.borrow_mut() = Some(*hash);
            Ok(())
        }

        fn get_children(&self, hash: &BlockHash) -> Result<Vec<BlockHash>, ()> {
            Ok(self
                .children
                .borrow()
                .get(hash)
                .cloned()
                .unwrap_or_default())
        }
    }

    fn block(height: u64, parent: BlockHash, txs: &[&[u8]]) -> Block {
        let mut b = Block {
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
            transactions: txs.iter().map(|t| RawTransaction(t.to_vec())).collect(),
        };
        b.header.transactions_root = b.compute_transaction_root();
        b
    }

    fn ctx(parent: Option<BlockHeader>) -> ChainValidationContext {
        ChainValidationContext {
            expected_chain_id: ChainId(1),
            expected_protocol_version: 1,
            max_block_size: 4096,
            max_transactions_per_block: 16,
            parent,
        }
    }

    #[test]
    fn stores_forks_without_picking_a_winner() {
        let chain = Chain::new(MemoryStore::new());
        let genesis = block(0, BlockHash::ZERO, &[]);
        assert_eq!(
            chain.insert(&genesis, &ctx(None)).unwrap(),
            InsertStatus::Inserted
        );
        assert_eq!(
            chain.insert(&genesis, &ctx(None)).unwrap(),
            InsertStatus::AlreadyKnown
        );

        let g_hash = genesis.block_hash().unwrap();
        let a = block(1, g_hash, &[b"a"]);
        assert_eq!(
            chain
                .insert(&a, &ctx(Some(genesis.header.clone())))
                .unwrap(),
            InsertStatus::Inserted
        );
        let a_hash = a.block_hash().unwrap();
        assert_eq!(
            a.header.previous_block_hash,
            header_hash(&genesis.header).unwrap()
        );

        let b = block(2, a_hash, &[b"b"]);
        let c = block(2, a_hash, &[b"c"]);
        assert_eq!(
            chain.insert(&b, &ctx(Some(a.header.clone()))).unwrap(),
            InsertStatus::Inserted
        );
        assert_eq!(
            chain.insert(&c, &ctx(Some(a.header.clone()))).unwrap(),
            InsertStatus::Inserted
        );

        let b_hash = b.block_hash().unwrap();
        let c_hash = c.block_hash().unwrap();
        assert_ne!(b_hash, c_hash);
        assert!(chain.contains(&b_hash).unwrap());
        assert!(chain.contains(&c_hash).unwrap());
        assert_eq!(chain.canonical_tip().unwrap(), None);

        let kids = chain.children(&a_hash).unwrap();
        assert_eq!(kids.len(), 2);
        assert!(kids.contains(&b_hash) && kids.contains(&c_hash));

        chain.set_canonical_tip(&b_hash).unwrap();
        assert_eq!(chain.canonical_tip().unwrap(), Some(b_hash));
        assert!(chain.contains(&c_hash).unwrap());
        assert_eq!(
            chain.get_block_by_height(BlockHeight(2)).unwrap(),
            Some(b_hash)
        );
        assert_eq!(chain.ancestors(&b_hash).unwrap(), vec![a_hash, g_hash]);
        assert_eq!(chain.parent(&g_hash).unwrap(), None);
    }
}
