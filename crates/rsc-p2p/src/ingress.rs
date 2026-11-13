//! Qué hacer con un mensaje de gossip antes de tocarlo la cadena.
//!
//! Esta capa no abre storage, no fabrica votos y no escribe bloques.

use crate::network::GossipTopic;

/// Decisión de entrada. Ninguna variante persiste un bloque.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ingress {
    /// La transacción puede entrar al mempool. El mempool sigue siendo quien acepta.
    OfferTransaction,
    /// Hay comité. El bloque se puede validar. No se escribe por haber llegado.
    HoldBlock,
    /// Hay comité. El voto se puede verificar. Este proceso no lo fabrica.
    HoldConsensus,
    /// Hay comité. La evidencia se puede verificar. No se escribe aquí.
    HoldEvidence,
    /// No entra a la cadena.
    Reject(&'static str),
}

/// Clasifica el tema. Sin comité, solo las transacciones siguen.
pub fn classify_ingress(topic: GossipTopic, committee: bool) -> Ingress {
    match topic {
        GossipTopic::Transactions => Ingress::OfferTransaction,
        GossipTopic::Blocks if committee => Ingress::HoldBlock,
        GossipTopic::Blocks => Ingress::Reject("bloque sin comité"),
        GossipTopic::Consensus if committee => Ingress::HoldConsensus,
        GossipTopic::Consensus => Ingress::Reject("consenso sin comité"),
        GossipTopic::Evidence if committee => Ingress::HoldEvidence,
        GossipTopic::Evidence => Ingress::Reject("evidencia sin comité"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_committee_offers_transactions_and_rejects_blocks() {
        assert_eq!(
            classify_ingress(GossipTopic::Transactions, false),
            Ingress::OfferTransaction
        );
        assert_eq!(
            classify_ingress(GossipTopic::Blocks, false),
            Ingress::Reject("bloque sin comité")
        );
        assert_eq!(
            classify_ingress(GossipTopic::Consensus, false),
            Ingress::Reject("consenso sin comité")
        );
        assert_eq!(
            classify_ingress(GossipTopic::Evidence, false),
            Ingress::Reject("evidencia sin comité")
        );
    }

    #[test]
    fn committee_holds_blocks_without_accepting_them() {
        assert_eq!(
            classify_ingress(GossipTopic::Blocks, true),
            Ingress::HoldBlock
        );
        assert_eq!(
            classify_ingress(GossipTopic::Consensus, true),
            Ingress::HoldConsensus
        );
    }
}
