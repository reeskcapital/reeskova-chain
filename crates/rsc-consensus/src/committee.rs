//! Comité y proposer deterministas. Participación ≠ voto.

use crate::ConsensusError;
use rsc_core::{AccountId, Hash};
use rsc_crypto::{KeyId, PublicKey};

/// Parámetros de genesis. Sin tamaños inventados en el motor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsensusConfig {
    /// Tamaño máximo del comité. Lo fija genesis.
    pub committee_size: usize,
    /// Bloques por epoch. Lo fija genesis.
    pub epoch_length: u64,
    /// Peso mínimo para ser validator.
    pub min_validator_weight: u128,
    /// Tope de peso por validator.
    pub max_validator_weight: u128,
    /// Umbral de validators para salir de bootstrap. Genesis.
    pub min_validators_for_distributed: usize,
}

/// Modo de red. La transición es por umbrales, no por un admin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetworkMode {
    /// Set inicial de genesis.
    Bootstrap,
    /// Más validators elegibles.
    Distributed,
    /// Red autónoma.
    Autonomous,
}

/// Score de la capa de participación. No es voting power.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct ParticipationScore(pub u128);

/// Registro de un validator. El peso ya viene calculado fuera del engine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatorRecord {
    /// Identidad de protocolo. Sale de `derive_account_id(owner_key_id)`.
    pub id: AccountId,
    /// `KeyId` de `owner_public_key`.
    pub owner_key_id: KeyId,
    /// Clave pública que verifica los votos de este validador.
    pub owner_public_key: PublicKey,
    /// Voting power ya acotado por la capa de pesos.
    pub voting_power: u128,
    /// Contribución móvil/red. No se suma sola al voto.
    pub participation_score: ParticipationScore,
    /// Elegible para comité.
    pub eligible: bool,
}

/// Snapshot de epoch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatorSet {
    /// Validadores del epoch.
    pub validators: Vec<ValidatorRecord>,
    /// Modo actual.
    pub mode: NetworkMode,
}

/// Entrada de selección. Misma entrada → mismo comité.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitteeSelectionInput {
    /// Epoch.
    pub epoch: u64,
    /// Altura.
    pub height: u64,
    /// Vista HotStuff.
    pub view: u64,
    /// Beacon verificable. No `rand`.
    pub randomness: Hash,
}

/// Miembro del comité.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitteeMember {
    /// Validator.
    pub validator_id: AccountId,
    /// Peso en esta ronda.
    pub voting_power: u128,
    /// Prueba de selección (digest verificable / futuro VRF).
    pub vrf_proof: Hash,
    /// Clave pública copiada del registro. Verifica el voto.
    pub public_key: PublicKey,
}

/// Comité de una vista.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Committee {
    /// Miembros ordenados por id.
    pub members: Vec<CommitteeMember>,
    /// Suma de pesos.
    pub total_voting_power: u128,
}

impl ConsensusConfig {
    /// Rechaza ceros que romperían el protocolo.
    pub fn validate(&self) -> Result<(), ConsensusError> {
        if self.committee_size == 0
            || self.epoch_length == 0
            || self.max_validator_weight == 0
            || self.min_validator_weight > self.max_validator_weight
        {
            return Err(ConsensusError::InvalidConfig);
        }
        Ok(())
    }
}

/// `owner_public_key` corresponde a `owner_key_id`, y `id` es `derive_account_id` de ese `KeyId`.
pub fn validate_validator_record(record: &ValidatorRecord) -> Result<(), ConsensusError> {
    if record.owner_public_key.key_id() != record.owner_key_id {
        return Err(ConsensusError::InvalidSignature);
    }
    let derived = AccountId::from(rsc_state::derive_account_id(&record.owner_key_id));
    if derived != record.id {
        return Err(ConsensusError::InvalidSignature);
    }
    Ok(())
}

/// Registro cuyo `owner_key_id` es el `KeyId` local y cuyo `AccountId` sale de esa clave.
pub fn find_validator_by_key_id<'a>(
    set: &'a ValidatorSet,
    owner_key_id: &KeyId,
) -> Result<&'a ValidatorRecord, ConsensusError> {
    let record = set
        .validators
        .iter()
        .find(|record| record.owner_key_id == *owner_key_id)
        .ok_or(ConsensusError::Ineligible)?;
    validate_validator_record(record)?;
    Ok(record)
}

impl CommitteeSelectionInput {
    fn seed(&self) -> [u8; 32 + 8 * 3] {
        let mut out = [0u8; 32 + 24];
        out[..32].copy_from_slice(&self.randomness.0);
        out[32..40].copy_from_slice(&self.epoch.to_le_bytes());
        out[40..48].copy_from_slice(&self.height.to_le_bytes());
        out[48..56].copy_from_slice(&self.view.to_le_bytes());
        out
    }
}

/// Bytes canónicos del beacon: seed || epoch_le || height_le || parent.
pub fn consensus_beacon_bytes(seed: &Hash, epoch: u64, height: u64, parent_block_hash: &Hash) -> Vec<u8> {
    let mut data = Vec::with_capacity(32 + 8 + 8 + 32);
    data.extend_from_slice(&seed.0);
    data.extend_from_slice(&epoch.to_le_bytes());
    data.extend_from_slice(&height.to_le_bytes());
    data.extend_from_slice(&parent_block_hash.0);
    data
}

/// Beacon determinista y verificable de una altura.
///
/// `Hash(hash_domain(ConsensusBeacon, V1, consensus_beacon_bytes(...)))`.
/// El mismo input produce el mismo digest en cualquier proceso. No es un VRF.
pub fn consensus_beacon(seed: &Hash, epoch: u64, height: u64, parent_block_hash: &Hash) -> Hash {
    let digest = rsc_crypto::hash_domain(
        rsc_crypto::CryptoDomain::ConsensusBeacon,
        rsc_crypto::CryptoVersion::V1,
        &consensus_beacon_bytes(seed, epoch, height, parent_block_hash),
    );
    Hash(digest.to_bytes())
}

/// Aplica solo min/max. No inventa la curva bond+participación.
pub fn bounded_weight(raw: u128, cfg: &ConsensusConfig) -> Result<u128, ConsensusError> {
    cfg.validate()?;
    if raw < cfg.min_validator_weight {
        return Err(ConsensusError::Ineligible);
    }
    Ok(core::cmp::min(raw, cfg.max_validator_weight))
}

/// Suma overflow-safe.
pub fn calculate_total_voting_power(members: &[CommitteeMember]) -> Result<u128, ConsensusError> {
    let mut total = 0u128;
    for m in members {
        total = total
            .checked_add(m.voting_power)
            .ok_or(ConsensusError::Overflow)?;
    }
    Ok(total)
}

fn eligible<'a>(
    set: &'a ValidatorSet,
    cfg: &ConsensusConfig,
) -> Result<Vec<&'a ValidatorRecord>, ConsensusError> {
    cfg.validate()?;
    let mut out = Vec::new();
    for v in &set.validators {
        if !v.eligible || v.voting_power < cfg.min_validator_weight {
            continue;
        }
        out.push(v);
    }
    out.sort_by_key(|a| a.id.0);
    Ok(out)
}

fn selection_digest(seed: &[u8], extra: &[u8]) -> Hash {
    let mut payload = Vec::with_capacity(seed.len() + extra.len());
    payload.extend_from_slice(seed);
    payload.extend_from_slice(extra);
    Hash(
        rsc_crypto::hash_domain(
            rsc_crypto::CryptoDomain::ValidatorIdentity,
            rsc_crypto::CryptoVersion::V1,
            &payload,
        )
        .to_bytes(),
    )
}

fn proof(input: &CommitteeSelectionInput, id: &AccountId) -> Hash {
    selection_digest(&input.seed(), &id.0)
}

/// Selección determinista. Tamaño ≤ `committee_size`.
pub fn select_committee(
    input: &CommitteeSelectionInput,
    set: &ValidatorSet,
    cfg: &ConsensusConfig,
) -> Result<Committee, ConsensusError> {
    let eligible = eligible(set, cfg)?;
    let mut ranked: Vec<CommitteeMember> = eligible
        .into_iter()
        .map(|v| CommitteeMember {
            validator_id: v.id,
            voting_power: core::cmp::min(v.voting_power, cfg.max_validator_weight),
            vrf_proof: proof(input, &v.id),
            public_key: v.owner_public_key,
        })
        .collect();
    ranked.sort_by(|a, b| {
        b.vrf_proof
            .0
            .cmp(&a.vrf_proof.0)
            .then(a.validator_id.0.cmp(&b.validator_id.0))
    });
    ranked.truncate(cfg.committee_size);
    ranked.sort_by_key(|a| a.validator_id.0);
    if ranked.is_empty() {
        return Err(ConsensusError::EmptyCommittee);
    }
    Ok(Committee {
        total_voting_power: calculate_total_voting_power(&ranked)?,
        members: ranked,
    })
}

/// Proposer ponderado y determinista sobre el comité.
pub fn select_proposer(
    input: &CommitteeSelectionInput,
    committee: &Committee,
) -> Result<AccountId, ConsensusError> {
    if committee.members.is_empty() || committee.total_voting_power == 0 {
        return Err(ConsensusError::EmptyCommittee);
    }
    let mut members = committee.members.clone();
    members.sort_by_key(|a| a.validator_id.0);
    let ticket_hash = selection_digest(&input.seed(), b"proposer");
    let mut ticket_bytes = [0u8; 16];
    ticket_bytes.copy_from_slice(&ticket_hash.0[..16]);
    let ticket = u128::from_be_bytes(ticket_bytes) % committee.total_voting_power;
    let mut acc = 0u128;
    for m in &members {
        acc = acc
            .checked_add(m.voting_power)
            .ok_or(ConsensusError::Overflow)?;
        if ticket < acc {
            return Ok(m.validator_id);
        }
    }
    members
        .last()
        .map(|m| m.validator_id)
        .ok_or(ConsensusError::EmptyCommittee)
}

/// Recomputa el comité y lo compara.
pub fn verify_selection(
    input: &CommitteeSelectionInput,
    set: &ValidatorSet,
    cfg: &ConsensusConfig,
    committee: &Committee,
) -> Result<(), ConsensusError> {
    if select_committee(input, set, cfg)? != *committee {
        return Err(ConsensusError::InvalidSelection);
    }
    Ok(())
}

/// Epoch de una altura. `epoch_length` viene de config.
pub fn epoch_of(height: u64, epoch_length: u64) -> Result<u64, ConsensusError> {
    if epoch_length == 0 {
        return Err(ConsensusError::InvalidConfig);
    }
    Ok(height / epoch_length)
}

#[cfg(test)]
pub(crate) fn test_id(n: u8) -> AccountId {
    let mut bytes = [0u8; 32];
    bytes[31] = n;
    AccountId(bytes)
}

#[cfg(test)]
pub(crate) fn test_cfg() -> ConsensusConfig {
    ConsensusConfig {
        committee_size: 3,
        epoch_length: 10,
        min_validator_weight: 1,
        max_validator_weight: 20,
        min_validators_for_distributed: 4,
    }
}

#[cfg(test)]
pub(crate) fn test_key(seed: u8) -> rsc_crypto::PrivateKey {
    rsc_crypto::PrivateKey::from_bytes(&[seed; 32]).expect("seed")
}

#[cfg(test)]
pub(crate) fn bound_record(seed: u8, weight: u128, eligible: bool) -> ValidatorRecord {
    let owner_public_key = test_key(seed).public_key();
    let owner_key_id = owner_public_key.key_id();
    ValidatorRecord {
        id: AccountId::from(rsc_state::derive_account_id(&owner_key_id)),
        owner_key_id,
        owner_public_key,
        voting_power: weight,
        participation_score: ParticipationScore(0),
        eligible,
    }
}

#[cfg(test)]
pub(crate) fn bound_id(seed: u8) -> AccountId {
    bound_record(seed, 1, true).id
}

#[cfg(test)]
pub(crate) fn test_set(weights: &[(u8, u128, bool)]) -> ValidatorSet {
    ValidatorSet {
        mode: NetworkMode::Bootstrap,
        validators: weights
            .iter()
            .map(|(id, w, ok)| bound_record(*id, *w, *ok))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn input(seed: u8, view: u64) -> CommitteeSelectionInput {
        CommitteeSelectionInput {
            epoch: 0,
            height: 1,
            view,
            randomness: Hash([seed; 32]),
        }
    }

    #[test]
    fn same_input_same_committee_and_proposer() {
        let cfg = test_cfg();
        let set = test_set(&[(1, 5, true), (2, 5, true), (3, 5, true), (4, 5, false)]);
        let a = select_committee(&input(1, 0), &set, &cfg).unwrap();
        let b = select_committee(&input(1, 0), &set, &cfg).unwrap();
        assert_eq!(a, b);
        verify_selection(&input(1, 0), &set, &cfg, &a).unwrap();
        assert!(a.members.iter().all(|m| m.validator_id != bound_id(4)));
        assert!(a.members.len() <= cfg.committee_size);
        assert_eq!(
            select_proposer(&input(1, 0), &a).unwrap(),
            select_proposer(&input(1, 0), &a).unwrap()
        );
        let other = select_committee(&input(9, 0), &set, &cfg).unwrap();
        assert_ne!(a, other);
        let mut tampered = a.clone();
        tampered.members[0].vrf_proof = Hash([0; 32]);
        assert!(verify_selection(&input(1, 0), &set, &cfg, &tampered).is_err());
    }

    #[test]
    fn participation_does_not_buy_weight() {
        let cfg = test_cfg();
        let mut set = test_set(&[(1, 4, true), (2, 4, true), (3, 4, true)]);
        let before = select_committee(&input(2, 0), &set, &cfg).unwrap();
        set.validators[0].participation_score = ParticipationScore(1_000_000);
        let after = select_committee(&input(2, 0), &set, &cfg).unwrap();
        assert_eq!(before.total_voting_power, after.total_voting_power);
        assert_eq!(
            before.members[0].voting_power,
            after.members[0].voting_power
        );
    }

    #[test]
    fn max_weight_is_capped() {
        let cfg = test_cfg();
        let set = test_set(&[(1, 99, true), (2, 4, true), (3, 4, true)]);
        let c = select_committee(&input(3, 0), &set, &cfg).unwrap();
        assert!(c
            .members
            .iter()
            .all(|m| m.voting_power <= cfg.max_validator_weight));
    }

    #[test]
    fn the_beacon_is_deterministic_and_input_sensitive() {
        let seed = Hash([0x5A; 32]);
        let parent = Hash([0xAB; 32]);
        let epoch = 4u64;
        let height = 9u64;
        let left = consensus_beacon(&seed, epoch, height, &parent);
        let right = consensus_beacon(&seed, epoch, height, &parent);
        assert_eq!(left, right);
        assert_ne!(seed, Hash::ZERO);
        let bytes = consensus_beacon_bytes(&seed, epoch, height, &parent);
        assert_eq!(bytes.len(), 80);
        assert_eq!(&bytes[..32], &seed.0);
        assert_eq!(&bytes[32..40], &epoch.to_le_bytes());
        assert_eq!(&bytes[40..48], &height.to_le_bytes());
        assert_eq!(&bytes[48..], &parent.0);
        let digest = rsc_crypto::hash_domain(
            rsc_crypto::CryptoDomain::ConsensusBeacon,
            rsc_crypto::CryptoVersion::V1,
            &bytes,
        );
        assert_eq!(left, Hash(digest.to_bytes()));
        let other_domain = rsc_crypto::hash_domain(
            rsc_crypto::CryptoDomain::ConsensusVote,
            rsc_crypto::CryptoVersion::V1,
            &bytes,
        );
        assert_ne!(left, Hash(other_domain.to_bytes()));
        let mut other_seed = seed;
        other_seed.0[0] ^= 0x01;
        assert_ne!(left, consensus_beacon(&other_seed, epoch, height, &parent));
        assert_ne!(left, consensus_beacon(&seed, epoch + 1, height, &parent));
        assert_ne!(left, consensus_beacon(&seed, epoch, height + 1, &parent));
        let mut other_parent = parent;
        other_parent.0[31] ^= 0x01;
        assert_ne!(left, consensus_beacon(&seed, epoch, height, &other_parent));
    }

    proptest! {
        #[test]
        fn selection_is_deterministic(seed: u8, view in 0u64..20) {
            let cfg = test_cfg();
            let set = test_set(&[(1, 3, true), (2, 4, true), (3, 5, true), (4, 6, true)]);
            let i = input(seed, view);
            let a = select_committee(&i, &set, &cfg).unwrap();
            let b = select_committee(&i, &set, &cfg).unwrap();
            prop_assert_eq!(&a, &b);
            prop_assert_eq!(select_proposer(&i, &a).unwrap(), select_proposer(&i, &b).unwrap());
        }
    }
}
