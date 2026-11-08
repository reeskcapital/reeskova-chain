//! QC, quorum >2/3 y finality. Sin floats. Sin slashing económico.

use crate::committee::{calculate_total_voting_power, Committee};
use crate::ConsensusError;
use rsc_core::{AccountId, BlockHash, Signature};
use rsc_crypto::{verify_signature_bundle, CryptoDomain, PrivateKey, PublicKey, SignatureBundle};

/// Fase de voto RSC-HS.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum VotePhase {
    /// Prepare.
    Prepare,
    /// Pre-commit.
    PreCommit,
    /// Commit.
    Commit,
}

/// Voto de un validator sobre `(epoch, height, view, hash, phase)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Vote {
    /// Autor.
    pub validator: AccountId,
    /// Epoch.
    pub epoch: u64,
    /// Altura.
    pub height: u64,
    /// Vista.
    pub view: u64,
    /// Bloque votado.
    pub block_hash: BlockHash,
    /// Fase.
    pub phase: VotePhase,
    /// Firma / prueba.
    pub signature: Signature,
}

/// Certificado de quorum de una fase.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuorumCertificate {
    /// Bloque.
    pub block_hash: BlockHash,
    /// Altura.
    pub height: u64,
    /// Vista.
    pub view: u64,
    /// Fase.
    pub phase: VotePhase,
    /// Epoch firmado por los votos de este certificado.
    pub epoch: u64,
    /// Voters ordenados por id.
    pub voters: Vec<(AccountId, u128)>,
    /// Suma de pesos.
    pub voting_power: u128,
    /// Firmas de `voters`, en el mismo orden, sobre el mensaje de voto.
    pub signatures: SignatureBundle,
}

/// Tres QC encadenados sobre el mismo bloque.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FinalityProof {
    /// Bloque finalizado.
    pub block_hash: BlockHash,
    /// Altura.
    pub height: u64,
    /// Vista.
    pub view: u64,
    /// QC prepare.
    pub prepare: QuorumCertificate,
    /// QC pre-commit.
    pub precommit: QuorumCertificate,
    /// QC commit.
    pub commit: QuorumCertificate,
}

/// Dos votos incompatibles del mismo validator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EquivocationEvidence {
    /// Primer voto.
    pub vote_a: Vote,
    /// Segundo voto.
    pub vote_b: Vote,
}

/// Mensaje canónico firmado.
pub fn vote_bytes(vote: &Vote) -> Vec<u8> {
    consensus_vote_message(
        vote.phase,
        vote.epoch,
        vote.height,
        vote.view,
        &vote.block_hash,
    )
}

fn consensus_vote_message(
    phase: VotePhase,
    epoch: u64,
    height: u64,
    view: u64,
    block_hash: &BlockHash,
) -> Vec<u8> {
    let mut msg = Vec::with_capacity(1 + 8 * 3 + 32);
    msg.push(phase as u8);
    msg.extend_from_slice(&epoch.to_le_bytes());
    msg.extend_from_slice(&height.to_le_bytes());
    msg.extend_from_slice(&view.to_le_bytes());
    msg.extend_from_slice(&block_hash.0 .0);
    msg
}

/// Firma un voto con la `PrivateKey` recibida, bajo `ConsensusVote`.
///
/// El `AccountId` del voto no entra en la clave.
pub fn sign_vote(vote: &mut Vote, key: &PrivateKey) -> Result<(), ConsensusError> {
    let sig = rsc_crypto::sign(key, CryptoDomain::ConsensusVote, &vote_bytes(vote))
        .map_err(|_| ConsensusError::InvalidSignature)?;
    vote.signature = rsc_core::Signature(sig.to_bytes());
    Ok(())
}

pub(crate) fn vote_signature_valid(vote: &Vote, public_key: &PublicKey) -> bool {
    let Ok(sig) = rsc_crypto::Signature::from_bytes(&vote.signature.0) else {
        return false;
    };
    rsc_crypto::verify(
        public_key,
        CryptoDomain::ConsensusVote,
        &vote_bytes(vote),
        &sig,
    )
    .is_ok()
}

/// Autor, binding del registro y firma contra `owner_public_key`.
pub fn verify_validator_vote(
    record: &crate::committee::ValidatorRecord,
    vote: &Vote,
) -> Result<(), ConsensusError> {
    if vote.validator != record.id {
        return Err(ConsensusError::InvalidSignature);
    }
    crate::committee::validate_validator_record(record)?;
    if !vote_signature_valid(vote, &record.owner_public_key) {
        return Err(ConsensusError::InvalidSignature);
    }
    Ok(())
}

/// `3 * acc > 2 * total` con overflow check.
pub fn has_quorum(accumulated: u128, total: u128) -> Result<bool, ConsensusError> {
    if total == 0 {
        return Ok(false);
    }
    let left = accumulated.checked_mul(3).ok_or(ConsensusError::Overflow)?;
    let right = total.checked_mul(2).ok_or(ConsensusError::Overflow)?;
    Ok(left > right)
}

/// Misma posición de consenso, distinto bloque.
pub fn votes_conflict(a: &Vote, b: &Vote) -> bool {
    a.validator == b.validator
        && a.epoch == b.epoch
        && a.height == b.height
        && a.view == b.view
        && a.phase == b.phase
        && a.block_hash != b.block_hash
}

/// Evidencia si los votos chocan.
pub fn detect_equivocation(a: &Vote, b: &Vote) -> Option<EquivocationEvidence> {
    votes_conflict(a, b).then(|| EquivocationEvidence {
        vote_a: a.clone(),
        vote_b: b.clone(),
    })
}

fn power_of(committee: &Committee, id: &AccountId) -> Option<u128> {
    committee
        .members
        .iter()
        .find(|m| m.validator_id == *id)
        .map(|m| m.voting_power)
}

/// Agrega votos de una fase. Ignora duplicados del mismo bloque. Expone equivocation.
pub fn aggregate_qc(
    votes: &[Vote],
    committee: &Committee,
    phase: VotePhase,
) -> Result<(Option<QuorumCertificate>, Option<EquivocationEvidence>), ConsensusError> {
    let mut chosen: Vec<Vote> = Vec::new();
    for vote in votes {
        if vote.phase != phase {
            continue;
        }
        let Some(member) = committee
            .members
            .iter()
            .find(|member| member.validator_id == vote.validator)
        else {
            continue;
        };
        if !vote_signature_valid(vote, &member.public_key) {
            continue;
        }
        if power_of(committee, &vote.validator).is_none() {
            continue;
        }
        if let Some(prev) = chosen.iter().find(|v| v.validator == vote.validator) {
            if let Some(ev) = detect_equivocation(prev, vote) {
                return Ok((None, Some(ev)));
            }
            continue;
        }
        chosen.push(vote.clone());
    }
    if chosen.is_empty() {
        return Ok((None, None));
    }
    let block_hash = chosen[0].block_hash;
    let height = chosen[0].height;
    let view = chosen[0].view;
    let epoch = chosen[0].epoch;
    if chosen.iter().any(|v| {
        v.block_hash != block_hash || v.height != height || v.view != view || v.epoch != epoch
    }) {
        return Ok((None, None));
    }
    chosen.sort_by_key(|vote| vote.validator.0);
    let mut voters: Vec<(AccountId, u128)> = Vec::with_capacity(chosen.len());
    let mut signatures = Vec::with_capacity(chosen.len());
    for vote in &chosen {
        let Some(power) = power_of(committee, &vote.validator) else {
            continue;
        };
        let signature = rsc_crypto::Signature::from_bytes(&vote.signature.0)
            .map_err(|_| ConsensusError::InvalidSignature)?;
        voters.push((vote.validator, power));
        signatures.push(signature);
    }
    if signatures.is_empty() {
        return Ok((None, None));
    }
    let signatures = SignatureBundle::from_signatures(&signatures)
        .map_err(|_| ConsensusError::InvalidSignature)?;
    let voting_power = calculate_total_voting_power(
        &voters
            .iter()
            .map(|(id, p)| {
                let member = committee
                    .members
                    .iter()
                    .find(|member| member.validator_id == *id)?;
                Some(crate::committee::CommitteeMember {
                    validator_id: *id,
                    voting_power: *p,
                    vrf_proof: rsc_core::Hash::ZERO,
                    public_key: member.public_key,
                })
            })
            .collect::<Option<Vec<_>>>()
            .ok_or(ConsensusError::InvalidQc)?,
    )?;
    if !has_quorum(voting_power, committee.total_voting_power)? {
        return Ok((None, None));
    }
    Ok((
        Some(QuorumCertificate {
            block_hash,
            height,
            view,
            phase,
            epoch,
            voters,
            voting_power,
            signatures,
        }),
        None,
    ))
}

/// Verifica un QC contra el comité.
pub fn verify_qc(qc: &QuorumCertificate, committee: &Committee) -> Result<(), ConsensusError> {
    if qc.voters.len() != {
        let mut s = qc.voters.clone();
        s.sort_by_key(|a| a.0 .0);
        s.dedup_by(|a, b| a.0 == b.0);
        s.len()
    } {
        return Err(ConsensusError::InvalidQc);
    }
    let mut power = 0u128;
    for (id, claimed) in &qc.voters {
        let real = power_of(committee, id).ok_or(ConsensusError::InvalidQc)?;
        if real != *claimed {
            return Err(ConsensusError::InvalidQc);
        }
        power = power
            .checked_add(*claimed)
            .ok_or(ConsensusError::Overflow)?;
    }
    if power != qc.voting_power || !has_quorum(power, committee.total_voting_power)? {
        return Err(ConsensusError::InvalidQc);
    }
    let keys = registered_public_keys(committee, &qc.voters)?;
    let message = consensus_vote_message(qc.phase, qc.epoch, qc.height, qc.view, &qc.block_hash);
    verify_signature_bundle(&keys, CryptoDomain::ConsensusVote, &message, &qc.signatures).map_err(
        |err| match err {
            rsc_crypto::CryptoError::SignatureVerificationFailed
            | rsc_crypto::CryptoError::InvalidSignature => ConsensusError::InvalidSignature,
            _ => ConsensusError::InvalidQc,
        },
    )?;
    Ok(())
}

fn registered_public_keys(
    committee: &Committee,
    voters: &[(AccountId, u128)],
) -> Result<Vec<PublicKey>, ConsensusError> {
    voters
        .iter()
        .map(|(id, _)| {
            committee
                .members
                .iter()
                .find(|member| member.validator_id == *id)
                .map(|member| member.public_key)
                .ok_or(ConsensusError::InvalidQc)
        })
        .collect()
}

/// Prepare + precommit + commit sobre el mismo bloque/vista.
pub fn verify_commit_chain(
    proof: &FinalityProof,
    committee: &Committee,
) -> Result<(), ConsensusError> {
    for qc in [&proof.prepare, &proof.precommit, &proof.commit] {
        verify_qc(qc, committee)?;
        if qc.block_hash != proof.block_hash
            || qc.height != proof.height
            || qc.view != proof.view
            || qc.epoch != proof.prepare.epoch
        {
            return Err(ConsensusError::InvalidFinality);
        }
    }
    if proof.prepare.phase != VotePhase::Prepare
        || proof.precommit.phase != VotePhase::PreCommit
        || proof.commit.phase != VotePhase::Commit
    {
        return Err(ConsensusError::InvalidFinality);
    }
    Ok(())
}

/// ¿La prueba es final bajo RSC-HS?
pub fn is_finalized(proof: &FinalityProof, committee: &Committee) -> bool {
    verify_commit_chain(proof, committee).is_ok()
}

/// Dos pruebas finales incompatibles en la misma altura.
pub fn detect_conflicting_finality(a: &FinalityProof, b: &FinalityProof) -> bool {
    a.height == b.height && a.block_hash != b.block_hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::committee::{
        bound_record, find_validator_by_key_id, select_committee, test_cfg, test_key, test_set,
        validate_validator_record, CommitteeSelectionInput,
    };
    use proptest::prelude::*;
    use rsc_core::Hash;

    fn committee() -> Committee {
        select_committee(
            &CommitteeSelectionInput {
                epoch: 0,
                height: 1,
                view: 0,
                randomness: Hash([1; 32]),
            },
            &test_set(&[(1, 10, true), (2, 10, true), (3, 10, true)]),
            &test_cfg(),
        )
        .unwrap()
    }

    fn vote(id: u8, phase: VotePhase, hash: u8) -> Vote {
        let mut v = Vote {
            validator: bound_record(id, 1, true).id,
            epoch: 0,
            height: 1,
            view: 0,
            block_hash: BlockHash(Hash([hash; 32])),
            phase,
            signature: Signature([0; 64]),
        };
        sign_vote(&mut v, &test_key(id)).unwrap();
        v
    }

    fn proof_from(c: &Committee, hash: u8) -> FinalityProof {
        let phases = [VotePhase::Prepare, VotePhase::PreCommit, VotePhase::Commit];
        let qcs: Vec<QuorumCertificate> = phases
            .into_iter()
            .map(|p| {
                let votes = [1u8, 2, 3].map(|id| vote(id, p, hash));
                aggregate_qc(&votes, c, p).unwrap().0.unwrap()
            })
            .collect();
        FinalityProof {
            block_hash: BlockHash(Hash([hash; 32])),
            height: 1,
            view: 0,
            prepare: qcs[0].clone(),
            precommit: qcs[1].clone(),
            commit: qcs[2].clone(),
        }
    }

    #[test]
    fn quorum_and_finality() {
        assert!(!has_quorum(0, 100).unwrap());
        assert!(!has_quorum(666, 1000).unwrap());
        assert!(has_quorum(667, 1000).unwrap());
        let c = committee();
        assert!(aggregate_qc(&[], &c, VotePhase::Prepare)
            .unwrap()
            .0
            .is_none());
        let few = [vote(1, VotePhase::Prepare, 7)];
        assert!(aggregate_qc(&few, &c, VotePhase::Prepare)
            .unwrap()
            .0
            .is_none());
        let enough = [1u8, 2, 3].map(|id| vote(id, VotePhase::Prepare, 7));
        let qc = aggregate_qc(&enough, &c, VotePhase::Prepare)
            .unwrap()
            .0
            .unwrap();
        verify_qc(&qc, &c).unwrap();
        let mut bad = qc.clone();
        bad.voting_power = 1;
        assert_eq!(verify_qc(&bad, &c).unwrap_err(), ConsensusError::InvalidQc);
        let mut forged = qc.signatures.as_slice().to_vec();
        let mut raw = forged[0].to_bytes();
        raw[0] ^= 0x01;
        forged[0] = rsc_crypto::Signature::from_bytes(&raw).unwrap();
        bad.voting_power = qc.voting_power;
        bad.signatures = SignatureBundle::from_signatures(&forged).unwrap();
        assert_eq!(
            verify_qc(&bad, &c).unwrap_err(),
            ConsensusError::InvalidSignature
        );

        let p = proof_from(&c, 7);
        assert!(is_finalized(&p, &c));
        assert_eq!(verify_commit_chain(&p, &c), Ok(()));
        let mut corrupt = p.clone();
        corrupt.commit.voting_power = 1;
        assert!(verify_commit_chain(&corrupt, &c).is_err());
        assert!(detect_conflicting_finality(&p, &proof_from(&c, 8)));
        assert!(!detect_conflicting_finality(&p, &p));
    }

    #[test]
    fn duplicate_and_equivocation() {
        let c = committee();
        let a = vote(1, VotePhase::Prepare, 1);
        let mut dup = a.clone();
        let votes = [a.clone(), dup.clone()];
        let qc = aggregate_qc(&votes, &c, VotePhase::Prepare).unwrap();
        assert!(qc.1.is_none());
        dup.block_hash = BlockHash(Hash([2; 32]));
        sign_vote(&mut dup, &test_key(1)).unwrap();
        let ev = aggregate_qc(&[a, dup], &c, VotePhase::Prepare).unwrap().1;
        assert!(ev.is_some());
        let mut bad_sig = vote(1, VotePhase::Prepare, 1);
        bad_sig.signature.0[0] ^= 1;
        assert!(aggregate_qc(&[bad_sig], &c, VotePhase::Prepare)
            .unwrap()
            .0
            .is_none());
    }

    #[test]
    fn real_key_signs_and_registered_public_key_verifies() {
        let key = test_key(0x41);
        let other = test_key(0x42);
        let record = bound_record(0x41, 10, true);
        let foreign = bound_record(0x42, 10, true);
        assert_eq!(record.owner_public_key.key_id(), record.owner_key_id);
        assert_eq!(
            rsc_core::AccountId::from(rsc_state::derive_account_id(&record.owner_key_id)),
            record.id
        );
        validate_validator_record(&record).unwrap();
        let set = crate::committee::ValidatorSet {
            mode: crate::committee::NetworkMode::Bootstrap,
            validators: vec![record.clone(), foreign.clone()],
        };
        assert_eq!(
            find_validator_by_key_id(&set, &key.key_id()).unwrap().id,
            record.id
        );
        assert_eq!(
            find_validator_by_key_id(&set, &other.key_id()).unwrap().id,
            foreign.id
        );
        assert_eq!(
            find_validator_by_key_id(&set, &test_key(0x43).key_id()).unwrap_err(),
            ConsensusError::Ineligible
        );

        let mut vote = Vote {
            validator: record.id,
            epoch: 4,
            height: 9,
            view: 2,
            block_hash: BlockHash(Hash([0xab; 32])),
            phase: VotePhase::Prepare,
            signature: Signature([0; 64]),
        };
        let mut expected = Vec::new();
        expected.push(vote.phase as u8);
        expected.extend_from_slice(&vote.epoch.to_le_bytes());
        expected.extend_from_slice(&vote.height.to_le_bytes());
        expected.extend_from_slice(&vote.view.to_le_bytes());
        expected.extend_from_slice(&vote.block_hash.0 .0);
        assert_eq!(vote_bytes(&vote), expected);
        assert_eq!(expected.len(), 1 + 8 * 3 + 32);
        let signed = rsc_crypto::sign(&key, CryptoDomain::ConsensusVote, &expected).unwrap();
        sign_vote(&mut vote, &key).unwrap();
        assert_eq!(vote.signature.0, signed.to_bytes());
        assert_eq!(vote_bytes(&vote), expected);
        verify_validator_vote(&record, &vote).unwrap();

        let synthetic = PrivateKey::from_bytes(&record.id.0).unwrap();
        assert_ne!(synthetic.public_key(), record.owner_public_key);
        assert!(!vote_signature_valid(&vote, &synthetic.public_key()));

        vote.signature.0[0] ^= 0xff;
        assert_eq!(
            verify_validator_vote(&record, &vote).unwrap_err(),
            ConsensusError::InvalidSignature
        );
        vote.signature.0[0] ^= 0xff;

        let mut wrong_key = record.clone();
        wrong_key.owner_public_key = other.public_key();
        assert_eq!(
            verify_validator_vote(&wrong_key, &vote).unwrap_err(),
            ConsensusError::InvalidSignature
        );
        let mut arbitrary = record.clone();
        arbitrary.id = rsc_core::AccountId::ZERO;
        assert_eq!(
            validate_validator_record(&arbitrary).unwrap_err(),
            ConsensusError::InvalidSignature
        );
        let mut wrong_owner = record.clone();
        wrong_owner.owner_key_id = other.key_id();
        assert_eq!(
            validate_validator_record(&wrong_owner).unwrap_err(),
            ConsensusError::InvalidSignature
        );
        assert_eq!(
            verify_validator_vote(&wrong_owner, &vote).unwrap_err(),
            ConsensusError::InvalidSignature
        );
        let mut other_author = vote.clone();
        other_author.validator = foreign.id;
        assert_eq!(
            verify_validator_vote(&record, &other_author).unwrap_err(),
            ConsensusError::InvalidSignature
        );

        assert_eq!(format!("{key:?}"), "PrivateKey(****)");
        assert_eq!(format!("{key}"), "PrivateKey(****)");

        assert!(!has_quorum(666, 1000).unwrap());
        assert!(has_quorum(667, 1000).unwrap());
        let committee = committee();
        let enough = [1u8, 2, 3].map(vote_of);
        let qc = aggregate_qc(&enough, &committee, VotePhase::Prepare)
            .unwrap()
            .0
            .unwrap();
        assert_eq!(qc.signatures.as_slice().len(), qc.voters.len());
        verify_qc(&qc, &committee).unwrap();
        assert!(has_quorum(qc.voting_power, committee.total_voting_power).unwrap());
    }

    fn vote_of(id: u8) -> Vote {
        vote(id, VotePhase::Prepare, 7)
    }

    proptest! {
        #[test]
        fn quorum_threshold(acc in 0u128..200, total in 1u128..200) {
            let expect = acc.saturating_mul(3) > total.saturating_mul(2);
            prop_assert_eq!(has_quorum(acc, total).unwrap(), expect);
        }
    }
}
