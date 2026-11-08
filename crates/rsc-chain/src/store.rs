//! Interfaz de persistencia. Sin RocksDB, Postgres ni HTTP.

use crate::block::Block;
use rsc_core::{BlockHash, BlockHeight};

/// Backend de bloques. La implementación concreta vive en `rsc-storage`.
pub trait ChainStore {
    /// Error del backend, opaco para este crate.
    type Error;

    /// Obtiene un bloque por hash.
    fn get_block(&self, hash: &BlockHash) -> Result<Option<Block>, Self::Error>;
    /// Guarda un bloque ya validado.
    fn put_block(&self, block: &Block) -> Result<(), Self::Error>;
    /// Hash canónico a esa altura, si el tip lo alcanza.
    fn get_block_by_height(&self, height: BlockHeight) -> Result<Option<BlockHash>, Self::Error>;
    /// Tip canónico fijado por consenso.
    fn get_canonical_tip(&self) -> Result<Option<BlockHash>, Self::Error>;
    /// Fija el tip. No elige la fork.
    fn set_canonical_tip(&self, hash: &BlockHash) -> Result<(), Self::Error>;
    /// Hijos conocidos de un bloque. Necesario para navegar forks.
    fn get_children(&self, hash: &BlockHash) -> Result<Vec<BlockHash>, Self::Error>;
}
