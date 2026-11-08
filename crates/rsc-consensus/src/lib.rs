//! RSC-HS: quién acepta el bloque y cuándo es final. No ejecuta ni paga.
//!
//! ```
//! use rsc_consensus::{Committee, ConsensusEngine, FinalityProof};
//! ```

#![forbid(unsafe_code)]

pub mod committee;
pub mod engine;
pub mod finality;

pub use committee::{
    consensus_beacon, consensus_beacon_bytes, find_validator_by_key_id, select_committee,
    select_proposer, validate_validator_record, verify_selection, Committee, CommitteeMember,
    CommitteeSelectionInput, ConsensusConfig, NetworkMode, ParticipationScore, ValidatorRecord,
    ValidatorSet,
};
pub use engine::{
    ConsensusEngine, ConsensusEvent, ConsensusMessageSink, ConsensusPhase, ConsensusState,
};
pub use finality::{
    detect_conflicting_finality, detect_equivocation, has_quorum, verify_commit_chain, verify_qc,
    verify_validator_vote, EquivocationEvidence, FinalityProof, QuorumCertificate, Vote, VotePhase,
};
pub use rsc_core::AccountId as ValidatorId;
pub use rsc_crypto::SignatureBundle;

/// Error de consenso. No de ledger, red ni storage.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConsensusError {
    /// Configuración incompleta o inválida.
    #[error("invalid config")]
    InvalidConfig,
    /// Validador no elegible.
    #[error("ineligible validator")]
    Ineligible,
    /// Comité vacío.
    #[error("empty committee")]
    EmptyCommittee,
    /// Poder de voto total es 0.
    #[error("zero voting power")]
    ZeroVotingPower,
    /// Overflow aritmético.
    #[error("overflow")]
    Overflow,
    /// Proposer no autorizado.
    #[error("invalid proposer")]
    InvalidProposer,
    /// Bloque estructuralmente inválido.
    #[error("invalid block")]
    InvalidBlock,
    /// Fase ilegal para esta operación.
    #[error("invalid phase")]
    InvalidPhase,
    /// Vista o altura distinta a la actual.
    #[error("wrong view")]
    WrongView,
    /// El engine está locked a otro bloque.
    #[error("locked on other block")]
    Locked,
    /// Firma inválida.
    #[error("invalid signature")]
    InvalidSignature,
    /// QC inválido.
    #[error("invalid qc")]
    InvalidQc,
    /// Prueba de finality inválida.
    #[error("invalid finality")]
    InvalidFinality,
    /// Selección no reproducible / prueba incorrecta.
    #[error("invalid selection")]
    InvalidSelection,
}
