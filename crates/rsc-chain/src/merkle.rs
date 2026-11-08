//! Merkle de transacciones. Respeta el orden. No interpreta payloads.

use rsc_core::Hash;
use rsc_crypto::hash;

/// `HASH("RSC-TX\0" || tx)`.
pub const TX_DOMAIN: &[u8] = b"RSC-TX\0";
/// `HASH("RSC-MERKLE\0" || left || right)`.
pub const MERKLE_DOMAIN: &[u8] = b"RSC-MERKLE\0";

/// Hoja de una transacción en bruto.
pub fn tx_leaf(tx: &[u8]) -> Hash {
    hash_concat(&[TX_DOMAIN, tx])
}

/// Raíz Merkle. Lista vacía → `Hash::ZERO`. Impar: se duplica el último.
pub fn merkle_root(leaves: &[Hash]) -> Hash {
    if leaves.is_empty() {
        return Hash::ZERO;
    }
    let mut level = leaves.to_vec();
    while level.len() > 1 {
        if !level.len().is_multiple_of(2) {
            if let Some(last) = level.last().copied() {
                level.push(last);
            }
        }
        let mut next = Vec::with_capacity(level.len() / 2);
        let mut chunks = level.chunks_exact(2);
        for pair in chunks.by_ref() {
            next.push(merkle_node(&pair[0], &pair[1]));
        }
        level = next;
    }
    match level.first().copied() {
        Some(root) => root,
        None => Hash::ZERO,
    }
}

fn merkle_node(left: &Hash, right: &Hash) -> Hash {
    hash_concat(&[MERKLE_DOMAIN, &left.0, &right.0])
}

fn hash_concat(parts: &[&[u8]]) -> Hash {
    let len = parts.iter().map(|p| p.len()).sum();
    let mut buf = Vec::with_capacity(len);
    for part in parts {
        buf.extend_from_slice(part);
    }
    Hash(hash(&buf).to_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn leaves(n: usize) -> Vec<Hash> {
        (0..n).map(|i| tx_leaf(&[i as u8])).collect()
    }

    #[test]
    fn known_vectors_and_order() {
        assert_eq!(merkle_root(&[]), Hash::ZERO);
        let a = tx_leaf(b"A");
        let b = tx_leaf(b"B");
        let c = tx_leaf(b"C");
        let d = tx_leaf(b"D");
        assert_eq!(merkle_root(&[a]), a);
        assert_eq!(merkle_root(&[a, b]), merkle_node(&a, &b));
        let abc = merkle_root(&[a, b, c]);
        assert_eq!(abc, merkle_node(&merkle_node(&a, &b), &merkle_node(&c, &c)));
        assert_eq!(
            merkle_root(&[a, b, c, d]),
            merkle_node(&merkle_node(&a, &b), &merkle_node(&c, &d))
        );
        assert_ne!(abc, merkle_root(&[a, b]));
        assert_ne!(abc, merkle_root(&[b, a, c]));
        assert_eq!(merkle_root(&leaves(3)), merkle_root(&leaves(3)));
    }

    proptest! {
        #[test]
        fn same_input_same_root(txs in prop::collection::vec(prop::collection::vec(any::<u8>(), 0..40), 0..8)) {
            let leaves: Vec<Hash> = txs.iter().map(|t| tx_leaf(t)).collect();
            prop_assert_eq!(merkle_root(&leaves), merkle_root(&leaves));
        }

        #[test]
        fn flip_byte_changes_root(
            mut txs in prop::collection::vec(prop::collection::vec(any::<u8>(), 1..40), 1..6)
        ) {
            let before = merkle_root(&txs.iter().map(|t| tx_leaf(t)).collect::<Vec<_>>());
            txs[0][0] ^= 0xff;
            let after = merkle_root(&txs.iter().map(|t| tx_leaf(t)).collect::<Vec<_>>());
            prop_assert_ne!(before, after);
        }
    }
}
