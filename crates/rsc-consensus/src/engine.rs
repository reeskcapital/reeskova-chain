//! Máquina de estados RSC-HS. No ejecuta txs ni abre sockets.

use crate::committee::{
    epoch_of, select_committee, select_proposer, Committee, CommitteeSelectionInput,
    ConsensusConfig, NetworkMode, ValidatorSet,
};
use crate::finality::{
    aggregate_qc, detect_equivocation, sign_vote, verify_commit_chain, FinalityProof,
    QuorumCertificate, Vote, VotePhase,
};
use crate::ConsensusError;
use rsc_chain::{validate_block, Block, ChainValidationContext};
use rsc_core::{AccountId, BlockHash, Hash};
use rsc_crypto::PrivateKey;
use std::collections::BTreeMap;

/// Fase explícita. Sin booleanos sueltos.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConsensusPhase {
    /// Nueva altura.
    NewHeight,
    /// Nueva vista / líder.
    NewView,
    /// Propuesta recibida.
    Propose,
    /// Prepare.
    Prepare,
    /// Pre-commit. Aquí se toma el lock.
    PreCommit,
    /// Commit.
    Commit,
    /// Decidido / final. Terminal para (height, view).
    Decide,
    /// Timeout de la vista.
    Timeout,
}

/// Estado de una altura.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsensusState {
    /// Epoch.
    pub epoch: u64,
    /// Altura.
    pub height: u64,
    /// Vista.
    pub view: u64,
    /// Fase.
    pub phase: ConsensusPhase,
    /// Comité de esta vista.
    pub committee: Committee,
    /// Hash propuesto.
    pub proposal: Option<BlockHash>,
    /// Lock de pre-commit.
    pub locked: Option<BlockHash>,
}

/// Evento de salida. El transporte lo hace `rsc-p2p`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConsensusEvent {
    /// Propuesta válida.
    ProposalAccepted,
    /// Hay que votar.
    VoteCreated(Vote),
    /// Quorum de una fase.
    QuorumReached(VotePhase),
    /// Bloque final.
    BlockFinalized(FinalityProof),
    /// Vista incrementada.
    ViewChanged {
        /// Nueva vista.
        view: u64,
    },
    /// Nuevo epoch.
    EpochChanged {
        /// Nuevo epoch.
        epoch: u64,
    },
    /// Timeout.
    Timeout,
    /// Doble voto.
    Equivocation(crate::finality::EquivocationEvidence),
}

/// Salida de mensajes. Implementa `rsc-p2p`, no este crate.
pub trait ConsensusMessageSink {
    /// Difunde bytes de consenso.
    fn broadcast(&mut self, bytes: &[u8]);
}

/// Motor RSC-HS.
pub struct ConsensusEngine {
    config: ConsensusConfig,
    validators: ValidatorSet,
    randomness: Hash,
    local: AccountId,
    state: ConsensusState,
    votes: BTreeMap<(AccountId, VotePhase), Vote>,
    qcs: BTreeMap<VotePhase, QuorumCertificate>,
}

impl ConsensusEngine {
    /// Crea el engine en `height` / `view` 0.
    pub fn new(
        config: ConsensusConfig,
        validators: ValidatorSet,
        randomness: Hash,
        local: AccountId,
        height: u64,
    ) -> Result<Self, ConsensusError> {
        config.validate()?;
        let epoch = epoch_of(height, config.epoch_length)?;
        let input = CommitteeSelectionInput {
            epoch,
            height,
            view: 0,
            randomness,
        };
        let committee = select_committee(&input, &validators, &config)?;
        Ok(Self {
            config,
            validators,
            randomness,
            local,
            state: ConsensusState {
                epoch,
                height,
                view: 0,
                phase: ConsensusPhase::NewView,
                committee,
                proposal: None,
                locked: None,
            },
            votes: BTreeMap::new(),
            qcs: BTreeMap::new(),
        })
    }

    /// Estado actual.
    pub fn state(&self) -> &ConsensusState {
        &self.state
    }

    /// Proposer de la vista.
    pub fn proposer(&self) -> Result<AccountId, ConsensusError> {
        select_proposer(&self.input(), &self.state.committee)
    }

    fn input(&self) -> CommitteeSelectionInput {
        CommitteeSelectionInput {
            epoch: self.state.epoch,
            height: self.state.height,
            view: self.state.view,
            randomness: self.randomness,
        }
    }

    fn refresh_committee(&mut self) -> Result<(), ConsensusError> {
        self.state.committee = select_committee(&self.input(), &self.validators, &self.config)?;
        Ok(())
    }

    /// Recibe un bloque candidato. Valida estructura con `rsc-chain` si hay contexto.
    pub fn receive_proposal(
        &mut self,
        block: &Block,
        ctx: Option<&ChainValidationContext>,
    ) -> Result<ConsensusEvent, ConsensusError> {
        if matches!(self.state.phase, ConsensusPhase::Decide) {
            return Err(ConsensusError::InvalidPhase);
        }
        if block.header.height.0 != self.state.height {
            return Err(ConsensusError::WrongView);
        }
        if let Some(ctx) = ctx {
            validate_block(block, ctx).map_err(|_| ConsensusError::InvalidBlock)?;
        }
        let proposer = self.proposer()?;
        if block.header.proposer != proposer {
            return Err(ConsensusError::InvalidProposer);
        }
        let hash = block
            .block_hash()
            .map_err(|_| ConsensusError::InvalidBlock)?;
        if let Some(locked) = self.state.locked {
            if locked != hash {
                return Err(ConsensusError::Locked);
            }
        }
        self.state.proposal = Some(hash);
        self.state.phase = ConsensusPhase::Propose;
        Ok(ConsensusEvent::ProposalAccepted)
    }

    /// Emite el voto local de la fase actual, firmado con la clave recibida.
    pub fn create_vote(
        &mut self,
        phase: VotePhase,
        key: &PrivateKey,
    ) -> Result<Vote, ConsensusError> {
        if matches!(self.state.phase, ConsensusPhase::Decide) {
            return Err(ConsensusError::InvalidPhase);
        }
        let hash = self.state.proposal.ok_or(ConsensusError::InvalidPhase)?;
        if !self
            .state
            .committee
            .members
            .iter()
            .any(|m| m.validator_id == self.local)
        {
            return Err(ConsensusError::Ineligible);
        }
        let record = self
            .validators
            .validators
            .iter()
            .find(|validator| validator.id == self.local)
            .ok_or(ConsensusError::Ineligible)?;
        if record.owner_public_key != key.public_key() || record.owner_key_id != key.key_id() {
            return Err(ConsensusError::InvalidSignature);
        }
        let mut vote = Vote {
            validator: self.local,
            epoch: self.state.epoch,
            height: self.state.height,
            view: self.state.view,
            block_hash: hash,
            phase,
            signature: rsc_core::Signature([0; 64]),
        };
        sign_vote(&mut vote, key)?;
        Ok(vote)
    }

    /// Incorpora un voto. Detecta equivocation. Avanza fases.
    pub fn receive_vote(&mut self, vote: Vote) -> Result<ConsensusEvent, ConsensusError> {
        if matches!(self.state.phase, ConsensusPhase::Decide) {
            return Err(ConsensusError::InvalidPhase);
        }
        if vote.height != self.state.height
            || vote.view != self.state.view
            || vote.epoch != self.state.epoch
        {
            return Err(ConsensusError::WrongView);
        }
        let Some(record) = self
            .validators
            .validators
            .iter()
            .find(|validator| validator.id == vote.validator)
        else {
            return Err(ConsensusError::InvalidSignature);
        };
        if record.owner_public_key.key_id() != record.owner_key_id
            || !crate::finality::vote_signature_valid(&vote, &record.owner_public_key)
        {
            return Err(ConsensusError::InvalidSignature);
        }
        let key = (vote.validator, vote.phase);
        if let Some(prev) = self.votes.get(&key) {
            if let Some(ev) = detect_equivocation(prev, &vote) {
                return Ok(ConsensusEvent::Equivocation(ev));
            }
            return Ok(ConsensusEvent::ProposalAccepted);
        }
        self.votes.insert(key, vote.clone());
        self.maybe_advance(vote.phase)
    }

    fn maybe_advance(&mut self, phase: VotePhase) -> Result<ConsensusEvent, ConsensusError> {
        let votes: Vec<Vote> = self.votes.values().cloned().collect();
        let (qc, ev) = aggregate_qc(&votes, &self.state.committee, phase)?;
        if let Some(ev) = ev {
            return Ok(ConsensusEvent::Equivocation(ev));
        }
        let Some(qc) = qc else {
            return Ok(ConsensusEvent::VoteCreated(votes[votes.len() - 1].clone()));
        };
        self.qcs.insert(phase, qc);
        match phase {
            VotePhase::Prepare => {
                self.state.phase = ConsensusPhase::Prepare;
                Ok(ConsensusEvent::QuorumReached(phase))
            }
            VotePhase::PreCommit => {
                self.state.locked = self.state.proposal;
                self.state.phase = ConsensusPhase::PreCommit;
                Ok(ConsensusEvent::QuorumReached(phase))
            }
            VotePhase::Commit => {
                let proof = self.finality_proof()?;
                verify_commit_chain(&proof, &self.state.committee)?;
                self.state.phase = ConsensusPhase::Decide;
                Ok(ConsensusEvent::BlockFinalized(proof))
            }
        }
    }

    fn finality_proof(&self) -> Result<FinalityProof, ConsensusError> {
        let prepare = self
            .qcs
            .get(&VotePhase::Prepare)
            .ok_or(ConsensusError::InvalidFinality)?;
        let precommit = self
            .qcs
            .get(&VotePhase::PreCommit)
            .ok_or(ConsensusError::InvalidFinality)?;
        let commit = self
            .qcs
            .get(&VotePhase::Commit)
            .ok_or(ConsensusError::InvalidFinality)?;
        let hash = self.state.proposal.ok_or(ConsensusError::InvalidFinality)?;
        Ok(FinalityProof {
            block_hash: hash,
            height: self.state.height,
            view: self.state.view,
            prepare: prepare.clone(),
            precommit: precommit.clone(),
            commit: commit.clone(),
        })
    }

    /// Timeout → siguiente vista. No des-finaliza.
    pub fn timeout(&mut self) -> Result<ConsensusEvent, ConsensusError> {
        if self.state.phase == ConsensusPhase::Decide {
            return Err(ConsensusError::InvalidPhase);
        }
        self.state.view = self
            .state
            .view
            .checked_add(1)
            .ok_or(ConsensusError::Overflow)?;
        self.state.phase = ConsensusPhase::Timeout;
        self.votes.clear();
        self.qcs.clear();
        if self.state.locked.is_none() {
            self.state.proposal = None;
        }
        self.refresh_committee()?;
        self.state.phase = ConsensusPhase::NewView;
        Ok(ConsensusEvent::ViewChanged {
            view: self.state.view,
        })
    }

    /// Tras `Decide`, avanza altura. Puede cambiar epoch y snapshot.
    pub fn new_height(
        &mut self,
        validators: Option<ValidatorSet>,
        randomness: Hash,
    ) -> Result<ConsensusEvent, ConsensusError> {
        if self.state.phase != ConsensusPhase::Decide {
            return Err(ConsensusError::InvalidPhase);
        }
        self.state.height = self
            .state
            .height
            .checked_add(1)
            .ok_or(ConsensusError::Overflow)?;
        let epoch = epoch_of(self.state.height, self.config.epoch_length)?;
        let epoch_changed = epoch != self.state.epoch;
        self.state.epoch = epoch;
        self.state.view = 0;
        self.state.phase = ConsensusPhase::NewHeight;
        self.state.proposal = None;
        self.state.locked = None;
        self.votes.clear();
        self.qcs.clear();
        self.randomness = randomness;
        if let Some(set) = validators {
            self.validators = set;
        }
        if self.validators.validators.len() >= self.config.min_validators_for_distributed
            && self.validators.mode == NetworkMode::Bootstrap
        {
            self.validators.mode = NetworkMode::Distributed;
        }
        self.refresh_committee()?;
        self.state.phase = ConsensusPhase::NewView;
        if epoch_changed {
            Ok(ConsensusEvent::EpochChanged { epoch })
        } else {
            Ok(ConsensusEvent::ViewChanged { view: 0 })
        }
    }

    /// Modo de red (bootstrap / distributed).
    pub fn network_mode(&self) -> NetworkMode {
        self.validators.mode
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::committee::{bound_id, test_cfg, test_id, test_key, test_set};
    use crate::finality::{aggregate_qc, detect_conflicting_finality};
    use rsc_chain::{BlockHeader, RawTransaction};
    use rsc_core::{BlockHeight, ChainId, Timestamp};

    fn block(proposer: AccountId, height: u64, salt: u8) -> Block {
        let mut b = Block {
            header: BlockHeader {
                version: 1,
                chain_id: ChainId(1),
                height: BlockHeight(height),
                previous_block_hash: BlockHash::ZERO,
                timestamp: Timestamp::from_secs(1),
                transactions_root: Hash::ZERO,
                state_root: Hash([salt; 32]),
                proposer,
            },
            transactions: vec![RawTransaction(vec![salt])],
        };
        b.header.transactions_root = b.compute_transaction_root();
        b
    }

    fn engine() -> ConsensusEngine {
        ConsensusEngine::new(
            test_cfg(),
            test_set(&[(1, 10, true), (2, 10, true), (3, 10, true)]),
            Hash([7; 32]),
            bound_id(1),
            1,
        )
        .unwrap()
    }

    fn feed_phase(engines: &mut [ConsensusEngine], phase: VotePhase) -> Vec<ConsensusEvent> {
        let votes: Vec<Vote> = engines
            .iter_mut()
            .enumerate()
            .map(|(index, engine)| engine.create_vote(phase, &test_key((index + 1) as u8)).unwrap())
            .collect();
        let mut events = Vec::new();
        for e in engines.iter_mut() {
            for v in &votes {
                events.push(e.receive_vote(v.clone()).unwrap());
            }
        }
        events
    }

    fn trio() -> [ConsensusEngine; 3] {
        let cfg = test_cfg();
        let set = test_set(&[(1, 10, true), (2, 10, true), (3, 10, true)]);
        let seed = Hash([7; 32]);
        [1u8, 2, 3]
            .map(|id| ConsensusEngine::new(cfg.clone(), set.clone(), seed, bound_id(id), 1).unwrap())
    }

    #[test]
    fn start_proposal_votes_finality_timeout_epoch() {
        let mut e = engine();
        assert_eq!(e.state().phase, ConsensusPhase::NewView);
        assert_eq!(e.proposer().unwrap(), e.proposer().unwrap());
        let p = e.proposer().unwrap();
        let good = block(p, 1, 1);
        assert_eq!(
            e.receive_proposal(&good, None).unwrap(),
            ConsensusEvent::ProposalAccepted
        );
        let bad_p = block(test_id(9), 1, 2);
        let mut e2 = engine();
        assert_eq!(
            e2.receive_proposal(&bad_p, None).unwrap_err(),
            ConsensusError::InvalidProposer
        );
        let mut e3 = engine();
        let wrong_h = block(e3.proposer().unwrap(), 9, 3);
        assert_eq!(
            e3.receive_proposal(&wrong_h, None).unwrap_err(),
            ConsensusError::WrongView
        );

        let mut nodes = trio();
        let proposer = nodes[0].proposer().unwrap();
        let blk = block(proposer, 1, 4);
        for n in &mut nodes {
            n.receive_proposal(&blk, None).unwrap();
        }
        feed_phase(&mut nodes, VotePhase::Prepare);
        feed_phase(&mut nodes, VotePhase::PreCommit);
        let evs = feed_phase(&mut nodes, VotePhase::Commit);
        assert!(evs
            .iter()
            .any(|e| matches!(e, ConsensusEvent::BlockFinalized(_))));
        assert_eq!(nodes[0].state().phase, ConsensusPhase::Decide);
        assert!(nodes[0]
            .create_vote(VotePhase::Prepare, &test_key(1))
            .is_err());

        let proof_a = match &evs[evs.len() - 1] {
            ConsensusEvent::BlockFinalized(p) => p.clone(),
            _ => nodes[0].finality_proof().unwrap(),
        };
        let mut other = trio();
        let other_blk = block(other[0].proposer().unwrap(), 1, 9);
        for n in &mut other {
            n.receive_proposal(&other_blk, None).unwrap();
        }
        feed_phase(&mut other, VotePhase::Prepare);
        feed_phase(&mut other, VotePhase::PreCommit);
        let evb = feed_phase(&mut other, VotePhase::Commit);
        let proof_b = evb
            .into_iter()
            .find_map(|e| match e {
                ConsensusEvent::BlockFinalized(p) => Some(p),
                _ => None,
            })
            .unwrap();
        assert!(detect_conflicting_finality(&proof_a, &proof_b));

        let mut t = engine();
        t.receive_proposal(&block(t.proposer().unwrap(), 1, 1), None)
            .unwrap();
        assert!(matches!(
            t.timeout().unwrap(),
            ConsensusEvent::ViewChanged { view: 1 }
        ));
        assert_eq!(t.state().view, 1);
        assert_eq!(t.state().phase, ConsensusPhase::NewView);

        let mut group = trio();
        let pb = block(group[0].proposer().unwrap(), 1, 5);
        for n in &mut group {
            n.receive_proposal(&pb, None).unwrap();
        }
        feed_phase(&mut group, VotePhase::Prepare);
        feed_phase(&mut group, VotePhase::PreCommit);
        feed_phase(&mut group, VotePhase::Commit);
        let mut finalized = group.into_iter().next().unwrap();
        let ev = finalized.new_height(None, Hash([8; 32])).unwrap();
        assert_eq!(finalized.state().height, 2);
        assert!(matches!(
            ev,
            ConsensusEvent::ViewChanged { .. } | ConsensusEvent::EpochChanged { .. }
        ));
        let _ = ev;
    }

    #[test]
    fn duplicate_and_conflicting_vote() {
        let mut nodes = trio();
        let blk = block(nodes[0].proposer().unwrap(), 1, 1);
        for n in &mut nodes {
            n.receive_proposal(&blk, None).unwrap();
        }
        let v = nodes[0]
            .create_vote(VotePhase::Prepare, &test_key(1))
            .unwrap();
        nodes[0].receive_vote(v.clone()).unwrap();
        nodes[0].receive_vote(v.clone()).unwrap();
        let mut other = v.clone();
        other.block_hash = BlockHash(Hash([9; 32]));
        crate::finality::sign_vote(&mut other, &test_key(1)).unwrap();
        assert!(matches!(
            nodes[0].receive_vote(other).unwrap(),
            ConsensusEvent::Equivocation(_)
        ));
        let mut bad = v;
        bad.signature.0[1] ^= 0xff;
        assert_eq!(
            nodes[1].receive_vote(bad).unwrap_err(),
            ConsensusError::InvalidSignature
        );
    }

    #[test]
    fn create_vote_rejects_a_different_private_key() {
        let mut engine = engine();
        let proposer = engine.proposer().unwrap();
        engine
            .receive_proposal(&block(proposer, 1, 1), None)
            .unwrap();
        assert_eq!(
            engine
                .create_vote(VotePhase::Prepare, &test_key(2))
                .unwrap_err(),
            ConsensusError::InvalidSignature
        );
    }

    #[test]
    fn byzantine_split_cannot_finalize_both_without_equivocation() {
        let c = {
            let e = engine();
            e.state().committee.clone()
        };
        let mk = |ids: [u8; 2], salt: u8, phase: VotePhase| {
            ids.map(|id| {
                let mut v = Vote {
                    validator: bound_id(id),
                    epoch: 0,
                    height: 1,
                    view: 0,
                    block_hash: BlockHash(Hash([salt; 32])),
                    phase,
                    signature: rsc_core::Signature([0; 64]),
                };
                crate::finality::sign_vote(&mut v, &test_key(id)).unwrap();
                v
            })
        };
        for phase in [VotePhase::Prepare, VotePhase::PreCommit, VotePhase::Commit] {
            let a = mk([1, 2], 1, phase);
            assert!(aggregate_qc(&a, &c, phase).unwrap().0.is_none());
        }
    }
}
