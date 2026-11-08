//! Cabecera estructural. No firma, no elige proposer, no lee el reloj.

use rsc_core::{AccountId, BlockHash, BlockHeight, ChainId, Hash, Timestamp};

/// Etiqueta del bloque. El hash del bloque se calcula sobre su encoding canónico.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockHeader {
    /// Versión de interpretación del bloque, no de la aplicación.
    pub version: u32,
    /// Red a la que pertenece el bloque.
    pub chain_id: ChainId,
    /// Posición. Genesis = 0.
    pub height: BlockHeight,
    /// Hash del padre. Genesis = `BlockHash::ZERO`.
    pub previous_block_hash: BlockHash,
    /// Timestamp del productor. Input, no `SystemTime::now()`.
    pub timestamp: Timestamp,
    /// Raíz Merkle de las transacciones.
    pub transactions_root: Hash,
    /// Raíz de estado recibida. Este crate no la calcula.
    pub state_root: Hash,
    /// Quien propone. La autorización es de consenso.
    pub proposer: AccountId,
}

impl BlockHeader {
    /// Genesis estructural: altura 0 y padre cero.
    pub fn is_genesis(&self) -> bool {
        self.height == BlockHeight::GENESIS && self.previous_block_hash == BlockHash::ZERO
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_fields_and_genesis() {
        let h = BlockHeader {
            version: 1,
            chain_id: ChainId(1),
            height: BlockHeight::GENESIS,
            previous_block_hash: BlockHash::ZERO,
            timestamp: Timestamp::from_secs(0),
            transactions_root: Hash::ZERO,
            state_root: Hash::ZERO,
            proposer: AccountId::ZERO,
        };
        assert!(h.is_genesis());
        assert_eq!(h.version, 1);
        assert_eq!(h.chain_id, ChainId(1));
        assert_eq!(h.height, BlockHeight(0));
        assert_eq!(h.timestamp, Timestamp::from_secs(0));
        assert_eq!(h.state_root, Hash::ZERO);
        assert_eq!(h.proposer, AccountId::ZERO);
        let mut child = h.clone();
        child.height = BlockHeight(1);
        child.previous_block_hash = BlockHash(Hash([1; 32]));
        assert!(!child.is_genesis());
    }
}
