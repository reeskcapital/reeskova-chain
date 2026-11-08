//! Validación estructural. Sin firmas, balances, stake ni consenso.

use crate::block::Block;
use crate::codec::{encode_block, header_hash};
use crate::errors::ChainError;
use crate::header::BlockHeader;
use rsc_core::{BlockHash, BlockHeight, ChainId};

/// Límites y padre. Nada de globals ni reloj.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChainValidationContext {
    /// `chain_id` esperado.
    pub expected_chain_id: ChainId,
    /// Versión de protocolo aceptada.
    pub expected_protocol_version: u32,
    /// Máximo de bytes canónicos del bloque.
    pub max_block_size: usize,
    /// Máximo de transacciones.
    pub max_transactions_per_block: usize,
    /// Padre. `None` solo para genesis.
    pub parent: Option<BlockHeader>,
}

/// Valida estructura. Sin efectos secundarios.
pub fn validate_block(block: &Block, ctx: &ChainValidationContext) -> Result<(), ChainError> {
    if block.header.chain_id != ctx.expected_chain_id {
        return Err(ChainError::WrongChainId);
    }
    if block.header.version != ctx.expected_protocol_version {
        return Err(ChainError::UnsupportedProtocolVersion);
    }
    if block.transaction_count() > ctx.max_transactions_per_block {
        return Err(ChainError::TooManyTransactions);
    }
    if encode_block(block)?.len() > ctx.max_block_size {
        return Err(ChainError::BlockTooLarge);
    }
    if block.compute_transaction_root() != block.header.transactions_root {
        return Err(ChainError::InvalidTransactionRoot);
    }
    header_hash(&block.header)?;

    if block.header.height == BlockHeight::GENESIS {
        return validate_genesis(block, ctx);
    }
    validate_child(block, ctx)
}

fn validate_genesis(block: &Block, ctx: &ChainValidationContext) -> Result<(), ChainError> {
    if ctx.parent.is_some()
        || block.header.previous_block_hash != BlockHash::ZERO
        || !block.header.is_genesis()
    {
        return Err(ChainError::InvalidGenesis);
    }
    Ok(())
}

fn validate_child(block: &Block, ctx: &ChainValidationContext) -> Result<(), ChainError> {
    let parent = ctx.parent.as_ref().ok_or(ChainError::ParentNotFound)?;
    let expected_height = parent
        .height
        .checked_next()
        .map_err(|_| ChainError::InvalidHeight)?;
    if block.header.height != expected_height {
        return Err(ChainError::InvalidHeight);
    }
    if block.header.previous_block_hash != header_hash(parent)? {
        return Err(ChainError::InvalidParentHash);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::RawTransaction;
    use crate::codec::encode_block;
    use rsc_core::{AccountId, Hash, Timestamp};

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

    fn ctx(parent: Option<BlockHeader>, max_size: usize) -> ChainValidationContext {
        ChainValidationContext {
            expected_chain_id: ChainId(1),
            expected_protocol_version: 1,
            max_block_size: max_size,
            max_transactions_per_block: 8,
            parent,
        }
    }

    #[test]
    fn genesis_parent_height_size_and_chain_id() {
        let genesis = block(0, BlockHash::ZERO, &[]);
        assert!(validate_block(&genesis, &ctx(None, 1024)).is_ok());

        let mut bad_genesis = genesis.clone();
        bad_genesis.header.previous_block_hash = BlockHash(Hash([1; 32]));
        assert_eq!(
            validate_block(&bad_genesis, &ctx(None, 1024)).unwrap_err(),
            ChainError::InvalidGenesis
        );

        let parent_hash = genesis.block_hash().unwrap();
        let child = block(1, parent_hash, &[b"tx"]);
        assert!(validate_block(&child, &ctx(Some(genesis.header.clone()), 1024)).is_ok());

        let skip = block(2, parent_hash, &[]);
        assert_eq!(
            validate_block(&skip, &ctx(Some(genesis.header.clone()), 1024)).unwrap_err(),
            ChainError::InvalidHeight
        );

        let wrong_parent = block(1, BlockHash(Hash([9; 32])), &[]);
        assert_eq!(
            validate_block(&wrong_parent, &ctx(Some(genesis.header.clone()), 1024)).unwrap_err(),
            ChainError::InvalidParentHash
        );

        let mut other_chain = child.clone();
        other_chain.header.chain_id = ChainId(2);
        assert_eq!(
            validate_block(&other_chain, &ctx(Some(genesis.header.clone()), 1024)).unwrap_err(),
            ChainError::WrongChainId
        );

        let encoded_len = encode_block(&child).unwrap().len();
        assert!(validate_block(&child, &ctx(Some(genesis.header.clone()), encoded_len)).is_ok());
        assert_eq!(
            validate_block(
                &child,
                &ctx(Some(genesis.header), encoded_len.saturating_sub(1))
            )
            .unwrap_err(),
            ChainError::BlockTooLarge
        );
    }
}
