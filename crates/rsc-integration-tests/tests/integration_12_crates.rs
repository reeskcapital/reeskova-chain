//! Un proceso: génesis, RocksDB, transacción, WASM, ledger, bloque, QC, RPC y reapertura.
//!
//! No define constantes de protocolo. Usa los tipos y las funciones que ya exportan los crates.

use std::collections::HashMap;
use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use rsc_chain::{
    header_hash, validate_block, Block, BlockHeader, ChainError, ChainStore,
    ChainValidationContext, RawTransaction, RocksDbChainStore,
};
use rsc_consensus::{
    select_committee, verify_commit_chain, verify_qc, Committee,
    CommitteeSelectionInput, ConsensusConfig, ConsensusError, FinalityProof, NetworkMode,
    ParticipationScore, QuorumCertificate, SignatureBundle, ValidatorRecord, ValidatorSet, Vote,
    VotePhase,
};
use rsc_core::{AccountId, BlockHash, BlockHeight, ChainId, Hash, Nonce, Timestamp};
use rsc_crypto::{PrivateKey, PublicKey, Signature};
use rsc_execution::{
    ExecutionConfig, ExecutionEngine, ExecutionError, ExecutionRequest, WasmLimits, WasmModule,
    WasmValue,
};
use rsc_genesis::{
    Genesis, GenesisAccount as DocumentAccount, NetworkParameters, DEVNET_CHAIN_ID,
    GENESIS_VERSION_V1,
};
use rsc_ledger::{Ledger, Transaction, TransactionError, TransactionId, TransactionPayload};
use rsc_mempool::{
    Mempool, MempoolConfig, MempoolError, MempoolTransaction, MempoolValidationError,
    MempoolValidator, NonceLookupError, PendingNonceProvider,
};
use rsc_p2p::{topic_name, validate_network_id, BootstrapPeer, LocalPeerIdentity, Multiaddr};
use rsc_rpc::methods::dispatch;
use rsc_rpc::{
    encode_hex, ChainService, ConsensusService, NodeService, RpcBalance, RpcBlock, RpcContext,
    RpcError, RpcFinality, RpcNodeInfo, RpcStatus, RpcTransaction, ServiceError, ServiceFut,
    StateService, TransactionService,
};
use rsc_state::{
    AccountId as StateAccountId, GenesisAccount as StateGenesisAccount, State, StateError,
    StateOperation, StateTransition,
};
use rsc_storage::{ColumnFamily, DatabaseConfig, RscDatabase, StorageError};

const ALICE_SEED: u8 = 0x11;
const BOB_SEED: u8 = 0x22;
const TRANSFER: u128 = 40;
const ALICE_BALANCE: u128 = 500;
const BOB_BALANCE: u128 = 50;
const GENESIS_TIME: u64 = 1_700_000_000;

const KEY_GENESIS: &[u8] = b"integ/genesis";
const KEY_FINALITY: &[u8] = b"integ/finality";
const KEY_LEDGER_ROOT: &[u8] = b"integ/ledger-root";

#[derive(Clone, Debug, PartialEq, Eq)]
struct Persisted {
    genesis_hash: [u8; 32],
    block_hash: [u8; 32],
    parent_hash: [u8; 32],
    tx_id: [u8; 32],
    state_root: [u8; 32],
    ledger_root: [u8; 32],
    height: u64,
    alice_balance: u128,
    bob_balance: u128,
    alice_nonce: u64,
    alice_id: [u8; 32],
    bob_id: [u8; 32],
    finalized_hash: [u8; 32],
    epoch: u64,
    view: u64,
}

fn db_config() -> DatabaseConfig {
    DatabaseConfig {
        max_value_size: 8 * 1024 * 1024,
        network_id: "devnet".into(),
        chain_id: "3".into(),
    }
}

fn parameters() -> NetworkParameters {
    NetworkParameters {
        amount_scale: 1_000_000,
        max_block_size: 1_048_576,
        max_transactions_per_block: 1_000,
        max_transaction_bytes: 128 * 1024,
        minimum_fee: 0,
        committee_size: 8,
        epoch_length: 32,
        min_validator_weight: 1,
        max_validator_weight: 1_000,
        min_validators_for_distributed: 8,
        consensus_randomness_seed: Hash([0xA5; 32]),
    }
}

fn seed_key(byte: u8) -> PrivateKey {
    PrivateKey::from_bytes(&[byte; 32]).expect("seed")
}

fn signer_for(n: u8) -> PrivateKey {
    PrivateKey::from_bytes(&validator_id(n).0).expect("32 bytes")
}

fn validator_id(n: u8) -> AccountId {
    let mut bytes = [0u8; 32];
    bytes[31] = n;
    AccountId(bytes)
}

fn core_account(id: StateAccountId) -> AccountId {
    AccountId(*id.as_bytes())
}

fn open_shared(path: &Path) -> (Arc<RscDatabase>, State, RocksDbChainStore) {
    let db = Arc::new(RscDatabase::open(path, db_config()).expect("open database"));
    let state = State::open(Arc::clone(&db)).expect("open state");
    let store = RocksDbChainStore::from_shared(Arc::clone(&db));
    (db, state, store)
}

fn document_for(
    alice: StateAccountId,
    alice_key: &PublicKey,
    bob: StateAccountId,
    bob_key: &PublicKey,
) -> Genesis {
    let accounts = vec![
        DocumentAccount {
            account_id: core_account(alice),
            owner_key_id: alice_key.key_id(),
            native_balance: ALICE_BALANCE,
            allocation: rsc_genesis::GenesisAllocationCategory::Community,
        },
        DocumentAccount {
            account_id: core_account(bob),
            owner_key_id: bob_key.key_id(),
            native_balance: BOB_BALANCE,
            allocation: rsc_genesis::GenesisAllocationCategory::Community,
        },
    ];
    Genesis::new(
        GENESIS_VERSION_V1,
        DEVNET_CHAIN_ID,
        GENESIS_TIME,
        parameters(),
        accounts,
        Vec::new(),
    )
}

fn encode_transfer(tx: &Transaction) -> Vec<u8> {
    let TransactionPayload::Transfer { recipient, amount } = tx.payload else {
        panic!("transfer");
    };
    let mut raw = Vec::with_capacity(157);
    raw.extend_from_slice(&tx.version.to_be_bytes());
    raw.extend_from_slice(&tx.sender.0);
    raw.extend_from_slice(&tx.nonce.to_be_bytes());
    raw.push(0x02);
    raw.extend_from_slice(&recipient.0);
    raw.extend_from_slice(&amount.to_be_bytes());
    raw.extend_from_slice(&tx.signature.to_bytes());
    raw
}

fn decode_transfer(raw: &[u8], signer: &PublicKey) -> Result<Transaction, TransactionError> {
    if raw.len() != 157 {
        return Err(TransactionError::InvalidEncoding);
    }
    let version = u32::from_be_bytes(raw[0..4].try_into().unwrap());
    let sender = AccountId(raw[4..36].try_into().unwrap());
    let nonce = u64::from_be_bytes(raw[36..44].try_into().unwrap());
    if raw[44] != 0x02 {
        return Err(TransactionError::InvalidPayload);
    }
    let recipient = AccountId(raw[45..77].try_into().unwrap());
    let amount = u128::from_be_bytes(raw[77..93].try_into().unwrap());
    let signature =
        Signature::from_bytes(&raw[93..157]).map_err(|_| TransactionError::InvalidSignature)?;
    let tx = Transaction {
        version,
        sender,
        nonce,
        payload: TransactionPayload::Transfer { recipient, amount },
        signature,
    };
    tx.validate(signer)?;
    Ok(tx)
}

struct KnownKeys {
    by_sender: HashMap<AccountId, PublicKey>,
}

impl MempoolValidator for KnownKeys {
    fn validate(
        &self,
        raw_transaction: &[u8],
    ) -> Result<MempoolTransaction, MempoolValidationError> {
        let sender = if raw_transaction.len() >= 36 {
            AccountId(raw_transaction[4..36].try_into().unwrap())
        } else {
            return Err(MempoolValidationError::InvalidTransaction);
        };
        let signer = self
            .by_sender
            .get(&sender)
            .ok_or(MempoolValidationError::ValidationFailed)?;
        let tx = decode_transfer(raw_transaction, signer).map_err(|err| match err {
            TransactionError::InvalidSignature => MempoolValidationError::ValidationFailed,
            _ => MempoolValidationError::InvalidTransaction,
        })?;
        Ok(MempoolTransaction {
            hash: core_tx(tx.id()),
            sender: tx.sender,
            nonce: Nonce(tx.nonce),
            raw_transaction: raw_transaction.to_vec(),
            received_at: Instant::now(),
        })
    }
}

struct FixedNonce;

impl PendingNonceProvider for FixedNonce {
    fn current_nonce(&self, _sender: &AccountId) -> Result<Nonce, NonceLookupError> {
        Ok(Nonce::ZERO)
    }
}

fn core_tx(id: TransactionId) -> rsc_core::TransactionId {
    rsc_core::TransactionId(Hash(id.0.to_bytes()))
}

fn signed_transfer(
    alice: &PrivateKey,
    sender: AccountId,
    recipient: AccountId,
    nonce: u64,
    amount: u128,
) -> Transaction {
    let mut tx = Transaction {
        version: rsc_ledger::transaction::TRANSACTION_VERSION_V1,
        sender,
        nonce,
        payload: TransactionPayload::Transfer { recipient, amount },
        signature: Signature::from_bytes(&[0; 64]).expect("blank signature"),
    };
    tx.sign(alice).expect("sign");
    tx
}

fn wasm_add() -> i32 {
    let bytes = wat::parse_str(
        r#"(module (func (export "add") (param i32 i32) (result i32)
            local.get 0 local.get 1 i32.add))"#,
    )
    .expect("wat");
    let module = WasmModule::validate(&bytes, &WasmLimits::default()).expect("module");
    let engine = ExecutionEngine::new(ExecutionConfig::default()).expect("engine");
    let compiled = engine.compile(&module).expect("compile");
    let result = engine
        .execute(
            &compiled,
            &ExecutionRequest {
                function: "add".into(),
                arguments: vec![WasmValue::I32(3), WasmValue::I32(4)],
                fuel_limit: 100_000,
            },
        )
        .expect("execute");
    match result.return_values.as_slice() {
        [WasmValue::I32(sum)] => *sum,
        other => panic!("unexpected wasm result {other:?}"),
    }
}

fn committee() -> Committee {
    let cfg = ConsensusConfig {
        committee_size: 3,
        epoch_length: 100,
        min_validator_weight: 1,
        max_validator_weight: 10,
        min_validators_for_distributed: 4,
    };
    let set = ValidatorSet {
        validators: (1..=3)
            .map(|n| {
                let key = signer_for(n);
                ValidatorRecord {
                    id: validator_id(n),
                    owner_key_id: key.key_id(),
                    owner_public_key: key.public_key(),
                    voting_power: 10,
                    participation_score: ParticipationScore(0),
                    eligible: true,
                }
            })
            .collect(),
        mode: NetworkMode::Bootstrap,
    };
    let input = CommitteeSelectionInput {
        epoch: 0,
        height: 1,
        view: 0,
        randomness: Hash([7; 32]),
    };
    select_committee(&input, &set, &cfg).expect("committee")
}

fn vote_for(id: u8, phase: VotePhase, hash: BlockHash) -> Vote {
    let mut vote = Vote {
        validator: validator_id(id),
        epoch: 0,
        height: 1,
        view: 0,
        block_hash: hash,
        phase,
        signature: rsc_core::Signature([0; 64]),
    };
    rsc_consensus::finality::sign_vote(&mut vote, &signer_for(id)).expect("firma");
    vote
}

fn certify(committee: &Committee, hash: BlockHash) -> FinalityProof {
    let phases = [VotePhase::Prepare, VotePhase::PreCommit, VotePhase::Commit];
    let qcs: Vec<QuorumCertificate> = phases
        .into_iter()
        .map(|phase| {
            let votes = [1u8, 2, 3].map(|id| vote_for(id, phase, hash));
            rsc_consensus::finality::aggregate_qc(&votes, committee, phase)
                .expect("aggregate")
                .0
                .expect("quorum")
        })
        .collect();
    let proof = FinalityProof {
        block_hash: hash,
        height: 1,
        view: 0,
        prepare: qcs[0].clone(),
        precommit: qcs[1].clone(),
        commit: qcs[2].clone(),
    };
    verify_commit_chain(&proof, committee).expect("finality");
    proof
}

fn encode_qc(qc: &QuorumCertificate) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&qc.block_hash.0 .0);
    body.extend_from_slice(&qc.height.to_le_bytes());
    body.extend_from_slice(&qc.view.to_le_bytes());
    body.extend_from_slice(&qc.epoch.to_le_bytes());
    body.push(qc.phase as u8);
    body.extend_from_slice(&qc.voting_power.to_le_bytes());
    body.extend_from_slice(&u32::try_from(qc.voters.len()).unwrap().to_le_bytes());
    for (id, power) in &qc.voters {
        body.extend_from_slice(&id.0);
        body.extend_from_slice(&power.to_le_bytes());
    }
    let sigs = qc.signatures.to_bytes();
    body.extend_from_slice(&u32::try_from(sigs.len()).unwrap().to_le_bytes());
    body.extend_from_slice(&sigs);
    let mut out = Vec::new();
    out.extend_from_slice(&u32::try_from(body.len()).unwrap().to_le_bytes());
    out.extend_from_slice(&body);
    out
}

fn encode_proof(proof: &FinalityProof) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend(encode_qc(&proof.prepare));
    out.extend(encode_qc(&proof.precommit));
    out.extend(encode_qc(&proof.commit));
    out
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize) -> &'a [u8] {
        let end = self.at + n;
        let slice = &self.bytes[self.at..end];
        self.at = end;
        slice
    }
}

fn decode_qc(cur: &mut Cursor<'_>) -> QuorumCertificate {
    let len = u32::from_le_bytes(cur.take(4).try_into().unwrap()) as usize;
    let start = cur.at;
    let block_hash = BlockHash(Hash(cur.take(32).try_into().unwrap()));
    let height = u64::from_le_bytes(cur.take(8).try_into().unwrap());
    let view = u64::from_le_bytes(cur.take(8).try_into().unwrap());
    let epoch = u64::from_le_bytes(cur.take(8).try_into().unwrap());
    let phase = match cur.take(1)[0] {
        0 => VotePhase::Prepare,
        1 => VotePhase::PreCommit,
        2 => VotePhase::Commit,
        _ => panic!("phase"),
    };
    let voting_power = u128::from_le_bytes(cur.take(16).try_into().unwrap());
    let voters_n = u32::from_le_bytes(cur.take(4).try_into().unwrap()) as usize;
    let mut voters = Vec::with_capacity(voters_n);
    for _ in 0..voters_n {
        let id = AccountId(cur.take(32).try_into().unwrap());
        let power = u128::from_le_bytes(cur.take(16).try_into().unwrap());
        voters.push((id, power));
    }
    let sig_len = u32::from_le_bytes(cur.take(4).try_into().unwrap()) as usize;
    let signatures = SignatureBundle::from_bytes(cur.take(sig_len)).expect("bundle");
    assert_eq!(cur.at - start, len);
    QuorumCertificate {
        block_hash,
        height,
        view,
        phase,
        epoch,
        voters,
        voting_power,
        signatures,
    }
}

fn validation_context(parent: Option<BlockHeader>) -> ChainValidationContext {
    let limits = parameters();
    ChainValidationContext {
        expected_chain_id: DEVNET_CHAIN_ID,
        expected_protocol_version: 1,
        max_block_size: limits.max_block_size as usize,
        max_transactions_per_block: limits.max_transactions_per_block as usize,
        parent,
    }
}

fn seal(header: BlockHeader, transactions: Vec<RawTransaction>) -> Block {
    let mut block = Block {
        header,
        transactions,
    };
    block.header.transactions_root = block.compute_transaction_root();
    block
}

fn commit(path: &Path) -> Persisted {
    let alice_sk = seed_key(ALICE_SEED);
    let bob_sk = seed_key(BOB_SEED);
    let alice_pk = alice_sk.public_key();
    let bob_pk = bob_sk.public_key();
    let alice_state = StateGenesisAccount::new(alice_pk.key_id(), ALICE_BALANCE);
    let bob_state = StateGenesisAccount::new(bob_pk.key_id(), BOB_BALANCE);
    let alice = alice_state.account_id();
    let bob = bob_state.account_id();
    let genesis = document_for(alice, &alice_pk, bob, &bob_pk);
    genesis.validate().expect("genesis");
    let genesis_bytes = genesis.canonical_bytes().expect("genesis bytes");
    let genesis_hash = genesis.genesis_hash().expect("genesis hash").0.to_bytes();

    let (_db, state, store) = open_shared(path);
    state
        .initialize_genesis(&[alice_state, bob_state], 1)
        .expect("state genesis");
    let genesis_root = Hash(*state.state_root().expect("root").as_bytes());
    let proposer = validator_id(1);
    let genesis_block = seal(
        BlockHeader {
            version: 1,
            chain_id: ChainId(DEVNET_CHAIN_ID.0),
            height: BlockHeight::GENESIS,
            previous_block_hash: BlockHash::ZERO,
            timestamp: Timestamp::from_secs(GENESIS_TIME),
            transactions_root: Hash::ZERO,
            state_root: genesis_root,
            proposer,
        },
        Vec::new(),
    );
    validate_block(&genesis_block, &validation_context(None)).expect("genesis block");
    store.put_block(&genesis_block).expect("put genesis");
    let genesis_hash_block = genesis_block.block_hash().expect("genesis block hash");
    store
        .set_canonical_tip(&genesis_hash_block)
        .expect("tip genesis");

    let transfer = signed_transfer(
        &alice_sk,
        core_account(alice),
        core_account(bob),
        0,
        TRANSFER,
    );
    transfer.validate(&alice_pk).expect("tx");
    assert_eq!(wasm_add(), 7);
    let raw = encode_transfer(&transfer);
    let keys = Arc::new(KnownKeys {
        by_sender: HashMap::from([(core_account(alice), alice_pk), (core_account(bob), bob_pk)]),
    });
    let pool = Mempool::new(MempoolConfig::default(), keys, Arc::new(FixedNonce));
    let admitted = pool.insert(raw.clone()).expect("mempool");
    assert_eq!(admitted, core_tx(transfer.id()));
    assert!(pool.contains(&admitted));
    assert_eq!(
        pool.insert(raw.clone()).unwrap_err(),
        MempoolError::AlreadyKnown
    );

    let mut ledger = Ledger::new();
    ledger.append(transfer.id()).expect("ledger");
    let ledger_root = ledger.root().0.to_bytes();

    let mut transition = StateTransition::new(
        state.state_root().expect("root"),
        state.version().expect("version"),
    );
    transition.push(StateOperation::Transfer {
        from: alice,
        to: bob,
        amount: TRANSFER,
        expected_nonce: 0,
    });
    state.apply_transition(&transition, 2).expect("transition");
    let alice_account = state
        .get_account(&alice)
        .expect("alice")
        .expect("alice exists");
    let bob_account = state.get_account(&bob).expect("bob").expect("bob exists");
    assert_eq!(alice_account.native_balance(), ALICE_BALANCE - TRANSFER);
    assert_eq!(bob_account.native_balance(), BOB_BALANCE + TRANSFER);
    assert_eq!(alice_account.nonce(), 1);

    let child = seal(
        BlockHeader {
            version: 1,
            chain_id: DEVNET_CHAIN_ID,
            height: BlockHeight(1),
            previous_block_hash: genesis_hash_block,
            timestamp: Timestamp::from_secs(GENESIS_TIME + 1),
            transactions_root: Hash::ZERO,
            state_root: Hash(*state.state_root().expect("root").as_bytes()),
            proposer,
        },
        vec![RawTransaction(raw)],
    );
    validate_block(
        &child,
        &validation_context(Some(genesis_block.header.clone())),
    )
    .expect("child");
    store.put_block(&child).expect("put child");
    let child_hash = child.block_hash().expect("child hash");
    store.set_canonical_tip(&child_hash).expect("tip child");

    let group = committee();
    let proof = certify(&group, child_hash);
    store
        .database()
        .put(ColumnFamily::Default, KEY_GENESIS, &genesis_bytes)
        .expect("store genesis");
    store
        .database()
        .put(ColumnFamily::Default, KEY_FINALITY, &encode_proof(&proof))
        .expect("store finality");
    store
        .database()
        .put(ColumnFamily::Default, KEY_LEDGER_ROOT, &ledger_root)
        .expect("store ledger root");
    drop(store);
    drop(state);
    drop(_db);

    let persisted = recover(path);
    assert_eq!(persisted.genesis_hash, genesis_hash);
    persisted
}

fn recover(path: &Path) -> Persisted {
    let alice_sk = seed_key(ALICE_SEED);
    let bob_sk = seed_key(BOB_SEED);
    let alice =
        StateGenesisAccount::new(alice_sk.public_key().key_id(), ALICE_BALANCE).account_id();
    let bob = StateGenesisAccount::new(bob_sk.public_key().key_id(), BOB_BALANCE).account_id();
    let (_db, state, store) = open_shared(path);
    let tip = store
        .get_canonical_tip()
        .expect("tip")
        .expect("tip present");
    let child = store
        .get_block(&tip)
        .expect("child")
        .expect("child present");
    let child_hash = child.block_hash().expect("hash");
    assert_eq!(child_hash, tip);
    let parent_hash = store
        .get_block_by_height(BlockHeight::GENESIS)
        .expect("height")
        .expect("genesis height");
    validate_block(
        &child,
        &validation_context(Some(
            store
                .get_block(&parent_hash)
                .expect("parent")
                .expect("parent present")
                .header,
        )),
    )
    .expect("revalidate");

    let raw = &child.transactions[0].0;
    let tx = decode_transfer(raw, &alice_sk.public_key()).expect("reload tx");
    let mut ledger = Ledger::new();
    ledger.append(tx.id()).expect("reload ledger");
    let recomputed = ledger.root().0.to_bytes();
    let stored_root = store
        .database()
        .get(ColumnFamily::Default, KEY_LEDGER_ROOT)
        .expect("ledger key")
        .expect("ledger value");
    assert_eq!(stored_root.as_bytes(), recomputed);

    let genesis_bytes = store
        .database()
        .get(ColumnFamily::Default, KEY_GENESIS)
        .expect("genesis key")
        .expect("genesis value");
    let genesis_hash = rsc_crypto::hash(genesis_bytes.as_bytes()).to_bytes();

    let finality = store
        .database()
        .get(ColumnFamily::Default, KEY_FINALITY)
        .expect("finality key")
        .expect("finality value");
    let mut cursor = Cursor {
        bytes: finality.as_bytes(),
        at: 0,
    };
    let prepare = decode_qc(&mut cursor);
    let precommit = decode_qc(&mut cursor);
    let commit_qc = decode_qc(&mut cursor);
    let proof = FinalityProof {
        block_hash: child_hash,
        height: 1,
        view: 0,
        prepare,
        precommit,
        commit: commit_qc,
    };
    verify_commit_chain(&proof, &committee()).expect("recovered finality");

    let alice_account = state.get_account(&alice).expect("alice").expect("alice");
    let bob_account = state.get_account(&bob).expect("bob").expect("bob");
    Persisted {
        genesis_hash,
        block_hash: child_hash.0 .0,
        parent_hash: parent_hash.0 .0,
        tx_id: tx.id().0.to_bytes(),
        state_root: *state.state_root().expect("root").as_bytes(),
        ledger_root: recomputed,
        height: child.header.height.0,
        alice_balance: alice_account.native_balance(),
        bob_balance: bob_account.native_balance(),
        alice_nonce: alice_account.nonce(),
        alice_id: *alice.as_bytes(),
        bob_id: *bob.as_bytes(),
        finalized_hash: proof.commit.block_hash.0 .0,
        epoch: proof.commit.epoch,
        view: proof.commit.view,
    }
}

struct View {
    persisted: Persisted,
}

impl NodeService for View {
    fn chain_id(&self) -> ServiceFut<'_, String> {
        Box::pin(async { Ok("3".to_string()) })
    }
    fn node_info(&self) -> ServiceFut<'_, RpcNodeInfo> {
        let info = RpcNodeInfo {
            node_id: "integration".into(),
            version: "0.1.0".into(),
            network_id: "devnet".into(),
            chain_id: "3".into(),
            protocol_version: "1".into(),
            sync_status: "ready".into(),
            latest_height: self.persisted.height,
            latest_block_hash: encode_hex(&self.persisted.block_hash),
        };
        Box::pin(async move { Ok(info) })
    }
    fn status(&self) -> ServiceFut<'_, RpcStatus> {
        let status = RpcStatus {
            syncing: false,
            latest_height: self.persisted.height,
            latest_block_hash: encode_hex(&self.persisted.block_hash),
            epoch: self.persisted.epoch,
            finalized_height: self.persisted.height,
        };
        Box::pin(async move { Ok(status) })
    }
}

impl ChainService for View {
    fn block_by_hash(&self, block_hash: &str) -> ServiceFut<'_, RpcBlock> {
        let persisted = self.persisted.clone();
        let block_hash = block_hash.to_string();
        Box::pin(async move {
            if block_hash != encode_hex(&persisted.block_hash) {
                return Err(ServiceError::NotFound);
            }
            Ok(rpc_block(&persisted))
        })
    }
    fn block_by_height(&self, height: u64) -> ServiceFut<'_, RpcBlock> {
        let persisted = self.persisted.clone();
        Box::pin(async move {
            if height != persisted.height {
                return Err(ServiceError::NotFound);
            }
            Ok(rpc_block(&persisted))
        })
    }
    fn transaction(&self, tx_hash: &str) -> ServiceFut<'_, RpcTransaction> {
        let persisted = self.persisted.clone();
        let tx_hash = tx_hash.to_string();
        Box::pin(async move {
            if tx_hash != encode_hex(&persisted.tx_id) {
                return Err(ServiceError::NotFound);
            }
            Ok(RpcTransaction {
                hash: tx_hash,
                raw: encode_hex(&persisted.tx_id),
                status: "finalized".into(),
                block_height: Some(persisted.height),
                block_hash: Some(encode_hex(&persisted.block_hash)),
                index: Some(0),
            })
        })
    }
}

impl TransactionService for View {
    fn submit_signed(&self, _signed_tx: &[u8]) -> ServiceFut<'_, rsc_rpc::RpcSubmitResult> {
        Box::pin(async { Err(ServiceError::InvalidTransaction) })
    }
}

impl StateService for View {
    fn account(&self, account: &str) -> ServiceFut<'_, rsc_rpc::RpcAccount> {
        let persisted = self.persisted.clone();
        let account = account.to_string();
        Box::pin(async move {
            let (balance, nonce) = if account == encode_hex(&persisted.alice_id) {
                (persisted.alice_balance, persisted.alice_nonce)
            } else if account == encode_hex(&persisted.bob_id) {
                (persisted.bob_balance, 0)
            } else {
                return Err(ServiceError::NotFound);
            };
            Ok(rsc_rpc::RpcAccount {
                account,
                balance: balance.to_string(),
                nonce,
            })
        })
    }
    fn balance(&self, account: &str) -> ServiceFut<'_, RpcBalance> {
        let persisted = self.persisted.clone();
        let account = account.to_string();
        Box::pin(async move {
            let balance = if account == encode_hex(&persisted.alice_id) {
                persisted.alice_balance
            } else if account == encode_hex(&persisted.bob_id) {
                persisted.bob_balance
            } else {
                return Err(ServiceError::NotFound);
            };
            Ok(RpcBalance {
                account,
                balance: balance.to_string(),
            })
        })
    }
}

impl ConsensusService for View {
    fn validators(&self) -> ServiceFut<'_, Vec<rsc_rpc::RpcValidator>> {
        Box::pin(async {
            Ok((1..=3)
                .map(|n| rsc_rpc::RpcValidator {
                    validator_id: encode_hex(&validator_id(n).0),
                    public_key: encode_hex(&validator_id(n).0),
                    voting_power: 10,
                    status: "eligible".into(),
                })
                .collect())
        })
    }
    fn finality(&self) -> ServiceFut<'_, RpcFinality> {
        let persisted = self.persisted.clone();
        Box::pin(async move {
            Ok(RpcFinality {
                finalized_height: persisted.height,
                finalized_hash: encode_hex(&persisted.finalized_hash),
                epoch: persisted.epoch,
                view: persisted.view,
                prepare: String::new(),
                precommit: String::new(),
                commit: String::new(),
            })
        })
    }
}

fn rpc_block(persisted: &Persisted) -> RpcBlock {
    RpcBlock {
        hash: encode_hex(&persisted.block_hash),
        height: persisted.height,
        parent_hash: encode_hex(&persisted.parent_hash),
        timestamp: GENESIS_TIME + 1,
        transaction_root: encode_hex(&persisted.tx_id),
        transactions: vec![encode_hex(&persisted.tx_id)],
    }
}

fn wait<T>(future: impl Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(future)
}

fn rpc_sees(persisted: &Persisted) {
    let view = Arc::new(View {
        persisted: persisted.clone(),
    });
    let ctx = RpcContext::new(
        Arc::clone(&view) as Arc<dyn NodeService>,
        Arc::clone(&view) as Arc<dyn ChainService>,
        Arc::clone(&view) as Arc<dyn TransactionService>,
        Arc::clone(&view) as Arc<dyn StateService>,
        Arc::clone(&view) as Arc<dyn ConsensusService>,
    );
    let chain_id =
        wait(dispatch(&ctx, "rsc_chainId", Some(&serde_json::json!({})))).expect("chain id");
    assert_eq!(chain_id, serde_json::json!("3"));
    let status = wait(dispatch(
        &ctx,
        "rsc_getStatus",
        Some(&serde_json::json!({})),
    ))
    .expect("status");
    assert_eq!(status["latest_height"], serde_json::json!(persisted.height));
    assert_eq!(
        status["latest_block_hash"],
        serde_json::json!(encode_hex(&persisted.block_hash))
    );
    let block = wait(dispatch(
        &ctx,
        "rsc_getBlock",
        Some(&serde_json::json!({ "block_hash": encode_hex(&persisted.block_hash) })),
    ))
    .expect("block");
    assert_eq!(
        block["hash"],
        serde_json::json!(encode_hex(&persisted.block_hash))
    );
    assert_eq!(
        block["parent_hash"],
        serde_json::json!(encode_hex(&persisted.parent_hash))
    );
    let tx = wait(dispatch(
        &ctx,
        "rsc_getTransaction",
        Some(&serde_json::json!({ "transaction_hash": encode_hex(&persisted.tx_id) })),
    ))
    .expect("tx");
    assert_eq!(tx["hash"], serde_json::json!(encode_hex(&persisted.tx_id)));
    let alice = encode_hex(&persisted.alice_id);
    let account = wait(dispatch(
        &ctx,
        "rsc_getAccount",
        Some(&serde_json::json!({ "account": alice })),
    ))
    .expect("account");
    assert_eq!(
        account["balance"],
        serde_json::json!(persisted.alice_balance.to_string())
    );
    assert_eq!(account["nonce"], serde_json::json!(persisted.alice_nonce));
    let bob = encode_hex(&persisted.bob_id);
    let balance = wait(dispatch(
        &ctx,
        "rsc_getBalance",
        Some(&serde_json::json!({ "account": bob })),
    ))
    .expect("balance");
    assert_eq!(
        balance["balance"],
        serde_json::json!(persisted.bob_balance.to_string())
    );
    let finality = wait(dispatch(
        &ctx,
        "rsc_getFinality",
        Some(&serde_json::json!({})),
    ))
    .expect("finality");
    assert_eq!(
        finality["finalized_hash"],
        serde_json::json!(encode_hex(&persisted.finalized_hash))
    );
    let missing = wait(dispatch(
        &ctx,
        "rsc_getBlock",
        Some(&serde_json::json!({ "block_hash": encode_hex(&[0; 32]) })),
    ));
    assert_eq!(missing.unwrap_err(), RpcError::NotFound);
}

#[test]
fn integration_12_crates_end_to_end() {
    let dir = tempfile::tempdir().expect("dir");
    let persisted = commit(&dir.path().join("db"));
    assert_eq!(persisted.height, 1);
    assert_eq!(persisted.alice_balance, ALICE_BALANCE - TRANSFER);
    assert_eq!(persisted.bob_balance, BOB_BALANCE + TRANSFER);
    assert_eq!(persisted.finalized_hash, persisted.block_hash);
    assert_eq!(wasm_add(), 7);
    rpc_sees(&persisted);
}

#[test]
fn integration_restart_recovery() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("db");
    let first = commit(&path);
    let second = recover(&path);
    assert_eq!(first, second);
    rpc_sees(&second);
}

#[test]
fn integration_determinism() {
    let left_dir = tempfile::tempdir().expect("left");
    let right_dir = tempfile::tempdir().expect("right");
    let left = commit(&left_dir.path().join("db"));
    let right = commit(&right_dir.path().join("db"));
    assert_eq!(left, right);
    assert_eq!(wasm_add(), wasm_add());
}

#[test]
fn negative_invalid_signature() {
    let alice = seed_key(ALICE_SEED);
    let bob = seed_key(BOB_SEED);
    let mut tx = signed_transfer(&alice, AccountId([1; 32]), AccountId([2; 32]), 0, 1);
    let mut raw = tx.signature.to_bytes();
    raw[0] ^= 0xff;
    tx.signature = Signature::from_bytes(&raw).expect("bytes");
    assert_eq!(
        tx.validate(&alice.public_key()).unwrap_err(),
        TransactionError::InvalidSignature
    );
    assert_eq!(
        signed_transfer(&bob, AccountId([1; 32]), AccountId([2; 32]), 0, 1)
            .validate(&alice.public_key())
            .unwrap_err(),
        TransactionError::InvalidSignature
    );
}

#[test]
fn negative_invalid_qc() {
    let group = committee();
    let hash = BlockHash(Hash([9; 32]));
    let one = [vote_for(1, VotePhase::Prepare, hash)];
    assert!(
        rsc_consensus::finality::aggregate_qc(&one, &group, VotePhase::Prepare)
            .unwrap()
            .0
            .is_none()
    );
    let mixed = [
        vote_for(1, VotePhase::Prepare, hash),
        vote_for(2, VotePhase::Prepare, BlockHash(Hash([8; 32]))),
        vote_for(3, VotePhase::Prepare, hash),
    ];
    assert!(
        rsc_consensus::finality::aggregate_qc(&mixed, &group, VotePhase::Prepare)
            .unwrap()
            .0
            .is_none()
    );
    let proof = certify(&group, hash);
    let mut bad = proof.prepare.clone();
    let mut sigs = bad.signatures.as_slice().to_vec();
    let mut raw = sigs[0].to_bytes();
    raw[0] ^= 0x01;
    sigs[0] = Signature::from_bytes(&raw).unwrap();
    bad.signatures = SignatureBundle::from_signatures(&sigs).unwrap();
    assert_eq!(
        verify_qc(&bad, &group).unwrap_err(),
        ConsensusError::InvalidSignature
    );
}

#[test]
fn negative_invalid_nonce() {
    let dir = tempfile::tempdir().expect("dir");
    let (_db, state, _store) = open_shared(&dir.path().join("db"));
    let alice_sk = seed_key(ALICE_SEED);
    let bob_sk = seed_key(BOB_SEED);
    let alice = StateGenesisAccount::new(alice_sk.public_key().key_id(), ALICE_BALANCE);
    let bob = StateGenesisAccount::new(bob_sk.public_key().key_id(), BOB_BALANCE);
    let alice_id = alice.account_id();
    let bob_id = bob.account_id();
    state.initialize_genesis(&[alice, bob], 1).unwrap();
    let mut transition =
        StateTransition::new(state.state_root().unwrap(), state.version().unwrap());
    transition.push(StateOperation::Transfer {
        from: alice_id,
        to: bob_id,
        amount: 1,
        expected_nonce: 9,
    });
    assert!(matches!(
        state.apply_transition(&transition, 2).unwrap_err(),
        StateError::NonceMismatch
    ));
}

#[test]
fn negative_invalid_balance() {
    let alice = seed_key(ALICE_SEED);
    let mut zero = signed_transfer(&alice, AccountId([1; 32]), AccountId([2; 32]), 0, 0);
    zero.payload = TransactionPayload::Transfer {
        recipient: AccountId([2; 32]),
        amount: 0,
    };
    zero.sign(&alice).unwrap();
    assert_eq!(
        zero.validate(&alice.public_key()).unwrap_err(),
        TransactionError::InvalidAmount
    );

    let dir = tempfile::tempdir().expect("dir");
    let (_db, state, _store) = open_shared(&dir.path().join("db"));
    let alice_sk = seed_key(ALICE_SEED);
    let bob_sk = seed_key(BOB_SEED);
    let alice_acc = StateGenesisAccount::new(alice_sk.public_key().key_id(), ALICE_BALANCE);
    let bob_acc = StateGenesisAccount::new(bob_sk.public_key().key_id(), BOB_BALANCE);
    let alice_id = alice_acc.account_id();
    let bob_id = bob_acc.account_id();
    state.initialize_genesis(&[alice_acc, bob_acc], 1).unwrap();
    let mut transition = StateTransition::new(state.state_root().unwrap(), 1);
    transition.push(StateOperation::Transfer {
        from: alice_id,
        to: bob_id,
        amount: ALICE_BALANCE + 1,
        expected_nonce: 0,
    });
    assert!(matches!(
        state.apply_transition(&transition, 2).unwrap_err(),
        StateError::InsufficientBalance
    ));
}

#[test]
fn negative_invalid_block() {
    let proposer = validator_id(1);
    let parent = seal(
        BlockHeader {
            version: 1,
            chain_id: DEVNET_CHAIN_ID,
            height: BlockHeight::GENESIS,
            previous_block_hash: BlockHash::ZERO,
            timestamp: Timestamp::from_secs(GENESIS_TIME),
            transactions_root: Hash::ZERO,
            state_root: Hash::ZERO,
            proposer,
        },
        Vec::new(),
    );
    let parent_hash = parent.block_hash().unwrap();
    let mut child = seal(
        BlockHeader {
            version: 1,
            chain_id: DEVNET_CHAIN_ID,
            height: BlockHeight(1),
            previous_block_hash: BlockHash(Hash([9; 32])),
            timestamp: Timestamp::from_secs(GENESIS_TIME + 1),
            transactions_root: Hash::ZERO,
            state_root: Hash::ZERO,
            proposer,
        },
        Vec::new(),
    );
    assert_eq!(
        validate_block(&child, &validation_context(Some(parent.header.clone()))).unwrap_err(),
        ChainError::InvalidParentHash
    );
    child.header.previous_block_hash = parent_hash;
    child.header.transactions_root = Hash([1; 32]);
    assert_eq!(
        validate_block(&child, &validation_context(Some(parent.header.clone()))).unwrap_err(),
        ChainError::InvalidTransactionRoot
    );

    let dir = tempfile::tempdir().expect("dir");
    let (_db, _state, store) = open_shared(&dir.path().join("db"));
    store.put_block(&parent).unwrap();
    store
        .database()
        .put(ColumnFamily::Blocks, &parent_hash.0 .0, b"corrupt")
        .unwrap();
    assert_eq!(
        store.get_block(&parent_hash).unwrap_err(),
        ChainError::InvalidEncoding
    );
}

#[test]
fn negative_corrupt_storage() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("db");
    let (db, _state, _store) = open_shared(&path);
    drop(_store);
    drop(_state);
    drop(db);
    match RscDatabase::open(
        &path,
        DatabaseConfig {
            network_id: "other".into(),
            ..db_config()
        },
    ) {
        Err(StorageError::NetworkMismatch) => {}
        Ok(_) => panic!("foreign network opened"),
        Err(err) => panic!("unexpected {err}"),
    }
    match RscDatabase::open(
        &path,
        DatabaseConfig {
            chain_id: "9".into(),
            ..db_config()
        },
    ) {
        Err(StorageError::ChainMismatch) => {}
        Ok(_) => panic!("foreign chain opened"),
        Err(err) => panic!("unexpected {err}"),
    }
}

#[test]
fn negative_invalid_wasm() {
    assert!(matches!(
        WasmModule::validate(b"not-wasm", &WasmLimits::default()).unwrap_err(),
        ExecutionError::InvalidModule | ExecutionError::ValidationFailed
    ));
    let limits = WasmLimits {
        max_memory_pages: 1,
        ..WasmLimits::default()
    };
    let memory = wat::parse_str("(module (memory 8) (func (export \"n\")))").unwrap();
    assert_eq!(
        WasmModule::validate(&memory, &limits).unwrap_err(),
        ExecutionError::MemoryLimitExceeded
    );
    let add = wat::parse_str(
        r#"(module (func (export "add") (param i32 i32) (result i32) local.get 0 local.get 1 i32.add))"#,
    )
    .unwrap();
    let module = WasmModule::validate(&add, &WasmLimits::default()).unwrap();
    let config = ExecutionConfig {
        max_output_size: 0,
        ..ExecutionConfig::default()
    };
    let engine = ExecutionEngine::new(config).unwrap();
    let compiled = engine.compile(&module).unwrap();
    assert_eq!(
        engine
            .execute(
                &compiled,
                &ExecutionRequest {
                    function: "add".into(),
                    arguments: vec![WasmValue::I32(1), WasmValue::I32(1)],
                    fuel_limit: 10_000,
                },
            )
            .unwrap_err(),
        ExecutionError::OutputTooLarge
    );
    let burn = wat::parse_str("(module (func (export \"burn\") (loop br 0)))").unwrap();
    let burn_module = WasmModule::validate(&burn, &WasmLimits::default()).unwrap();
    let engine = ExecutionEngine::new(ExecutionConfig::default()).unwrap();
    let compiled = engine.compile(&burn_module).unwrap();
    assert_eq!(
        engine
            .execute(
                &compiled,
                &ExecutionRequest {
                    function: "burn".into(),
                    arguments: Vec::new(),
                    fuel_limit: 1_000,
                },
            )
            .unwrap_err(),
        ExecutionError::FuelExhausted
    );
}

#[test]
fn negative_malformed_p2p() {
    assert!(validate_network_id("Bad").is_err());
    assert!(topic_name("devnet", "nope").is_err());
    assert!("not-a-multiaddr".parse::<Multiaddr>().is_err());
    let peer = BootstrapPeer {
        peer_id: LocalPeerIdentity::generate().0.peer_id(),
        addresses: Vec::new(),
    };
    assert!(peer.validate().is_err());
}

#[test]
fn negative_invalid_merkle_root() {
    let block = seal(
        BlockHeader {
            version: 1,
            chain_id: DEVNET_CHAIN_ID,
            height: BlockHeight(1),
            previous_block_hash: BlockHash(Hash([1; 32])),
            timestamp: Timestamp::from_secs(1),
            transactions_root: Hash([9; 32]),
            state_root: Hash::ZERO,
            proposer: validator_id(1),
        },
        vec![RawTransaction(b"tx".to_vec())],
    );
    let parent = BlockHeader {
        version: 1,
        chain_id: DEVNET_CHAIN_ID,
        height: BlockHeight::GENESIS,
        previous_block_hash: BlockHash::ZERO,
        timestamp: Timestamp::from_secs(0),
        transactions_root: Hash::ZERO,
        state_root: Hash::ZERO,
        proposer: validator_id(1),
    };
    let parent_hash = header_hash(&parent).unwrap();
    let mut child = block;
    child.header.previous_block_hash = parent_hash;
    child.header.transactions_root = Hash([9; 32]);
    assert_eq!(
        validate_block(&child, &validation_context(Some(parent))).unwrap_err(),
        ChainError::InvalidTransactionRoot
    );
}
