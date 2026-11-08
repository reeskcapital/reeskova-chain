//! Encoding canónico Borsh. No JSON. Orden de campos fijo.

use crate::block::{Block, RawTransaction};
use crate::errors::ChainError;
use crate::header::BlockHeader;
use borsh::{BorshDeserialize, BorshSerialize};
use rsc_core::{AccountId, BlockHash, BlockHeight, ChainId, Hash, Timestamp};

/// `HASH("RSC-BLOCK-HEADER\0" || header_bytes)`.
pub const HEADER_DOMAIN: &[u8] = b"RSC-BLOCK-HEADER\0";

#[derive(BorshSerialize, BorshDeserialize)]
struct HeaderWire {
    version: u32,
    chain_id: u32,
    height: u64,
    previous_block_hash: [u8; 32],
    timestamp: u64,
    transactions_root: [u8; 32],
    state_root: [u8; 32],
    proposer: [u8; 32],
}

#[derive(BorshSerialize, BorshDeserialize)]
struct BlockWire {
    header: HeaderWire,
    transactions: Vec<Vec<u8>>,
}

fn to_wire(h: &BlockHeader) -> HeaderWire {
    HeaderWire {
        version: h.version,
        chain_id: h.chain_id.0,
        height: h.height.0,
        previous_block_hash: h.previous_block_hash.0 .0,
        timestamp: h.timestamp.0,
        transactions_root: h.transactions_root.0,
        state_root: h.state_root.0,
        proposer: h.proposer.0,
    }
}

fn from_wire(w: HeaderWire) -> BlockHeader {
    BlockHeader {
        version: w.version,
        chain_id: ChainId(w.chain_id),
        height: BlockHeight(w.height),
        previous_block_hash: BlockHash(Hash(w.previous_block_hash)),
        timestamp: Timestamp(w.timestamp),
        transactions_root: Hash(w.transactions_root),
        state_root: Hash(w.state_root),
        proposer: AccountId(w.proposer),
    }
}

/// Serializa el header en orden canónico.
pub fn encode_header(header: &BlockHeader) -> Result<Vec<u8>, ChainError> {
    borsh::to_vec(&to_wire(header)).map_err(|_| ChainError::InvalidEncoding)
}

/// Deserializa un header canónico.
pub fn decode_header(bytes: &[u8]) -> Result<BlockHeader, ChainError> {
    let wire = HeaderWire::try_from_slice(bytes).map_err(|_| ChainError::InvalidEncoding)?;
    Ok(from_wire(wire))
}

/// Serializa el bloque completo.
pub fn encode_block(block: &Block) -> Result<Vec<u8>, ChainError> {
    let wire = BlockWire {
        header: to_wire(&block.header),
        transactions: block.transactions.iter().map(|tx| tx.0.clone()).collect(),
    };
    borsh::to_vec(&wire).map_err(|_| ChainError::InvalidEncoding)
}

/// Deserializa un bloque canónico.
pub fn decode_block(bytes: &[u8]) -> Result<Block, ChainError> {
    let wire = BlockWire::try_from_slice(bytes).map_err(|_| ChainError::InvalidEncoding)?;
    Ok(Block {
        header: from_wire(wire.header),
        transactions: wire.transactions.into_iter().map(RawTransaction).collect(),
    })
}

/// Hash del header: dominio + bytes canónicos.
pub fn header_hash(header: &BlockHeader) -> Result<BlockHash, ChainError> {
    let encoded = encode_header(header)?;
    let mut buf = Vec::with_capacity(HEADER_DOMAIN.len() + encoded.len());
    buf.extend_from_slice(HEADER_DOMAIN);
    buf.extend_from_slice(&encoded);
    Ok(BlockHash(Hash(rsc_crypto::hash(&buf).to_bytes())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::Block;
    use proptest::prelude::*;
    use rsc_core::BlockHeight;

    fn sample_header() -> BlockHeader {
        BlockHeader {
            version: 1,
            chain_id: ChainId(1),
            height: BlockHeight(3),
            previous_block_hash: BlockHash(Hash([2; 32])),
            timestamp: Timestamp::from_secs(10),
            transactions_root: Hash([3; 32]),
            state_root: Hash([4; 32]),
            proposer: AccountId([5; 32]),
        }
    }

    #[test]
    fn header_and_block_roundtrip() {
        let header = sample_header();
        let bytes = encode_header(&header).unwrap();
        assert_eq!(decode_header(&bytes).unwrap(), header);
        assert_eq!(
            encode_header(&decode_header(&bytes).unwrap()).unwrap(),
            bytes
        );

        let block = Block {
            header,
            transactions: vec![
                RawTransaction(b"tx-a".to_vec()),
                RawTransaction(vec![1, 2, 3]),
            ],
        };
        let encoded = encode_block(&block).unwrap();
        assert_eq!(decode_block(&encoded).unwrap(), block);
        assert_eq!(
            encode_block(&decode_block(&encoded).unwrap()).unwrap(),
            encoded
        );
        assert_eq!(
            decode_block(&[0xff, 0x00]).unwrap_err(),
            ChainError::InvalidEncoding
        );
    }

    #[test]
    fn same_header_same_hash() {
        let h = sample_header();
        assert_eq!(header_hash(&h).unwrap(), header_hash(&h).unwrap());
        let mut other = h.clone();
        other.height = BlockHeight(4);
        assert_ne!(header_hash(&h).unwrap(), header_hash(&other).unwrap());
    }

    proptest! {
        #[test]
        fn encode_decode_encode(
            version in 0u32..8,
            height in 0u64..10_000,
            payload in prop::collection::vec(any::<u8>(), 0..32),
        ) {
            let block = Block {
                header: BlockHeader {
                    version,
                    chain_id: ChainId(2),
                    height: BlockHeight(height),
                    previous_block_hash: BlockHash::ZERO,
                    timestamp: Timestamp::from_secs(1),
                    transactions_root: Hash::ZERO,
                    state_root: Hash::ZERO,
                    proposer: AccountId::ZERO,
                },
                transactions: vec![RawTransaction(payload)],
            };
            let a = encode_block(&block).unwrap();
            let again = encode_block(&decode_block(&a).unwrap()).unwrap();
            prop_assert_eq!(a, again);
        }
    }
}
