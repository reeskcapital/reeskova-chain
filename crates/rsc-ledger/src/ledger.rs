//! Secuencia lógica y append-only de identificadores de transacción.
//!
//! No es una base de datos, no abre archivos y no decide el orden de
//! consenso. Quien llama entrega el siguiente [`TransactionId`]. El índice
//! es la posición en esta secuencia: no es altura de bloque.

use std::collections::BTreeSet;

use rsc_crypto::{hash, Hash256};
use thiserror::Error;

use crate::TransactionId;

/// Dominio de la raíz vacía.
const DOMAIN_LEDGER_EMPTY: &[u8] = b"RSC-LEDGER-EMPTY\0";
/// Dominio de una hoja.
const DOMAIN_LEDGER_LEAF: &[u8] = b"RSC-LEDGER-LEAF\0";
/// Dominio de un nodo interno.
const DOMAIN_LEDGER_NODE: &[u8] = b"RSC-LEDGER-NODE\0";

/// Posición lógica de una transacción ya aceptada en un [`Ledger`].
///
/// `index` empieza en cero y es contiguo. No es altura de bloque, ronda de
/// consenso ni nonce de cuenta.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LedgerEntry {
    /// Posición en la secuencia. La primera entrada vale 0.
    pub index: u64,
    /// Identidad de la transacción incluida en esa posición.
    pub transaction_id: TransactionId,
}

/// Compromiso determinista con la secuencia ordenada de [`TransactionId`].
///
/// Responde qué transacciones forman el ledger y en qué orden. No responde
/// cuál es el estado de las cuentas. No es un `StateRoot`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LedgerRoot(pub Hash256);

impl core::fmt::Debug for LedgerRoot {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "LedgerRoot({:02x}{:02x}..)",
            self.0.as_bytes()[0],
            self.0.as_bytes()[1]
        )
    }
}

/// Fallo de la secuencia lógica.
///
/// No incluye almacenamiento, consenso, ejecución ni red.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum LedgerError {
    /// El [`TransactionId`] ya ocupa una posición.
    #[error("duplicate transaction")]
    DuplicateTransaction,
    /// El siguiente índice no cabe en `u64`.
    #[error("ledger index overflow")]
    IndexOverflow,
    /// La secuencia interna dejó de ser contigua.
    #[error("invalid ledger sequence")]
    InvalidSequence,
}

/// Secuencia en memoria de transacciones aceptadas.
///
/// Es append-only. No reordena, no borra y no elige qué transacción entra.
/// Dos instancias son independientes: no hay un ledger global del proceso.
#[derive(Clone, Debug, Default)]
pub struct Ledger {
    entries: Vec<LedgerEntry>,
    seen: BTreeSet<TransactionId>,
}

impl Ledger {
    /// Secuencia vacía. El próximo índice es 0.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            seen: BTreeSet::new(),
        }
    }

    /// Añade `transaction_id` al final.
    ///
    /// El índice asignado es [`Self::next_index`]. Un identificador repetido
    /// se rechaza y no modifica la secuencia. No ordena por fee ni por emisor.
    pub fn append(&mut self, transaction_id: TransactionId) -> Result<LedgerEntry, LedgerError> {
        if self.seen.contains(&transaction_id) {
            return Err(LedgerError::DuplicateTransaction);
        }
        let index = u64::try_from(self.entries.len()).map_err(|_| LedgerError::IndexOverflow)?;
        if let Some(last) = self.entries.last() {
            let expected = last
                .index
                .checked_add(1)
                .ok_or(LedgerError::IndexOverflow)?;
            if expected != index {
                return Err(LedgerError::InvalidSequence);
            }
        }
        let entry = LedgerEntry {
            index,
            transaction_id,
        };
        self.entries.push(entry);
        self.seen.insert(transaction_id);
        Ok(entry)
    }

    /// Índice que recibirá el próximo [`Self::append`] aceptado.
    pub fn next_index(&self) -> u64 {
        u64::try_from(self.entries.len()).unwrap_or(u64::MAX)
    }

    /// Número de entradas. Cero si el ledger está vacío.
    pub fn len(&self) -> u64 {
        self.next_index()
    }

    /// `true` cuando no hay entradas.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// `true` si `id` ya fue aceptado.
    pub fn contains(&self, id: &TransactionId) -> bool {
        self.seen.contains(id)
    }

    /// Entrada en `index`, si existe.
    ///
    /// Un índice ausente es `None`. No es un error de almacenamiento.
    pub fn get(&self, index: u64) -> Option<&LedgerEntry> {
        let position = usize::try_from(index).ok()?;
        self.entries.get(position)
    }

    /// Compromiso con el orden actual de identificadores.
    ///
    /// La misma secuencia produce la misma raíz. Cambiar el orden la cambia.
    /// Un ledger vacío usa `SHA-256(RSC-LEDGER-EMPTY)`.
    pub fn root(&self) -> LedgerRoot {
        LedgerRoot(merkle_root(&self.entries))
    }
}

fn merkle_root(entries: &[LedgerEntry]) -> Hash256 {
    if entries.is_empty() {
        return hash(DOMAIN_LEDGER_EMPTY);
    }
    let mut level: Vec<Hash256> = entries
        .iter()
        .map(|entry| leaf_hash(&entry.transaction_id))
        .collect();
    while level.len() > 1 {
        if !level.len().is_multiple_of(2) {
            let last = level[level.len() - 1];
            level.push(last);
        }
        let mut next = Vec::with_capacity(level.len() / 2);
        let mut pairs = level.chunks_exact(2);
        for pair in pairs.by_ref() {
            next.push(node_hash(&pair[0], &pair[1]));
        }
        level = next;
    }
    level[0]
}

fn leaf_hash(id: &TransactionId) -> Hash256 {
    let mut preimage = Vec::with_capacity(DOMAIN_LEDGER_LEAF.len() + 32);
    preimage.extend_from_slice(DOMAIN_LEDGER_LEAF);
    preimage.extend_from_slice(id.0.as_bytes());
    hash(&preimage)
}

fn node_hash(left: &Hash256, right: &Hash256) -> Hash256 {
    let mut preimage = Vec::with_capacity(DOMAIN_LEDGER_NODE.len() + 64);
    preimage.extend_from_slice(DOMAIN_LEDGER_NODE);
    preimage.extend_from_slice(left.as_bytes());
    preimage.extend_from_slice(right.as_bytes());
    hash(&preimage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsc_crypto::hash;

    fn tx_id(byte: u8) -> TransactionId {
        TransactionId(hash(&[byte]))
    }

    #[test]
    fn new_ledger_is_empty() {
        let ledger = Ledger::new();
        assert!(ledger.is_empty());
        assert_eq!(ledger.len(), 0);
        assert_eq!(ledger.next_index(), 0);
    }

    #[test]
    fn first_entry_has_index_zero() {
        let mut ledger = Ledger::new();
        let entry = ledger.append(tx_id(1)).unwrap();
        assert_eq!(entry.index, 0);
        assert_eq!(ledger.get(0).unwrap().index, 0);
    }

    #[test]
    fn indices_are_contiguous() {
        let mut ledger = Ledger::new();
        for byte in 1..=5 {
            ledger.append(tx_id(byte)).unwrap();
        }
        let indexes: Vec<u64> = (0..5).map(|i| ledger.get(i).unwrap().index).collect();
        assert_eq!(indexes, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn append_increments_length() {
        let mut ledger = Ledger::new();
        assert_eq!(ledger.len(), 0);
        ledger.append(tx_id(1)).unwrap();
        assert_eq!(ledger.len(), 1);
        ledger.append(tx_id(2)).unwrap();
        assert_eq!(ledger.len(), 2);
    }

    #[test]
    fn next_index_is_correct() {
        let mut ledger = Ledger::new();
        assert_eq!(ledger.next_index(), 0);
        ledger.append(tx_id(1)).unwrap();
        assert_eq!(ledger.next_index(), 1);
        ledger.append(tx_id(2)).unwrap();
        assert_eq!(ledger.next_index(), 2);
    }

    #[test]
    fn contains_existing_transaction() {
        let mut ledger = Ledger::new();
        let id = tx_id(7);
        ledger.append(id).unwrap();
        assert!(ledger.contains(&id));
    }

    #[test]
    fn contains_missing_transaction() {
        let ledger = Ledger::new();
        assert!(!ledger.contains(&tx_id(9)));
    }

    #[test]
    fn get_existing_entry() {
        let mut ledger = Ledger::new();
        let id = tx_id(4);
        ledger.append(tx_id(3)).unwrap();
        ledger.append(id).unwrap();
        let entry = ledger.get(1).unwrap();
        assert_eq!(entry.index, 1);
        assert_eq!(entry.transaction_id, id);
    }

    #[test]
    fn get_missing_entry() {
        let mut ledger = Ledger::new();
        ledger.append(tx_id(1)).unwrap();
        assert!(ledger.get(1).is_none());
        assert!(ledger.get(u64::MAX).is_none());
    }

    #[test]
    fn duplicate_transaction_is_rejected() {
        let mut ledger = Ledger::new();
        let id = tx_id(5);
        ledger.append(id).unwrap();
        ledger.append(tx_id(6)).unwrap();
        assert_eq!(ledger.append(id), Err(LedgerError::DuplicateTransaction));
    }

    #[test]
    fn duplicate_does_not_modify_ledger() {
        let mut ledger = Ledger::new();
        ledger.append(tx_id(1)).unwrap();
        ledger.append(tx_id(2)).unwrap();
        let before = ledger.root();
        let len = ledger.len();
        assert!(ledger.append(tx_id(1)).is_err());
        assert_eq!(ledger.len(), len);
        assert_eq!(ledger.root(), before);
        assert_eq!(ledger.get(0).unwrap().transaction_id, tx_id(1));
        assert_eq!(ledger.get(1).unwrap().transaction_id, tx_id(2));
    }

    #[test]
    fn ledger_root_is_deterministic() {
        let mut a = Ledger::new();
        let mut b = Ledger::new();
        for byte in [1u8, 2, 3] {
            a.append(tx_id(byte)).unwrap();
            b.append(tx_id(byte)).unwrap();
        }
        assert_eq!(a.root(), b.root());
    }

    #[test]
    fn empty_ledger_root_is_deterministic() {
        assert_eq!(Ledger::new().root(), Ledger::new().root());
        assert_eq!(Ledger::new().root().0, hash(DOMAIN_LEDGER_EMPTY));
    }

    #[test]
    fn order_changes_ledger_root() {
        let mut ab = Ledger::new();
        ab.append(tx_id(1)).unwrap();
        ab.append(tx_id(2)).unwrap();
        let mut ba = Ledger::new();
        ba.append(tx_id(2)).unwrap();
        ba.append(tx_id(1)).unwrap();
        assert_ne!(ab.root(), ba.root());
    }

    #[test]
    fn transaction_replacement_changes_ledger_root() {
        let mut first = Ledger::new();
        first.append(tx_id(1)).unwrap();
        first.append(tx_id(2)).unwrap();
        let mut second = Ledger::new();
        second.append(tx_id(1)).unwrap();
        second.append(tx_id(3)).unwrap();
        assert_ne!(first.root(), second.root());
    }

    #[test]
    fn repeated_same_sequence_produces_same_root() {
        let build = || {
            let mut ledger = Ledger::new();
            ledger.append(tx_id(8)).unwrap();
            ledger.append(tx_id(9)).unwrap();
            ledger.append(tx_id(10)).unwrap();
            ledger.root()
        };
        assert_eq!(build(), build());
    }
}
