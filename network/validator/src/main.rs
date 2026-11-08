//! Lee `--key-file` y `--genesis`, deriva el `KeyId`, abre la base, reconstruye State y Ledger, y deja el motor en Decide con la FinalityProof del tip. Después sirve `POST /rpc` con el `RpcServer` existente hasta la señal de apagado.

use std::env;
use std::process::ExitCode;
use std::sync::Arc;

use rsc_chain::{Block, BlockHash, ChainStore, RocksDbChainStore};
use rsc_consensus::{
    consensus_beacon, find_validator_by_key_id, verify_validator_vote, ConsensusEngine,
    ConsensusEvent, FinalityProof, QuorumCertificate, ValidatorRecord, Vote, VotePhase,
};
use rsc_core::BlockHeight;
use rsc_crypto::{KeyId, PrivateKey};
use rsc_genesis::Genesis;
use rsc_ledger::{Ledger, Transaction};
use rsc_rpc::{
    ChainService, ConsensusService, NodeService, RpcAccount, RpcBalance, RpcBlock, RpcConfig,
    RpcContext, RpcNodeInfo, RpcServer, RpcStatus, RpcSubmitResult, RpcTransaction, RpcValidator,
    ServiceError, ServiceFut, StateService, TransactionService,
};
use rsc_state::State;
use rsc_storage::{ColumnFamily, DatabaseConfig, RscDatabase, StorageError};

/// Tope de valor que `DatabaseConfig` exige para abrir. No entra en la identidad.
const MAX_VALUE_SIZE: usize = 8 * 1024 * 1024;

/// Clave de la `FinalityProof` en `ColumnFamily::Default`.
const FINALITY_KEY: &[u8] = b"finality";

#[derive(Debug)]
struct Config {
    key_file: String,
    data_dir: String,
    network_id: String,
    chain_id: u32,
    genesis: String,
}

struct Recovered {
    key_id: KeyId,
    state: State,
    ledger: Ledger,
    engine: ConsensusEngine,
    vote: Vote,
    qc: QuorumCertificate,
    precommit: Vote,
    precommit_qc: QuorumCertificate,
    commit: Vote,
    commit_qc: QuorumCertificate,
    proof: FinalityProof,
    store: RocksDbChainStore,
    rpc: rsc_rpc::RpcFinality,
}

fn main() -> ExitCode {
    match bootstrap(env::args().skip(1)) {
        Ok(ready) => {
            if let Err(err) = ready.state.version() {
                eprintln!("{err}");
                return ExitCode::from(1);
            }
            let _root = ready.ledger.root();
            let _height = ready.engine.state().height;
            let _voted = ready.vote.height;
            let _qc = ready.qc.height;
            let _precommit = ready.precommit.height;
            let _precommit_qc = ready.precommit_qc.height;
            let _commit = ready.commit.height;
            let _commit_qc = ready.commit_qc.height;
            let _proof = ready.proof.height;
            let _rpc = ready.rpc.finalized_height;
            let _store = ready.store.database();
            println!("{}", format_key_id(&ready.key_id));
            if std::io::Write::flush(&mut std::io::stdout()).is_err() {
                return ExitCode::from(1);
            }
            if let Err(err) = serve(ready) {
                eprintln!("{err}");
                return ExitCode::from(1);
            }
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("{err}");
            ExitCode::from(1)
        }
    }
}

fn bootstrap(args: impl IntoIterator<Item = String>) -> Result<Recovered, String> {
    let config = parse_args(args)?;
    let key = read_signing_key(&config.key_file)?;
    let key_id = key.key_id();
    let db = Arc::new(open_database(&config).map_err(|err| err.to_string())?);
    let (state, store) = share_database(db).map_err(|err| err.to_string())?;
    let ledger = rebuild_ledger(&store)?;
    let (mut engine, record) = open_engine(&config, &store, &key_id)?;
    let vote = prepare_vote(&store, &mut engine, &key, &record)?;
    let qc = prepare_quorum(&mut engine, &vote)?;
    let precommit = precommit_vote(&mut engine, &key, &record, &vote, &qc)?;
    let precommit_qc = precommit_quorum(&mut engine, &precommit)?;
    let commit = commit_vote(&mut engine, &key, &record, &precommit, &precommit_qc)?;
    let (commit_qc, generated) = commit_finality(&mut engine, &commit)?;
    let proof = recover_proof(&store, &engine, generated)?;
    let rpc = read_finality(&store, &engine.state().committee)?;
    if rpc.finalized_height != proof.height
        || rpc.view != proof.view
        || rpc.epoch != proof.prepare.epoch
        || rpc.finalized_hash != rsc_rpc::encode_hex(&proof.block_hash.0 .0)
        || rpc.prepare != rsc_rpc::encode_hex(&encode_qc(&proof.prepare)?)
        || rpc.precommit != rsc_rpc::encode_hex(&encode_qc(&proof.precommit)?)
        || rpc.commit != rsc_rpc::encode_hex(&encode_qc(&proof.commit)?)
    {
        return Err("finality distinta".into());
    }
    drop(key);
    Ok(Recovered {
        key_id,
        state,
        ledger,
        engine,
        vote,
        qc,
        precommit,
        precommit_qc,
        commit,
        commit_qc,
        proof,
        store,
        rpc,
    })
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Config, String> {
    let mut key_file = None;
    let mut data_dir = None;
    let mut network_id = None;
    let mut chain_id = None;
    let mut genesis = None;
    let mut args = args.into_iter();
    while let Some(flag) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| format!("falta el valor de {flag}"))?;
        match flag.as_str() {
            "--key-file" => set(&mut key_file, value, "key-file")?,
            "--data-dir" => set(&mut data_dir, value, "data-dir")?,
            "--network-id" => set(&mut network_id, value, "network-id")?,
            "--genesis" => set(&mut genesis, value, "genesis")?,
            "--chain-id" => {
                if value.is_empty() {
                    return Err("chain-id vacío".into());
                }
                if chain_id.is_some() {
                    return Err("chain-id repetido".into());
                }
                chain_id = Some(
                    value
                        .parse::<u32>()
                        .map_err(|_| "chain-id no es u32".to_string())?,
                );
            }
            other => return Err(format!("argumento desconocido {other}")),
        }
    }
    Ok(Config {
        key_file: key_file.ok_or("falta key-file")?,
        data_dir: data_dir.ok_or("falta data-dir")?,
        network_id: network_id.ok_or("falta network-id")?,
        chain_id: chain_id.ok_or("falta chain-id")?,
        genesis: genesis.ok_or("falta genesis")?,
    })
}

fn open_engine(
    config: &Config,
    store: &RocksDbChainStore,
    key_id: &KeyId,
) -> Result<(ConsensusEngine, ValidatorRecord), String> {
    let text = std::fs::read_to_string(&config.genesis).map_err(|err| err.to_string())?;
    let genesis: Genesis = serde_json::from_str(&text).map_err(|err| err.to_string())?;
    genesis.validate().map_err(|err| err.to_string())?;
    if genesis.chain_id.0 != config.chain_id {
        return Err("chain-id del genesis distinto".into());
    }
    let consensus_config = genesis
        .parameters
        .consensus_config()
        .map_err(|err| err.to_string())?;
    let validators = genesis.validator_set().map_err(|err| err.to_string())?;
    let local_record = find_validator_by_key_id(&validators, key_id)
        .map_err(|err| err.to_string())?
        .clone();
    let local = local_record.id;
    let tip_hash = store
        .get_canonical_tip()
        .map_err(|err| err.to_string())?
        .ok_or("falta tip")?;
    let tip = store
        .get_block(&tip_hash)
        .map_err(|err| err.to_string())?
        .ok_or("bloque ausente")?;
    let height = tip.header.height.0;
    let epoch = rsc_consensus::committee::epoch_of(height, consensus_config.epoch_length)
        .map_err(|err| err.to_string())?;
    let beacon = consensus_beacon(
        &genesis.parameters.consensus_randomness_seed,
        epoch,
        height,
        &tip.header.previous_block_hash.0,
    );
    let engine = ConsensusEngine::new(consensus_config, validators, beacon, local, height)
        .map_err(|err| err.to_string())?;
    Ok((engine, local_record))
}

fn prepare_vote(
    store: &RocksDbChainStore,
    engine: &mut ConsensusEngine,
    key: &PrivateKey,
    record: &ValidatorRecord,
) -> Result<Vote, String> {
    let tip = store
        .get_canonical_tip()
        .map_err(|err| err.to_string())?
        .ok_or("falta tip")?;
    let block = store
        .get_block(&tip)
        .map_err(|err| err.to_string())?
        .ok_or("bloque ausente")?;
    let event = engine
        .receive_proposal(&block, None)
        .map_err(|err| err.to_string())?;
    if event != ConsensusEvent::ProposalAccepted {
        return Err("propuesta no aceptada".into());
    }
    let vote = engine
        .create_vote(VotePhase::Prepare, key)
        .map_err(|err| err.to_string())?;
    verify_validator_vote(record, &vote).map_err(|err| err.to_string())?;
    let state = engine.state();
    if vote.validator != record.id
        || record.owner_key_id != key.key_id()
        || vote.phase != VotePhase::Prepare
        || vote.height != block.header.height.0
        || vote.height != state.height
        || vote.epoch != state.epoch
        || vote.view != state.view
        || vote.block_hash != tip
        || state.proposal != Some(tip)
        || state.locked.is_some()
        || state.phase != rsc_consensus::ConsensusPhase::Propose
    {
        return Err("voto distinto".into());
    }
    Ok(vote)
}

fn prepare_quorum(engine: &mut ConsensusEngine, vote: &Vote) -> Result<QuorumCertificate, String> {
    let event = engine
        .receive_vote(vote.clone())
        .map_err(|err| err.to_string())?;
    if event != ConsensusEvent::QuorumReached(VotePhase::Prepare) {
        return Err("quorum distinto".into());
    }
    let committee = {
        let state = engine.state();
        if state.phase != rsc_consensus::ConsensusPhase::Prepare
            || state.locked.is_some()
            || state.height != vote.height
            || state.epoch != vote.epoch
            || state.view != vote.view
            || state.proposal != Some(vote.block_hash)
        {
            return Err("fase distinta".into());
        }
        state.committee.clone()
    };
    let (certificate, equivocation) = rsc_consensus::finality::aggregate_qc(
        std::slice::from_ref(vote),
        &committee,
        VotePhase::Prepare,
    )
    .map_err(|err| err.to_string())?;
    if equivocation.is_some() {
        return Err("equivocación".into());
    }
    let certificate = certificate.ok_or("falta qc")?;
    rsc_consensus::verify_qc(&certificate, &committee).map_err(|err| err.to_string())?;
    if certificate.phase != VotePhase::Prepare
        || certificate.epoch != vote.epoch
        || certificate.height != vote.height
        || certificate.view != vote.view
        || certificate.block_hash != vote.block_hash
    {
        return Err("qc distinto".into());
    }
    Ok(certificate)
}

fn precommit_vote(
    engine: &mut ConsensusEngine,
    key: &PrivateKey,
    record: &ValidatorRecord,
    prepare: &Vote,
    prepare_qc: &QuorumCertificate,
) -> Result<Vote, String> {
    if prepare_qc.phase != VotePhase::Prepare
        || prepare_qc.block_hash != prepare.block_hash
        || prepare_qc.epoch != prepare.epoch
        || prepare_qc.height != prepare.height
        || prepare_qc.view != prepare.view
    {
        return Err("qc distinto".into());
    }
    let vote = engine
        .create_vote(VotePhase::PreCommit, key)
        .map_err(|err| err.to_string())?;
    verify_validator_vote(record, &vote).map_err(|err| err.to_string())?;
    let state = engine.state();
    if vote.validator != record.id
        || record.owner_key_id != key.key_id()
        || vote.phase != VotePhase::PreCommit
        || vote.block_hash != prepare.block_hash
        || vote.epoch != prepare.epoch
        || vote.height != prepare.height
        || vote.view != prepare.view
        || state.phase != rsc_consensus::ConsensusPhase::Prepare
        || state.locked.is_some()
        || state.proposal != Some(prepare.block_hash)
        || state.height != prepare.height
    {
        return Err("voto distinto".into());
    }
    Ok(vote)
}

fn precommit_quorum(engine: &mut ConsensusEngine, vote: &Vote) -> Result<QuorumCertificate, String> {
    let event = engine
        .receive_vote(vote.clone())
        .map_err(|err| err.to_string())?;
    if event != ConsensusEvent::QuorumReached(VotePhase::PreCommit) {
        return Err("quorum distinto".into());
    }
    let committee = {
        let state = engine.state();
        if state.phase != rsc_consensus::ConsensusPhase::PreCommit
            || state.locked != Some(vote.block_hash)
            || state.height != vote.height
            || state.epoch != vote.epoch
            || state.view != vote.view
            || state.proposal != Some(vote.block_hash)
        {
            return Err("fase distinta".into());
        }
        state.committee.clone()
    };
    let (certificate, equivocation) = rsc_consensus::finality::aggregate_qc(
        std::slice::from_ref(vote),
        &committee,
        VotePhase::PreCommit,
    )
    .map_err(|err| err.to_string())?;
    if equivocation.is_some() {
        return Err("equivocación".into());
    }
    let certificate = certificate.ok_or("falta qc")?;
    rsc_consensus::verify_qc(&certificate, &committee).map_err(|err| err.to_string())?;
    if certificate.phase != VotePhase::PreCommit
        || certificate.epoch != vote.epoch
        || certificate.height != vote.height
        || certificate.view != vote.view
        || certificate.block_hash != vote.block_hash
    {
        return Err("qc distinto".into());
    }
    Ok(certificate)
}

fn commit_vote(
    engine: &mut ConsensusEngine,
    key: &PrivateKey,
    record: &ValidatorRecord,
    precommit: &Vote,
    precommit_qc: &QuorumCertificate,
) -> Result<Vote, String> {
    if precommit_qc.phase != VotePhase::PreCommit
        || precommit_qc.block_hash != precommit.block_hash
        || precommit_qc.epoch != precommit.epoch
        || precommit_qc.height != precommit.height
        || precommit_qc.view != precommit.view
    {
        return Err("qc distinto".into());
    }
    let vote = engine
        .create_vote(VotePhase::Commit, key)
        .map_err(|err| err.to_string())?;
    verify_validator_vote(record, &vote).map_err(|err| err.to_string())?;
    let state = engine.state();
    if vote.validator != record.id
        || record.owner_key_id != key.key_id()
        || vote.phase != VotePhase::Commit
        || vote.block_hash != precommit.block_hash
        || vote.epoch != precommit.epoch
        || vote.height != precommit.height
        || vote.view != precommit.view
        || state.phase != rsc_consensus::ConsensusPhase::PreCommit
        || state.locked != Some(precommit.block_hash)
        || state.proposal != Some(precommit.block_hash)
        || state.height != precommit.height
    {
        return Err("voto distinto".into());
    }
    Ok(vote)
}

fn commit_finality(
    engine: &mut ConsensusEngine,
    vote: &Vote,
) -> Result<(QuorumCertificate, FinalityProof), String> {
    let event = engine
        .receive_vote(vote.clone())
        .map_err(|err| err.to_string())?;
    let ConsensusEvent::BlockFinalized(proof) = event else {
        return Err("finality distinta".into());
    };
    let committee = {
        let state = engine.state();
        if state.phase != rsc_consensus::ConsensusPhase::Decide
            || state.locked != Some(vote.block_hash)
            || state.height != vote.height
            || state.epoch != vote.epoch
            || state.view != vote.view
            || state.proposal != Some(vote.block_hash)
        {
            return Err("fase distinta".into());
        }
        state.committee.clone()
    };
    let (certificate, equivocation) = rsc_consensus::finality::aggregate_qc(
        std::slice::from_ref(vote),
        &committee,
        VotePhase::Commit,
    )
    .map_err(|err| err.to_string())?;
    if equivocation.is_some() {
        return Err("equivocación".into());
    }
    let certificate = certificate.ok_or("falta qc")?;
    rsc_consensus::verify_qc(&certificate, &committee).map_err(|err| err.to_string())?;
    rsc_consensus::verify_commit_chain(&proof, &committee).map_err(|err| err.to_string())?;
    if certificate.phase != VotePhase::Commit
        || certificate.epoch != vote.epoch
        || certificate.height != vote.height
        || certificate.view != vote.view
        || certificate.block_hash != vote.block_hash
        || proof.block_hash != vote.block_hash
        || proof.height != vote.height
        || proof.view != vote.view
        || proof.prepare.phase != VotePhase::Prepare
        || proof.precommit.phase != VotePhase::PreCommit
        || proof.commit.phase != VotePhase::Commit
        || proof.commit.block_hash != vote.block_hash
    {
        return Err("finality distinta".into());
    }
    Ok((certificate, proof))
}

fn recover_proof(
    store: &RocksDbChainStore,
    engine: &ConsensusEngine,
    generated: FinalityProof,
) -> Result<FinalityProof, String> {
    let database = store.database();
    if let Some(stored) = database
        .get(ColumnFamily::Default, FINALITY_KEY)
        .map_err(|err| err.to_string())?
    {
        let proof = decode_proof(stored.as_bytes())?;
        rsc_consensus::verify_commit_chain(&proof, &engine.state().committee)
            .map_err(|err| err.to_string())?;
        if proof != generated {
            return Err("finality distinta".into());
        }
        return Ok(proof);
    }
    database
        .put(ColumnFamily::Default, FINALITY_KEY, &encode_proof(&generated)?)
        .map_err(|err| err.to_string())?;
    Ok(generated)
}

fn read_finality(
    store: &RocksDbChainStore,
    committee: &rsc_consensus::Committee,
) -> Result<rsc_rpc::RpcFinality, String> {
    let Some(stored) = store
        .database()
        .get(ColumnFamily::Default, FINALITY_KEY)
        .map_err(|err| err.to_string())?
    else {
        return Err(rsc_rpc::RpcError::ServiceUnavailable
            .public_message()
            .into());
    };
    let proof = decode_proof(stored.as_bytes())?;
    rsc_consensus::verify_commit_chain(&proof, committee).map_err(|err| err.to_string())?;
    Ok(rsc_rpc::RpcFinality {
        finalized_height: proof.height,
        finalized_hash: rsc_rpc::encode_hex(&proof.block_hash.0 .0),
        epoch: proof.prepare.epoch,
        view: proof.view,
        prepare: rsc_rpc::encode_hex(&encode_qc(&proof.prepare)?),
        precommit: rsc_rpc::encode_hex(&encode_qc(&proof.precommit)?),
        commit: rsc_rpc::encode_hex(&encode_qc(&proof.commit)?),
    })
}

struct ValidatorRpc {
    store: RocksDbChainStore,
    engine: ConsensusEngine,
}

fn validator_context(ready: Recovered) -> RpcContext {
    let node = Arc::new(ValidatorRpc {
        store: ready.store,
        engine: ready.engine,
    });
    RpcContext::new(
        Arc::clone(&node) as Arc<dyn NodeService>,
        Arc::clone(&node) as Arc<dyn ChainService>,
        Arc::clone(&node) as Arc<dyn TransactionService>,
        Arc::clone(&node) as Arc<dyn StateService>,
        node,
    )
}

fn serve(ready: Recovered) -> Result<(), String> {
    let server = RpcServer::new(validator_context(ready), RpcConfig::default());
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|err| err.to_string())?
        .block_on(async move { server.run().await.map_err(|err| err.to_string()) })
}

fn rpc_rejected<'a, T: Send + 'a>() -> ServiceFut<'a, T> {
    Box::pin(async { Err(ServiceError::Unavailable) })
}

impl NodeService for ValidatorRpc {
    fn chain_id(&self) -> ServiceFut<'_, String> {
        rpc_rejected()
    }
    fn node_info(&self) -> ServiceFut<'_, RpcNodeInfo> {
        rpc_rejected()
    }
    fn status(&self) -> ServiceFut<'_, RpcStatus> {
        rpc_rejected()
    }
}

impl ChainService for ValidatorRpc {
    fn block_by_hash(&self, _: &str) -> ServiceFut<'_, RpcBlock> {
        rpc_rejected()
    }
    fn block_by_height(&self, _: u64) -> ServiceFut<'_, RpcBlock> {
        rpc_rejected()
    }
    fn transaction(&self, _: &str) -> ServiceFut<'_, RpcTransaction> {
        rpc_rejected()
    }
}

impl TransactionService for ValidatorRpc {
    fn submit_signed(&self, _: &[u8]) -> ServiceFut<'_, RpcSubmitResult> {
        rpc_rejected()
    }
}

impl StateService for ValidatorRpc {
    fn account(&self, _: &str) -> ServiceFut<'_, RpcAccount> {
        rpc_rejected()
    }
    fn balance(&self, _: &str) -> ServiceFut<'_, RpcBalance> {
        rpc_rejected()
    }
}

impl ConsensusService for ValidatorRpc {
    fn validators(&self) -> ServiceFut<'_, Vec<RpcValidator>> {
        rpc_rejected()
    }
    fn finality(&self) -> ServiceFut<'_, rsc_rpc::RpcFinality> {
        let result = read_finality(&self.store, &self.engine.state().committee).map_err(|err| {
            if err == rsc_rpc::RpcError::ServiceUnavailable.public_message() {
                ServiceError::ConsensusUnavailable
            } else {
                ServiceError::Internal
            }
        });
        Box::pin(async move { result })
    }
}

fn encode_proof(proof: &FinalityProof) -> Result<Vec<u8>, String> {
    let mut out = encode_qc(&proof.prepare)?;
    out.extend(encode_qc(&proof.precommit)?);
    out.extend(encode_qc(&proof.commit)?);
    Ok(out)
}

fn encode_qc(qc: &QuorumCertificate) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    body.extend_from_slice(&qc.block_hash.0 .0);
    body.extend_from_slice(&qc.height.to_le_bytes());
    body.extend_from_slice(&qc.view.to_le_bytes());
    body.extend_from_slice(&qc.epoch.to_le_bytes());
    body.push(match qc.phase {
        VotePhase::Prepare => 0,
        VotePhase::PreCommit => 1,
        VotePhase::Commit => 2,
    });
    body.extend_from_slice(&qc.voting_power.to_le_bytes());
    let voters = u32::try_from(qc.voters.len()).map_err(|_| "invalid finality")?;
    body.extend_from_slice(&voters.to_le_bytes());
    for (id, power) in &qc.voters {
        body.extend_from_slice(&id.0);
        body.extend_from_slice(&power.to_le_bytes());
    }
    let signatures = qc.signatures.to_bytes();
    let sig_len = u32::try_from(signatures.len()).map_err(|_| "invalid finality")?;
    body.extend_from_slice(&sig_len.to_le_bytes());
    body.extend_from_slice(&signatures);
    let len = u32::try_from(body.len()).map_err(|_| "invalid finality")?;
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

fn decode_proof(bytes: &[u8]) -> Result<FinalityProof, String> {
    let mut cursor = bytes;
    let prepare = decode_qc(&mut cursor)?;
    let precommit = decode_qc(&mut cursor)?;
    let commit = decode_qc(&mut cursor)?;
    if !cursor.is_empty() {
        return Err("invalid finality".into());
    }
    Ok(FinalityProof {
        block_hash: prepare.block_hash,
        height: prepare.height,
        view: prepare.view,
        prepare,
        precommit,
        commit,
    })
}

fn decode_qc(bytes: &mut &[u8]) -> Result<QuorumCertificate, String> {
    let len = u32::from_le_bytes(read_array(bytes)?) as usize;
    if bytes.len() < len {
        return Err("invalid finality".into());
    }
    let (body, rest) = bytes.split_at(len);
    *bytes = rest;
    let mut body = body;
    let block_hash = rsc_core::BlockHash(rsc_core::Hash(read_array(&mut body)?));
    let height = u64::from_le_bytes(read_array(&mut body)?);
    let view = u64::from_le_bytes(read_array(&mut body)?);
    let epoch = u64::from_le_bytes(read_array(&mut body)?);
    let phase = match read_array::<1>(&mut body)?[0] {
        0 => VotePhase::Prepare,
        1 => VotePhase::PreCommit,
        2 => VotePhase::Commit,
        _ => return Err("invalid finality".into()),
    };
    let voting_power = u128::from_le_bytes(read_array(&mut body)?);
    let voters_n = u32::from_le_bytes(read_array(&mut body)?) as usize;
    let mut voters = Vec::with_capacity(voters_n);
    for _ in 0..voters_n {
        let id = rsc_core::AccountId(read_array(&mut body)?);
        let power = u128::from_le_bytes(read_array(&mut body)?);
        voters.push((id, power));
    }
    let sig_len = u32::from_le_bytes(read_array(&mut body)?) as usize;
    if body.len() != sig_len {
        return Err("invalid finality".into());
    }
    let signatures = rsc_crypto::SignatureBundle::from_bytes(body).map_err(|err| err.to_string())?;
    Ok(QuorumCertificate {
        block_hash,
        height,
        view,
        phase,
        epoch,
        voters,
        voting_power,
        signatures,
    })
}

fn read_array<const N: usize>(bytes: &mut &[u8]) -> Result<[u8; N], String> {
    if bytes.len() < N {
        return Err("invalid finality".into());
    }
    let (head, tail) = bytes.split_at(N);
    *bytes = tail;
    head.try_into().map_err(|_| "invalid finality".into())
}

fn set(slot: &mut Option<String>, value: String, name: &str) -> Result<(), String> {
    if slot.is_some() {
        return Err(format!("{name} repetido"));
    }
    if value.is_empty() {
        return Err(format!("{name} vacío"));
    }
    *slot = Some(value);
    Ok(())
}

fn read_signing_key(path: &str) -> Result<PrivateKey, String> {
    let bytes = std::fs::read(path).map_err(|err| err.to_string())?;
    PrivateKey::from_bytes(&bytes).map_err(|err| err.to_string())
}

fn share_database(db: Arc<RscDatabase>) -> Result<(State, RocksDbChainStore), rsc_state::StateError> {
    let state = State::open(Arc::clone(&db))?;
    let store = RocksDbChainStore::from_shared(db);
    Ok((state, store))
}

fn rebuild_ledger(store: &RocksDbChainStore) -> Result<Ledger, String> {
    let blocks = canonical_blocks(store)?;
    let mut ledger = Ledger::new();
    for block in &blocks {
        for raw in &block.transactions {
            let transaction =
                Transaction::from_canonical_bytes(&raw.0).map_err(|err| err.to_string())?;
            ledger
                .append(transaction.id())
                .map_err(|err| err.to_string())?;
        }
    }
    Ok(ledger)
}

fn canonical_blocks(store: &RocksDbChainStore) -> Result<Vec<Block>, String> {
    let mut current = store
        .get_canonical_tip()
        .map_err(|err| err.to_string())?
        .ok_or("falta tip")?;
    let mut toward_genesis = Vec::new();
    let mut seen = Vec::new();
    loop {
        if seen.contains(&current) {
            return Err("ciclo en la cadena".into());
        }
        seen.push(current);
        let block = store
            .get_block(&current)
            .map_err(|err| err.to_string())?
            .ok_or("bloque ausente")?;
        let hash = block.block_hash().map_err(|err| err.to_string())?;
        if hash != current {
            return Err("hash de bloque distinto".into());
        }
        let parent = block.header.previous_block_hash;
        let is_genesis = block.header.height == BlockHeight::GENESIS;
        toward_genesis.push(block);
        if is_genesis {
            if parent != BlockHash::ZERO {
                return Err("genesis inconsistente".into());
            }
            break;
        }
        current = parent;
    }
    toward_genesis.reverse();
    for (index, block) in toward_genesis.iter().enumerate() {
        let height = u64::try_from(index).map_err(|_| "altura inconsistente")?;
        if block.header.height != BlockHeight(height) {
            return Err("altura inconsistente".into());
        }
        if index == 0 && block.header.previous_block_hash != BlockHash::ZERO {
            return Err("genesis inconsistente".into());
        }
        if index > 0 {
            let parent = toward_genesis[index - 1]
                .block_hash()
                .map_err(|err| err.to_string())?;
            if block.header.previous_block_hash != parent {
                return Err("padre distinto".into());
            }
        }
    }
    Ok(toward_genesis)
}

fn open_database(config: &Config) -> Result<RscDatabase, StorageError> {
    RscDatabase::open(
        &config.data_dir,
        DatabaseConfig {
            max_value_size: MAX_VALUE_SIZE,
            network_id: config.network_id.clone(),
            chain_id: config.chain_id.to_string(),
        },
    )
}

fn format_key_id(id: &KeyId) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(64);
    for byte in id.to_bytes() {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{
        bootstrap, format_key_id, open_database, open_engine, parse_args, read_finality,
        rebuild_ledger, share_database, FINALITY_KEY,
    };
    use rsc_chain::{
        encode_block, validate_block, Block, BlockHeader, ChainStore, ChainValidationContext,
        RawTransaction, RocksDbChainStore,
    };
    use rsc_consensus::{
        validate_validator_record, Committee, ConsensusEngine, ConsensusEvent, ConsensusPhase,
        FinalityProof, QuorumCertificate, ValidatorRecord, Vote, VotePhase,
    };
    use rsc_rpc::{
        methods::dispatch, ChainService, ConsensusService, NodeService, RpcAccount, RpcBalance,
        RpcBlock, RpcContext, RpcError, RpcNodeInfo, RpcStatus, RpcSubmitResult, RpcTransaction,
        RpcValidator, ServiceError, ServiceFut, StateService, TransactionService,
    };
    use rsc_core::{AccountId, BlockHash, BlockHeight, ChainId, Hash, Timestamp};
    use rsc_crypto::PrivateKey;
    use rsc_genesis::GENESIS_VERSION_V1;
    use rsc_ledger::transaction::TRANSACTION_VERSION_V1;
    use rsc_ledger::{Ledger, Transaction, TransactionPayload};
    use rsc_storage::StorageError;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    fn write_key(bytes: &[u8]) -> tempfile::NamedTempFile {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut file, bytes).unwrap();
        file
    }

    fn command(key: &str, dir: &str, network: &str, chain: &str) -> Vec<String> {
        args(&[
            "--key-file",
            key,
            "--data-dir",
            dir,
            "--network-id",
            network,
            "--chain-id",
            chain,
            "--genesis",
            "unused",
        ])
    }

    fn command_with_genesis(
        key: &str,
        dir: &str,
        network: &str,
        chain: &str,
        genesis: &str,
    ) -> Vec<String> {
        let mut values = command(key, dir, network, chain);
        *values.last_mut().unwrap() = genesis.to_string();
        values
    }

    fn failure(args: Vec<String>) -> String {
        match bootstrap(args) {
            Err(err) => err,
            Ok(_) => panic!("el arranque debía fallar"),
        }
    }

    fn write_document(dir: &std::path::Path, seed: u8, chain_id: u32, randomness: Hash) -> String {
        let secret = PrivateKey::from_bytes(&[seed; 32]).unwrap();
        let owner_public_key = secret.public_key();
        let owner_key_id = owner_public_key.key_id();
        let account_id = AccountId::from(rsc_state::derive_account_id(&owner_key_id));
        let document = rsc_genesis::Genesis::new(
            GENESIS_VERSION_V1,
            ChainId(chain_id),
            1_700_000_000,
            rsc_genesis::NetworkParameters {
                amount_scale: 1_000_000,
                max_block_size: 1_048_576,
                max_transactions_per_block: 1_000,
                max_transaction_bytes: 128 * 1024,
                minimum_fee: 0,
                committee_size: 7,
                epoch_length: 20,
                min_validator_weight: 3,
                max_validator_weight: 30,
                min_validators_for_distributed: 5,
                consensus_randomness_seed: randomness,
            },
            vec![rsc_genesis::GenesisAccount {
                account_id,
                owner_key_id,
                native_balance: 500,
                allocation: rsc_genesis::GenesisAllocationCategory::Community,
            }],
            vec![rsc_genesis::GenesisValidator {
                account_id,
                owner_key_id,
                owner_public_key,
                voting_power: 3,
                eligible: true,
            }],
        );
        let path = dir.join("genesis.json");
        std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
        path.to_str().unwrap().to_string()
    }

    fn config_at(path: &str, network_id: &str, chain_id: u32) -> super::Config {
        super::Config {
            key_file: "unused".into(),
            data_dir: path.to_string(),
            network_id: network_id.to_string(),
            chain_id,
            genesis: "unused".into(),
        }
    }

    fn local_proposer() -> AccountId {
        let owner = PrivateKey::from_bytes(&[0x11; 32])
            .unwrap()
            .public_key()
            .key_id();
        AccountId::from(rsc_state::derive_account_id(&owner))
    }

    fn seed_genesis(path: &str) {
        seed_chain(path, local_proposer());
    }

    fn seed_chain(path: &str, proposer: AccountId) {
        let db = Arc::new(open_database(&config_at(path, "devnet", 3)).unwrap());
        let (state, store) = share_database(db).unwrap();
        let owner = PrivateKey::from_bytes(&[0x11; 32])
            .unwrap()
            .public_key()
            .key_id();
        state
            .initialize_genesis(
                &[rsc_state::GenesisAccount::new(owner, 500)],
                u64::from(GENESIS_VERSION_V1),
            )
            .unwrap();
        let root = state.state_root().unwrap();
        let mut block = Block {
            header: BlockHeader {
                version: GENESIS_VERSION_V1,
                chain_id: ChainId(3),
                height: BlockHeight::GENESIS,
                previous_block_hash: BlockHash::ZERO,
                timestamp: Timestamp::from_secs(1_700_000_000),
                transactions_root: Hash::ZERO,
                state_root: Hash(*root.as_bytes()),
                proposer,
            },
            transactions: Vec::new(),
        };
        block.header.transactions_root = block.compute_transaction_root();
        validate_block(
            &block,
            &ChainValidationContext {
                expected_chain_id: ChainId(3),
                expected_protocol_version: GENESIS_VERSION_V1,
                max_block_size: 1_048_576,
                max_transactions_per_block: 1_000,
                parent: None,
            },
        )
        .unwrap();
        store.put_block(&block).unwrap();
        store
            .set_canonical_tip(&block.block_hash().unwrap())
            .unwrap();
    }

    fn tip_block(path: &str) -> Block {
        let db = Arc::new(open_database(&config_at(path, "devnet", 3)).unwrap());
        let (_state, store) = share_database(db).unwrap();
        let tip = store.get_canonical_tip().unwrap().unwrap();
        store.get_block(&tip).unwrap().unwrap()
    }

    fn append_block(path: &str, raw: Vec<Vec<u8>>) -> Block {
        let db = Arc::new(open_database(&config_at(path, "devnet", 3)).unwrap());
        let (_state, store) = share_database(db).unwrap();
        let parent = {
            let tip = store.get_canonical_tip().unwrap().unwrap();
            store.get_block(&tip).unwrap().unwrap()
        };
        let mut block = Block {
            header: BlockHeader {
                version: parent.header.version,
                chain_id: parent.header.chain_id,
                height: parent.header.height.checked_next().unwrap(),
                previous_block_hash: parent.block_hash().unwrap(),
                timestamp: parent.header.timestamp,
                transactions_root: Hash::ZERO,
                state_root: parent.header.state_root,
                proposer: local_proposer(),
            },
            transactions: raw.into_iter().map(RawTransaction).collect(),
        };
        block.header.transactions_root = block.compute_transaction_root();
        validate_block(
            &block,
            &ChainValidationContext {
                expected_chain_id: parent.header.chain_id,
                expected_protocol_version: parent.header.version,
                max_block_size: 1_048_576,
                max_transactions_per_block: 1_000,
                parent: Some(parent.header.clone()),
            },
        )
        .unwrap();
        store.put_block(&block).unwrap();
        store
            .set_canonical_tip(&block.block_hash().unwrap())
            .unwrap();
        block
    }

    fn snapshot(path: &str) -> (BlockHash, Vec<u8>, rsc_state::StateRoot, u64) {
        let db = Arc::new(open_database(&config_at(path, "devnet", 3)).unwrap());
        let (state, store) = share_database(db).unwrap();
        let tip = store.get_canonical_tip().unwrap().unwrap();
        let block = store.get_block(&tip).unwrap().unwrap();
        (
            tip,
            encode_block(&block).unwrap(),
            state.state_root().unwrap(),
            state.version().unwrap(),
        )
    }

    fn transfer(nonce: u64, amount: u128) -> Transaction {
        let key = PrivateKey::from_bytes(&[0x21; 32]).unwrap();
        let mut transaction = Transaction {
            version: TRANSACTION_VERSION_V1,
            sender: AccountId([0x11; 32]),
            nonce,
            payload: TransactionPayload::Transfer {
                recipient: AccountId([0x22; 32]),
                amount,
            },
            signature: rsc_crypto::Signature::from_bytes(&[0u8; 64]).unwrap(),
        };
        transaction.sign(&key).unwrap();
        transaction
    }

    #[test]
    fn a_valid_key_opens_a_new_database_and_the_same_identity() {
        let seed = [0x11; 32];
        let file = write_key(&seed);
        let key = file.path().to_str().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        seed_genesis(path);
        let document = write_document(dir.path(), 0x11, 3, Hash([0x3C; 32]));
        let key_id = bootstrap(command_with_genesis(key, path, "devnet", "3", &document))
            .unwrap()
            .key_id;
        let expected = PrivateKey::from_bytes(&seed).unwrap().key_id();
        assert_eq!(key_id, expected);
        let again = bootstrap(command_with_genesis(key, path, "devnet", "3", &document))
            .unwrap()
            .key_id;
        assert_eq!(again, key_id);
        let printed = format_key_id(&key_id);
        let secret = "11".repeat(32);
        assert_eq!(printed.len(), 64);
        assert!(!printed.contains(&secret));
    }

    #[test]
    fn genesis_rebuilds_an_empty_ledger_without_writing() {
        let file = write_key(&[0x11; 32]);
        let key = file.path().to_str().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        seed_genesis(path);
        let document = write_document(dir.path(), 0x11, 3, Hash([0x3C; 32]));
        let before = snapshot(path);
        let ready = bootstrap(command_with_genesis(key, path, "devnet", "3", &document)).unwrap();
        assert!(ready.ledger.is_empty());
        assert_eq!(
            ready.engine.network_mode(),
            rsc_consensus::NetworkMode::Bootstrap
        );
        let local = AccountId::from(rsc_state::derive_account_id(&ready.key_id));
        let beacon = rsc_consensus::consensus_beacon(&Hash([0x3C; 32]), 0, 0, &Hash::ZERO);
        assert_ne!(beacon, Hash::ZERO);
        let parsed: rsc_genesis::Genesis =
            serde_json::from_str(&std::fs::read_to_string(&document).unwrap()).unwrap();
        let expected = rsc_consensus::select_committee(
            &rsc_consensus::CommitteeSelectionInput {
                epoch: 0,
                height: 0,
                view: 0,
                randomness: beacon,
            },
            &parsed.validator_set().unwrap(),
            &parsed.parameters.consensus_config().unwrap(),
        )
        .unwrap();
        {
            let state = ready.engine.state();
            assert_eq!(state.height, 0);
            assert_eq!(state.view, 0);
            assert_eq!(state.phase, rsc_consensus::ConsensusPhase::Decide);
            assert_eq!(state.epoch, 0);
            assert_eq!(state.proposal, Some(before.0));
            assert_eq!(state.locked, Some(before.0));
            assert_eq!(state.committee.members[0].validator_id, local);
            assert_eq!(state.committee, expected);
        }
        assert_eq!(ready.vote.validator, local);
        assert_eq!(ready.vote.height, 0);
        assert_eq!(ready.vote.block_hash, before.0);
        assert_eq!(ready.qc.phase, rsc_consensus::VotePhase::Prepare);
        assert_eq!(ready.qc.height, 0);
        assert_eq!(ready.qc.block_hash, before.0);
        assert_eq!(ready.precommit.phase, rsc_consensus::VotePhase::PreCommit);
        assert_eq!(ready.precommit.block_hash, before.0);
        assert_eq!(ready.precommit_qc.phase, rsc_consensus::VotePhase::PreCommit);
        assert_eq!(ready.precommit_qc.block_hash, before.0);
        let root = ready.ledger.root();
        let secret = "11".repeat(32);
        assert!(!format_key_id(&ready.key_id).contains(&secret));
        drop(ready);
        assert_eq!(root, Ledger::new().root());
        assert_eq!(snapshot(path), before);
        assert!(tip_block(path).header.is_genesis());
    }

    #[test]
    fn a_persisted_block_rebuilds_its_ledger_root() {
        let file = write_key(&[0x11; 32]);
        let key = file.path().to_str().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        seed_genesis(path);
        let tx = transfer(0, 7);
        let child = append_block(path, vec![tx.canonical_bytes()]);
        let before = snapshot(path);
        let document = write_document(dir.path(), 0x11, 3, Hash([0x3C; 32]));
        let ready = bootstrap(command_with_genesis(key, path, "devnet", "3", &document)).unwrap();
        assert_eq!(ready.engine.state().height, 1);
        assert_eq!(ready.engine.state().epoch, 0);
        assert_eq!(ready.engine.state().view, 0);
        assert_eq!(ready.engine.state().phase, rsc_consensus::ConsensusPhase::Decide);
        assert_eq!(ready.engine.state().locked, Some(child.block_hash().unwrap()));
        assert_eq!(ready.qc.phase, rsc_consensus::VotePhase::Prepare);
        assert_eq!(ready.qc.height, 1);
        assert_eq!(ready.qc.block_hash, child.block_hash().unwrap());
        assert_eq!(ready.precommit_qc.phase, rsc_consensus::VotePhase::PreCommit);
        assert_eq!(ready.precommit_qc.height, 1);
        assert_eq!(ready.precommit_qc.block_hash, child.block_hash().unwrap());
        assert_eq!(ready.vote.height, 1);
        assert_eq!(ready.vote.epoch, 0);
        assert_eq!(ready.vote.view, 0);
        assert_eq!(ready.vote.phase, rsc_consensus::VotePhase::Prepare);
        assert_eq!(ready.vote.block_hash, child.block_hash().unwrap());
        let parent = child.header.previous_block_hash.0;
        let beacon = rsc_consensus::consensus_beacon(&Hash([0x3C; 32]), 0, 1, &parent);
        let parsed: rsc_genesis::Genesis =
            serde_json::from_str(&std::fs::read_to_string(&document).unwrap()).unwrap();
        let expected = rsc_consensus::select_committee(
            &rsc_consensus::CommitteeSelectionInput {
                epoch: 0,
                height: 1,
                view: 0,
                randomness: beacon,
            },
            &parsed.validator_set().unwrap(),
            &parsed.parameters.consensus_config().unwrap(),
        )
        .unwrap();
        assert_eq!(ready.engine.state().committee, expected);
        let mut expected = Ledger::new();
        expected.append(tx.id()).unwrap();
        assert_eq!(ready.ledger.root(), expected.root());
        assert_eq!(ready.ledger.get(0).unwrap().transaction_id, tx.id());
        drop(ready);
        assert_eq!(snapshot(path), before);
        assert_eq!(
            tip_block(path).block_hash().unwrap(),
            child.block_hash().unwrap()
        );
    }

    #[test]
    fn state_and_chain_store_share_one_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        seed_genesis(path);
        let db = Arc::new(open_database(&config_at(path, "devnet", 3)).unwrap());
        assert_eq!(Arc::strong_count(&db), 1);
        let (state, store) = share_database(Arc::clone(&db)).unwrap();
        assert!(std::sync::Arc::ptr_eq(&store.database(), &db));
        assert_eq!(Arc::strong_count(&db), 3);
        drop(state);
        drop(store);
        assert_eq!(Arc::strong_count(&db), 1);
    }

    #[test]
    fn a_missing_block_stops_recovery() {
        let file = write_key(&[0x11; 32]);
        let key = file.path().to_str().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        seed_genesis(path);
        let genesis = tip_block(path);
        let mut orphan = genesis.clone();
        orphan.header.height = BlockHeight(1);
        orphan.header.previous_block_hash = BlockHash(Hash([9u8; 32]));
        orphan.header.transactions_root = orphan.compute_transaction_root();
        {
            let db = Arc::new(open_database(&config_at(path, "devnet", 3)).unwrap());
            let (_state, store) = share_database(db).unwrap();
            store.put_block(&orphan).unwrap();
            store
                .set_canonical_tip(&orphan.block_hash().unwrap())
                .unwrap();
        }
        let before = snapshot(path);
        let err = failure(command(key, path, "devnet", "3"));
        assert_eq!(err, "bloque ausente");
        assert!(!err.contains("11"));
        assert_eq!(snapshot(path), before);
    }

    #[test]
    fn a_duplicate_transaction_stops_recovery() {
        let file = write_key(&[0x11; 32]);
        let key = file.path().to_str().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        seed_genesis(path);
        let tx = transfer(0, 7);
        append_block(path, vec![tx.canonical_bytes()]);
        append_block(path, vec![tx.canonical_bytes()]);
        let before = snapshot(path);
        let err = failure(command(key, path, "devnet", "3"));
        assert_eq!(err, "duplicate transaction");
        assert!(!err.contains("11"));
        assert_eq!(snapshot(path), before);
    }

    #[test]
    fn another_network_is_a_storage_mismatch() {
        let file = write_key(&[0x11; 32]);
        let key = file.path().to_str().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        drop(open_database(&config_at(path, "devnet", 3)).unwrap());
        let err = failure(command(key, path, "other", "3"));
        assert_eq!(err, StorageError::NetworkMismatch.to_string());
        assert!(!err.contains("11"));
    }

    #[test]
    fn another_chain_is_a_storage_mismatch() {
        let file = write_key(&[0x11; 32]);
        let key = file.path().to_str().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        drop(open_database(&config_at(path, "devnet", 3)).unwrap());
        let err = failure(command(key, path, "devnet", "9"));
        assert_eq!(err, StorageError::ChainMismatch.to_string());
        assert!(!err.contains("11"));
    }

    #[test]
    fn rejects_a_database_path_with_a_parent() {
        let file = write_key(&[0x11; 32]);
        let key = file.path().to_str().unwrap();
        let err = failure(command(key, "../rejected", "devnet", "3"));
        assert_eq!(err, StorageError::InvalidPath.to_string());
    }

    #[test]
    fn rejects_a_missing_flag() {
        let err = parse_args(args(&[])).unwrap_err();
        assert_eq!(err, "falta key-file");
        let err = parse_args(args(&["--key-file", "k"])).unwrap_err();
        assert_eq!(err, "falta data-dir");
        let err = parse_args(args(&["--key-file", "k", "--data-dir", "d"])).unwrap_err();
        assert_eq!(err, "falta network-id");
        let err = parse_args(args(&[
            "--key-file",
            "k",
            "--data-dir",
            "d",
            "--network-id",
            "devnet",
        ]))
        .unwrap_err();
        assert_eq!(err, "falta chain-id");
        let err = parse_args(args(&[
            "--key-file",
            "k",
            "--data-dir",
            "d",
            "--network-id",
            "devnet",
            "--chain-id",
            "3",
        ]))
        .unwrap_err();
        assert_eq!(err, "falta genesis");
    }

    #[test]
    fn rejects_a_missing_value() {
        let err = parse_args(args(&["--key-file"])).unwrap_err();
        assert_eq!(err, "falta el valor de --key-file");
    }

    #[test]
    fn rejects_an_empty_value() {
        let err = parse_args(args(&["--key-file", ""])).unwrap_err();
        assert_eq!(err, "key-file vacío");
        let err = parse_args(args(&["--data-dir", ""])).unwrap_err();
        assert_eq!(err, "data-dir vacío");
        let err = parse_args(args(&["--network-id", ""])).unwrap_err();
        assert_eq!(err, "network-id vacío");
        let err = parse_args(args(&["--chain-id", ""])).unwrap_err();
        assert_eq!(err, "chain-id vacío");
        let err = parse_args(args(&["--genesis", ""])).unwrap_err();
        assert_eq!(err, "genesis vacío");
    }

    #[test]
    fn rejects_a_repeated_flag() {
        let err = parse_args(args(&["--key-file", "a", "--key-file", "b"])).unwrap_err();
        assert_eq!(err, "key-file repetido");
        let err = parse_args(args(&["--data-dir", "a", "--data-dir", "b"])).unwrap_err();
        assert_eq!(err, "data-dir repetido");
        let err = parse_args(args(&["--network-id", "a", "--network-id", "b"])).unwrap_err();
        assert_eq!(err, "network-id repetido");
        let err = parse_args(args(&["--chain-id", "3", "--chain-id", "4"])).unwrap_err();
        assert_eq!(err, "chain-id repetido");
        let err = parse_args(args(&["--genesis", "a", "--genesis", "b"])).unwrap_err();
        assert_eq!(err, "genesis repetido");
    }

    #[test]
    fn rejects_a_chain_id_that_is_not_u32() {
        let err = parse_args(args(&["--chain-id", "no"])).unwrap_err();
        assert_eq!(err, "chain-id no es u32");
    }

    #[test]
    fn rejects_an_unknown_flag() {
        let err = parse_args(args(&["--listen", "/ip4/127.0.0.1/tcp/0"])).unwrap_err();
        assert_eq!(err, "argumento desconocido --listen");
    }

    #[test]
    fn rejects_a_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let err = failure(command(
            "/tmp/rsc-validator-missing-key",
            dir.path().to_str().unwrap(),
            "devnet",
            "3",
        ));
        assert!(err.contains("No such file"), "{err}");
        assert!(!err.contains("11"));
    }

    #[test]
    fn rejects_a_short_key() {
        let file = write_key(&[0x11; 16]);
        let dir = tempfile::tempdir().unwrap();
        let err = failure(command(
            file.path().to_str().unwrap(),
            dir.path().to_str().unwrap(),
            "devnet",
            "3",
        ));
        assert_eq!(err, "invalid private key");
        assert!(!err.contains("11"));
    }

    #[test]
    fn a_key_outside_the_genesis_set_is_rejected() {
        let file = write_key(&[0x11; 32]);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        seed_genesis(path);
        let document = write_document(dir.path(), 0x22, 3, Hash([0x3C; 32]));
        let err = failure(command_with_genesis(
            file.path().to_str().unwrap(),
            path,
            "devnet",
            "3",
            &document,
        ));
        assert_eq!(err, "ineligible validator");
        assert!(!err.contains("11"));
    }

    #[test]
    fn a_zero_seed_and_a_foreign_chain_are_rejected() {
        let file = write_key(&[0x11; 32]);
        let key = file.path().to_str().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        seed_genesis(path);
        let zero = write_document(dir.path(), 0x11, 3, Hash::ZERO);
        let err = failure(command_with_genesis(key, path, "devnet", "3", &zero));
        assert_eq!(err, "invalid genesis parameters");
        let foreign = write_document(dir.path(), 0x11, 9, Hash([0x3C; 32]));
        let err = failure(command_with_genesis(key, path, "devnet", "3", &foreign));
        assert_eq!(err, "chain-id del genesis distinto");
    }

    #[test]
    fn prepare_vote_is_signed_by_the_real_key() {
        let file = write_key(&[0x11; 32]);
        let key_path = file.path().to_str().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        seed_genesis(path);
        let document = write_document(dir.path(), 0x11, 3, Hash([0x3C; 32]));
        let before = snapshot(path);
        let ready = bootstrap(command_with_genesis(key_path, path, "devnet", "3", &document)).unwrap();
        let parsed: rsc_genesis::Genesis =
            serde_json::from_str(&std::fs::read_to_string(&document).unwrap()).unwrap();
        let record = parsed
            .validator_set()
            .unwrap()
            .validators
            .into_iter()
            .find(|validator| validator.owner_key_id == ready.key_id)
            .unwrap();
        assert_eq!(record.owner_key_id, ready.key_id);
        assert_ne!(record.id, AccountId::ZERO);
        assert_eq!(ready.vote.validator, record.id);
        assert_eq!(ready.vote.validator, local_proposer());
        assert_eq!(ready.vote.phase, rsc_consensus::VotePhase::Prepare);
        assert_eq!(ready.vote.height, ready.engine.state().height);
        assert_eq!(ready.vote.epoch, ready.engine.state().epoch);
        assert_eq!(ready.vote.view, 0);
        assert_eq!(ready.vote.block_hash, before.0);
        let message = rsc_consensus::finality::vote_bytes(&ready.vote);
        assert_eq!(message.len(), 57);
        assert_eq!(message[0], rsc_consensus::VotePhase::Prepare as u8);
        assert_eq!(&message[1..9], &ready.vote.epoch.to_le_bytes());
        assert_eq!(&message[9..17], &ready.vote.height.to_le_bytes());
        assert_eq!(&message[17..25], &ready.vote.view.to_le_bytes());
        assert_eq!(&message[25..], &ready.vote.block_hash.0 .0);
        let real = PrivateKey::from_bytes(&[0x11; 32]).unwrap();
        let mut resigned = ready.vote.clone();
        resigned.signature = rsc_core::Signature([0; 64]);
        rsc_consensus::finality::sign_vote(&mut resigned, &real).unwrap();
        assert_eq!(resigned.signature, ready.vote.signature);
        rsc_consensus::verify_validator_vote(&record, &ready.vote).unwrap();
        let wrong = PrivateKey::from_bytes(&[0x44; 32]).unwrap();
        let mut other = ready.vote.clone();
        other.signature = rsc_core::Signature([0; 64]);
        rsc_consensus::finality::sign_vote(&mut other, &wrong).unwrap();
        let err = rsc_consensus::verify_validator_vote(&record, &other).unwrap_err();
        assert_eq!(err.to_string(), "invalid signature");
        let mut tampered = ready.vote.clone();
        tampered.signature.0[0] ^= 0xff;
        let err = rsc_consensus::verify_validator_vote(&record, &tampered).unwrap_err();
        assert_eq!(err.to_string(), "invalid signature");
        assert!(!err.to_string().contains("11"));
        assert_eq!(ready.engine.state().height, 0);
        assert_eq!(ready.engine.state().phase, rsc_consensus::ConsensusPhase::Decide);
        assert_eq!(ready.engine.state().locked, Some(before.0));
        let secret = "11".repeat(32);
        assert!(!format_key_id(&ready.key_id).contains(&secret));
        drop(ready);
        assert_eq!(snapshot(path), before);
        assert_eq!(tip_block(path).block_hash().unwrap(), before.0);
    }

    #[test]
    fn a_tip_proposed_by_another_account_is_rejected() {
        let file = write_key(&[0x11; 32]);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        seed_chain(path, AccountId::ZERO);
        let document = write_document(dir.path(), 0x11, 3, Hash([0x3C; 32]));
        let err = failure(command_with_genesis(
            file.path().to_str().unwrap(),
            path,
            "devnet",
            "3",
            &document,
        ));
        assert_eq!(err, "invalid proposer");
        assert!(!err.contains("11"));
    }

    #[test]
    fn prepare_quorum_stays_in_memory() {
        let file = write_key(&[0x11; 32]);
        let key_path = file.path().to_str().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        seed_genesis(path);
        let document = write_document(dir.path(), 0x11, 3, Hash([0x3C; 32]));
        let before = snapshot(path);
        let mut ready =
            bootstrap(command_with_genesis(key_path, path, "devnet", "3", &document)).unwrap();
        let committee = ready.engine.state().committee.clone();
        assert_eq!(ready.engine.state().phase, rsc_consensus::ConsensusPhase::Decide);
        assert_eq!(ready.engine.state().locked, Some(before.0));
        assert!(
            rsc_consensus::has_quorum(ready.qc.voting_power, committee.total_voting_power).unwrap()
        );
        rsc_consensus::verify_qc(&ready.qc, &committee).unwrap();
        let (again, equivocation) = rsc_consensus::finality::aggregate_qc(
            std::slice::from_ref(&ready.vote),
            &committee,
            rsc_consensus::VotePhase::Prepare,
        )
        .unwrap();
        assert!(equivocation.is_none());
        assert_eq!(again.unwrap(), ready.qc);
        assert_eq!(ready.qc.epoch, ready.vote.epoch);
        assert_eq!(ready.qc.phase, rsc_consensus::VotePhase::Prepare);
        assert_eq!(ready.qc.height, ready.vote.height);
        assert_eq!(ready.qc.view, ready.vote.view);
        assert_eq!(ready.qc.block_hash, ready.vote.block_hash);
        assert_eq!(ready.qc.voters, vec![(ready.vote.validator, ready.qc.voting_power)]);
        let err = ready.engine.receive_vote(ready.vote.clone()).unwrap_err();
        assert_eq!(err.to_string(), "invalid phase");
        assert_eq!(ready.engine.state().phase, rsc_consensus::ConsensusPhase::Decide);
        let mut other_phase = ready.vote.clone();
        other_phase.phase = rsc_consensus::VotePhase::PreCommit;
        let (none, equivocation) = rsc_consensus::finality::aggregate_qc(
            std::slice::from_ref(&other_phase),
            &committee,
            rsc_consensus::VotePhase::Prepare,
        )
        .unwrap();
        assert!(none.is_none());
        assert!(equivocation.is_none());
        assert_eq!(ready.engine.state().phase, rsc_consensus::ConsensusPhase::Decide);
        assert_eq!(ready.engine.state().locked, Some(before.0));
        assert_eq!(ready.engine.state().height, 0);
        rsc_consensus::verify_qc(&ready.qc, &ready.engine.state().committee).unwrap();
        drop(ready);
        assert_eq!(snapshot(path), before);
    }

    #[test]
    fn precommit_quorum_locks_the_tip() {
        let file = write_key(&[0x11; 32]);
        let key_path = file.path().to_str().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        seed_genesis(path);
        let document = write_document(dir.path(), 0x11, 3, Hash([0x3C; 32]));
        let before = snapshot(path);
        let mut ready =
            bootstrap(command_with_genesis(key_path, path, "devnet", "3", &document)).unwrap();
        let committee = ready.engine.state().committee.clone();
        assert_eq!(ready.qc.phase, rsc_consensus::VotePhase::Prepare);
        assert_eq!(ready.precommit.validator, ready.vote.validator);
        assert_ne!(ready.precommit.validator, AccountId::ZERO);
        assert_eq!(ready.precommit.epoch, ready.vote.epoch);
        assert_eq!(ready.precommit.height, ready.vote.height);
        assert_eq!(ready.precommit.view, ready.vote.view);
        assert_eq!(ready.precommit.block_hash, ready.vote.block_hash);
        assert_eq!(ready.precommit.phase, rsc_consensus::VotePhase::PreCommit);
        let real = PrivateKey::from_bytes(&[0x11; 32]).unwrap();
        let mut resigned = ready.precommit.clone();
        resigned.signature = rsc_core::Signature([0; 64]);
        rsc_consensus::finality::sign_vote(&mut resigned, &real).unwrap();
        assert_eq!(resigned.signature, ready.precommit.signature);
        assert_eq!(ready.engine.state().phase, rsc_consensus::ConsensusPhase::Decide);
        assert_eq!(ready.engine.state().locked, Some(ready.precommit.block_hash));
        assert!(rsc_consensus::has_quorum(
            ready.precommit_qc.voting_power,
            committee.total_voting_power
        )
        .unwrap());
        rsc_consensus::verify_qc(&ready.precommit_qc, &committee).unwrap();
        let (again, equivocation) = rsc_consensus::finality::aggregate_qc(
            std::slice::from_ref(&ready.precommit),
            &committee,
            rsc_consensus::VotePhase::PreCommit,
        )
        .unwrap();
        assert!(equivocation.is_none());
        assert_eq!(again.unwrap(), ready.precommit_qc);
        let err = ready.engine.receive_vote(ready.precommit.clone()).unwrap_err();
        assert_eq!(err.to_string(), "invalid phase");
        let mut commit = ready.precommit.clone();
        commit.phase = rsc_consensus::VotePhase::Commit;
        commit.signature = rsc_core::Signature([0; 64]);
        rsc_consensus::finality::sign_vote(&mut commit, &real).unwrap();
        let (none, equivocation) = rsc_consensus::finality::aggregate_qc(
            std::slice::from_ref(&commit),
            &committee,
            rsc_consensus::VotePhase::PreCommit,
        )
        .unwrap();
        assert!(none.is_none());
        assert!(equivocation.is_none());
        assert_eq!(ready.engine.state().phase, rsc_consensus::ConsensusPhase::Decide);
        assert_eq!(ready.engine.state().locked, Some(before.0));
        assert_eq!(ready.engine.state().height, 0);
        rsc_consensus::verify_qc(&ready.qc, &committee).unwrap();
        rsc_consensus::verify_qc(&ready.precommit_qc, &committee).unwrap();
        drop(ready);
        assert_eq!(snapshot(path), before);
        assert_eq!(tip_block(path).block_hash().unwrap(), before.0);
    }

    #[test]
    fn commit_finality_stays_in_memory() {
        let file = write_key(&[0x11; 32]);
        let key_path = file.path().to_str().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        seed_genesis(path);
        let document = write_document(dir.path(), 0x11, 3, Hash([0x3C; 32]));
        let before = snapshot(path);
        let mut ready =
            bootstrap(command_with_genesis(key_path, path, "devnet", "3", &document)).unwrap();
        let parsed: rsc_genesis::Genesis =
            serde_json::from_str(&std::fs::read_to_string(&document).unwrap()).unwrap();
        let record = parsed
            .validator_set()
            .unwrap()
            .validators
            .into_iter()
            .find(|validator| validator.owner_key_id == ready.key_id)
            .unwrap();
        let committee = ready.engine.state().committee.clone();
        assert_eq!(ready.precommit_qc.phase, rsc_consensus::VotePhase::PreCommit);
        assert_eq!(ready.commit.validator, record.id);
        assert_ne!(record.id, AccountId::ZERO);
        assert_eq!(ready.commit.phase, rsc_consensus::VotePhase::Commit);
        assert_eq!(ready.commit.epoch, ready.precommit.epoch);
        assert_eq!(ready.commit.height, ready.precommit.height);
        assert_eq!(ready.commit.view, ready.precommit.view);
        assert_eq!(ready.commit.block_hash, ready.precommit.block_hash);
        let real = PrivateKey::from_bytes(&[0x11; 32]).unwrap();
        let mut resigned = ready.commit.clone();
        resigned.signature = rsc_core::Signature([0; 64]);
        rsc_consensus::finality::sign_vote(&mut resigned, &real).unwrap();
        assert_eq!(resigned.signature, ready.commit.signature);
        rsc_consensus::verify_validator_vote(&record, &ready.commit).unwrap();
        assert_eq!(ready.engine.state().phase, rsc_consensus::ConsensusPhase::Decide);
        assert_eq!(ready.engine.state().locked, Some(ready.commit.block_hash));
        assert_eq!(ready.engine.state().height, ready.commit.height);
        assert!(rsc_consensus::has_quorum(
            ready.commit_qc.voting_power,
            committee.total_voting_power
        )
        .unwrap());
        rsc_consensus::verify_qc(&ready.commit_qc, &committee).unwrap();
        let (again, equivocation) = rsc_consensus::finality::aggregate_qc(
            std::slice::from_ref(&ready.commit),
            &committee,
            rsc_consensus::VotePhase::Commit,
        )
        .unwrap();
        assert!(equivocation.is_none());
        assert_eq!(again.unwrap(), ready.commit_qc);
        rsc_consensus::verify_commit_chain(&ready.proof, &committee).unwrap();
        assert_eq!(ready.proof.prepare, ready.qc);
        assert_eq!(ready.proof.precommit, ready.precommit_qc);
        assert_eq!(ready.proof.commit, ready.commit_qc);
        let mut altered = ready.proof.clone();
        altered.block_hash = BlockHash(Hash([0xAB; 32]));
        let err = rsc_consensus::verify_commit_chain(&altered, &committee).unwrap_err();
        assert_eq!(err.to_string(), "invalid finality");
        let mut tampered = ready.commit.clone();
        tampered.signature.0[0] ^= 0xff;
        let err = rsc_consensus::verify_validator_vote(&record, &tampered).unwrap_err();
        assert_eq!(err.to_string(), "invalid signature");
        let mut outsider = ready.commit.clone();
        outsider.validator = AccountId::ZERO;
        let err = rsc_consensus::verify_validator_vote(&record, &outsider).unwrap_err();
        assert_eq!(err.to_string(), "invalid signature");
        let err = ready.engine.receive_vote(ready.commit.clone()).unwrap_err();
        assert_eq!(err.to_string(), "invalid phase");
        for (epoch, height, view, hash) in [
            (9, ready.commit.height, ready.commit.view, ready.commit.block_hash),
            (ready.commit.epoch, 4, ready.commit.view, ready.commit.block_hash),
            (ready.commit.epoch, ready.commit.height, 2, ready.commit.block_hash),
            (ready.commit.epoch, ready.commit.height, ready.commit.view, BlockHash(Hash([0xEE; 32]))),
        ] {
            let mut shifted = ready.commit.clone();
            shifted.epoch = epoch;
            shifted.height = height;
            shifted.view = view;
            shifted.block_hash = hash;
            shifted.signature = rsc_core::Signature([0; 64]);
            rsc_consensus::finality::sign_vote(&mut shifted, &real).unwrap();
            let err = ready.engine.receive_vote(shifted.clone()).unwrap_err();
            assert_eq!(err.to_string(), "invalid phase");
            let (other, equivocation) = rsc_consensus::finality::aggregate_qc(
                std::slice::from_ref(&shifted),
                &committee,
                rsc_consensus::VotePhase::Commit,
            )
            .unwrap();
            assert!(equivocation.is_none());
            let mut broken = ready.proof.clone();
            broken.commit = other.unwrap();
            let err = rsc_consensus::verify_commit_chain(&broken, &committee).unwrap_err();
            assert_eq!(err.to_string(), "invalid finality");
        }
        let (none, equivocation) = rsc_consensus::finality::aggregate_qc(
            std::slice::from_ref(&ready.precommit),
            &committee,
            rsc_consensus::VotePhase::Commit,
        )
        .unwrap();
        assert!(none.is_none());
        assert!(equivocation.is_none());
        assert_eq!(ready.engine.state().phase, rsc_consensus::ConsensusPhase::Decide);
        assert_eq!(ready.engine.state().locked, Some(before.0));
        assert_eq!(ready.engine.state().height, 0);
        drop(ready);
        assert_eq!(snapshot(path), before);
        assert_eq!(tip_block(path).block_hash().unwrap(), before.0);
    }

    #[test]
    fn finality_survives_reopen() {
        let file = write_key(&[0x11; 32]);
        let key_path = file.path().to_str().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        seed_genesis(path);
        let document = write_document(dir.path(), 0x11, 3, Hash([0x3C; 32]));
        let before = snapshot(path);
        let command = command_with_genesis(key_path, path, "devnet", "3", &document);
        let first = bootstrap(command.clone()).unwrap();
        let proof = first.proof.clone();
        let height = first.engine.state().height;
        let locked = first.engine.state().locked;
        assert_eq!(first.engine.state().phase, rsc_consensus::ConsensusPhase::Decide);
        drop(first);
        let stored = stored_finality(path);
        let again = bootstrap(command).unwrap();
        assert_eq!(again.proof, proof);
        let committee = again.engine.state().committee.clone();
        rsc_consensus::verify_commit_chain(&again.proof, &committee).unwrap();
        assert_eq!(again.proof.block_hash, proof.block_hash);
        assert_eq!(again.proof.prepare.epoch, proof.prepare.epoch);
        assert_eq!(again.proof.height, proof.height);
        assert_eq!(again.proof.view, proof.view);
        assert_eq!(again.proof.prepare, proof.prepare);
        assert_eq!(again.proof.precommit, proof.precommit);
        assert_eq!(again.proof.commit, proof.commit);
        assert_eq!(again.engine.state().phase, rsc_consensus::ConsensusPhase::Decide);
        assert_eq!(again.engine.state().height, height);
        assert_eq!(again.engine.state().locked, locked);
        let mut other_hash = again.proof.clone();
        other_hash.block_hash = BlockHash(Hash([0xAB; 32]));
        let err = rsc_consensus::verify_commit_chain(&other_hash, &committee).unwrap_err();
        assert_eq!(err.to_string(), "invalid finality");
        let mut other_proof = again.proof.clone();
        other_proof.height = proof.height.wrapping_add(1);
        let err = rsc_consensus::verify_commit_chain(&other_proof, &committee).unwrap_err();
        assert_eq!(err.to_string(), "invalid finality");
        let mut bad_sig = again.proof.clone();
        let mut signatures = bad_sig.commit.signatures.as_slice().to_vec();
        let mut raw = signatures[0].to_bytes();
        raw[0] ^= 0xff;
        signatures[0] = rsc_crypto::Signature::from_bytes(&raw).unwrap();
        bad_sig.commit.signatures = rsc_crypto::SignatureBundle::from_signatures(&signatures).unwrap();
        let err = rsc_consensus::verify_commit_chain(&bad_sig, &committee).unwrap_err();
        assert_eq!(err.to_string(), "invalid signature");
        drop(again);
        assert_eq!(stored_finality(path), stored);
        assert_eq!(snapshot(path), before);
        assert_eq!(tip_block(path).header.height.0, height);
        assert_eq!(tip_block(path).block_hash().unwrap(), before.0);
    }

    #[test]
    fn get_finality_reads_persisted_proof() {
        let file = write_key(&[0x11; 32]);
        let key_path = file.path().to_str().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        seed_genesis(path);
        let document = write_document(dir.path(), 0x11, 3, Hash([0x3C; 32]));
        let command = command_with_genesis(key_path, path, "devnet", "3", &document);
        let first = bootstrap(command.clone()).unwrap();
        let proof = first.proof.clone();
        let committee = first.engine.state().committee.clone();
        let tip = first.store.get_canonical_tip().unwrap().unwrap();
        let root = first.state.state_root().unwrap();
        let ledger = first.ledger.root();
        let height = first.engine.state().height;
        let phase = first.engine.state().phase;
        let locked = first.engine.state().locked;
        let seen = ask_finality(&first.store, &committee).unwrap();
        assert_eq!(seen, serde_json::to_value(&first.rpc).unwrap());
        let served = proof_from_rpc(&seen);
        rsc_consensus::verify_commit_chain(&served, &committee).unwrap();
        assert_eq!(served, proof);
        assert_eq!(seen["epoch"], serde_json::json!(proof.prepare.epoch));
        assert_eq!(first.store.get_canonical_tip().unwrap().unwrap(), tip);
        assert_eq!(first.state.state_root().unwrap(), root);
        assert_eq!(first.ledger.root(), ledger);
        assert_eq!(first.engine.state().height, height);
        assert_eq!(first.engine.state().phase, phase);
        assert_eq!(first.engine.state().locked, locked);
        assert!(first
            .store
            .get_block_by_height(BlockHeight(height + 1))
            .unwrap()
            .is_none());
        drop(first);

        let again = bootstrap(command).unwrap();
        let again_committee = again.engine.state().committee.clone();
        let again_seen = ask_finality(&again.store, &again_committee).unwrap();
        assert_eq!(again_seen, seen);
        let again_served = proof_from_rpc(&again_seen);
        rsc_consensus::verify_commit_chain(&again_served, &again_committee).unwrap();
        assert_eq!(again_served, proof);
        assert_eq!(again.engine.state().height, height);
        assert_eq!(again.engine.state().phase, rsc_consensus::ConsensusPhase::Decide);
        assert_eq!(again.engine.state().locked, locked);

        put_finality(&again.store, b"broken");
        let err = read_finality(&again.store, &again_committee).unwrap_err();
        assert_eq!(err, "invalid finality");
        assert!(ask_finality(&again.store, &again_committee).is_err());

        let mut bad_sig = proof.clone();
        let mut signatures = bad_sig.commit.signatures.as_slice().to_vec();
        let mut raw = signatures[0].to_bytes();
        raw[0] ^= 0xff;
        signatures[0] = rsc_crypto::Signature::from_bytes(&raw).unwrap();
        bad_sig.commit.signatures =
            rsc_crypto::SignatureBundle::from_signatures(&signatures).unwrap();
        put_finality(&again.store, &super::encode_proof(&bad_sig).unwrap());
        let err = read_finality(&again.store, &again_committee).unwrap_err();
        assert_eq!(err, "invalid signature");
        assert!(ask_finality(&again.store, &again_committee).is_err());

        let key = PrivateKey::from_bytes(&[0x11; 32]).unwrap();
        let mut shifted = again.commit.clone();
        shifted.block_hash = BlockHash(Hash([0xAB; 32]));
        shifted.signature = rsc_core::Signature([0; 64]);
        rsc_consensus::finality::sign_vote(&mut shifted, &key).unwrap();
        let (other, equivocation) = rsc_consensus::finality::aggregate_qc(
            std::slice::from_ref(&shifted),
            &again_committee,
            rsc_consensus::VotePhase::Commit,
        )
        .unwrap();
        assert!(equivocation.is_none());
        let mut other_block = proof.clone();
        other_block.commit = other.unwrap();
        put_finality(&again.store, &super::encode_proof(&other_block).unwrap());
        let err = read_finality(&again.store, &again_committee).unwrap_err();
        assert_eq!(err, "invalid finality");
        assert!(ask_finality(&again.store, &again_committee).is_err());

        again
            .store
            .database()
            .delete(rsc_storage::ColumnFamily::Default, super::FINALITY_KEY)
            .unwrap();
        let err = read_finality(&again.store, &again_committee).unwrap_err();
        assert_eq!(err, RpcError::ServiceUnavailable.public_message());
        assert_eq!(
            ask_finality(&again.store, &again_committee).unwrap_err(),
            RpcError::ServiceUnavailable
        );
        assert_eq!(again.store.get_canonical_tip().unwrap().unwrap(), tip);
        assert_eq!(again.state.state_root().unwrap(), root);
        assert_eq!(again.ledger.root(), ledger);
        assert_eq!(again.engine.state().height, height);
        assert_eq!(again.engine.state().phase, phase);
        assert_eq!(again.engine.state().locked, locked);
        assert!(again
            .store
            .get_block_by_height(BlockHeight(height + 1))
            .unwrap()
            .is_none());
    }

    fn put_finality(store: &RocksDbChainStore, bytes: &[u8]) {
        store
            .database()
            .put(rsc_storage::ColumnFamily::Default, super::FINALITY_KEY, bytes)
            .unwrap();
    }

    fn proof_from_rpc(value: &serde_json::Value) -> rsc_consensus::FinalityProof {
        let prepare = qc_from_hex(value["prepare"].as_str().unwrap());
        let precommit = qc_from_hex(value["precommit"].as_str().unwrap());
        let commit = qc_from_hex(value["commit"].as_str().unwrap());
        rsc_consensus::FinalityProof {
            block_hash: BlockHash(Hash(
                rsc_rpc::decode_hex(value["finalized_hash"].as_str().unwrap())
                    .unwrap()
                    .try_into()
                    .unwrap(),
            )),
            height: value["finalized_height"].as_u64().unwrap(),
            view: value["view"].as_u64().unwrap(),
            prepare,
            precommit,
            commit,
        }
    }

    fn qc_from_hex(hex: &str) -> rsc_consensus::QuorumCertificate {
        let bytes = rsc_rpc::decode_hex(hex).unwrap();
        let mut cursor = bytes.as_slice();
        let qc = super::decode_qc(&mut cursor).unwrap();
        assert!(cursor.is_empty());
        qc
    }

    fn ask_finality(
        store: &RocksDbChainStore,
        committee: &Committee,
    ) -> Result<serde_json::Value, RpcError> {
        let source = std::sync::Arc::new(FinalitySource {
            store: RocksDbChainStore::from_shared(store.database()),
            committee: committee.clone(),
        });
        let ctx = RpcContext::new(
            std::sync::Arc::clone(&source) as std::sync::Arc<dyn NodeService>,
            std::sync::Arc::clone(&source) as std::sync::Arc<dyn ChainService>,
            std::sync::Arc::clone(&source) as std::sync::Arc<dyn TransactionService>,
            std::sync::Arc::clone(&source) as std::sync::Arc<dyn StateService>,
            source,
        );
        wait(dispatch(&ctx, "rsc_getFinality", None))
    }

    fn wait<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(future)
    }

    struct FinalitySource {
        store: RocksDbChainStore,
        committee: Committee,
    }

    fn unavailable<'a, T: Send + 'a>() -> ServiceFut<'a, T> {
        Box::pin(async { Err(ServiceError::Unavailable) })
    }

    impl NodeService for FinalitySource {
        fn chain_id(&self) -> ServiceFut<'_, String> {
            unavailable()
        }
        fn node_info(&self) -> ServiceFut<'_, RpcNodeInfo> {
            unavailable()
        }
        fn status(&self) -> ServiceFut<'_, RpcStatus> {
            unavailable()
        }
    }

    impl ChainService for FinalitySource {
        fn block_by_hash(&self, _: &str) -> ServiceFut<'_, RpcBlock> {
            unavailable()
        }
        fn block_by_height(&self, _: u64) -> ServiceFut<'_, RpcBlock> {
            unavailable()
        }
        fn transaction(&self, _: &str) -> ServiceFut<'_, RpcTransaction> {
            unavailable()
        }
    }

    impl TransactionService for FinalitySource {
        fn submit_signed(&self, _: &[u8]) -> ServiceFut<'_, RpcSubmitResult> {
            unavailable()
        }
    }

    impl StateService for FinalitySource {
        fn account(&self, _: &str) -> ServiceFut<'_, RpcAccount> {
            unavailable()
        }
        fn balance(&self, _: &str) -> ServiceFut<'_, RpcBalance> {
            unavailable()
        }
    }

    impl ConsensusService for FinalitySource {
        fn validators(&self) -> ServiceFut<'_, Vec<RpcValidator>> {
            unavailable()
        }
        fn finality(&self) -> ServiceFut<'_, rsc_rpc::RpcFinality> {
            let result = read_finality(&self.store, &self.committee).map_err(|err| {
                if err == RpcError::ServiceUnavailable.public_message() {
                    ServiceError::ConsensusUnavailable
                } else {
                    ServiceError::Internal
                }
            });
            Box::pin(async move { result })
        }
    }

    #[test]
    fn validator_process_serves_finality_over_http() {
        let file = write_key(&[0x11; 32]);
        let key_path = file.path().to_str().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        seed_genesis(path);
        let document = write_document(dir.path(), 0x11, 3, Hash([0x3C; 32]));
        let command = command_with_genesis(key_path, path, "devnet", "3", &document);

        let mut process = spawn_validator(&command);
        let printed = read_key_line(&mut process);
        assert_eq!(printed.trim().len(), 64);
        wait_for_rpc();
        let first = post_finality();
        assert!(first.get("result").is_some(), "{first}");
        assert!(first.get("error").is_none(), "{first}");
        stop_validator(&mut process);
        let after_first = chain_view(path);

        let mut process = spawn_validator(&command);
        let _printed = read_key_line(&mut process);
        wait_for_rpc();
        let second = post_finality();
        assert_eq!(second["result"], first["result"]);
        stop_validator(&mut process);
        let after_second = chain_view(path);
        assert_eq!(after_second.tip, after_first.tip);
        assert_eq!(after_second.block, after_first.block);
        assert_eq!(after_second.height, after_first.height);
        assert_eq!(after_second.root, after_first.root);
        assert_eq!(after_second.ledger, after_first.ledger);
        assert_eq!(after_second.finality, after_first.finality);
        assert_eq!(after_second.commits, after_first.commits);
        assert!(after_second.next.is_none());

        let ready = bootstrap(command.clone()).unwrap();
        let served = proof_from_rpc(&first["result"]);
        rsc_consensus::verify_commit_chain(&served, &ready.engine.state().committee).unwrap();
        assert_eq!(served, ready.proof);
        assert_eq!(
            first["result"]["epoch"],
            serde_json::json!(ready.proof.prepare.epoch)
        );
        assert_eq!(
            first["result"]["finalized_height"],
            serde_json::json!(ready.proof.height)
        );
        assert_eq!(first["result"]["view"], serde_json::json!(ready.proof.view));
        assert_eq!(first["result"]["finalized_hash"], second["result"]["finalized_hash"]);
        assert_eq!(first["result"]["prepare"], second["result"]["prepare"]);
        assert_eq!(first["result"]["precommit"], second["result"]["precommit"]);
        assert_eq!(first["result"]["commit"], second["result"]["commit"]);
        assert_eq!(
            ready.engine.state().phase,
            rsc_consensus::ConsensusPhase::Decide
        );
        assert_eq!(ready.engine.state().height, ready.proof.height);
        assert_eq!(ready.store.get_canonical_tip().unwrap().unwrap(), after_second.tip);
        assert_eq!(ready.state.state_root().unwrap(), after_second.root);
        assert_eq!(ready.ledger.root(), after_second.ledger);
        let committee = ready.engine.state().committee.clone();
        let commit = ready.commit.clone();
        let proof = ready.proof.clone();
        let db = ready.store.database();
        listen(super::validator_context(ready), || {
            db.delete(rsc_storage::ColumnFamily::Default, super::FINALITY_KEY)
                .unwrap();
            let missing = post_finality();
            assert!(missing.get("result").is_none(), "{missing}");
            assert_eq!(
                missing["error"]["code"],
                serde_json::json!(rsc_rpc::RpcError::ServiceUnavailable.code())
            );
            assert_eq!(
                missing["error"]["message"],
                serde_json::json!(rsc_rpc::RpcError::ServiceUnavailable.public_message())
            );

            let store = RocksDbChainStore::from_shared(Arc::clone(&db));
            put_finality(&store, b"broken");
            let corrupt = post_finality();
            assert!(corrupt.get("result").is_none(), "{corrupt}");
            assert!(corrupt.get("error").is_some(), "{corrupt}");
            assert_eq!(
                read_finality(&store, &committee).unwrap_err(),
                "invalid finality"
            );

            let mut bad_sig = proof.clone();
            let mut signatures = bad_sig.commit.signatures.as_slice().to_vec();
            let mut raw = signatures[0].to_bytes();
            raw[0] ^= 0xff;
            signatures[0] = rsc_crypto::Signature::from_bytes(&raw).unwrap();
            bad_sig.commit.signatures =
                rsc_crypto::SignatureBundle::from_signatures(&signatures).unwrap();
            put_finality(&store, &super::encode_proof(&bad_sig).unwrap());
            let signed = post_finality();
            assert!(signed.get("result").is_none(), "{signed}");
            assert_eq!(
                read_finality(&store, &committee).unwrap_err(),
                "invalid signature"
            );

            let key = PrivateKey::from_bytes(&[0x11; 32]).unwrap();
            let mut shifted = commit.clone();
            shifted.block_hash = BlockHash(Hash([0xAB; 32]));
            shifted.signature = rsc_core::Signature([0; 64]);
            rsc_consensus::finality::sign_vote(&mut shifted, &key).unwrap();
            let (other, equivocation) = rsc_consensus::finality::aggregate_qc(
                std::slice::from_ref(&shifted),
                &committee,
                rsc_consensus::VotePhase::Commit,
            )
            .unwrap();
            assert!(equivocation.is_none());
            let mut other_block = proof.clone();
            other_block.commit = other.unwrap();
            let other_bytes = super::encode_proof(&other_block).unwrap();
            put_finality(&store, &other_bytes);
            let mismatched = post_finality();
            assert!(mismatched.get("result").is_none(), "{mismatched}");
            assert_eq!(
                read_finality(&store, &committee).unwrap_err(),
                "invalid finality"
            );
        });
        drop(db);
        let after_reject = chain_view(path);
        assert_eq!(after_reject.tip, after_first.tip);
        assert_eq!(after_reject.height, after_first.height);
        assert_eq!(after_reject.root, after_first.root);
        assert_eq!(after_reject.ledger, after_first.ledger);
        assert!(after_reject.next.is_none());

        put_finality_at(path, b"broken");
        let (status, err) = refuse_validator(&command);
        assert_eq!(status.code(), Some(1));
        assert!(err.contains("invalid finality"), "{err}");

        put_finality_at(path, &super::encode_proof(&bad_signature(&proof)).unwrap());
        let (status, err) = refuse_validator(&command);
        assert_eq!(status.code(), Some(1));
        assert!(err.contains("invalid signature"), "{err}");

        put_finality_at(path, &other_commit_proof(&proof, &commit, &committee));
        let (status, err) = refuse_validator(&command);
        assert_eq!(status.code(), Some(1));
        assert!(err.contains("invalid finality"), "{err}");
        let after_refuse = chain_view(path);
        assert_eq!(after_refuse.tip, after_first.tip);
        assert_eq!(after_refuse.height, after_first.height);
        assert!(after_refuse.next.is_none());
    }

    fn bad_signature(proof: &rsc_consensus::FinalityProof) -> rsc_consensus::FinalityProof {
        let mut bad_sig = proof.clone();
        let mut signatures = bad_sig.commit.signatures.as_slice().to_vec();
        let mut raw = signatures[0].to_bytes();
        raw[0] ^= 0xff;
        signatures[0] = rsc_crypto::Signature::from_bytes(&raw).unwrap();
        bad_sig.commit.signatures =
            rsc_crypto::SignatureBundle::from_signatures(&signatures).unwrap();
        bad_sig
    }

    fn other_commit_proof(
        proof: &rsc_consensus::FinalityProof,
        commit: &rsc_consensus::Vote,
        committee: &Committee,
    ) -> Vec<u8> {
        let key = PrivateKey::from_bytes(&[0x11; 32]).unwrap();
        let mut shifted = commit.clone();
        shifted.block_hash = BlockHash(Hash([0xAB; 32]));
        shifted.signature = rsc_core::Signature([0; 64]);
        rsc_consensus::finality::sign_vote(&mut shifted, &key).unwrap();
        let (other, equivocation) = rsc_consensus::finality::aggregate_qc(
            std::slice::from_ref(&shifted),
            committee,
            rsc_consensus::VotePhase::Commit,
        )
        .unwrap();
        assert!(equivocation.is_none());
        let mut other_block = proof.clone();
        other_block.commit = other.unwrap();
        super::encode_proof(&other_block).unwrap()
    }

    fn put_finality_at(path: &str, bytes: &[u8]) {
        let db = Arc::new(open_database(&config_at(path, "devnet", 3)).unwrap());
        let (_state, store) = share_database(db).unwrap();
        put_finality(&store, bytes);
    }

    struct ChainView {
        tip: BlockHash,
        block: Vec<u8>,
        height: u64,
        root: rsc_state::StateRoot,
        ledger: rsc_ledger::LedgerRoot,
        finality: Vec<u8>,
        commits: u64,
        next: Option<BlockHash>,
    }

    fn chain_view(path: &str) -> ChainView {
        let db = Arc::new(open_database(&config_at(path, "devnet", 3)).unwrap());
        let commits = db.commit_count();
        let (state, store) = share_database(db).unwrap();
        let tip = store.get_canonical_tip().unwrap().unwrap();
        let block = store.get_block(&tip).unwrap().unwrap();
        let height = block.header.height.0;
        let finality = store
            .database()
            .get(rsc_storage::ColumnFamily::Default, super::FINALITY_KEY)
            .unwrap()
            .unwrap()
            .as_bytes()
            .to_vec();
        let ledger = super::rebuild_ledger(&store).unwrap().root();
        let next = store
            .get_block_by_height(BlockHeight(height + 1))
            .unwrap();
        ChainView {
            tip,
            block: encode_block(&block).unwrap(),
            height,
            root: state.state_root().unwrap(),
            ledger,
            finality,
            commits,
            next,
        }
    }

    struct Running {
        child: Option<std::process::Child>,
        stdout: Option<std::thread::JoinHandle<String>>,
    }

    impl Drop for Running {
        fn drop(&mut self) {
            if let Some(mut child) = self.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    fn spawn_validator(args: &[String]) -> Running {
        let mut child = std::process::Command::new(validator_bin())
            .args(args)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let stdout = std::thread::spawn(move || {
            let mut line = String::new();
            let _ = std::io::BufRead::read_line(&mut std::io::BufReader::new(stdout), &mut line);
            line
        });
        Running {
            child: Some(child),
            stdout: Some(stdout),
        }
    }

    fn validator_bin() -> std::path::PathBuf {
        if let Ok(path) = std::env::var("CARGO_BIN_EXE_rsc-validator") {
            return path.into();
        }
        let mut path = std::env::current_exe().unwrap();
        path.pop();
        path.pop();
        path.push("rsc-validator");
        path
    }

    fn read_key_line(running: &mut Running) -> String {
        let handle = running.stdout.take().unwrap();
        let started = std::time::Instant::now();
        loop {
            if handle.is_finished() {
                return handle.join().unwrap();
            }
            if let Some(status) = running.child.as_mut().unwrap().try_wait().unwrap() {
                let err = read_stderr(running.child.as_mut().unwrap());
                panic!("el proceso salió {status}: {err}");
            }
            if started.elapsed() > std::time::Duration::from_secs(60) {
                panic!("el proceso no imprimió el key id");
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    fn stop_validator(running: &mut Running) {
        let mut child = running.child.take().unwrap();
        let _ = std::process::Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status();
        let started = std::time::Instant::now();
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                let err = read_stderr(&mut child);
                assert!(status.success(), "{err}");
                return;
            }
            if started.elapsed() > std::time::Duration::from_secs(10) {
                let _ = child.kill();
                panic!("el proceso no cerró con TERM");
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    fn refuse_validator(args: &[String]) -> (std::process::ExitStatus, String) {
        let mut running = spawn_validator(args);
        let mut child = running.child.take().unwrap();
        let status = child.wait().unwrap();
        let err = read_stderr(&mut child);
        (status, err)
    }

    fn read_stderr(child: &mut std::process::Child) -> String {
        let mut err = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            std::io::Read::read_to_string(&mut stderr, &mut err).unwrap();
        }
        err
    }

    fn wait_for_rpc() {
        let bind = rsc_rpc::RpcConfig::default().bind_address;
        let started = std::time::Instant::now();
        loop {
            if std::net::TcpStream::connect(bind).is_ok() {
                return;
            }
            if started.elapsed() > std::time::Duration::from_secs(10) {
                panic!("rpc no escuchó en {bind}");
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    fn post_finality() -> serde_json::Value {
        let bind = rsc_rpc::RpcConfig::default().bind_address;
        let body = r#"{"jsonrpc":"2.0","method":"rsc_getFinality","id":1}"#;
        let mut stream = std::net::TcpStream::connect(bind).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let request = format!(
            "POST /rpc HTTP/1.1\r\nHost: {bind}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        std::io::Write::write_all(&mut stream, request.as_bytes()).unwrap();
        let mut raw = Vec::new();
        let _ = std::io::Read::read_to_end(&mut stream, &mut raw);
        let raw = String::from_utf8(raw).unwrap();
        let json = raw.split("\r\n\r\n").nth(1).unwrap_or(raw.as_str());
        serde_json::from_str(json.trim()).unwrap()
    }

    fn listen(ctx: RpcContext, during: impl FnOnce()) {
        let server = rsc_rpc::RpcServer::new(ctx, rsc_rpc::RpcConfig::default());
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let task = runtime.spawn(async move {
            let _ = server.run().await;
        });
        wait_for_rpc();
        during();
        task.abort();
        let _ = runtime.block_on(task);
    }

    struct FourValidators {
        _dir: tempfile::TempDir,
        genesis: String,
        state: rsc_state::State,
        store: RocksDbChainStore,
        tip: BlockHash,
        root: rsc_state::StateRoot,
        ledger_root: rsc_ledger::LedgerRoot,
        commits: u64,
        seats: Vec<ValidatorSeat>,
        keys: Vec<PrivateKey>,
    }

    struct ValidatorSeat {
        engine: ConsensusEngine,
        record: ValidatorRecord,
    }

    struct SignedRound {
        prepare: Vote,
        precommit: Vote,
        commit: Vote,
    }

    fn write_four_genesis(dir: &std::path::Path, powers: [u128; 4]) -> (String, Vec<PrivateKey>) {
        let mut keys = Vec::new();
        let mut accounts = Vec::new();
        let mut validators = Vec::new();
        for (seed, power) in [0x11_u8, 0x22, 0x33, 0x44].into_iter().zip(powers) {
            let secret = PrivateKey::from_bytes(&[seed; 32]).unwrap();
            let owner_public_key = secret.public_key();
            let owner_key_id = owner_public_key.key_id();
            let account_id = AccountId::from(rsc_state::derive_account_id(&owner_key_id));
            accounts.push(rsc_genesis::GenesisAccount {
                account_id,
                owner_key_id,
                native_balance: 500,
                allocation: rsc_genesis::GenesisAllocationCategory::Community,
            });
            validators.push(rsc_genesis::GenesisValidator {
                account_id,
                owner_key_id,
                owner_public_key,
                voting_power: power,
                eligible: true,
            });
            keys.push(secret);
        }
        let document = rsc_genesis::Genesis::new(
            GENESIS_VERSION_V1,
            ChainId(3),
            1_700_000_000,
            rsc_genesis::NetworkParameters {
                amount_scale: 1_000_000,
                max_block_size: 1_048_576,
                max_transactions_per_block: 1_000,
                max_transaction_bytes: 128 * 1024,
                minimum_fee: 0,
                committee_size: 4,
                epoch_length: 20,
                min_validator_weight: 1,
                max_validator_weight: 30,
                min_validators_for_distributed: 5,
                consensus_randomness_seed: Hash([0x3C; 32]),
            },
            accounts,
            validators,
        );
        let path = dir.join("genesis.json");
        std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
        (path.to_str().unwrap().to_string(), keys)
    }

    fn proposer_at_genesis(path: &str) -> AccountId {
        let text = std::fs::read_to_string(path).unwrap();
        let genesis: rsc_genesis::Genesis = serde_json::from_str(&text).unwrap();
        genesis.validate().unwrap();
        let config = genesis.parameters.consensus_config().unwrap();
        let validators = genesis.validator_set().unwrap();
        let epoch = rsc_consensus::committee::epoch_of(0, config.epoch_length).unwrap();
        let beacon = rsc_consensus::consensus_beacon(
            &genesis.parameters.consensus_randomness_seed,
            epoch,
            0,
            &Hash::ZERO,
        );
        let local = validators.validators[0].id;
        ConsensusEngine::new(config, validators, beacon, local, 0)
            .unwrap()
            .proposer()
            .unwrap()
    }

    fn config_with_genesis(path: &str, genesis: &str) -> super::Config {
        let mut cfg = config_at(path, "devnet", 3);
        cfg.genesis = genesis.to_string();
        cfg
    }

    fn open_four() -> FourValidators {
        open_with_powers([1, 1, 1, 1])
    }

    fn open_with_powers(powers: [u128; 4]) -> FourValidators {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap().to_string();
        let (genesis, keys) = write_four_genesis(dir.path(), powers);
        seed_chain(&path, proposer_at_genesis(&genesis));
        let cfg = config_with_genesis(&path, &genesis);
        let db = Arc::new(open_database(&cfg).unwrap());
        let (state, store) = share_database(db).unwrap();
        let ledger_root = rebuild_ledger(&store).unwrap().root();
        let tip = store.get_canonical_tip().unwrap().unwrap();
        let root = state.state_root().unwrap();
        let commits = store.database().commit_count();
        let block = store.get_block(&tip).unwrap().unwrap();
        let mut seats = Vec::new();
        for key in &keys {
            let (mut engine, record) = open_engine(&cfg, &store, &key.key_id()).unwrap();
            assert_eq!(
                engine.receive_proposal(&block, None).unwrap(),
                ConsensusEvent::ProposalAccepted
            );
            seats.push(ValidatorSeat { engine, record });
        }
        FourValidators {
            _dir: dir,
            genesis,
            state,
            store,
            tip,
            root,
            ledger_root,
            commits,
            seats,
            keys,
        }
    }

    fn assert_committee(opened: &FourValidators) {
        let text = std::fs::read_to_string(&opened.genesis).unwrap();
        let genesis: rsc_genesis::Genesis = serde_json::from_str(&text).unwrap();
        let set = genesis.validator_set().unwrap();
        assert_eq!(set.validators.len(), 4);
        assert!(rsc_consensus::has_quorum(3, 4).unwrap());
        assert!(!rsc_consensus::has_quorum(2, 4).unwrap());
        let committee = &opened.seats[0].engine.state().committee;
        assert_eq!(committee.members.len(), 4);
        assert_eq!(committee.total_voting_power, 4);
        let mut ids = Vec::new();
        for (seat, key) in opened.seats.iter().zip(&opened.keys) {
            assert_eq!(seat.record.owner_key_id, key.key_id());
            assert_eq!(seat.record.owner_public_key.key_id(), key.key_id());
            assert_eq!(seat.record.owner_public_key, key.public_key());
            assert_eq!(
                seat.record.id,
                AccountId::from(rsc_state::derive_account_id(&seat.record.owner_key_id))
            );
            assert!(seat.record.eligible);
            assert_eq!(seat.record.voting_power, 1);
            assert_ne!(seat.record.id, AccountId::ZERO);
            validate_validator_record(&seat.record).unwrap();
            assert!(set.validators.iter().any(|validator| {
                validator.id == seat.record.id && validator.owner_key_id == seat.record.owner_key_id
            }));
            assert!(committee.members.iter().any(|member| {
                member.validator_id == seat.record.id
                    && member.voting_power == 1
                    && member.public_key == seat.record.owner_public_key
            }));
            ids.push(seat.record.id);
        }
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 4);
    }

    fn assert_same_context(opened: &FourValidators) {
        let anchor = opened.seats[0].engine.state();
        for seat in &opened.seats {
            let state = seat.engine.state();
            assert_eq!(state.epoch, anchor.epoch);
            assert_eq!(state.height, anchor.height);
            assert_eq!(state.height, 0);
            assert_eq!(state.view, anchor.view);
            assert_eq!(state.view, 0);
            assert_eq!(state.proposal, Some(opened.tip));
            assert_eq!(state.phase, ConsensusPhase::Propose);
            assert!(state.locked.is_none());
            assert_eq!(state.committee.total_voting_power, 4);
        }
    }

    fn sign_phases(seats: &mut [ValidatorSeat], keys: &[PrivateKey]) -> Vec<SignedRound> {
        let mut rounds = Vec::new();
        for (seat, key) in seats.iter_mut().zip(keys) {
            let prepare = seat.engine.create_vote(VotePhase::Prepare, key).unwrap();
            rsc_consensus::verify_validator_vote(&seat.record, &prepare).unwrap();
            let precommit = seat.engine.create_vote(VotePhase::PreCommit, key).unwrap();
            rsc_consensus::verify_validator_vote(&seat.record, &precommit).unwrap();
            let commit = seat.engine.create_vote(VotePhase::Commit, key).unwrap();
            rsc_consensus::verify_validator_vote(&seat.record, &commit).unwrap();
            let state = seat.engine.state();
            assert_eq!(state.phase, ConsensusPhase::Propose);
            assert_eq!(prepare.validator, seat.record.id);
            assert_eq!(prepare.epoch, state.epoch);
            assert_eq!(prepare.height, state.height);
            assert_eq!(prepare.view, state.view);
            assert_eq!(prepare.block_hash, state.proposal.unwrap());
            assert_eq!(precommit.block_hash, prepare.block_hash);
            assert_eq!(commit.block_hash, prepare.block_hash);
            assert_eq!(precommit.epoch, prepare.epoch);
            assert_eq!(commit.epoch, prepare.epoch);
            rounds.push(SignedRound {
                prepare,
                precommit,
                commit,
            });
        }
        rounds
    }

    fn first_three(rounds: &[SignedRound], phase: VotePhase) -> Vec<Vote> {
        rounds
            .iter()
            .take(3)
            .map(|round| match phase {
                VotePhase::Prepare => round.prepare.clone(),
                VotePhase::PreCommit => round.precommit.clone(),
                VotePhase::Commit => round.commit.clone(),
            })
            .collect()
    }

    fn accept_two(engine: &mut ConsensusEngine, votes: &[Vote]) {
        for vote in &votes[..2] {
            assert!(matches!(
                engine.receive_vote(vote.clone()).unwrap(),
                ConsensusEvent::VoteCreated(_)
            ));
        }
    }

    fn checked_qc(
        engine: &ConsensusEngine,
        votes: &[Vote],
        phase: VotePhase,
    ) -> QuorumCertificate {
        let (qc, equiv) =
            rsc_consensus::finality::aggregate_qc(votes, &engine.state().committee, phase)
                .unwrap();
        assert!(equiv.is_none());
        let qc = qc.unwrap();
        rsc_consensus::verify_qc(&qc, &engine.state().committee).unwrap();
        assert_eq!(qc.voting_power, 3);
        assert_eq!(qc.voters.len(), 3);
        assert!(qc.voters.iter().all(|(_, power)| *power == 1));
        assert_eq!(qc.phase, phase);
        qc
    }

    fn reach_quorum(
        engine: &mut ConsensusEngine,
        votes: &[Vote],
        phase: VotePhase,
    ) -> QuorumCertificate {
        accept_two(engine, votes);
        let event = engine.receive_vote(votes[2].clone()).unwrap();
        assert!(matches!(
            event,
            ConsensusEvent::QuorumReached(got) if got == phase
        ));
        checked_qc(engine, votes, phase)
    }

    fn finalize_three(
        engine: &mut ConsensusEngine,
        rounds: &[SignedRound],
        tip: BlockHash,
    ) -> FinalityProof {
        let prepare = reach_quorum(engine, &first_three(rounds, VotePhase::Prepare), VotePhase::Prepare);
        assert_eq!(prepare.block_hash, tip);
        assert_eq!(engine.state().phase, ConsensusPhase::Prepare);
        assert!(engine.state().locked.is_none());
        assert_eq!(engine.state().height, 0);
        let precommit = reach_quorum(
            engine,
            &first_three(rounds, VotePhase::PreCommit),
            VotePhase::PreCommit,
        );
        assert_eq!(precommit.block_hash, tip);
        assert_eq!(engine.state().phase, ConsensusPhase::PreCommit);
        assert_eq!(engine.state().locked, Some(tip));
        let commits = first_three(rounds, VotePhase::Commit);
        accept_two(engine, &commits);
        let event = engine.receive_vote(commits[2].clone()).unwrap();
        let ConsensusEvent::BlockFinalized(proof) = event else {
            panic!("el tercer Commit no finalizó");
        };
        let commit = checked_qc(engine, &commits, VotePhase::Commit);
        assert_eq!(proof.prepare, prepare);
        assert_eq!(proof.precommit, precommit);
        assert_eq!(proof.commit, commit);
        assert_eq!(proof.block_hash, tip);
        assert_eq!(proof.height, 0);
        assert_eq!(proof.view, 0);
        rsc_consensus::verify_commit_chain(&proof, &engine.state().committee).unwrap();
        assert_eq!(engine.state().phase, ConsensusPhase::Decide);
        assert_eq!(engine.state().height, 0);
        assert_eq!(engine.state().locked, Some(tip));
        let absent = rounds[3].prepare.validator;
        assert!(proof.prepare.voters.iter().all(|(id, _)| *id != absent));
        assert!(proof
            .precommit
            .voters
            .iter()
            .all(|(id, _)| *id != absent));
        assert!(proof.commit.voters.iter().all(|(id, _)| *id != absent));
        proof
    }

    fn assert_two_short(engine: &mut ConsensusEngine, rounds: &[SignedRound]) {
        let votes = first_three(rounds, VotePhase::Prepare);
        accept_two(engine, &votes);
        assert_eq!(engine.state().phase, ConsensusPhase::Propose);
        assert!(engine.state().locked.is_none());
        assert_eq!(engine.state().height, 0);
        let (qc, equiv) = rsc_consensus::finality::aggregate_qc(
            &votes[..2],
            &engine.state().committee,
            VotePhase::Prepare,
        )
        .unwrap();
        assert!(qc.is_none());
        assert!(equiv.is_none());
        assert!(!rsc_consensus::has_quorum(2, engine.state().committee.total_voting_power).unwrap());
    }

    fn resigned(vote: &Vote, key: &PrivateKey) -> Vote {
        let mut vote = vote.clone();
        vote.signature = rsc_core::Signature([0; 64]);
        rsc_consensus::finality::sign_vote(&mut vote, key).unwrap();
        vote
    }

    fn assert_rejected_identity(
        seat: &mut ValidatorSeat,
        keys: &[PrivateKey],
        rounds: &[SignedRound],
        record0: &ValidatorRecord,
    ) {
        accept_two(
            &mut seat.engine,
            &first_three(rounds, VotePhase::Prepare),
        );
        let mut outsider = rounds[2].prepare.clone();
        outsider.validator = AccountId::ZERO;
        assert_eq!(
            seat.engine
                .receive_vote(resigned(&outsider, &keys[2]))
                .unwrap_err()
                .to_string(),
            "invalid signature"
        );
        let mismatched = resigned(&rounds[0].prepare, &keys[3]);
        assert_eq!(
            rsc_consensus::verify_validator_vote(record0, &mismatched)
                .unwrap_err()
                .to_string(),
            "invalid signature"
        );
        assert_eq!(
            seat.engine
                .receive_vote(mismatched)
                .unwrap_err()
                .to_string(),
            "invalid signature"
        );
        let mut wrong_key_id = record0.clone();
        wrong_key_id.owner_key_id = keys[1].key_id();
        assert_eq!(
            validate_validator_record(&wrong_key_id)
                .unwrap_err()
                .to_string(),
            "invalid signature"
        );
        let mut wrong_public = record0.clone();
        wrong_public.owner_public_key = keys[1].public_key();
        assert_eq!(
            validate_validator_record(&wrong_public)
                .unwrap_err()
                .to_string(),
            "invalid signature"
        );
        assert_eq!(
            rsc_consensus::verify_validator_vote(&wrong_public, &rounds[0].prepare)
                .unwrap_err()
                .to_string(),
            "invalid signature"
        );
        assert_eq!(
            seat.engine
                .create_vote(VotePhase::Prepare, &keys[1])
                .unwrap_err()
                .to_string(),
            "invalid signature"
        );
        assert_eq!(seat.engine.state().phase, ConsensusPhase::Propose);
        assert!(seat.engine.state().locked.is_none());
        assert_eq!(seat.engine.state().height, 0);
    }

    fn reject_shifted(engine: &mut ConsensusEngine, sample: &Vote, key: &PrivateKey) {
        let mut epoch = sample.clone();
        epoch.epoch = sample.epoch.wrapping_add(1);
        let mut height = sample.clone();
        height.height = sample.height.wrapping_add(1);
        let mut view = sample.clone();
        view.view = sample.view.wrapping_add(1);
        for vote in [resigned(&epoch, key), resigned(&height, key), resigned(&view, key)] {
            assert_eq!(
                engine.receive_vote(vote).unwrap_err().to_string(),
                "wrong view"
            );
            assert_eq!(engine.state().phase, ConsensusPhase::Propose);
            assert_eq!(engine.state().height, 0);
        }
    }

    fn assert_context_and_duplicates(
        engine: &mut ConsensusEngine,
        keys: &[PrivateKey],
        rounds: &[SignedRound],
    ) {
        assert!(matches!(
            engine.receive_vote(rounds[0].prepare.clone()).unwrap(),
            ConsensusEvent::VoteCreated(_)
        ));
        assert_eq!(
            engine.receive_vote(rounds[0].prepare.clone()).unwrap(),
            ConsensusEvent::ProposalAccepted
        );
        assert!(matches!(
            engine.receive_vote(rounds[1].prepare.clone()).unwrap(),
            ConsensusEvent::VoteCreated(_)
        ));
        assert_eq!(engine.state().phase, ConsensusPhase::Propose);
        reject_shifted(engine, &rounds[2].prepare, &keys[2]);
        let mut other = rounds[0].prepare.clone();
        other.block_hash = BlockHash(Hash([0xAB; 32]));
        assert!(matches!(
            engine
                .receive_vote(resigned(&other, &keys[0]))
                .unwrap(),
            ConsensusEvent::Equivocation(_)
        ));
        assert_eq!(engine.state().phase, ConsensusPhase::Propose);
        assert_eq!(engine.state().height, 0);
        assert!(matches!(
            engine.receive_vote(rounds[2].prepare.clone()).unwrap(),
            ConsensusEvent::QuorumReached(VotePhase::Prepare)
        ));
        assert_eq!(engine.state().phase, ConsensusPhase::Prepare);
        assert_eq!(engine.state().height, 0);
        let originals = [
            rounds[0].prepare.clone(),
            rounds[1].prepare.clone(),
            rounds[2].prepare.clone(),
        ];
        let qc = checked_qc(engine, &originals, VotePhase::Prepare);
        assert_eq!(qc.block_hash, rounds[0].prepare.block_hash);
    }

    fn assert_chain_still(opened: &FourValidators) {
        assert_eq!(
            opened.store.get_canonical_tip().unwrap().unwrap(),
            opened.tip
        );
        assert_eq!(opened.state.state_root().unwrap(), opened.root);
        assert_eq!(rebuild_ledger(&opened.store).unwrap().root(), opened.ledger_root);
        assert!(opened
            .store
            .get_block_by_height(BlockHeight(1))
            .unwrap()
            .is_none());
        assert_eq!(opened.store.database().commit_count(), opened.commits);
        assert!(opened
            .store
            .database()
            .get(rsc_storage::ColumnFamily::Default, FINALITY_KEY)
            .unwrap()
            .is_none());
        assert_eq!(opened.seats[0].engine.state().height, 0);
        assert_eq!(
            opened.seats[0].engine.state().phase,
            ConsensusPhase::Decide
        );
    }

    #[test]
    fn four_validators_finalize_with_three_votes() {
        let mut opened = open_four();
        assert_committee(&opened);
        assert_same_context(&opened);
        let signed = sign_phases(&mut opened.seats, &opened.keys);
        let tip = opened.tip;
        let proof = finalize_three(&mut opened.seats[0].engine, &signed, tip);
        assert_two_short(&mut opened.seats[1].engine, &signed);
        let record0 = opened.seats[0].record.clone();
        assert_rejected_identity(&mut opened.seats[2], &opened.keys, &signed, &record0);
        assert_context_and_duplicates(&mut opened.seats[3].engine, &opened.keys, &signed);
        assert_chain_still(&opened);
        assert_eq!(proof.height, 0);
        assert_eq!(proof.block_hash, tip);
    }

    fn phase_votes(rounds: &[SignedRound], indexes: &[usize], phase: VotePhase) -> Vec<Vote> {
        indexes
            .iter()
            .map(|index| {
                let round = &rounds[*index];
                match phase {
                    VotePhase::Prepare => round.prepare.clone(),
                    VotePhase::PreCommit => round.precommit.clone(),
                    VotePhase::Commit => round.commit.clone(),
                }
            })
            .collect()
    }

    fn assert_weighted_identity(opened: &FourValidators, powers: [u128; 4]) {
        let text = std::fs::read_to_string(&opened.genesis).unwrap();
        let genesis: rsc_genesis::Genesis = serde_json::from_str(&text).unwrap();
        let set = genesis.validator_set().unwrap();
        let committee = &opened.seats[0].engine.state().committee;
        assert_eq!(committee.members.len(), 4);
        assert_eq!(committee.total_voting_power, 6);
        assert!(!rsc_consensus::has_quorum(4, 6).unwrap());
        assert!(rsc_consensus::has_quorum(5, 6).unwrap());
        assert!(!rsc_consensus::has_quorum(2, 6).unwrap());
        assert!(!rsc_consensus::has_quorum(3, 6).unwrap());
        for (index, (seat, key)) in opened.seats.iter().zip(&opened.keys).enumerate() {
            assert_eq!(seat.record.voting_power, powers[index]);
            assert_eq!(seat.record.owner_key_id, key.key_id());
            assert_eq!(seat.record.owner_public_key.key_id(), key.key_id());
            assert_eq!(seat.record.owner_public_key, key.public_key());
            assert_eq!(
                seat.record.id,
                AccountId::from(rsc_state::derive_account_id(&seat.record.owner_key_id))
            );
            assert!(seat.record.eligible);
            validate_validator_record(&seat.record).unwrap();
            assert!(set.validators.iter().any(|validator| {
                validator.id == seat.record.id && validator.voting_power == powers[index]
            }));
            assert!(committee.members.iter().any(|member| {
                member.validator_id == seat.record.id && member.voting_power == powers[index]
            }));
        }
    }

    fn accept_created(engine: &mut ConsensusEngine, votes: &[Vote]) {
        for vote in votes {
            assert!(matches!(
                engine.receive_vote(vote.clone()).unwrap(),
                ConsensusEvent::VoteCreated(_)
            ));
        }
    }

    fn no_certificate(engine: &ConsensusEngine, votes: &[Vote], phase: VotePhase) {
        let (qc, equiv) =
            rsc_consensus::finality::aggregate_qc(votes, &engine.state().committee, phase)
                .unwrap();
        assert!(qc.is_none());
        assert!(equiv.is_none());
        assert_eq!(engine.state().phase, ConsensusPhase::Propose);
        assert!(engine.state().locked.is_none());
        assert_eq!(engine.state().height, 0);
    }

    fn stay_short(engine: &mut ConsensusEngine, rounds: &[SignedRound], indexes: &[usize]) {
        for phase in [VotePhase::Prepare, VotePhase::PreCommit, VotePhase::Commit] {
            let votes = phase_votes(rounds, indexes, phase);
            accept_created(engine, &votes);
            no_certificate(engine, &votes, phase);
        }
    }

    fn weighted_qc(
        engine: &ConsensusEngine,
        votes: &[Vote],
        phase: VotePhase,
        power: u128,
    ) -> QuorumCertificate {
        let (qc, equiv) =
            rsc_consensus::finality::aggregate_qc(votes, &engine.state().committee, phase)
                .unwrap();
        assert!(equiv.is_none());
        let qc = qc.unwrap();
        rsc_consensus::verify_qc(&qc, &engine.state().committee).unwrap();
        assert_eq!(qc.voting_power, power);
        assert_eq!(qc.voters.len(), votes.len());
        assert_eq!(qc.phase, phase);
        for vote in votes {
            let (_, claimed) = qc
                .voters
                .iter()
                .find(|(id, _)| *id == vote.validator)
                .unwrap();
            let member = engine
                .state()
                .committee
                .members
                .iter()
                .find(|member| member.validator_id == vote.validator)
                .unwrap();
            assert_eq!(*claimed, member.voting_power);
        }
        qc
    }

    fn finalize_at_five(
        engine: &mut ConsensusEngine,
        rounds: &[SignedRound],
        tip: BlockHash,
    ) -> FinalityProof {
        let prepares = phase_votes(rounds, &[0, 1, 2], VotePhase::Prepare);
        accept_created(engine, &prepares[..2]);
        assert_eq!(engine.state().phase, ConsensusPhase::Propose);
        let event = engine.receive_vote(prepares[2].clone()).unwrap();
        assert!(matches!(
            event,
            ConsensusEvent::QuorumReached(VotePhase::Prepare)
        ));
        let prepare = weighted_qc(engine, &prepares, VotePhase::Prepare, 5);
        assert_eq!(prepare.block_hash, tip);
        assert_eq!(engine.state().phase, ConsensusPhase::Prepare);
        assert!(engine.state().locked.is_none());
        let precommits = phase_votes(rounds, &[0, 1, 2], VotePhase::PreCommit);
        accept_created(engine, &precommits[..2]);
        assert_eq!(engine.state().phase, ConsensusPhase::Prepare);
        let event = engine.receive_vote(precommits[2].clone()).unwrap();
        assert!(matches!(
            event,
            ConsensusEvent::QuorumReached(VotePhase::PreCommit)
        ));
        let precommit = weighted_qc(engine, &precommits, VotePhase::PreCommit, 5);
        assert_eq!(engine.state().phase, ConsensusPhase::PreCommit);
        assert_eq!(engine.state().locked, Some(tip));
        let commits = phase_votes(rounds, &[0, 1, 2], VotePhase::Commit);
        accept_created(engine, &commits[..2]);
        assert_eq!(engine.state().phase, ConsensusPhase::PreCommit);
        let event = engine.receive_vote(commits[2].clone()).unwrap();
        let ConsensusEvent::BlockFinalized(proof) = event else {
            panic!("el Commit de poder 5 no finalizó");
        };
        let commit = weighted_qc(engine, &commits, VotePhase::Commit, 5);
        assert_eq!(proof.prepare, prepare);
        assert_eq!(proof.precommit, precommit);
        assert_eq!(proof.commit, commit);
        assert_eq!(proof.block_hash, tip);
        rsc_consensus::verify_commit_chain(&proof, &engine.state().committee).unwrap();
        assert_eq!(engine.state().phase, ConsensusPhase::Decide);
        assert_eq!(engine.state().height, 0);
        let absent = rounds[3].prepare.validator;
        assert!(proof.prepare.voters.iter().all(|(id, _)| *id != absent));
        proof
    }

    fn power_is_not_counted_twice(
        engine: &mut ConsensusEngine,
        keys: &[PrivateKey],
        rounds: &[SignedRound],
    ) {
        assert!(matches!(
            engine.receive_vote(rounds[0].prepare.clone()).unwrap(),
            ConsensusEvent::VoteCreated(_)
        ));
        assert_eq!(
            engine.receive_vote(rounds[0].prepare.clone()).unwrap(),
            ConsensusEvent::ProposalAccepted
        );
        assert!(matches!(
            engine.receive_vote(rounds[1].prepare.clone()).unwrap(),
            ConsensusEvent::VoteCreated(_)
        ));
        assert_eq!(engine.state().phase, ConsensusPhase::Propose);
        reject_shifted(engine, &rounds[2].prepare, &keys[2]);
        let mut other = rounds[0].prepare.clone();
        other.block_hash = BlockHash(Hash([0xAB; 32]));
        assert!(matches!(
            engine.receive_vote(resigned(&other, &keys[0])).unwrap(),
            ConsensusEvent::Equivocation(_)
        ));
        assert_eq!(engine.state().phase, ConsensusPhase::Propose);
        assert_eq!(engine.state().height, 0);
        let event = engine.receive_vote(rounds[2].prepare.clone()).unwrap();
        assert!(matches!(
            event,
            ConsensusEvent::QuorumReached(VotePhase::Prepare)
        ));
        let originals = phase_votes(rounds, &[0, 1, 2], VotePhase::Prepare);
        let qc = weighted_qc(engine, &originals, VotePhase::Prepare, 5);
        assert_eq!(qc.block_hash, rounds[0].prepare.block_hash);
        assert_eq!(engine.state().height, 0);
    }

    #[test]
    fn weighted_voting_power_reaches_quorum_at_five() {
        let powers = [2, 2, 1, 1];
        let mut opened = open_with_powers(powers);
        assert_weighted_identity(&opened, powers);
        let anchor = opened.seats[0].engine.state();
        for seat in &opened.seats {
            let state = seat.engine.state();
            assert_eq!(state.epoch, anchor.epoch);
            assert_eq!(state.height, 0);
            assert_eq!(state.view, 0);
            assert_eq!(state.proposal, Some(opened.tip));
            assert_eq!(state.committee.total_voting_power, 6);
        }
        let signed = sign_phases(&mut opened.seats, &opened.keys);
        let tip = opened.tip;
        let proof = finalize_at_five(&mut opened.seats[0].engine, &signed, tip);
        stay_short(&mut opened.seats[1].engine, &signed, &[0, 1]);
        stay_short(&mut opened.seats[2].engine, &signed, &[2, 3]);
        power_is_not_counted_twice(&mut opened.seats[3].engine, &opened.keys, &signed);
        assert_chain_still(&opened);
        assert_eq!(proof.commit.voting_power, 5);
        assert_eq!(proof.prepare.voting_power, 5);
        assert_eq!(proof.precommit.voting_power, 5);
        assert_eq!(proof.height, 0);
        assert_eq!(proof.block_hash, tip);
    }

    fn proof_of(
        engine: &ConsensusEngine,
        rounds: &[SignedRound],
        indexes: &[usize],
        power: u128,
    ) -> FinalityProof {
        let prepare = weighted_qc(
            engine,
            &phase_votes(rounds, indexes, VotePhase::Prepare),
            VotePhase::Prepare,
            power,
        );
        let precommit = weighted_qc(
            engine,
            &phase_votes(rounds, indexes, VotePhase::PreCommit),
            VotePhase::PreCommit,
            power,
        );
        let commit = weighted_qc(
            engine,
            &phase_votes(rounds, indexes, VotePhase::Commit),
            VotePhase::Commit,
            power,
        );
        FinalityProof {
            block_hash: prepare.block_hash,
            height: prepare.height,
            view: prepare.view,
            prepare,
            precommit,
            commit,
        }
    }

    fn without_unit_voter(qc: &QuorumCertificate, id: AccountId) -> QuorumCertificate {
        let mut qc = qc.clone();
        let index = qc
            .voters
            .iter()
            .position(|(voter, power)| *voter == id && *power == 1)
            .unwrap();
        let removed = qc.voters.remove(index).1;
        qc.voting_power -= removed;
        let mut signatures = qc.signatures.as_slice().to_vec();
        signatures.remove(index);
        qc.signatures = rsc_crypto::SignatureBundle::from_signatures(&signatures).unwrap();
        qc
    }

    fn proof_without_unit(proof: &FinalityProof, id: AccountId) -> FinalityProof {
        let mut proof = proof.clone();
        proof.prepare = without_unit_voter(&proof.prepare, id);
        proof.precommit = without_unit_voter(&proof.precommit, id);
        proof.commit = without_unit_voter(&proof.commit, id);
        proof
    }

    fn replace_voter(
        qc: &QuorumCertificate,
        from: AccountId,
        to: AccountId,
        power: u128,
        signature: &rsc_core::Signature,
    ) -> QuorumCertificate {
        let mut qc = qc.clone();
        let index = qc.voters.iter().position(|(id, _)| *id == from).unwrap();
        let previous = qc.voters[index].1;
        qc.voters[index] = (to, power);
        qc.voting_power = qc.voting_power - previous + power;
        let mut signatures = qc.signatures.as_slice().to_vec();
        signatures[index] = rsc_crypto::Signature::from_bytes(&signature.0).unwrap();
        qc.signatures = rsc_crypto::SignatureBundle::from_signatures(&signatures).unwrap();
        qc
    }

    fn commit_on_other_hash(
        proof: &FinalityProof,
        rounds: &[SignedRound],
        keys: &[PrivateKey],
        committee: &Committee,
    ) -> FinalityProof {
        let hash = BlockHash(Hash([0xAB; 32]));
        let votes: Vec<Vote> = (0..3)
            .map(|index| {
                let mut vote = rounds[index].commit.clone();
                vote.block_hash = hash;
                resigned(&vote, &keys[index])
            })
            .collect();
        let (qc, equiv) =
            rsc_consensus::finality::aggregate_qc(&votes, committee, VotePhase::Commit).unwrap();
        assert!(equiv.is_none());
        let mut proof = proof.clone();
        proof.commit = qc.unwrap();
        proof
    }

    #[test]
    fn weighted_finality_roundtrips_through_storage_and_rpc() {
        let powers = [2, 2, 1, 1];
        let mut opened = open_with_powers(powers);
        assert_weighted_identity(&opened, powers);
        let signed = sign_phases(&mut opened.seats, &opened.keys);
        let tip = opened.tip;
        let proof = finalize_at_five(&mut opened.seats[0].engine, &signed, tip);
        let committee = opened.seats[0].engine.state().committee.clone();
        for qc in [&proof.prepare, &proof.precommit, &proof.commit] {
            rsc_consensus::verify_qc(qc, &committee).unwrap();
            assert_eq!(qc.voting_power, 5);
            assert_eq!(qc.voters.len(), 3);
        }
        rsc_consensus::verify_commit_chain(&proof, &committee).unwrap();
        let kept = super::recover_proof(&opened.store, &opened.seats[0].engine, proof.clone())
            .unwrap();
        assert_eq!(kept, proof);
        let stored = opened
            .store
            .database()
            .get(rsc_storage::ColumnFamily::Default, FINALITY_KEY)
            .unwrap()
            .unwrap()
            .as_bytes()
            .to_vec();
        let root = opened.root;
        let ledger_root = opened.ledger_root;
        let genesis = opened.genesis.clone();
        let keys = std::mem::take(&mut opened.keys);
        let FourValidators {
            _dir,
            state,
            store,
            ..
        } = opened;
        let path = _dir.path().to_str().unwrap().to_string();
        drop(state);
        drop(store);

        let cfg = config_with_genesis(&path, &genesis);
        let db = Arc::new(open_database(&cfg).unwrap());
        let (state, store) = share_database(db).unwrap();
        assert_eq!(store.get_canonical_tip().unwrap().unwrap(), tip);
        assert_eq!(state.state_root().unwrap(), root);
        assert_eq!(rebuild_ledger(&store).unwrap().root(), ledger_root);
        assert!(store.get_block_by_height(BlockHeight(1)).unwrap().is_none());
        let again = store
            .database()
            .get(rsc_storage::ColumnFamily::Default, FINALITY_KEY)
            .unwrap()
            .unwrap()
            .as_bytes()
            .to_vec();
        assert_eq!(again, stored);
        let (engine, _) = open_engine(&cfg, &store, &keys[0].key_id()).unwrap();
        let committee = engine.state().committee.clone();
        let decoded = super::decode_proof(&again).unwrap();
        assert_eq!(decoded, proof);
        rsc_consensus::verify_commit_chain(&decoded, &committee).unwrap();
        assert_eq!(engine.state().height, 0);

        let rpc = read_finality(&store, &committee).unwrap();
        assert_eq!(rpc.finalized_height, proof.height);
        assert_eq!(
            rpc.finalized_hash,
            rsc_rpc::encode_hex(&proof.block_hash.0 .0)
        );
        assert_eq!(rpc.epoch, proof.prepare.epoch);
        assert_eq!(rpc.view, proof.view);
        assert_eq!(
            rpc.prepare,
            rsc_rpc::encode_hex(&super::encode_qc(&proof.prepare).unwrap())
        );
        assert_eq!(
            rpc.precommit,
            rsc_rpc::encode_hex(&super::encode_qc(&proof.precommit).unwrap())
        );
        assert_eq!(
            rpc.commit,
            rsc_rpc::encode_hex(&super::encode_qc(&proof.commit).unwrap())
        );
        let seen = ask_finality(&store, &committee).unwrap();
        assert_eq!(proof_from_rpc(&seen), proof);
        assert_eq!(seen["finalized_hash"], serde_json::json!(rpc.finalized_hash));
        assert_eq!(seen["finalized_height"], serde_json::json!(rpc.finalized_height));
        assert_eq!(seen["epoch"], serde_json::json!(rpc.epoch));
        assert_eq!(seen["view"], serde_json::json!(rpc.view));
        assert_eq!(seen["prepare"], serde_json::json!(rpc.prepare));
        assert_eq!(seen["precommit"], serde_json::json!(rpc.precommit));
        assert_eq!(seen["commit"], serde_json::json!(rpc.commit));
        assert_eq!(proof_from_rpc(&seen).prepare.voting_power, 5);
        assert_eq!(proof_from_rpc(&seen).precommit.voting_power, 5);
        assert_eq!(proof_from_rpc(&seen).commit.voting_power, 5);

        store
            .database()
            .delete(rsc_storage::ColumnFamily::Default, FINALITY_KEY)
            .unwrap();
        assert_eq!(
            read_finality(&store, &committee).unwrap_err(),
            RpcError::ServiceUnavailable.public_message()
        );
        assert_eq!(
            ask_finality(&store, &committee).unwrap_err(),
            RpcError::ServiceUnavailable
        );
        put_finality(&store, &stored);

        let light = signed[2].prepare.validator;
        let mut altered = committee.clone();
        altered
            .members
            .iter_mut()
            .find(|member| member.validator_id == light)
            .unwrap()
            .voting_power = 2;
        assert_eq!(
            rsc_consensus::verify_commit_chain(&proof, &altered)
                .unwrap_err()
                .to_string(),
            "invalid qc"
        );
        assert_eq!(read_finality(&store, &altered).unwrap_err(), "invalid qc");
        assert!(ask_finality(&store, &altered).is_err());
        assert_eq!(read_finality(&store, &committee).unwrap(), rpc);

        let other = commit_on_other_hash(&proof, &signed, &keys, &committee);
        assert_eq!(
            rsc_consensus::verify_commit_chain(&other, &committee)
                .unwrap_err()
                .to_string(),
            "invalid finality"
        );
        put_finality(&store, &super::encode_proof(&other).unwrap());
        assert_eq!(
            read_finality(&store, &committee).unwrap_err(),
            "invalid finality"
        );
        assert!(ask_finality(&store, &committee).is_err());

        put_finality(&store, &super::encode_proof(&bad_signature(&proof)).unwrap());
        assert_eq!(
            read_finality(&store, &committee).unwrap_err(),
            "invalid signature"
        );
        assert!(ask_finality(&store, &committee).is_err());

        let reduced = proof_without_unit(&proof, light);
        assert_eq!(reduced.prepare.voting_power, 4);
        assert!(!rsc_consensus::has_quorum(4, committee.total_voting_power).unwrap());
        assert_eq!(
            rsc_consensus::verify_qc(&reduced.prepare, &committee)
                .unwrap_err()
                .to_string(),
            "invalid qc"
        );
        put_finality(&store, &super::encode_proof(&reduced).unwrap());
        assert_eq!(read_finality(&store, &committee).unwrap_err(), "invalid qc");
        assert!(ask_finality(&store, &committee).is_err());

        let mut swapped = proof.prepare.clone();
        let slot = swapped
            .voters
            .iter()
            .position(|(id, _)| *id == light)
            .unwrap();
        swapped.voters[slot].0 = signed[3].prepare.validator;
        assert_eq!(
            rsc_consensus::verify_qc(&swapped, &committee)
                .unwrap_err()
                .to_string(),
            "invalid signature"
        );
        let replacement = proof_of(&engine, &signed, &[0, 1, 3], 5);
        rsc_consensus::verify_commit_chain(&replacement, &committee).unwrap();
        assert_eq!(replacement.commit.voting_power, 5);
        let short = phase_votes(&signed, &[1, 2, 3], VotePhase::Prepare);
        let (none, equiv) =
            rsc_consensus::finality::aggregate_qc(&short, &committee, VotePhase::Prepare).unwrap();
        assert!(none.is_none());
        assert!(equiv.is_none());
        let forced = replace_voter(
            &proof.prepare,
            signed[0].prepare.validator,
            signed[3].prepare.validator,
            1,
            &signed[3].prepare.signature,
        );
        assert_eq!(forced.voting_power, 4);
        assert_eq!(
            rsc_consensus::verify_qc(&forced, &committee)
                .unwrap_err()
                .to_string(),
            "invalid qc"
        );

        assert_eq!(store.get_canonical_tip().unwrap().unwrap(), tip);
        assert_eq!(state.state_root().unwrap(), root);
        assert_eq!(rebuild_ledger(&store).unwrap().root(), ledger_root);
        assert!(store.get_block_by_height(BlockHeight(1)).unwrap().is_none());
        assert_eq!(engine.state().height, 0);
    }

    fn stored_finality(path: &str) -> Vec<u8> {
        let db = open_database(&config_at(path, "devnet", 3)).unwrap();
        db.get(rsc_storage::ColumnFamily::Default, super::FINALITY_KEY)
            .unwrap()
            .unwrap()
            .as_bytes()
            .to_vec()
    }
}
