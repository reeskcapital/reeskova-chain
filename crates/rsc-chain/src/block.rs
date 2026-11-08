//! Bloque: header + payloads opacos. No interpreta transferencias.

use crate::codec::{encode_block, header_hash};
use crate::errors::ChainError;
use crate::header::BlockHeader;
use crate::merkle::{merkle_root, tx_leaf};
use rsc_core::{BlockHash, Hash};

/// Payload de transacción. `rsc-chain` no conoce su significado.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawTransaction(pub Vec<u8>);

/// Bloque estructural.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    /// Cabecera.
    pub header: BlockHeader,
    /// Transacciones en el orden del bloque.
    pub transactions: Vec<RawTransaction>,
}

impl Block {
    /// Número de transacciones.
    pub fn transaction_count(&self) -> usize {
        self.transactions.len()
    }

    /// Suma de bytes de los payloads. El límite de tamaño usa el encoding canónico.
    pub fn transactions_size(&self) -> usize {
        self.transactions.iter().map(|tx| tx.0.len()).sum()
    }

    /// Merkle de los payloads, en este orden.
    pub fn compute_transaction_root(&self) -> Hash {
        let leaves: Vec<Hash> = self.transactions.iter().map(|tx| tx_leaf(&tx.0)).collect();
        merkle_root(&leaves)
    }

    /// Hash del header canónico.
    pub fn block_hash(&self) -> Result<BlockHash, ChainError> {
        header_hash(&self.header)
    }

    /// Tamaño del encoding canónico.
    pub fn encoded_size(&self) -> Result<usize, ChainError> {
        encode_block(self).map(|b| b.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::{decode_block, encode_block};
    use rsc_core::{AccountId, BlockHeight, ChainId, Timestamp};

    fn sample(txs: Vec<&[u8]>) -> Block {
        let transactions: Vec<RawTransaction> = txs
            .into_iter()
            .map(|t| RawTransaction(t.to_vec()))
            .collect();
        let mut block = Block {
            header: BlockHeader {
                version: 1,
                chain_id: ChainId(1),
                height: BlockHeight(1),
                previous_block_hash: BlockHash::ZERO,
                timestamp: Timestamp::from_secs(1),
                transactions_root: Hash::ZERO,
                state_root: Hash::ZERO,
                proposer: AccountId::ZERO,
            },
            transactions,
        };
        block.header.transactions_root = block.compute_transaction_root();
        block
    }

    #[test]
    fn hash_and_root_follow_header_and_txs() {
        let a = sample(vec![b"one", b"two"]);
        assert_eq!(a.transaction_count(), 2);
        assert_eq!(a.transactions_size(), 6);
        assert_eq!(a.header.transactions_root, a.compute_transaction_root());
        assert_eq!(a.block_hash().unwrap(), a.block_hash().unwrap());

        let encoded = encode_block(&a).unwrap();
        assert_eq!(decode_block(&encoded).unwrap(), a);
        assert_eq!(
            encode_block(&decode_block(&encoded).unwrap()).unwrap(),
            encoded
        );

        let mut header_changed = a.clone();
        header_changed.header.height = BlockHeight(2);
        assert_ne!(
            a.block_hash().unwrap(),
            header_changed.block_hash().unwrap()
        );

        let mut tx_changed = a.clone();
        tx_changed.transactions[0] = RawTransaction(b"ONE".to_vec());
        assert_ne!(
            a.compute_transaction_root(),
            tx_changed.compute_transaction_root()
        );
    }
}
