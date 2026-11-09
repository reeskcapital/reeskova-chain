//! Lee la configuración, abre una base, escribe el bloque 0 si falta el tip, reconstruye el ledger, arranca mempool, ejecución y consenso, propone el tip, cierra el FinalityProof local, avanza la altura del motor, admite transacciones, confirma en un lote la que está Ready, alinea el motor con ese tip, sirve RPC, escucha P2P y sella un bloque por intervalo.

use std::collections::VecDeque;
use std::env;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use rsc_amm::native::{AmmRuntime, BlockContext};
use rsc_chain::{
    encode_block, validate_block, Block, BlockHeader, ChainStore, ChainValidationContext,
    RawTransaction, RocksDbChainStore,
};
use rsc_consensus::{
    ConsensusConfig, ConsensusEngine, ConsensusEvent, ConsensusPhase, FinalityProof, NetworkMode,
    ParticipationScore, QuorumCertificate, ValidatorRecord, ValidatorSet, Vote, VotePhase,
};
use rsc_core::{AccountId, BlockHash, BlockHeight, Hash, Nonce, Timestamp};
use rsc_execution::{ExecutionConfig, ExecutionEngine};
use rsc_genesis::GenesisAllocationCategory;
use rsc_genesis::{Genesis, NetworkParameters};
use rsc_ledger::{Ledger, Transaction, TransactionPayload, TRANSACTION_VERSION_V1};
use rsc_mempool::{
    Mempool, MempoolConfig, MempoolTransaction, MempoolValidationError, MempoolValidator,
    NonceLookupError, PendingNonceProvider,
};
use rsc_monetary::{protocol_fee_pool_key_id, EmissionCategory, MonetaryRuntime};
use rsc_p2p::{DiscoveryConfig, LocalPeerIdentity, NetworkConfig, NetworkEvent, RscNetwork};
use rsc_rpc::{
    decode_hex, encode_hex, merge_rpc_endpoints, normalize_rpc_url, AmmService, ChainService,
    ConsensusService, NodeService, RpcAccount, RpcAddLiquidityQuote, RpcAsset, RpcAssetBalance,
    RpcBalance, RpcBlock, RpcConfig, RpcContext, RpcEndpoint, RpcFinality, RpcLpPosition,
    RpcNodeInfo, RpcPool, RpcServer, RpcStatus, RpcSubmitResult, RpcSwapQuote, RpcTransaction,
    RpcValidator, ServiceError, ServiceFut, StateService, TransactionService,
};
use rsc_state::derive_account_id;
use rsc_state::{
    GenesisAccount as StateGenesisAccount, State, StateError, StateOperation, StateTransition,
};
use rsc_storage::{ColumnFamily, DatabaseConfig, RscDatabase, StorageBatch, StorageError};

/// Tope de valor que `DatabaseConfig` exige para abrir. No entra en la identidad.
const MAX_VALUE_SIZE: usize = 8 * 1024 * 1024;

/// Segundos entre un bloque sellado y el siguiente. El bloque sale aunque no haya transacciones.
const BLOCK_INTERVAL_SECS: u64 = 4;

/// Clave de la `FinalityProof` en `ColumnFamily::Default`. La misma que usa el validador.
const FINALITY_KEY: &[u8] = b"finality";

/// Parámetros de red persistidos tras el génesis (`NetworkParameters` JSON).
const NETWORK_PARAMS_KEY: &[u8] = b"RSC/NETWORK/V1/PARAMETERS";

/// Runtime monetario persistido (`MonetaryRuntime` JSON).
const MONETARY_RUNTIME_KEY: &[u8] = b"RSC/MONETARY/V1/RUNTIME";

/// Alta de cuenta firmada, pendiente de entrar en el próximo bloque.
#[derive(Clone)]
struct WalletOpening {
    public_key: [u8; 32],
    raw: Vec<u8>,
}

#[derive(Debug)]
struct Config {
    data_dir: String,
    network_id: String,
    chain_id: u32,
    genesis: Option<String>,
    /// Token admin AMM (testnet). Flag `--amm-admin-token` o env `AMM_ADMIN_TOKEN`.
    /// No activar en un RPC público de mainnet.
    amm_admin_token: Option<String>,
    /// `--rpc-bind` o `RSC_RPC_BIND`. Default `127.0.0.1:8545`.
    rpc_bind: SocketAddr,
    /// `--rpc-cors <origin>`, repetible. Vacío desactiva CORS.
    rpc_cors: Vec<String>,
    /// URL pública de este nodo (`--rpc-public-url` / `RSC_RPC_PUBLIC_URL`).
    /// Se anuncia en `rsc_getRpcEndpoints` y `GET /network`.
    rpc_public_url: Option<String>,
    /// Semillas RPC (`--rpc-seed` repetible / `RSC_RPC_SEEDS` CSV / `rpc-seeds.json`).
    rpc_seeds: Vec<String>,
}

fn main() -> ExitCode {
    let config = match parse_args(env::args().skip(1)) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(1);
        }
    };
    let db = match open_database(&config) {
        Ok(db) => Arc::new(db),
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(1);
        }
    };
    let (state, store) = match share_database(db) {
        Ok(shared) => shared,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(1);
        }
    };
    if let Err(err) = ensure_tip(&state, &store, &config) {
        eprintln!("{err}");
        return ExitCode::from(1);
    }
    if let Err(err) = rebuild_ledger(&store) {
        eprintln!("{err}");
        return ExitCode::from(1);
    }
    let (pool, execution, mut consensus) = match open_services(&store) {
        Ok(services) => services,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(1);
        }
    };
    if let Err(err) = resume(&state, &store, &pool, &mut consensus) {
        eprintln!("{err}");
        return ExitCode::from(1);
    }
    let _ = execution;
    if let Err(err) = serve_rpc(&config, state, store, consensus, pool) {
        eprintln!("{err}");
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}

fn ensure_tip(state: &State, store: &RocksDbChainStore, config: &Config) -> Result<(), String> {
    if store
        .get_canonical_tip()
        .map_err(|err| err.to_string())?
        .is_some()
    {
        return Ok(());
    }
    let path = config.genesis.as_deref().ok_or("falta genesis")?;
    let text = std::fs::read_to_string(path).map_err(|err| err.to_string())?;
    let genesis: Genesis = serde_json::from_str(&text).map_err(|err| err.to_string())?;
    genesis.validate().map_err(|err| err.to_string())?;
    if genesis.chain_id.0 != config.chain_id {
        return Err("chain-id del genesis distinto".into());
    }
    let mut accounts = Vec::with_capacity(genesis.initial_accounts.len());
    for account in &genesis.initial_accounts {
        let staged = StateGenesisAccount::new(account.owner_key_id, account.native_balance);
        let derived = AccountId::from(staged.account_id());
        if derived != account.account_id {
            return Err("genesis account id distinto".into());
        }
        accounts.push(staged);
    }
    state
        .initialize_genesis(&accounts, u64::from(genesis.version))
        .map_err(|err| err.to_string())?;
    apply_amm_genesis(store.database(), &genesis)?;

    let fee_pool_key = protocol_fee_pool_key_id();
    let fee_pool_state = derive_account_id(&fee_pool_key);
    let mut version = u64::from(genesis.version);
    if state
        .get_account(&fee_pool_state)
        .map_err(|err| err.to_string())?
        .is_none()
    {
        let root = state.state_root().map_err(|err| err.to_string())?;
        let mut transition = StateTransition::new(root, version);
        transition.push(StateOperation::CreateAccount {
            owner_key_id: fee_pool_key,
        });
        version = version.checked_add(1).ok_or("versión de estado agotada")?;
        state
            .apply_transition(&transition, version)
            .map_err(|err| err.to_string())?;
    }

    persist_network_parameters(store.database().as_ref(), &genesis.parameters)?;
    let mut monetary = MonetaryRuntime::v1();
    let allocations: Vec<(EmissionCategory, rsc_core::Amount)> = genesis
        .initial_accounts
        .iter()
        .map(|account| {
            (
                emission_category(account.allocation),
                rsc_core::Amount(account.native_balance),
            )
        })
        .collect();
    monetary
        .bootstrap_from_allocations(&allocations)
        .map_err(|err| err.to_string())?;
    persist_monetary_runtime(store.database().as_ref(), &monetary)?;

    let root = state.state_root().map_err(|err| err.to_string())?;
    let mut block = Block {
        header: BlockHeader {
            version: genesis.version,
            chain_id: genesis.chain_id,
            height: BlockHeight::GENESIS,
            previous_block_hash: BlockHash::ZERO,
            timestamp: Timestamp::from_secs(genesis.timestamp),
            transactions_root: Hash::ZERO,
            state_root: Hash(*root.as_bytes()),
            proposer: AccountId::ZERO,
        },
        transactions: Vec::new(),
    };
    block.header.transactions_root = block.compute_transaction_root();
    let limits = genesis.parameters;
    validate_block(
        &block,
        &ChainValidationContext {
            expected_chain_id: genesis.chain_id,
            expected_protocol_version: genesis.version,
            max_block_size: limits.max_block_size as usize,
            max_transactions_per_block: limits.max_transactions_per_block as usize,
            parent: None,
        },
    )
    .map_err(|err| err.to_string())?;
    store.put_block(&block).map_err(|err| err.to_string())?;
    let hash = block.block_hash().map_err(|err| err.to_string())?;
    store
        .set_canonical_tip(&hash)
        .map_err(|err| err.to_string())?;
    Ok(())
}

fn emission_category(allocation: GenesisAllocationCategory) -> EmissionCategory {
    match allocation {
        GenesisAllocationCategory::Mining => EmissionCategory::Mining,
        GenesisAllocationCategory::Validators => EmissionCategory::Validators,
        GenesisAllocationCategory::Community => EmissionCategory::Community,
        GenesisAllocationCategory::Treasury => EmissionCategory::Treasury,
        GenesisAllocationCategory::HolderRewards => EmissionCategory::HolderRewards,
        GenesisAllocationCategory::OperatorRewards => EmissionCategory::OperatorRewards,
    }
}

/// Registra activos AMM, fondea saldos, fija governors y policy solo en génesis.
///
/// En restart `AmmStore` / `AssetLedger` ya traen los bytes; `ensure_tip` no
/// vuelve a entrar aquí porque el tip canónico ya existe.
fn apply_amm_genesis(db: Arc<RscDatabase>, genesis: &Genesis) -> Result<(), String> {
    let needs_amm = !genesis.amm_assets.is_empty()
        || !genesis.amm_balances.is_empty()
        || !genesis.amm_governors.is_empty()
        || !genesis.amm_permissionless_creation;
    if !needs_amm {
        return Ok(());
    }
    let state = State::open(Arc::clone(&db)).map_err(|err| err.to_string())?;
    let runtime = AmmRuntime::new(db, state);
    for asset in &genesis.amm_assets {
        runtime
            .register_asset(
                rsc_amm::AssetId(asset.asset_id),
                asset.decimals,
                &asset.symbol,
            )
            .map_err(|err| err.to_string())?;
    }
    for balance in &genesis.amm_balances {
        runtime
            .credit_asset(
                balance.account_id,
                rsc_amm::AssetId(balance.asset_id),
                rsc_core::Amount(balance.amount),
            )
            .map_err(|err| err.to_string())?;
    }
    runtime
        .set_policy(rsc_amm::PoolPolicy {
            permissionless_creation: genesis.amm_permissionless_creation,
            min_liquidity: rsc_amm::DEFAULT_MIN_LIQUIDITY,
        })
        .map_err(|err| err.to_string())?;
    if !genesis.amm_governors.is_empty() {
        runtime
            .set_governors(genesis.amm_governors.clone())
            .map_err(|err| err.to_string())?;
    }
    Ok(())
}

fn persist_network_parameters(db: &RscDatabase, params: &NetworkParameters) -> Result<(), String> {
    let bytes = serde_json::to_vec(params).map_err(|err| err.to_string())?;
    db.put(ColumnFamily::Default, NETWORK_PARAMS_KEY, &bytes)
        .map_err(|err| err.to_string())
}

fn load_network_parameters(db: &RscDatabase) -> Result<NetworkParameters, String> {
    let bytes = db
        .get(ColumnFamily::Default, NETWORK_PARAMS_KEY)
        .map_err(|err| err.to_string())?
        .ok_or("faltan parámetros de red")?;
    serde_json::from_slice(bytes.as_bytes()).map_err(|err| err.to_string())
}

fn persist_monetary_runtime(db: &RscDatabase, runtime: &MonetaryRuntime) -> Result<(), String> {
    let bytes = serde_json::to_vec(runtime).map_err(|err| err.to_string())?;
    db.put(ColumnFamily::Default, MONETARY_RUNTIME_KEY, &bytes)
        .map_err(|err| err.to_string())
}

fn load_monetary_runtime(db: &RscDatabase) -> Result<MonetaryRuntime, String> {
    let bytes = db
        .get(ColumnFamily::Default, MONETARY_RUNTIME_KEY)
        .map_err(|err| err.to_string())?
        .ok_or("falta runtime monetario")?;
    serde_json::from_slice(bytes.as_bytes()).map_err(|err| err.to_string())
}

fn push_transfer_operation(
    transition: &mut StateTransition,
    sender: AccountId,
    recipient: AccountId,
    amount: u128,
    nonce: u64,
    minimum_fee: u128,
) -> Result<Option<rsc_monetary::ProtocolFeePlan>, String> {
    let plan = MonetaryRuntime::plan_minimum_fee(minimum_fee).map_err(|err| err.to_string())?;
    match plan {
        None => {
            transition.push(StateOperation::Transfer {
                from: sender.into(),
                to: recipient.into(),
                amount,
                expected_nonce: nonce,
            });
            Ok(None)
        }
        Some(plan) => {
            transition.push(StateOperation::TransferWithFee {
                from: sender.into(),
                to: recipient.into(),
                amount,
                validator_fee: plan.validator_fee.0,
                burn_fee: plan.burn_fee.0,
                fee_pool: derive_account_id(&protocol_fee_pool_key_id()),
                expected_nonce: nonce,
            });
            Ok(Some(plan))
        }
    }
}

struct ChainAdmission {
    state: State,
    minimum_fee: u128,
}

impl MempoolValidator for ChainAdmission {
    fn validate(
        &self,
        raw_transaction: &[u8],
    ) -> Result<MempoolTransaction, MempoolValidationError> {
        let transaction = Transaction::from_canonical_bytes(raw_transaction)
            .map_err(|_| MempoolValidationError::InvalidTransaction)?;
        if transaction.version != TRANSACTION_VERSION_V1 {
            return Err(MempoolValidationError::ValidationFailed);
        }
        match &transaction.payload {
            TransactionPayload::Transfer { amount, .. } if *amount == 0 => {
                return Err(MempoolValidationError::ValidationFailed);
            }
            TransactionPayload::Amm { instruction } if instruction.is_empty() => {
                return Err(MempoolValidationError::ValidationFailed);
            }
            _ => {}
        }
        let sender = rsc_state::AccountId::from(transaction.sender);
        let account = match self.state.get_account(&sender) {
            Ok(Some(account)) => account,
            Ok(None) => return Err(MempoolValidationError::ValidationFailed),
            Err(_) => return Err(MempoolValidationError::ValidationUnavailable),
        };
        if let TransactionPayload::Transfer { amount, .. } = &transaction.payload {
            let debit = amount
                .checked_add(self.minimum_fee)
                .ok_or(MempoolValidationError::ValidationFailed)?;
            if account.native_balance() < debit {
                return Err(MempoolValidationError::ValidationFailed);
            }
        }
        let id = transaction.id();
        Ok(MempoolTransaction {
            hash: rsc_core::TransactionId(Hash(*id.0.as_bytes())),
            sender: transaction.sender,
            nonce: Nonce(transaction.nonce),
            raw_transaction: raw_transaction.to_vec(),
            received_at: Instant::now(),
        })
    }
}

impl PendingNonceProvider for ChainAdmission {
    fn current_nonce(&self, sender: &AccountId) -> Result<Nonce, NonceLookupError> {
        match self.state.get_account(&rsc_state::AccountId::from(*sender)) {
            Ok(Some(account)) => Ok(Nonce(account.nonce())),
            Ok(None) | Err(_) => Err(NonceLookupError::Unavailable),
        }
    }
}

fn node_bootstrap_signer() -> rsc_crypto::PrivateKey {
    rsc_crypto::PrivateKey::from_bytes(&[0u8; 32]).expect("32 bytes")
}

fn open_services(
    store: &RocksDbChainStore,
) -> Result<(Arc<Mempool>, ExecutionEngine, ConsensusEngine), String> {
    let tip = store
        .get_canonical_tip()
        .map_err(|err| err.to_string())?
        .ok_or("falta tip")?;
    let block = store
        .get_block(&tip)
        .map_err(|err| err.to_string())?
        .ok_or("bloque ausente")?;
    let minimum_fee = load_network_parameters(store.database().as_ref())
        .map(|params| params.minimum_fee)
        .unwrap_or(0);
    let admission = Arc::new(ChainAdmission {
        state: State::open(store.database()).map_err(|err| err.to_string())?,
        minimum_fee,
    });
    let pool = Arc::new(Mempool::new(
        MempoolConfig::default(),
        Arc::clone(&admission) as Arc<dyn MempoolValidator>,
        admission,
    ));
    let execution =
        ExecutionEngine::new(ExecutionConfig::default()).map_err(|err| err.to_string())?;
    let signer = node_bootstrap_signer();
    let consensus = ConsensusEngine::new(
        ConsensusConfig {
            committee_size: 1,
            epoch_length: 1,
            min_validator_weight: 1,
            max_validator_weight: 1,
            min_validators_for_distributed: 1,
        },
        ValidatorSet {
            validators: vec![ValidatorRecord {
                id: AccountId::ZERO,
                owner_key_id: signer.key_id(),
                owner_public_key: signer.public_key(),
                voting_power: 1,
                participation_score: ParticipationScore(0),
                eligible: true,
            }],
            mode: NetworkMode::Bootstrap,
        },
        Hash::ZERO,
        AccountId::ZERO,
        block.header.height.0,
    )
    .map_err(|err| err.to_string())?;
    Ok((pool, execution, consensus))
}

#[cfg(test)]
fn prepare_ready(state: &State, pool: &Mempool) -> Result<(), String> {
    let Some(pending) = pool.ready_transactions(1).into_iter().next() else {
        return Ok(());
    };
    let transaction = Transaction::from_canonical_bytes(&pending.raw_transaction)
        .map_err(|err| err.to_string())?;
    let id = transaction.id();
    if pending.hash != rsc_core::TransactionId(Hash(*id.0.as_bytes())) {
        return Err("hash distinto".into());
    }
    let TransactionPayload::Transfer { recipient, amount } = transaction.payload else {
        return Err("payload distinto".into());
    };
    let root = state.state_root().map_err(|err| err.to_string())?;
    let version = state.version().map_err(|err| err.to_string())?;
    let mut transition = StateTransition::new(root, version);
    let _plan = push_transfer_operation(
        &mut transition,
        transaction.sender,
        recipient,
        amount,
        transaction.nonce,
        0,
    )?;
    let next = version.checked_add(1).ok_or("versión de estado agotada")?;
    let commitment = state
        .apply_transition(&transition, next)
        .map_err(|err| err.to_string())?;
    if commitment.previous_root() != root
        || commitment.previous_version() != version
        || commitment.new_version() != next
        || commitment.new_root() == root
    {
        return Err("transición distinta".into());
    }
    Ok(())
}

fn commit_ready(state: &State, store: &RocksDbChainStore, pool: &Mempool) -> Result<(), String> {
    let Some(pending) = pool.ready_transactions(1).into_iter().next() else {
        return Ok(());
    };
    let transaction = Transaction::from_canonical_bytes(&pending.raw_transaction)
        .map_err(|err| err.to_string())?;
    let id = transaction.id();
    if pending.hash != rsc_core::TransactionId(Hash(*id.0.as_bytes())) {
        return Err("hash distinto".into());
    }
    if matches!(&transaction.payload, TransactionPayload::Amm { .. }) {
        return commit_ready_amm(state, store, pool, &transaction, &pending.raw_transaction);
    }
    let TransactionPayload::Transfer { recipient, amount } = transaction.payload.clone() else {
        return Err("payload distinto".into());
    };
    let minimum_fee = load_network_parameters(store.database().as_ref())
        .map(|params| params.minimum_fee)
        .unwrap_or(0);
    let root = state.state_root().map_err(|err| err.to_string())?;
    let version = state.version().map_err(|err| err.to_string())?;
    let mut transition = StateTransition::new(root, version);
    let fee_plan = push_transfer_operation(
        &mut transition,
        transaction.sender,
        recipient,
        amount,
        transaction.nonce,
        minimum_fee,
    )?;
    let next = version.checked_add(1).ok_or("versión de estado agotada")?;
    let tip = store
        .get_canonical_tip()
        .map_err(|err| err.to_string())?
        .ok_or("falta tip")?;
    let parent = store
        .get_block(&tip)
        .map_err(|err| err.to_string())?
        .ok_or("bloque ausente")?;
    let mut batch = StorageBatch::new();
    let commitment = state
        .stage_transition(&mut batch, &transition, next)
        .map_err(|err| err.to_string())?;
    if commitment.previous_root() != root
        || commitment.previous_version() != version
        || commitment.new_version() != next
        || commitment.new_root() == root
    {
        return Err("transición distinta".into());
    }
    let mut block = Block {
        header: BlockHeader {
            version: parent.header.version,
            chain_id: parent.header.chain_id,
            height: parent
                .header
                .height
                .checked_next()
                .map_err(|err| err.to_string())?,
            previous_block_hash: tip,
            timestamp: parent.header.timestamp,
            transactions_root: Hash::ZERO,
            state_root: Hash(*commitment.new_root().as_bytes()),
            proposer: AccountId::ZERO,
        },
        transactions: vec![RawTransaction(pending.raw_transaction.clone())],
    };
    block.header.transactions_root = block.compute_transaction_root();
    let encoded = encode_block(&block).map_err(|err| err.to_string())?;
    validate_block(
        &block,
        &ChainValidationContext {
            expected_chain_id: parent.header.chain_id,
            expected_protocol_version: parent.header.version,
            max_block_size: encoded.len(),
            max_transactions_per_block: block.transaction_count(),
            parent: Some(parent.header.clone()),
        },
    )
    .map_err(|err| err.to_string())?;
    let mut ledger = rebuild_ledger(store)?;
    ledger.append(id).map_err(|err| err.to_string())?;
    let ledger_root = ledger.root();
    let hash = store
        .stage_canonical(&mut batch, &block)
        .map_err(|err| err.to_string())?;
    store
        .database()
        .write(batch)
        .map_err(|err| err.to_string())?;
    if store.get_canonical_tip().map_err(|err| err.to_string())? != Some(hash) {
        return Err("tip distinto".into());
    }
    if state.state_root().map_err(|err| err.to_string())? != commitment.new_root() {
        return Err("state root distinto".into());
    }
    if rebuild_ledger(store)?.root() != ledger_root {
        return Err("ledger distinto".into());
    }
    if let Some(plan) = fee_plan {
        let mut monetary = load_monetary_runtime(store.database().as_ref())?;
        monetary
            .settle_protocol_fee(plan)
            .map_err(|err| err.to_string())?;
        persist_monetary_runtime(store.database().as_ref(), &monetary)?;
    }
    pool.remove_committed(&[pending.hash]);
    Ok(())
}

fn commit_ready_amm(
    state: &State,
    store: &RocksDbChainStore,
    pool: &Mempool,
    transaction: &Transaction,
    raw_transaction: &[u8],
) -> Result<(), String> {
    let id = transaction.id();
    let version = state.version().map_err(|err| err.to_string())?;
    let next = version.checked_add(1).ok_or("versión de estado agotada")?;
    let tip = store
        .get_canonical_tip()
        .map_err(|err| err.to_string())?
        .ok_or("falta tip")?;
    let parent = store
        .get_block(&tip)
        .map_err(|err| err.to_string())?
        .ok_or("bloque ausente")?;
    let height = parent
        .header
        .height
        .checked_next()
        .map_err(|err| err.to_string())?;
    // Misma base que `state`; el stage usa un State hermano sobre el Arc compartido.
    let runtime = AmmRuntime::new(
        store.database(),
        State::open(store.database()).map_err(|err| err.to_string())?,
    );
    let mut batch = StorageBatch::new();
    let result = runtime
        .stage_transaction(
            &mut batch,
            transaction,
            BlockContext {
                height,
                new_state_version: next,
            },
        )
        .map_err(|err| err.to_string())?;
    let commitment = result.state_commitment;
    if commitment.previous_version() != version || commitment.new_version() != next {
        return Err("transición amm distinta".into());
    }
    // ConsumeNonce cambia la hoja de cuenta → raíz distinta.
    if commitment.new_root() == commitment.previous_root() {
        return Err("transición amm sin cambio de raíz".into());
    }
    let mut block = Block {
        header: BlockHeader {
            version: parent.header.version,
            chain_id: parent.header.chain_id,
            height,
            previous_block_hash: tip,
            timestamp: parent.header.timestamp,
            transactions_root: Hash::ZERO,
            state_root: Hash(*commitment.new_root().as_bytes()),
            proposer: AccountId::ZERO,
        },
        transactions: vec![RawTransaction(raw_transaction.to_vec())],
    };
    block.header.transactions_root = block.compute_transaction_root();
    let encoded = encode_block(&block).map_err(|err| err.to_string())?;
    validate_block(
        &block,
        &ChainValidationContext {
            expected_chain_id: parent.header.chain_id,
            expected_protocol_version: parent.header.version,
            max_block_size: encoded.len(),
            max_transactions_per_block: block.transaction_count(),
            parent: Some(parent.header.clone()),
        },
    )
    .map_err(|err| err.to_string())?;
    let mut ledger = rebuild_ledger(store)?;
    ledger.append(id).map_err(|err| err.to_string())?;
    let ledger_root = ledger.root();
    let hash = store
        .stage_canonical(&mut batch, &block)
        .map_err(|err| err.to_string())?;
    store
        .database()
        .write(batch)
        .map_err(|err| err.to_string())?;
    if store.get_canonical_tip().map_err(|err| err.to_string())? != Some(hash) {
        return Err("tip distinto".into());
    }
    if state.state_root().map_err(|err| err.to_string())? != commitment.new_root() {
        return Err("state root distinto".into());
    }
    if rebuild_ledger(store)?.root() != ledger_root {
        return Err("ledger distinto".into());
    }
    let pending_hash = rsc_core::TransactionId(Hash(*id.0.as_bytes()));
    pool.remove_committed(&[pending_hash]);
    Ok(())
}

fn creation_transaction(wallet: &rsc_wallet::Wallet) -> Result<WalletOpening, String> {
    let key = rsc_crypto::PrivateKey::from_bytes(&wallet.signing_key_bytes())
        .map_err(|err| err.to_string())?;
    let mut transaction = Transaction {
        version: TRANSACTION_VERSION_V1,
        sender: AccountId(*wallet.account_id()),
        nonce: 0,
        payload: TransactionPayload::CreateAccount {
            owner_key_id: wallet.key_id(),
        },
        signature: rsc_crypto::Signature::from_bytes(&[0u8; 64]).map_err(|err| err.to_string())?,
    };
    transaction.sign(&key).map_err(|err| err.to_string())?;
    Ok(WalletOpening {
        public_key: *wallet.public_key(),
        raw: transaction.canonical_bytes(),
    })
}

fn commit_created(
    state: &State,
    store: &RocksDbChainStore,
    opening: &WalletOpening,
) -> Result<(), String> {
    let public =
        rsc_crypto::PublicKey::from_bytes(&opening.public_key).map_err(|err| err.to_string())?;
    let transaction =
        Transaction::from_canonical_bytes(&opening.raw).map_err(|err| err.to_string())?;
    transaction
        .validate(&public)
        .map_err(|err| err.to_string())?;
    let TransactionPayload::CreateAccount { owner_key_id } = transaction.payload else {
        return Err("payload distinto".into());
    };
    if owner_key_id != public.key_id() {
        return Err("clave distinta".into());
    }
    let derived = rsc_state::derive_account_id(&owner_key_id);
    if transaction.sender.0 != *derived.as_bytes() || transaction.nonce != 0 {
        return Err("cuenta distinta".into());
    }
    let root = state.state_root().map_err(|err| err.to_string())?;
    let version = state.version().map_err(|err| err.to_string())?;
    let mut transition = StateTransition::new(root, version);
    transition.push(StateOperation::CreateAccount { owner_key_id });
    let next = version.checked_add(1).ok_or("versión de estado agotada")?;
    let tip = store
        .get_canonical_tip()
        .map_err(|err| err.to_string())?
        .ok_or("falta tip")?;
    let parent = store
        .get_block(&tip)
        .map_err(|err| err.to_string())?
        .ok_or("bloque ausente")?;
    let mut batch = StorageBatch::new();
    let commitment = state
        .stage_transition(&mut batch, &transition, next)
        .map_err(|err| err.to_string())?;
    if commitment.previous_root() != root
        || commitment.previous_version() != version
        || commitment.new_version() != next
        || commitment.new_root() == root
    {
        return Err("transición distinta".into());
    }
    let mut block = Block {
        header: BlockHeader {
            version: parent.header.version,
            chain_id: parent.header.chain_id,
            height: parent
                .header
                .height
                .checked_next()
                .map_err(|err| err.to_string())?,
            previous_block_hash: tip,
            timestamp: parent.header.timestamp,
            transactions_root: Hash::ZERO,
            state_root: Hash(*commitment.new_root().as_bytes()),
            proposer: AccountId::ZERO,
        },
        transactions: vec![RawTransaction(opening.raw.clone())],
    };
    block.header.transactions_root = block.compute_transaction_root();
    let encoded = encode_block(&block).map_err(|err| err.to_string())?;
    validate_block(
        &block,
        &ChainValidationContext {
            expected_chain_id: parent.header.chain_id,
            expected_protocol_version: parent.header.version,
            max_block_size: encoded.len(),
            max_transactions_per_block: block.transaction_count(),
            parent: Some(parent.header.clone()),
        },
    )
    .map_err(|err| err.to_string())?;
    let id = transaction.id();
    let mut ledger = rebuild_ledger(store)?;
    ledger.append(id).map_err(|err| err.to_string())?;
    let ledger_root = ledger.root();
    let hash = store
        .stage_canonical(&mut batch, &block)
        .map_err(|err| err.to_string())?;
    store
        .database()
        .write(batch)
        .map_err(|err| err.to_string())?;
    if store.get_canonical_tip().map_err(|err| err.to_string())? != Some(hash) {
        return Err("tip distinto".into());
    }
    if state.state_root().map_err(|err| err.to_string())? != commitment.new_root() {
        return Err("state root distinto".into());
    }
    if rebuild_ledger(store)?.root() != ledger_root {
        return Err("ledger distinto".into());
    }
    Ok(())
}

fn align_engine(
    state: &State,
    store: &RocksDbChainStore,
    consensus: &mut ConsensusEngine,
) -> Result<(), String> {
    let tip = store
        .get_canonical_tip()
        .map_err(|err| err.to_string())?
        .ok_or("falta tip")?;
    let block = store
        .get_block(&tip)
        .map_err(|err| err.to_string())?
        .ok_or("bloque ausente")?;
    let height = consensus.state().height;
    if block.header.height.0 != height {
        return Ok(());
    }
    let phase = consensus.state().phase;
    let view = consensus.state().view;
    let proposal = consensus.state().proposal;
    let locked = consensus.state().locked;
    if phase != ConsensusPhase::NewView || view != 0 || proposal.is_some() || locked.is_some() {
        return Err("fase distinta".into());
    }
    propose_tip(store, consensus)?;
    let prepare = cast_local_vote(store, consensus, VotePhase::Prepare)?;
    accept_prepare_quorum(consensus, &prepare)?;
    let precommit = cast_local_vote(store, consensus, VotePhase::PreCommit)?;
    accept_precommit_quorum(consensus, &precommit)?;
    let commit = cast_local_vote(store, consensus, VotePhase::Commit)?;
    let proof = accept_commit(consensus, &commit)?;
    apply_finalized(state, store, consensus, &proof)?;
    if store
        .get_block_by_height(BlockHeight(consensus.state().height))
        .map_err(|err| err.to_string())?
        .is_some()
    {
        return Err("bloque de más".into());
    }
    Ok(())
}

fn resume(
    state: &State,
    store: &RocksDbChainStore,
    pool: &Mempool,
    consensus: &mut ConsensusEngine,
) -> Result<(), String> {
    propose_tip(store, consensus)?;
    let prepare = cast_local_vote(store, consensus, VotePhase::Prepare)?;
    accept_prepare_quorum(consensus, &prepare)?;
    let precommit = cast_local_vote(store, consensus, VotePhase::PreCommit)?;
    accept_precommit_quorum(consensus, &precommit)?;
    let commit = cast_local_vote(store, consensus, VotePhase::Commit)?;
    let proof = accept_commit(consensus, &commit)?;
    apply_finalized(state, store, consensus, &proof)?;
    commit_ready(state, store, pool)?;
    align_engine(state, store, consensus)?;
    Ok(())
}

/// Sella el bloque que el motor ya está esperando y lo finaliza en local.
///
/// Con una wallet pendiente, el bloque abre esa cuenta. Si no, entra una
/// transferencia Ready, o el bloque queda vacío. El llamador es `serve_rpc`.
fn produce_block(
    state: &State,
    store: &RocksDbChainStore,
    pool: &Mempool,
    consensus: &mut ConsensusEngine,
    wallets: &Mutex<VecDeque<WalletOpening>>,
) -> Result<u64, String> {
    let engine = consensus.state();
    if engine.phase != ConsensusPhase::NewView
        || engine.view != 0
        || engine.proposal.is_some()
        || engine.locked.is_some()
    {
        return Err("fase distinta".into());
    }
    let engine_height = engine.height;
    let tip = store
        .get_canonical_tip()
        .map_err(|err| err.to_string())?
        .ok_or("falta tip")?;
    let parent = store
        .get_block(&tip)
        .map_err(|err| err.to_string())?
        .ok_or("bloque ausente")?;
    let parent_height = parent.header.height.0;
    if parent_height + 1 == engine_height {
        let opening = wallets
            .lock()
            .map_err(|err| err.to_string())?
            .front()
            .cloned();
        if let Some(opening) = opening {
            match commit_created(state, store, &opening) {
                Ok(()) => {
                    wallets.lock().map_err(|err| err.to_string())?.pop_front();
                }
                Err(err) if err.contains("account already exists") => {
                    wallets.lock().map_err(|err| err.to_string())?.pop_front();
                    seal_pending(state, store, pool)?;
                }
                Err(err) => return Err(err),
            }
        } else {
            seal_pending(state, store, pool)?;
        }
    } else if parent_height != engine_height {
        return Err("altura distinta".into());
    }
    let sealed_hash = store
        .get_canonical_tip()
        .map_err(|err| err.to_string())?
        .ok_or("falta tip")?;
    let sealed = store
        .get_block(&sealed_hash)
        .map_err(|err| err.to_string())?
        .ok_or("bloque ausente")?;
    if sealed.header.height.0 != engine_height {
        return Err("altura distinta".into());
    }
    align_engine(state, store, consensus)?;
    Ok(engine_height)
}

fn seal_pending(state: &State, store: &RocksDbChainStore, pool: &Mempool) -> Result<(), String> {
    if pool.ready_transactions(1).is_empty() {
        seal_empty(state, store)
    } else {
        commit_ready(state, store, pool)
    }
}

fn seal_empty(state: &State, store: &RocksDbChainStore) -> Result<(), String> {
    let root = state.state_root().map_err(|err| err.to_string())?;
    let version = state.version().map_err(|err| err.to_string())?;
    let ledger_root = rebuild_ledger(store)?.root();
    let tip = store
        .get_canonical_tip()
        .map_err(|err| err.to_string())?
        .ok_or("falta tip")?;
    let parent = store
        .get_block(&tip)
        .map_err(|err| err.to_string())?
        .ok_or("bloque ausente")?;
    let mut block = Block {
        header: BlockHeader {
            version: parent.header.version,
            chain_id: parent.header.chain_id,
            height: parent
                .header
                .height
                .checked_next()
                .map_err(|err| err.to_string())?,
            previous_block_hash: tip,
            timestamp: next_timestamp(parent.header.timestamp),
            transactions_root: Hash::ZERO,
            state_root: Hash(*root.as_bytes()),
            proposer: AccountId::ZERO,
        },
        transactions: Vec::new(),
    };
    block.header.transactions_root = block.compute_transaction_root();
    let encoded = encode_block(&block).map_err(|err| err.to_string())?;
    validate_block(
        &block,
        &ChainValidationContext {
            expected_chain_id: parent.header.chain_id,
            expected_protocol_version: parent.header.version,
            max_block_size: encoded.len(),
            max_transactions_per_block: block.transaction_count(),
            parent: Some(parent.header.clone()),
        },
    )
    .map_err(|err| err.to_string())?;
    let mut batch = StorageBatch::new();
    let hash = store
        .stage_canonical(&mut batch, &block)
        .map_err(|err| err.to_string())?;
    store
        .database()
        .write(batch)
        .map_err(|err| err.to_string())?;
    if store.get_canonical_tip().map_err(|err| err.to_string())? != Some(hash) {
        return Err("tip distinto".into());
    }
    if state.state_root().map_err(|err| err.to_string())? != root
        || state.version().map_err(|err| err.to_string())? != version
    {
        return Err("state root distinto".into());
    }
    if rebuild_ledger(store)?.root() != ledger_root {
        return Err("ledger distinto".into());
    }
    Ok(())
}

fn next_timestamp(parent: Timestamp) -> Timestamp {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(parent.0);
    Timestamp(now.max(parent.0))
}

fn propose_tip(store: &RocksDbChainStore, consensus: &mut ConsensusEngine) -> Result<(), String> {
    let tip = store
        .get_canonical_tip()
        .map_err(|err| err.to_string())?
        .ok_or("falta tip")?;
    let block = store
        .get_block(&tip)
        .map_err(|err| err.to_string())?
        .ok_or("bloque ausente")?;
    let event = consensus
        .receive_proposal(&block, None)
        .map_err(|err| err.to_string())?;
    if event != ConsensusEvent::ProposalAccepted {
        return Err("propuesta no aceptada".into());
    }
    Ok(())
}

fn cast_local_vote(
    store: &RocksDbChainStore,
    consensus: &mut ConsensusEngine,
    phase: VotePhase,
) -> Result<Vote, String> {
    let expected = match phase {
        VotePhase::Prepare => ConsensusPhase::Propose,
        VotePhase::PreCommit => ConsensusPhase::Prepare,
        VotePhase::Commit => ConsensusPhase::PreCommit,
    };
    let tip = store
        .get_canonical_tip()
        .map_err(|err| err.to_string())?
        .ok_or("falta tip")?;
    let vote = consensus
        .create_vote(phase, &node_bootstrap_signer())
        .map_err(|err| err.to_string())?;
    let state = consensus.state();
    let lock_ok = match phase {
        VotePhase::Commit => state.locked == Some(tip),
        VotePhase::Prepare | VotePhase::PreCommit => state.locked.is_none(),
    };
    if vote.block_hash != tip
        || state.proposal != Some(tip)
        || vote.epoch != state.epoch
        || vote.height != state.height
        || vote.view != state.view
        || vote.phase != phase
        || state.phase != expected
        || !lock_ok
    {
        return Err("voto distinto".into());
    }
    verify_vote_signature(&vote)?;
    Ok(vote)
}

fn verify_vote_signature(vote: &Vote) -> Result<(), String> {
    if vote.validator != AccountId::ZERO {
        return Err("invalid signature".into());
    }
    let key = node_bootstrap_signer();
    let signature =
        rsc_crypto::Signature::from_bytes(&vote.signature.0).map_err(|err| err.to_string())?;
    rsc_crypto::verify(
        &key.public_key(),
        rsc_crypto::CryptoDomain::ConsensusVote,
        &rsc_consensus::finality::vote_bytes(vote),
        &signature,
    )
    .map_err(|err| err.to_string())
}

fn accept_prepare_quorum(
    consensus: &mut ConsensusEngine,
    vote: &Vote,
) -> Result<QuorumCertificate, String> {
    let event = consensus
        .receive_vote(vote.clone())
        .map_err(|err| err.to_string())?;
    if event != ConsensusEvent::QuorumReached(VotePhase::Prepare) {
        return Err("quorum distinto".into());
    }
    {
        let state = consensus.state();
        if state.phase != ConsensusPhase::Prepare
            || state.locked.is_some()
            || state.proposal != Some(vote.block_hash)
            || state.height != vote.height
            || state.view != vote.view
        {
            return Err("fase distinta".into());
        }
    }
    checked_certificate(consensus, vote, VotePhase::Prepare)
}

fn accept_precommit_quorum(
    consensus: &mut ConsensusEngine,
    vote: &Vote,
) -> Result<QuorumCertificate, String> {
    let event = consensus
        .receive_vote(vote.clone())
        .map_err(|err| err.to_string())?;
    if event != ConsensusEvent::QuorumReached(VotePhase::PreCommit) {
        return Err("quorum distinto".into());
    }
    {
        let state = consensus.state();
        if state.phase != ConsensusPhase::PreCommit
            || state.locked != Some(vote.block_hash)
            || state.proposal != Some(vote.block_hash)
            || state.height != vote.height
            || state.view != vote.view
        {
            return Err("fase distinta".into());
        }
    }
    checked_certificate(consensus, vote, VotePhase::PreCommit)
}

fn accept_commit(consensus: &mut ConsensusEngine, vote: &Vote) -> Result<FinalityProof, String> {
    let event = consensus
        .receive_vote(vote.clone())
        .map_err(|err| err.to_string())?;
    let ConsensusEvent::BlockFinalized(proof) = event else {
        return Err("finality distinta".into());
    };
    {
        let state = consensus.state();
        if state.phase != ConsensusPhase::Decide
            || state.locked != Some(vote.block_hash)
            || state.proposal != Some(vote.block_hash)
            || state.height != vote.height
            || state.view != vote.view
        {
            return Err("fase distinta".into());
        }
    }
    checked_certificate(consensus, vote, VotePhase::Commit)?;
    rsc_consensus::verify_commit_chain(&proof, &consensus.state().committee)
        .map_err(|err| err.to_string())?;
    if proof.block_hash != vote.block_hash
        || proof.height != vote.height
        || proof.view != vote.view
        || proof.commit.phase != VotePhase::Commit
    {
        return Err("finality distinta".into());
    }
    Ok(proof)
}

fn apply_finalized(
    state: &State,
    store: &RocksDbChainStore,
    consensus: &mut ConsensusEngine,
    proof: &FinalityProof,
) -> Result<(), String> {
    let tip = store
        .get_canonical_tip()
        .map_err(|err| err.to_string())?
        .ok_or("falta tip")?;
    if proof.block_hash != tip {
        return Err("finality distinta".into());
    }
    let block = store
        .get_block(&tip)
        .map_err(|err| err.to_string())?
        .ok_or("bloque ausente")?;
    if block.header.height.0 != proof.height {
        return Err("altura distinta".into());
    }
    let root = state.state_root().map_err(|err| err.to_string())?;
    if block.header.state_root != Hash(*root.as_bytes()) {
        return Err("state root distinto".into());
    }
    let ledger_root = rebuild_ledger(store)?.root();
    let next = proof.height.checked_add(1).ok_or("altura distinta")?;
    let event = consensus
        .new_height(None, Hash::ZERO)
        .map_err(|err| err.to_string())?;
    if !matches!(
        event,
        ConsensusEvent::EpochChanged { .. } | ConsensusEvent::ViewChanged { .. }
    ) {
        return Err("altura distinta".into());
    }
    let engine = consensus.state();
    if engine.height != next
        || engine.phase != ConsensusPhase::NewView
        || engine.view != 0
        || engine.proposal.is_some()
        || engine.locked.is_some()
    {
        return Err("fase distinta".into());
    }
    if store.get_canonical_tip().map_err(|err| err.to_string())? != Some(tip) {
        return Err("tip distinto".into());
    }
    if state.state_root().map_err(|err| err.to_string())? != root {
        return Err("state root distinto".into());
    }
    if rebuild_ledger(store)?.root() != ledger_root {
        return Err("ledger distinto".into());
    }
    store_finality(store, proof)?;
    Ok(())
}

fn store_finality(store: &RocksDbChainStore, proof: &FinalityProof) -> Result<(), String> {
    store
        .database()
        .put(ColumnFamily::Default, FINALITY_KEY, &encode_proof(proof)?)
        .map_err(|err| err.to_string())
}

fn load_finality(
    store: &RocksDbChainStore,
    committee: &rsc_consensus::Committee,
) -> Result<Option<RpcFinality>, ServiceError> {
    let stored = store
        .database()
        .get(ColumnFamily::Default, FINALITY_KEY)
        .map_err(|_| ServiceError::Internal)?;
    let Some(stored) = stored else {
        return Ok(None);
    };
    let proof = decode_proof(stored.as_bytes()).map_err(|_| ServiceError::Internal)?;
    rsc_consensus::verify_commit_chain(&proof, committee).map_err(|_| ServiceError::Internal)?;
    Ok(Some(RpcFinality {
        finalized_height: proof.height,
        finalized_hash: encode_hex(&proof.block_hash.0 .0),
        epoch: proof.prepare.epoch,
        view: proof.view,
        prepare: encode_hex(&encode_qc(&proof.prepare).map_err(|_| ServiceError::Internal)?),
        precommit: encode_hex(&encode_qc(&proof.precommit).map_err(|_| ServiceError::Internal)?),
        commit: encode_hex(&encode_qc(&proof.commit).map_err(|_| ServiceError::Internal)?),
    }))
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
    let block_hash = BlockHash(Hash(read_array(&mut body)?));
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
        let id = AccountId(read_array(&mut body)?);
        let power = u128::from_le_bytes(read_array(&mut body)?);
        voters.push((id, power));
    }
    let sig_len = u32::from_le_bytes(read_array(&mut body)?) as usize;
    if body.len() != sig_len {
        return Err("invalid finality".into());
    }
    let signatures =
        rsc_crypto::SignatureBundle::from_bytes(body).map_err(|err| err.to_string())?;
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

fn checked_certificate(
    consensus: &ConsensusEngine,
    vote: &Vote,
    phase: VotePhase,
) -> Result<QuorumCertificate, String> {
    let state = consensus.state();
    let (certificate, equivocation) =
        rsc_consensus::finality::aggregate_qc(std::slice::from_ref(vote), &state.committee, phase)
            .map_err(|err| err.to_string())?;
    if equivocation.is_some() {
        return Err("equivocación".into());
    }
    let certificate = certificate.ok_or("falta qc")?;
    rsc_consensus::verify_qc(&certificate, &state.committee).map_err(|err| err.to_string())?;
    if certificate.block_hash != vote.block_hash
        || certificate.phase != phase
        || certificate.height != vote.height
        || certificate.view != vote.view
        || certificate.epoch != vote.epoch
    {
        return Err("qc distinto".into());
    }
    Ok(certificate)
}

struct ChainBody {
    state: State,
    store: RocksDbChainStore,
    consensus: ConsensusEngine,
}

struct NodeRpc {
    network_id: String,
    chain_id: u32,
    chain: Arc<Mutex<ChainBody>>,
    pool: Arc<Mempool>,
    wallets: Arc<Mutex<VecDeque<WalletOpening>>>,
    /// Directorio discovery: self + seeds (+ file). Sin API fija obligatoria.
    rpc_endpoints: Vec<RpcEndpoint>,
}

#[cfg(test)]
fn rpc_context(
    config: &Config,
    state: State,
    store: RocksDbChainStore,
    consensus: ConsensusEngine,
    pool: Arc<Mempool>,
) -> RpcContext {
    shared_chain(config, state, store, consensus, pool).0
}

fn shared_chain(
    config: &Config,
    state: State,
    store: RocksDbChainStore,
    consensus: ConsensusEngine,
    pool: Arc<Mempool>,
) -> (
    RpcContext,
    Arc<Mutex<ChainBody>>,
    Arc<Mutex<VecDeque<WalletOpening>>>,
) {
    let chain = Arc::new(Mutex::new(ChainBody {
        state,
        store,
        consensus,
    }));
    let wallets = Arc::new(Mutex::new(VecDeque::new()));
    let rpc_endpoints = build_rpc_directory(config);
    let node = Arc::new(NodeRpc {
        network_id: config.network_id.clone(),
        chain_id: config.chain_id,
        chain: Arc::clone(&chain),
        pool,
        wallets: Arc::clone(&wallets),
        rpc_endpoints,
    });
    let context = RpcContext::new(
        Arc::clone(&node) as Arc<dyn NodeService>,
        Arc::clone(&node) as Arc<dyn ChainService>,
        Arc::clone(&node) as Arc<dyn TransactionService>,
        Arc::clone(&node) as Arc<dyn StateService>,
        Arc::clone(&node) as Arc<dyn ConsensusService>,
    )
    .with_amm(Arc::clone(&node) as Arc<dyn AmmService>);
    #[cfg(feature = "amm-admin-rpc")]
    let context = match &config.amm_admin_token {
        Some(token) => context.with_amm_admin_token(token.clone()),
        None => context,
    };
    #[cfg(not(feature = "amm-admin-rpc"))]
    let _ = &config.amm_admin_token;
    (context, chain, wallets)
}

fn p2p_config(config: &Config) -> Result<NetworkConfig, String> {
    let network = NetworkConfig {
        network_id: config.network_id.clone(),
        chain_id: rsc_core::ChainId(config.chain_id),
        discovery: DiscoveryConfig::for_network(&config.network_id, Vec::new())
            .map_err(|err| err.to_string())?,
        ..NetworkConfig::default()
    };
    network.validate().map_err(|err| err.to_string())?;
    Ok(network)
}

async fn start_p2p(config: NetworkConfig) -> Result<RscNetwork, String> {
    let (_, keypair) = LocalPeerIdentity::generate();
    let mut network = RscNetwork::start(config, keypair)
        .await
        .map_err(|err| err.to_string())?;
    let mut listening = false;
    let mut bound = false;
    while !listening || !bound {
        match network.next_event().await {
            Some(NetworkEvent::ListeningStarted) => listening = true,
            Some(NetworkEvent::ListeningAddressAdded(_)) => bound = true,
            Some(_) => {}
            None => return Err("p2p cerrado antes de escuchar".into()),
        }
    }
    Ok(network)
}

async fn stop_p2p(network: &mut RscNetwork) -> Result<(), String> {
    network.handle().shutdown().map_err(|err| err.to_string())?;
    while network.next_event().await.is_some() {}
    Ok(())
}

/// `0.0.0.0` / `::` no aceptan un connect de prueba. Se sondea loopback en el mismo puerto.
fn rpc_probe_addr(bind: SocketAddr) -> SocketAddr {
    match bind.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => {
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), bind.port())
        }
        IpAddr::V6(ip) if ip.is_unspecified() => {
            SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), bind.port())
        }
        _ => bind,
    }
}

async fn wait_rpc(
    rpc: &mut tokio::task::JoinHandle<Result<(), String>>,
    bind: SocketAddr,
) -> Result<(), String> {
    let probe = rpc_probe_addr(bind);
    loop {
        if tokio::net::TcpStream::connect(probe).await.is_ok() {
            return Ok(());
        }
        if rpc.is_finished() {
            return match rpc.await {
                Ok(Ok(())) => Err("rpc cerrado antes de escuchar".into()),
                Ok(Err(err)) => Err(err),
                Err(err) => Err(err.to_string()),
            };
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

async fn process_shutdown() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            signal.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {}
        _ = term => {}
    }
}

fn serve_rpc(
    config: &Config,
    state: State,
    store: RocksDbChainStore,
    consensus: ConsensusEngine,
    pool: Arc<Mempool>,
) -> Result<(), String> {
    let p2p = p2p_config(config)?;
    let rpc_config = RpcConfig {
        bind_address: config.rpc_bind,
        allowed_origins: config.rpc_cors.clone(),
        ..RpcConfig::default()
    };
    let bind = rpc_config.bind_address;
    let (context, chain, wallets) =
        shared_chain(config, state, store, consensus, Arc::clone(&pool));
    let server = RpcServer::new(context, rpc_config);
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|err| err.to_string())?
        .block_on(async move {
            let mut rpc =
                tokio::spawn(async move { server.run().await.map_err(|err| err.to_string()) });
            wait_rpc(&mut rpc, bind).await?;
            eprintln!("rpc bind {bind} GET /health GET /network GET /swap POST /rpc");
            let mut network = match start_p2p(p2p).await {
                Ok(network) => network,
                Err(err) => {
                    rpc.abort();
                    return Err(err);
                }
            };
            let mut tick =
                tokio::time::interval(std::time::Duration::from_secs(BLOCK_INTERVAL_SECS));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    result = &mut rpc => {
                        let rpc_result = join_rpc(result);
                        stop_p2p(&mut network).await?;
                        return rpc_result;
                    }
                    _ = process_shutdown() => {
                        stop_p2p(&mut network).await?;
                        return join_rpc(rpc.await);
                    }
                    _ = tick.tick() => {
                        let outcome = {
                            let mut body = match chain.lock() {
                                Ok(body) => body,
                                Err(err) => return Err(err.to_string()),
                            };
                            let ChainBody {
                                state,
                                store,
                                consensus,
                            } = &mut *body;
                            produce_block(state, store, &pool, consensus, &wallets)
                        };
                        match outcome {
                            Ok(height) => {
                                println!("bloque {height}");
                                let _ = std::io::Write::flush(&mut std::io::stdout());
                            }
                            Err(err) => eprintln!("bloque: {err}"),
                        }
                    }
                }
            }
        })
}

fn join_rpc(result: Result<Result<(), String>, tokio::task::JoinError>) -> Result<(), String> {
    match result {
        Ok(result) => result,
        Err(err) => Err(err.to_string()),
    }
}

fn ready<T>(value: T) -> ServiceFut<'static, T>
where
    T: Send + 'static,
{
    Box::pin(async move { Ok(value) })
}

fn failed<T>(err: ServiceError) -> ServiceFut<'static, T>
where
    T: Send + 'static,
{
    Box::pin(async move { Err(err) })
}

fn tip_block(store: &RocksDbChainStore) -> Result<Block, ServiceError> {
    let tip = store
        .get_canonical_tip()
        .map_err(|_| ServiceError::Internal)?
        .ok_or(ServiceError::Unavailable)?;
    store
        .get_block(&tip)
        .map_err(|_| ServiceError::Internal)?
        .ok_or(ServiceError::NotFound)
}

fn hash_hex(hash: &BlockHash) -> String {
    encode_hex(&hash.0 .0)
}

fn rpc_block(block: &Block) -> Result<RpcBlock, ServiceError> {
    let hash = block.block_hash().map_err(|_| ServiceError::Internal)?;
    let mut transactions = Vec::with_capacity(block.transactions.len());
    for raw in &block.transactions {
        let transaction =
            Transaction::from_canonical_bytes(&raw.0).map_err(|_| ServiceError::Internal)?;
        transactions.push(encode_hex(transaction.id().0.as_bytes()));
    }
    Ok(RpcBlock {
        hash: hash_hex(&hash),
        height: block.header.height.0,
        parent_hash: hash_hex(&block.header.previous_block_hash),
        timestamp: block.header.timestamp.0,
        transaction_root: encode_hex(&block.header.transactions_root.0),
        transactions,
    })
}

impl NodeRpc {
    fn lock_chain(&self) -> Result<std::sync::MutexGuard<'_, ChainBody>, ServiceError> {
        self.chain.lock().map_err(|_| ServiceError::Internal)
    }

    fn open_amm_runtime(
        &self,
    ) -> Result<(std::sync::MutexGuard<'_, ChainBody>, AmmRuntime), ServiceError> {
        let chain = self.lock_chain()?;
        let db = chain.store.database();
        let state = State::open(Arc::clone(&db)).map_err(|_| ServiceError::Internal)?;
        let runtime = AmmRuntime::new(db, state);
        Ok((chain, runtime))
    }

    fn read_assets(&self) -> Result<Vec<RpcAsset>, ServiceError> {
        let (_chain, runtime) = self.open_amm_runtime()?;
        let ledger = runtime.load_assets().map_err(|_| ServiceError::Internal)?;
        Ok(ledger
            .registered_assets()
            .map(|(id, meta)| RpcAsset {
                asset_id: encode_hex(id.as_bytes()),
                decimals: meta.decimals,
                symbol: asset_symbol(&meta.symbol),
            })
            .collect())
    }

    fn read_asset_balance(&self, account: &str, asset_id: &str) -> Result<String, ServiceError> {
        let account = parse_account_id(account)?;
        let asset = parse_assets_asset_id(asset_id)?;
        let (_chain, runtime) = self.open_amm_runtime()?;
        let ledger = runtime.load_assets().map_err(|_| ServiceError::Internal)?;
        if !ledger.is_registered(&asset) {
            return Err(ServiceError::InvalidState);
        }
        Ok(ledger.balance_of(&account, &asset).0.to_string())
    }

    fn read_asset_balances(
        &self,
        account: &str,
    ) -> Result<std::collections::BTreeMap<String, String>, ServiceError> {
        let account = parse_account_id(account)?;
        let (_chain, runtime) = self.open_amm_runtime()?;
        let ledger = runtime.load_assets().map_err(|_| ServiceError::Internal)?;
        let mut out = std::collections::BTreeMap::new();
        for (id, _) in ledger.registered_assets() {
            let bal = ledger.balance_of(&account, &id);
            out.insert(encode_hex(id.as_bytes()), bal.0.to_string());
        }
        Ok(out)
    }

    fn read_pools(&self) -> Result<Vec<RpcPool>, ServiceError> {
        let (_chain, runtime) = self.open_amm_runtime()?;
        let store = runtime.load_store().map_err(|_| ServiceError::Internal)?;
        let assets = runtime.load_assets().map_err(|_| ServiceError::Internal)?;
        let mut out = Vec::new();
        for pool_id in store.pools.list_pools() {
            let pool = store.pools.get_pool(&pool_id).map_err(map_amm_read_err)?;
            out.push(rpc_pool_from(pool, &assets)?);
        }
        Ok(out)
    }

    fn read_pool(&self, pool_id: &str) -> Result<RpcPool, ServiceError> {
        let pool_id = parse_pool_id(pool_id)?;
        let (_chain, runtime) = self.open_amm_runtime()?;
        let store = runtime.load_store().map_err(|_| ServiceError::Internal)?;
        let assets = runtime.load_assets().map_err(|_| ServiceError::Internal)?;
        let pool = store.pools.get_pool(&pool_id).map_err(map_amm_read_err)?;
        rpc_pool_from(pool, &assets)
    }

    fn read_quote_swap(
        &self,
        pool_id: &str,
        token_in: &str,
        token_out: &str,
        amount_in: Option<&str>,
        amount_out: Option<&str>,
    ) -> Result<RpcSwapQuote, ServiceError> {
        let pool_id = parse_pool_id(pool_id)?;
        let token_in = parse_amm_asset_id(token_in)?;
        let token_out = parse_amm_asset_id(token_out)?;
        let (chain, runtime) = self.open_amm_runtime()?;
        let tip = tip_block(&chain.store)?;
        let quote_block = tip.header.height;
        let store = runtime.load_store().map_err(|_| ServiceError::Internal)?;
        let quote = match (amount_in, amount_out) {
            (Some(amount_in), None) => {
                let amount_in = parse_amount(amount_in)?;
                rsc_amm::get_quote(
                    &store.pools,
                    pool_id,
                    token_in,
                    token_out,
                    amount_in,
                    quote_block,
                    None,
                )
            }
            (None, Some(amount_out)) => {
                let amount_out = parse_amount(amount_out)?;
                rsc_amm::get_quote_exact_out(
                    &store.pools,
                    pool_id,
                    token_in,
                    token_out,
                    amount_out,
                    quote_block,
                    None,
                )
            }
            _ => return Err(ServiceError::InvalidState),
        }
        .map_err(map_amm_read_err)?;
        let assets = runtime.load_assets().map_err(|_| ServiceError::Internal)?;
        let decimals_in = assets
            .decimals(&rsc_amm::native::assets_asset_id(token_in))
            .map_err(|_| ServiceError::InvalidState)?;
        let decimals_out = assets
            .decimals(&rsc_amm::native::assets_asset_id(token_out))
            .map_err(|_| ServiceError::InvalidState)?;
        let pool = store.pools.get_pool(&pool_id).map_err(map_amm_read_err)?;
        let path = rsc_amm::resolve_path(pool, token_in, token_out).map_err(map_amm_read_err)?;
        let spot_price_normalized = rsc_amm::calculate_spot_price_normalized(
            path.reserve_in,
            path.reserve_out,
            decimals_in,
            decimals_out,
        )
        .map_err(map_amm_read_err)?;
        Ok(RpcSwapQuote {
            pool_id: encode_hex(&quote.pool_id.0 .0),
            token_in: encode_hex(&quote.token_in.0 .0),
            token_out: encode_hex(&quote.token_out.0 .0),
            amount_in: quote.amount_in.0.to_string(),
            amount_out: quote.amount_out.0.to_string(),
            fee_amount: quote.fee_amount.0.to_string(),
            execution_price: quote.execution_price.to_string(),
            spot_price: quote.spot_price.to_string(),
            spot_price_normalized: spot_price_normalized.to_string(),
            price_impact_bps: quote.price_impact_bps.to_string(),
            decimals_in,
            decimals_out,
            quote_block: quote.quote_block.0,
        })
    }

    fn read_quote_add_liquidity(
        &self,
        pool_id: &str,
        max_amount_a: &str,
        max_amount_b: &str,
    ) -> Result<RpcAddLiquidityQuote, ServiceError> {
        let pool_id = parse_pool_id(pool_id)?;
        let max_a = parse_amount(max_amount_a)?;
        let max_b = parse_amount(max_amount_b)?;
        let (_chain, runtime) = self.open_amm_runtime()?;
        let store = runtime.load_store().map_err(|_| ServiceError::Internal)?;
        store
            .pools
            .ensure_mint_allowed(&pool_id)
            .map_err(map_amm_read_err)?;
        let pool = store.pools.get_pool(&pool_id).map_err(map_amm_read_err)?;
        let (amount_a, amount_b) = rsc_amm::quote_balanced_deposit(max_a, max_b, pool.reserves)
            .map_err(map_amm_read_err)?;
        Ok(RpcAddLiquidityQuote {
            pool_id: encode_hex(&pool_id.0 .0),
            amount_a: amount_a.0.to_string(),
            amount_b: amount_b.0.to_string(),
        })
    }

    fn read_lp_position(
        &self,
        pool_id: &str,
        account: &str,
    ) -> Result<RpcLpPosition, ServiceError> {
        let pool_id = parse_pool_id(pool_id)?;
        let account = parse_account_id(account)?;
        let (_chain, runtime) = self.open_amm_runtime()?;
        let store = runtime.load_store().map_err(|_| ServiceError::Internal)?;
        // Pool inexistente → -32000; saldo cero en pool existente es válido.
        store.pools.get_pool(&pool_id).map_err(map_amm_read_err)?;
        let lp = rsc_amm::get_user_lp_balance(&store.accounting, pool_id, account);
        Ok(RpcLpPosition {
            pool_id: encode_hex(&pool_id.0 .0),
            account: encode_hex(&account.0),
            lp_shares: lp.0.to_string(),
        })
    }

    fn host_register_asset(
        &self,
        asset_id: &str,
        decimals: u8,
        symbol: &str,
    ) -> Result<(), ServiceError> {
        let asset = parse_amm_asset_id(asset_id)?;
        let (_chain, runtime) = self.open_amm_runtime()?;
        runtime
            .register_asset(asset, decimals, symbol)
            .map_err(map_native_amm_err)
    }

    fn host_credit_asset(
        &self,
        account: &str,
        asset_id: &str,
        amount: &str,
    ) -> Result<(), ServiceError> {
        let account = parse_account_id(account)?;
        let asset = parse_amm_asset_id(asset_id)?;
        let amount = amount
            .parse::<u128>()
            .map_err(|_| ServiceError::InvalidState)
            .map(rsc_core::Amount)?;
        let (_chain, runtime) = self.open_amm_runtime()?;
        runtime
            .credit_asset(account, asset, amount)
            .map_err(map_native_amm_err)
    }
}

fn asset_symbol(raw: &[u8; 8]) -> String {
    let end = raw.iter().position(|&b| b == 0).unwrap_or(8);
    String::from_utf8_lossy(&raw[..end]).into_owned()
}

fn parse_account_id(hex: &str) -> Result<AccountId, ServiceError> {
    let bytes = decode_hex(hex).map_err(|_| ServiceError::InvalidState)?;
    let raw = <[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| ServiceError::InvalidState)?;
    Ok(AccountId(raw))
}

fn parse_amm_asset_id(hex: &str) -> Result<rsc_amm::AssetId, ServiceError> {
    let bytes = decode_hex(hex).map_err(|_| ServiceError::InvalidState)?;
    let raw = <[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| ServiceError::InvalidState)?;
    Ok(rsc_amm::AssetId(Hash(raw)))
}

fn parse_assets_asset_id(hex: &str) -> Result<rsc_assets::AssetId, ServiceError> {
    let bytes = decode_hex(hex).map_err(|_| ServiceError::InvalidState)?;
    rsc_assets::AssetId::from_slice(&bytes).map_err(|_| ServiceError::InvalidState)
}

fn parse_pool_id(hex: &str) -> Result<rsc_amm::PoolId, ServiceError> {
    let bytes = decode_hex(hex).map_err(|_| ServiceError::InvalidState)?;
    let raw = <[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| ServiceError::InvalidState)?;
    Ok(rsc_amm::PoolId(Hash(raw)))
}

fn parse_amount(s: &str) -> Result<rsc_core::Amount, ServiceError> {
    s.parse::<u128>()
        .map_err(|_| ServiceError::InvalidState)
        .map(rsc_core::Amount)
}

fn pool_status_str(status: rsc_amm::PoolStatus) -> &'static str {
    match status {
        rsc_amm::PoolStatus::Created => "created",
        rsc_amm::PoolStatus::Active => "active",
        rsc_amm::PoolStatus::Paused => "paused",
    }
}

fn rpc_pool_from(
    pool: &rsc_amm::LiquidityPool,
    assets: &rsc_assets::AssetLedger,
) -> Result<RpcPool, ServiceError> {
    let asset_a = rsc_amm::native::assets_asset_id(pool.identity.asset_a);
    let asset_b = rsc_amm::native::assets_asset_id(pool.identity.asset_b);
    let decimals_a = assets
        .decimals(&asset_a)
        .map_err(|_| ServiceError::InvalidState)?;
    let decimals_b = assets
        .decimals(&asset_b)
        .map_err(|_| ServiceError::InvalidState)?;
    Ok(RpcPool {
        pool_id: encode_hex(&pool.pool_id.0 .0),
        asset_a: encode_hex(&pool.identity.asset_a.0 .0),
        asset_b: encode_hex(&pool.identity.asset_b.0 .0),
        fee_bps: pool.identity.fee_bps.get(),
        status: pool_status_str(pool.status).to_owned(),
        reserve_a: pool.reserves.reserve_a.0.to_string(),
        reserve_b: pool.reserves.reserve_b.0.to_string(),
        total_supply: pool.total_supply.0.to_string(),
        locked_supply: pool.locked_supply.0.to_string(),
        decimals_a,
        decimals_b,
    })
}

fn map_amm_read_err(err: rsc_amm::AmmError) -> ServiceError {
    match err {
        rsc_amm::AmmError::PoolNotFound => ServiceError::NotFound,
        rsc_amm::AmmError::SwapNotPermitted
        | rsc_amm::AmmError::MintNotPermitted
        | rsc_amm::AmmError::BurnNotPermitted
        | rsc_amm::AmmError::InvalidPoolStatus
        | rsc_amm::AmmError::InvalidReserves
        | rsc_amm::AmmError::InsufficientLiquidity
        | rsc_amm::AmmError::ZeroAmount
        | rsc_amm::AmmError::InvalidPair
        | rsc_amm::AmmError::UnbalancedDeposit
        | rsc_amm::AmmError::AssetNotRegistered
        | rsc_amm::AmmError::ZeroSupply
        | rsc_amm::AmmError::Overflow
        | rsc_amm::AmmError::Underflow
        | rsc_amm::AmmError::DivisionByZero
        | rsc_amm::AmmError::ConversionFailed
        | rsc_amm::AmmError::InvariantViolation
        | rsc_amm::AmmError::QuoteExpired
        | rsc_amm::AmmError::InvalidFee
        | rsc_amm::AmmError::InvalidEncoding => ServiceError::InvalidState,
        _ => ServiceError::Internal,
    }
}

fn map_native_amm_err(err: rsc_amm::NativeAmmError) -> ServiceError {
    match err {
        rsc_amm::NativeAmmError::Amm(inner) => map_amm_read_err(inner),
        rsc_amm::NativeAmmError::Storage | rsc_amm::NativeAmmError::State(_) => {
            ServiceError::Internal
        }
        _ => ServiceError::Internal,
    }
}

impl NodeService for NodeRpc {
    fn chain_id(&self) -> ServiceFut<'_, String> {
        ready(self.chain_id.to_string())
    }

    fn node_info(&self) -> ServiceFut<'_, RpcNodeInfo> {
        let chain = match self.lock_chain() {
            Ok(chain) => chain,
            Err(err) => return failed(err),
        };
        let block = match tip_block(&chain.store) {
            Ok(block) => block,
            Err(err) => return failed(err),
        };
        let hash = match block.block_hash() {
            Ok(hash) => hash,
            Err(_) => return failed(ServiceError::Internal),
        };
        ready(RpcNodeInfo {
            node_id: "rsc-node".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            network_id: self.network_id.clone(),
            chain_id: self.chain_id.to_string(),
            protocol_version: block.header.version.to_string(),
            sync_status: "ready".into(),
            latest_height: block.header.height.0,
            latest_block_hash: hash_hex(&hash),
        })
    }

    fn rpc_endpoints(&self) -> ServiceFut<'_, Vec<RpcEndpoint>> {
        ready(self.rpc_endpoints.clone())
    }

    fn status(&self) -> ServiceFut<'_, RpcStatus> {
        let chain = match self.lock_chain() {
            Ok(chain) => chain,
            Err(err) => return failed(err),
        };
        let block = match tip_block(&chain.store) {
            Ok(block) => block,
            Err(err) => return failed(err),
        };
        let hash = match block.block_hash() {
            Ok(hash) => hash,
            Err(_) => return failed(ServiceError::Internal),
        };
        let epoch = chain.consensus.state().epoch;
        let finalized_height = match load_finality(&chain.store, &chain.consensus.state().committee)
        {
            Ok(Some(finality)) => finality.finalized_height,
            Ok(None) => 0,
            Err(err) => return failed(err),
        };
        ready(RpcStatus {
            syncing: false,
            latest_height: block.header.height.0,
            latest_block_hash: hash_hex(&hash),
            epoch,
            finalized_height,
        })
    }

    fn create_wallet(&self) -> ServiceFut<'_, rsc_rpc::RpcWallet> {
        let wallet = match rsc_wallet::Wallet::generate() {
            Ok(wallet) => wallet,
            Err(_) => return failed(ServiceError::Internal),
        };
        let opening = match creation_transaction(&wallet) {
            Ok(opening) => opening,
            Err(_) => return failed(ServiceError::Internal),
        };
        let response = rsc_rpc::RpcWallet {
            address: wallet.address().to_string(),
            account_id: wallet.account_id_hex(),
            public_key: wallet.public_key_hex(),
            signing_key: wallet.signing_key_hex(),
            seed_phrase: wallet.seed_phrase().to_string(),
        };
        match self.wallets.lock() {
            Ok(mut queue) => queue.push_back(opening),
            Err(_) => return failed(ServiceError::Internal),
        }
        ready(response)
    }
}

impl ChainService for NodeRpc {
    fn block_by_hash(&self, block_hash: &str) -> ServiceFut<'_, RpcBlock> {
        let bytes = match decode_hex(block_hash) {
            Ok(bytes) => bytes,
            Err(_) => return failed(ServiceError::InvalidState),
        };
        let hash = match <[u8; 32]>::try_from(bytes.as_slice()) {
            Ok(bytes) => BlockHash(Hash(bytes)),
            Err(_) => return failed(ServiceError::InvalidState),
        };
        let chain = match self.lock_chain() {
            Ok(chain) => chain,
            Err(err) => return failed(err),
        };
        let block = match chain.store.get_block(&hash) {
            Ok(Some(block)) => block,
            Ok(None) => return failed(ServiceError::NotFound),
            Err(_) => return failed(ServiceError::Internal),
        };
        match rpc_block(&block) {
            Ok(block) => ready(block),
            Err(err) => failed(err),
        }
    }

    fn block_by_height(&self, height: u64) -> ServiceFut<'_, RpcBlock> {
        let chain = match self.lock_chain() {
            Ok(chain) => chain,
            Err(err) => return failed(err),
        };
        let hash = match chain.store.get_block_by_height(BlockHeight(height)) {
            Ok(Some(hash)) => hash,
            Ok(None) => return failed(ServiceError::NotFound),
            Err(_) => return failed(ServiceError::Internal),
        };
        let block = match chain.store.get_block(&hash) {
            Ok(Some(block)) => block,
            Ok(None) => return failed(ServiceError::NotFound),
            Err(_) => return failed(ServiceError::Internal),
        };
        match rpc_block(&block) {
            Ok(block) => ready(block),
            Err(err) => failed(err),
        }
    }

    fn transaction(&self, tx_hash: &str) -> ServiceFut<'_, RpcTransaction> {
        match self.find_transaction(tx_hash) {
            Ok(tx) => ready(tx),
            Err(err) => failed(err),
        }
    }
}

impl TransactionService for NodeRpc {
    fn submit_signed(&self, signed_tx: &[u8]) -> ServiceFut<'_, RpcSubmitResult> {
        match self.pool.insert(signed_tx.to_vec()) {
            Ok(hash) => ready(RpcSubmitResult {
                transaction_hash: encode_hex(&hash.0 .0),
            }),
            Err(_) => failed(ServiceError::InvalidTransaction),
        }
    }
}

impl StateService for NodeRpc {
    fn account(&self, account: &str) -> ServiceFut<'_, RpcAccount> {
        match self.read_account(account) {
            Ok((balance, nonce)) => ready(RpcAccount {
                account: account.to_string(),
                balance: balance.to_string(),
                nonce,
            }),
            Err(err) => failed(err),
        }
    }

    fn balance(&self, account: &str) -> ServiceFut<'_, RpcBalance> {
        match self.read_account(account) {
            Ok((balance, _)) => ready(RpcBalance {
                account: account.to_string(),
                balance: balance.to_string(),
            }),
            Err(err) => failed(err),
        }
    }
}

impl AmmService for NodeRpc {
    fn assets(&self) -> ServiceFut<'_, Vec<RpcAsset>> {
        match self.read_assets() {
            Ok(assets) => ready(assets),
            Err(err) => failed(err),
        }
    }

    fn asset_balance(&self, account: &str, asset_id: &str) -> ServiceFut<'_, RpcAssetBalance> {
        match self.read_asset_balance(account, asset_id) {
            Ok(balance) => ready(RpcAssetBalance { balance }),
            Err(err) => failed(err),
        }
    }

    fn asset_balances(
        &self,
        account: &str,
    ) -> ServiceFut<'_, std::collections::BTreeMap<String, String>> {
        match self.read_asset_balances(account) {
            Ok(map) => ready(map),
            Err(err) => failed(err),
        }
    }

    fn pools(&self) -> ServiceFut<'_, Vec<RpcPool>> {
        match self.read_pools() {
            Ok(pools) => ready(pools),
            Err(err) => failed(err),
        }
    }

    fn pool(&self, pool_id: &str) -> ServiceFut<'_, RpcPool> {
        match self.read_pool(pool_id) {
            Ok(pool) => ready(pool),
            Err(err) => failed(err),
        }
    }

    fn quote_swap(
        &self,
        pool_id: &str,
        token_in: &str,
        token_out: &str,
        amount_in: Option<&str>,
        amount_out: Option<&str>,
    ) -> ServiceFut<'_, RpcSwapQuote> {
        match self.read_quote_swap(pool_id, token_in, token_out, amount_in, amount_out) {
            Ok(quote) => ready(quote),
            Err(err) => failed(err),
        }
    }

    fn quote_add_liquidity(
        &self,
        pool_id: &str,
        max_amount_a: &str,
        max_amount_b: &str,
    ) -> ServiceFut<'_, RpcAddLiquidityQuote> {
        match self.read_quote_add_liquidity(pool_id, max_amount_a, max_amount_b) {
            Ok(quote) => ready(quote),
            Err(err) => failed(err),
        }
    }

    fn lp_position(&self, pool_id: &str, account: &str) -> ServiceFut<'_, RpcLpPosition> {
        match self.read_lp_position(pool_id, account) {
            Ok(pos) => ready(pos),
            Err(err) => failed(err),
        }
    }

    fn admin_register_asset(
        &self,
        asset_id: &str,
        decimals: u8,
        symbol: &str,
    ) -> ServiceFut<'_, ()> {
        match self.host_register_asset(asset_id, decimals, symbol) {
            Ok(()) => ready(()),
            Err(err) => failed(err),
        }
    }

    fn admin_credit_asset(
        &self,
        account: &str,
        asset_id: &str,
        amount: &str,
    ) -> ServiceFut<'_, ()> {
        match self.host_credit_asset(account, asset_id, amount) {
            Ok(()) => ready(()),
            Err(err) => failed(err),
        }
    }
}

impl ConsensusService for NodeRpc {
    fn validators(&self) -> ServiceFut<'_, Vec<RpcValidator>> {
        let chain = match self.lock_chain() {
            Ok(chain) => chain,
            Err(err) => return failed(err),
        };
        let mut validators = Vec::new();
        for member in &chain.consensus.state().committee.members {
            let Ok(voting_power) = u64::try_from(member.voting_power) else {
                return failed(ServiceError::Internal);
            };
            validators.push(RpcValidator {
                validator_id: encode_hex(&member.validator_id.0),
                public_key: String::new(),
                voting_power,
                status: "eligible".into(),
            });
        }
        ready(validators)
    }

    fn finality(&self) -> ServiceFut<'_, RpcFinality> {
        let chain = match self.lock_chain() {
            Ok(chain) => chain,
            Err(err) => return failed(err),
        };
        match load_finality(&chain.store, &chain.consensus.state().committee) {
            Ok(Some(finality)) => ready(finality),
            Ok(None) => failed(ServiceError::ConsensusUnavailable),
            Err(err) => failed(err),
        }
    }
}

impl NodeRpc {
    fn read_account(&self, account: &str) -> Result<(u128, u64), ServiceError> {
        let bytes = decode_hex(account).map_err(|_| ServiceError::InvalidState)?;
        let raw = <[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| ServiceError::InvalidState)?;
        let chain = self.lock_chain()?;
        let account = chain
            .state
            .get_account(&AccountId(raw).into())
            .map_err(|_| ServiceError::Internal)?
            .ok_or(ServiceError::NotFound)?;
        Ok((account.native_balance(), account.nonce()))
    }

    fn find_transaction(&self, tx_hash: &str) -> Result<RpcTransaction, ServiceError> {
        let bytes = decode_hex(tx_hash).map_err(|_| ServiceError::InvalidState)?;
        if bytes.len() != 32 {
            return Err(ServiceError::InvalidState);
        }
        let chain = self.lock_chain()?;
        let blocks = canonical_blocks(&chain.store).map_err(|_| ServiceError::Internal)?;
        let mut index = 0u64;
        for block in &blocks {
            let block_hash = block.block_hash().map_err(|_| ServiceError::Internal)?;
            for raw in &block.transactions {
                let transaction = Transaction::from_canonical_bytes(&raw.0)
                    .map_err(|_| ServiceError::Internal)?;
                let id = encode_hex(transaction.id().0.as_bytes());
                if id == tx_hash {
                    return Ok(RpcTransaction {
                        hash: id,
                        raw: encode_hex(&raw.0),
                        status: "included".into(),
                        block_height: Some(block.header.height.0),
                        block_hash: Some(hash_hex(&block_hash)),
                        index: Some(index),
                    });
                }
                index = index.checked_add(1).ok_or(ServiceError::Internal)?;
            }
        }
        Err(ServiceError::NotFound)
    }
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

fn share_database(db: Arc<RscDatabase>) -> Result<(State, RocksDbChainStore), StateError> {
    let state = State::open(Arc::clone(&db))?;
    let store = RocksDbChainStore::from_shared(db);
    Ok((state, store))
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

fn default_rpc_bind() -> SocketAddr {
    SocketAddr::from((Ipv4Addr::LOCALHOST, 8545))
}

fn parse_rpc_bind(value: &str) -> Result<SocketAddr, String> {
    value
        .parse::<SocketAddr>()
        .map_err(|_| "rpc-bind inválido".to_string())
}

fn parse_cors_origin(value: &str) -> Result<String, String> {
    if value.is_empty() {
        return Err("rpc-cors vacío".into());
    }
    if value.bytes().any(|b| b.is_ascii_whitespace() || b == 0) {
        return Err("rpc-cors inválido".into());
    }
    Ok(value.to_string())
}

/// Flags: `--data-dir`, `--network-id`, `--chain-id`, `--genesis`,
/// `--amm-admin-token`, `--rpc-bind <ip:port>`, `--rpc-cors <origin>` (repetible),
/// `--rpc-public-url <url>`, `--rpc-seed <url>` (repetible).
/// `RSC_RPC_BIND` / `RSC_RPC_PUBLIC_URL` / `RSC_RPC_SEEDS` (CSV) como respaldo.
/// Sin `--rpc-bind`, el bind es `127.0.0.1:8545`.
fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Config, String> {
    let from_env = env::var("RSC_RPC_BIND").ok();
    parse_args_with_rpc_env(args, from_env.as_deref())
}

fn parse_args_with_rpc_env(
    args: impl IntoIterator<Item = String>,
    rpc_bind_env: Option<&str>,
) -> Result<Config, String> {
    let mut data_dir = None;
    let mut network_id = None;
    let mut chain_id = None;
    let mut genesis = None;
    let mut amm_admin_token = None;
    let mut rpc_bind = None;
    let mut rpc_cors = Vec::new();
    let mut rpc_public_url = None;
    let mut rpc_seeds = Vec::new();
    let mut args = args.into_iter();
    while let Some(flag) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| format!("falta el valor de {flag}"))?;
        match flag.as_str() {
            "--data-dir" => set(&mut data_dir, value, "data-dir")?,
            "--network-id" => set(&mut network_id, value, "network-id")?,
            "--genesis" => set(&mut genesis, value, "genesis")?,
            "--amm-admin-token" => set(&mut amm_admin_token, value, "amm-admin-token")?,
            "--rpc-bind" => {
                if rpc_bind.is_some() {
                    return Err("rpc-bind repetido".into());
                }
                if value.is_empty() {
                    return Err("rpc-bind vacío".into());
                }
                rpc_bind = Some(parse_rpc_bind(&value)?);
            }
            "--rpc-cors" => rpc_cors.push(parse_cors_origin(&value)?),
            "--rpc-public-url" => {
                let url = normalize_rpc_url(&value).ok_or("rpc-public-url inválida")?;
                set(&mut rpc_public_url, url, "rpc-public-url")?;
            }
            "--rpc-seed" => {
                let url = normalize_rpc_url(&value).ok_or("rpc-seed inválida")?;
                rpc_seeds.push(url);
            }
            "--chain-id" => {
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
    if amm_admin_token.is_none() {
        if let Ok(from_env) = env::var("AMM_ADMIN_TOKEN") {
            if !from_env.is_empty() {
                amm_admin_token = Some(from_env);
            }
        }
    }
    if rpc_public_url.is_none() {
        if let Ok(from_env) = env::var("RSC_RPC_PUBLIC_URL") {
            if !from_env.is_empty() {
                rpc_public_url =
                    Some(normalize_rpc_url(&from_env).ok_or("RSC_RPC_PUBLIC_URL inválida")?);
            }
        }
    }
    if rpc_seeds.is_empty() {
        if let Ok(from_env) = env::var("RSC_RPC_SEEDS") {
            for part in from_env.split(',') {
                let part = part.trim();
                if part.is_empty() {
                    continue;
                }
                rpc_seeds.push(normalize_rpc_url(part).ok_or("RSC_RPC_SEEDS inválida")?);
            }
        }
    }
    let rpc_bind = match rpc_bind {
        Some(addr) => addr,
        None => match rpc_bind_env.filter(|value| !value.is_empty()) {
            Some(value) => parse_rpc_bind(value)?,
            None => default_rpc_bind(),
        },
    };
    Ok(Config {
        data_dir: data_dir.ok_or("falta data-dir")?,
        network_id: network_id.ok_or("falta network-id")?,
        chain_id: chain_id.ok_or("falta chain-id")?,
        genesis,
        amm_admin_token,
        rpc_bind,
        rpc_cors,
        rpc_public_url,
        rpc_seeds,
    })
}

/// Arma el directorio RPC: public-url + seeds CLI/env + `data_dir/rpc-seeds.json`.
fn build_rpc_directory(config: &Config) -> Vec<RpcEndpoint> {
    let network_id = config.network_id.clone();
    let chain_id = config.chain_id.to_string();
    let mut entries = Vec::new();
    if let Some(url) = config.rpc_public_url.as_ref() {
        entries.push(RpcEndpoint {
            url: url.clone(),
            network_id: network_id.clone(),
            chain_id: chain_id.clone(),
            source: "self".into(),
        });
    }
    for url in &config.rpc_seeds {
        entries.push(RpcEndpoint {
            url: url.clone(),
            network_id: network_id.clone(),
            chain_id: chain_id.clone(),
            source: "seed".into(),
        });
    }
    let file_path = std::path::Path::new(&config.data_dir).join("rpc-seeds.json");
    if let Ok(text) = std::fs::read_to_string(&file_path) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
            let list = value
                .get("endpoints")
                .and_then(|v| v.as_array())
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str());
            for raw in list {
                if let Some(url) = normalize_rpc_url(raw) {
                    entries.push(RpcEndpoint {
                        url,
                        network_id: network_id.clone(),
                        chain_id: chain_id.clone(),
                        source: "file".into(),
                    });
                }
            }
        }
    }
    merge_rpc_endpoints(entries)
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

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Arc;

    use super::{
        open_database, parse_args, parse_args_with_rpc_env, rpc_probe_addr, share_database,
    };

    fn no_wallets() -> std::sync::Mutex<VecDeque<super::WalletOpening>> {
        std::sync::Mutex::new(VecDeque::new())
    }
    use rsc_amm::native::AmmRuntime;
    use rsc_chain::{
        validate_block, Block, BlockHeader, ChainStore, ChainValidationContext, RawTransaction,
        RocksDbChainStore,
    };
    use rsc_consensus::ConsensusPhase;
    use rsc_core::{AccountId, BlockHeight, Hash};
    use rsc_execution::EXECUTION_ENVIRONMENT_ID;
    use rsc_ledger::transaction::TRANSACTION_VERSION_V1;
    use rsc_ledger::{Ledger, Transaction, TransactionPayload};
    use rsc_mempool::{MempoolStatus, TransactionQueueState};
    use rsc_state::State;
    use rsc_storage::StorageError;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn accepts_the_three_flags() {
        let config = parse_args(args(&[
            "--data-dir",
            "/tmp/rsc",
            "--network-id",
            "devnet",
            "--chain-id",
            "3",
        ]))
        .unwrap();
        assert_eq!(config.data_dir, "/tmp/rsc");
        assert_eq!(config.network_id, "devnet");
        assert_eq!(config.chain_id, 3);
        assert_eq!(config.rpc_bind, "127.0.0.1:8545".parse().unwrap());
        assert!(config.rpc_cors.is_empty());
    }

    fn base_flags() -> Vec<&'static str> {
        vec![
            "--data-dir",
            "/tmp/rsc",
            "--network-id",
            "devnet",
            "--chain-id",
            "3",
        ]
    }

    #[test]
    fn parses_rpc_bind_flag_env_and_cors() {
        let mut with_flag = base_flags();
        with_flag.extend([
            "--rpc-bind",
            "0.0.0.0:8545",
            "--rpc-cors",
            "https://app.example",
            "--rpc-cors",
            "http://localhost:3000",
        ]);
        let config = parse_args_with_rpc_env(args(&with_flag), Some("127.0.0.1:9999")).unwrap();
        assert_eq!(config.rpc_bind, "0.0.0.0:8545".parse().unwrap());
        assert_eq!(
            config.rpc_cors,
            vec![
                "https://app.example".to_string(),
                "http://localhost:3000".to_string()
            ]
        );

        let from_env =
            parse_args_with_rpc_env(args(&base_flags()), Some("192.168.1.12:8545")).unwrap();
        assert_eq!(from_env.rpc_bind, "192.168.1.12:8545".parse().unwrap());

        let missing = parse_args_with_rpc_env(args(&base_flags()), None).unwrap();
        assert_eq!(missing.rpc_bind, "127.0.0.1:8545".parse().unwrap());

        let err = parse_args_with_rpc_env(
            args(&[
                "--data-dir",
                "/tmp/rsc",
                "--network-id",
                "devnet",
                "--chain-id",
                "3",
                "--rpc-bind",
                "8545",
            ]),
            None,
        )
        .unwrap_err();
        assert_eq!(err, "rpc-bind inválido");

        let err = parse_args_with_rpc_env(args(&base_flags()), Some("no-es-addr")).unwrap_err();
        assert_eq!(err, "rpc-bind inválido");

        let wildcard: std::net::SocketAddr = "0.0.0.0:8545".parse().unwrap();
        assert_eq!(rpc_probe_addr(wildcard), "127.0.0.1:8545".parse().unwrap());
        let specific: std::net::SocketAddr = "192.168.1.12:8545".parse().unwrap();
        assert_eq!(rpc_probe_addr(specific), specific);
    }

    #[test]
    fn parses_rpc_public_url_and_seeds_builds_directory() {
        let mut flags = base_flags();
        flags.extend([
            "--rpc-public-url",
            "http://127.0.0.1:8545",
            "--rpc-seed",
            "https://peer-a.example/rpc",
            "--rpc-seed",
            "https://peer-b.example",
        ]);
        let config = parse_args_with_rpc_env(args(&flags), None).unwrap();
        assert_eq!(
            config.rpc_public_url.as_deref(),
            Some("http://127.0.0.1:8545/rpc")
        );
        assert_eq!(config.rpc_seeds.len(), 2);
        let dir = super::build_rpc_directory(&config);
        assert_eq!(dir.len(), 3);
        assert_eq!(dir[0].source, "self");
        assert_eq!(dir[1].url, "https://peer-a.example/rpc");
        assert_eq!(dir[2].source, "seed");
    }

    #[test]
    fn rejects_a_missing_flag() {
        let err =
            parse_args(args(&["--data-dir", "/tmp/rsc", "--network-id", "devnet"])).unwrap_err();
        assert_eq!(err, "falta chain-id");
    }

    #[test]
    fn rejects_an_empty_value() {
        let err = parse_args(args(&[
            "--data-dir",
            "/tmp/rsc",
            "--network-id",
            "",
            "--chain-id",
            "3",
        ]))
        .unwrap_err();
        assert_eq!(err, "network-id vacío");
    }

    #[test]
    fn rejects_a_chain_id_that_is_not_u32() {
        let err = parse_args(args(&[
            "--data-dir",
            "/tmp/rsc",
            "--network-id",
            "devnet",
            "--chain-id",
            "devnet",
        ]))
        .unwrap_err();
        assert_eq!(err, "chain-id no es u32");
    }

    #[test]
    fn rejects_an_unknown_flag() {
        let err = parse_args(args(&[
            "--data-dir",
            "/tmp/rsc",
            "--listen",
            "/ip4/127.0.0.1/tcp/0",
            "--network-id",
            "devnet",
            "--chain-id",
            "3",
        ]))
        .unwrap_err();
        assert_eq!(err, "argumento desconocido --listen");
    }

    fn config_at(path: &str, network_id: &str, chain_id: u32) -> super::Config {
        super::Config {
            data_dir: path.to_string(),
            network_id: network_id.to_string(),
            chain_id,
            genesis: None,
            amm_admin_token: None,
            rpc_bind: "127.0.0.1:8545".parse().unwrap(),
            rpc_cors: Vec::new(),
            rpc_public_url: None,
            rpc_seeds: Vec::new(),
        }
    }

    #[test]
    fn opens_and_reopens_the_same_identity() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        let config = config_at(path, "devnet", 3);
        drop(open_database(&config).unwrap());
        open_database(&config).unwrap();
    }

    #[test]
    fn rejects_a_different_network() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        drop(open_database(&config_at(path, "devnet", 3)).unwrap());
        match open_database(&config_at(path, "other", 3)) {
            Err(StorageError::NetworkMismatch) => {}
            Ok(_) => panic!("foreign network opened"),
            Err(err) => panic!("unexpected {err}"),
        }
    }

    #[test]
    fn rejects_a_different_chain() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        drop(open_database(&config_at(path, "devnet", 3)).unwrap());
        match open_database(&config_at(path, "devnet", 9)) {
            Err(StorageError::ChainMismatch) => {}
            Ok(_) => panic!("foreign chain opened"),
            Err(err) => panic!("unexpected {err}"),
        }
    }

    #[test]
    fn state_and_chain_store_share_one_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        let db = Arc::new(open_database(&config_at(path, "devnet", 3)).unwrap());
        assert_eq!(Arc::strong_count(&db), 1);
        let state = State::open(Arc::clone(&db)).unwrap();
        assert_eq!(Arc::strong_count(&db), 2);
        let store = RocksDbChainStore::from_shared(Arc::clone(&db));
        assert!(Arc::ptr_eq(&store.database(), &db));
        assert_eq!(Arc::strong_count(&db), 3);
        drop(state);
        drop(store);
        assert_eq!(Arc::strong_count(&db), 1);
        share_database(Arc::clone(&db)).unwrap();
    }

    fn fixture_parameters() -> rsc_genesis::NetworkParameters {
        rsc_genesis::NetworkParameters {
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
            consensus_randomness_seed: rsc_core::Hash([0xA5; 32]),
        }
    }

    fn write_genesis(
        dir: &std::path::Path,
        chain_id: u32,
        account_id: rsc_core::AccountId,
    ) -> String {
        let key = rsc_crypto::PrivateKey::from_bytes(&[0x11; 32])
            .unwrap()
            .public_key()
            .key_id();
        let genesis = rsc_genesis::Genesis::new(
            rsc_genesis::GENESIS_VERSION_V1,
            rsc_core::ChainId(chain_id),
            1_700_000_000,
            fixture_parameters(),
            vec![rsc_genesis::GenesisAccount {
                account_id,
                owner_key_id: key,
                native_balance: 500,
                allocation: rsc_genesis::GenesisAllocationCategory::Community,
            }],
            Vec::new(),
        );
        let path = dir.join("genesis.json");
        std::fs::write(&path, serde_json::to_vec(&genesis).unwrap()).unwrap();
        path.to_str().unwrap().to_string()
    }

    fn derived_account() -> rsc_core::AccountId {
        let key = rsc_crypto::PrivateKey::from_bytes(&[0x11; 32])
            .unwrap()
            .public_key()
            .key_id();
        rsc_state::GenesisAccount::new(key, 500).account_id().into()
    }

    #[test]
    fn writes_block_zero_when_there_is_no_tip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        let genesis = write_genesis(dir.path(), 3, derived_account());
        let mut config = config_at(path, "devnet", 3);
        config.genesis = Some(genesis);
        let db = Arc::new(open_database(&config).unwrap());
        let (state, store) = share_database(Arc::clone(&db)).unwrap();
        super::ensure_tip(&state, &store, &config).unwrap();
        let tip = store.get_canonical_tip().unwrap().unwrap();
        let block = store.get_block(&tip).unwrap().unwrap();
        assert!(block.header.is_genesis());
        assert_eq!(block.header.chain_id, rsc_core::ChainId(3));
        let account = state
            .get_account(&derived_account().into())
            .unwrap()
            .unwrap();
        assert_eq!(account.native_balance(), 500);
        config.genesis = Some("/no/such/genesis.json".into());
        super::ensure_tip(&state, &store, &config).unwrap();
        assert_eq!(store.get_canonical_tip().unwrap().unwrap(), tip);
    }

    #[test]
    fn amm_genesis_funds_assets_and_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        let account = derived_account();
        let key = rsc_crypto::PrivateKey::from_bytes(&[0x11; 32])
            .unwrap()
            .public_key()
            .key_id();
        let asset_a = rsc_core::Hash([0xaa; 32]);
        let asset_b = rsc_core::Hash([0xbb; 32]);
        let mut genesis = rsc_genesis::Genesis::new(
            rsc_genesis::GENESIS_VERSION_V1,
            rsc_core::ChainId(3),
            1_700_000_000,
            fixture_parameters(),
            vec![rsc_genesis::GenesisAccount {
                account_id: account,
                owner_key_id: key,
                native_balance: 500,
                allocation: rsc_genesis::GenesisAllocationCategory::Community,
            }],
            Vec::new(),
        );
        genesis.amm_assets = vec![
            rsc_genesis::GenesisAmmAsset {
                asset_id: asset_a,
                decimals: 18,
                symbol: "tRSK".into(),
            },
            rsc_genesis::GenesisAmmAsset {
                asset_id: asset_b,
                decimals: 6,
                symbol: "tUSDC".into(),
            },
        ];
        genesis.amm_balances = vec![
            rsc_genesis::GenesisAmmBalance {
                account_id: account,
                asset_id: asset_a,
                amount: 10_000,
            },
            rsc_genesis::GenesisAmmBalance {
                account_id: account,
                asset_id: asset_b,
                amount: 20_000,
            },
        ];
        genesis.amm_governors = vec![account];
        let genesis_path = dir.path().join("genesis.json");
        std::fs::write(&genesis_path, serde_json::to_vec(&genesis).unwrap()).unwrap();
        let mut config = config_at(path, "devnet", 3);
        config.genesis = Some(genesis_path.to_str().unwrap().to_string());
        let db = Arc::new(open_database(&config).unwrap());
        let (state, store) = share_database(Arc::clone(&db)).unwrap();
        super::ensure_tip(&state, &store, &config).unwrap();

        let runtime = AmmRuntime::new(Arc::clone(&db), State::open(Arc::clone(&db)).unwrap());
        let assets = runtime.load_assets().unwrap();
        let id_a = rsc_amm::AssetId(asset_a);
        let id_b = rsc_amm::AssetId(asset_b);
        assert_eq!(
            assets
                .balance_of(&account, &rsc_amm::native::assets_asset_id(id_a))
                .0,
            10_000
        );
        assert_eq!(
            assets
                .balance_of(&account, &rsc_amm::native::assets_asset_id(id_b))
                .0,
            20_000
        );
        assert_eq!(runtime.load_store().unwrap().governors, vec![account]);
        let first_bytes = assets.to_canonical_bytes();
        drop(runtime);
        drop(assets);
        // Tip ya existe → ensure_tip no re-credita.
        super::ensure_tip(&state, &store, &config).unwrap();
        drop(state);
        drop(store);
        drop(db);

        let reopened = Arc::new(open_database(&config).unwrap());
        let runtime2 = AmmRuntime::new(
            Arc::clone(&reopened),
            State::open(Arc::clone(&reopened)).unwrap(),
        );
        let assets2 = runtime2.load_assets().unwrap();
        assert_eq!(assets2.to_canonical_bytes(), first_bytes);
        assert_eq!(
            assets2
                .balance_of(&account, &rsc_amm::native::assets_asset_id(id_a))
                .0,
            10_000
        );
        assert_eq!(runtime2.load_store().unwrap().governors, vec![account]);
        assert!(
            runtime2
                .load_store()
                .unwrap()
                .pools
                .policy()
                .permissionless_creation
        );
    }

    /// Génesis con governor + assets + policy cerrada; cuenta outsider sin poder de create.
    fn opened_amm_closed_policy() -> (
        tempfile::TempDir,
        State,
        RocksDbChainStore,
        rsc_core::AccountId,
        rsc_core::AccountId,
        rsc_core::Hash,
        rsc_core::Hash,
        String,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap().to_string();
        let gov_key = rsc_crypto::PrivateKey::from_bytes(&[0x11; 32]).unwrap();
        let out_key = rsc_crypto::PrivateKey::from_bytes(&[0x22; 32]).unwrap();
        let gov_id = rsc_core::AccountId::from(
            rsc_state::GenesisAccount::new(gov_key.public_key().key_id(), 500).account_id(),
        );
        let out_id = rsc_core::AccountId::from(
            rsc_state::GenesisAccount::new(out_key.public_key().key_id(), 100).account_id(),
        );
        let asset_a = rsc_core::Hash([0xaa; 32]);
        let asset_b = rsc_core::Hash([0xbb; 32]);
        let mut genesis = rsc_genesis::Genesis::new(
            rsc_genesis::GENESIS_VERSION_V1,
            rsc_core::ChainId(3),
            1_700_000_000,
            fixture_parameters(),
            vec![
                rsc_genesis::GenesisAccount {
                    account_id: gov_id,
                    owner_key_id: gov_key.public_key().key_id(),
                    native_balance: 500,
                    allocation: rsc_genesis::GenesisAllocationCategory::Community,
                },
                rsc_genesis::GenesisAccount {
                    account_id: out_id,
                    owner_key_id: out_key.public_key().key_id(),
                    native_balance: 100,
                    allocation: rsc_genesis::GenesisAllocationCategory::Community,
                },
            ],
            Vec::new(),
        );
        genesis.amm_assets = vec![
            rsc_genesis::GenesisAmmAsset {
                asset_id: asset_a,
                decimals: 18,
                symbol: "tRSK".into(),
            },
            rsc_genesis::GenesisAmmAsset {
                asset_id: asset_b,
                decimals: 6,
                symbol: "tUSDC".into(),
            },
        ];
        genesis.amm_balances = vec![
            rsc_genesis::GenesisAmmBalance {
                account_id: gov_id,
                asset_id: asset_a,
                amount: 10_000,
            },
            rsc_genesis::GenesisAmmBalance {
                account_id: gov_id,
                asset_id: asset_b,
                amount: 20_000,
            },
            rsc_genesis::GenesisAmmBalance {
                account_id: out_id,
                asset_id: asset_a,
                amount: 5_000,
            },
            rsc_genesis::GenesisAmmBalance {
                account_id: out_id,
                asset_id: asset_b,
                amount: 5_000,
            },
        ];
        genesis.amm_governors = vec![gov_id];
        genesis.amm_permissionless_creation = false;
        let genesis_path = dir.path().join("genesis.json");
        std::fs::write(&genesis_path, serde_json::to_vec(&genesis).unwrap()).unwrap();
        let mut config = config_at(&path, "devnet", 3);
        config.genesis = Some(genesis_path.to_str().unwrap().to_string());
        let db = Arc::new(open_database(&config).unwrap());
        let (state, store) = share_database(db).unwrap();
        super::ensure_tip(&state, &store, &config).unwrap();
        (dir, state, store, gov_id, out_id, asset_a, asset_b, path)
    }

    #[test]
    fn amm_governor_create_pool_ok_outsider_rejected_survives_restart() {
        let (dir, state, store, gov_id, out_id, asset_a, asset_b, path) =
            opened_amm_closed_policy();
        let runtime = AmmRuntime::new(store.database(), State::open(store.database()).unwrap());
        let loaded = runtime.load_store().unwrap();
        assert_eq!(loaded.governors, vec![gov_id]);
        assert!(!loaded.pools.policy().permissionless_creation);

        let (pool, _execution, mut consensus) = super::open_services(&store).unwrap();
        reach_new_view(&state, &store, &mut consensus);

        let fee = rsc_amm::FeeBps::new(30).unwrap();
        let create = rsc_amm::native::AmmInstruction::CreatePool {
            asset_left: rsc_amm::AssetId(asset_a),
            asset_right: rsc_amm::AssetId(asset_b),
            fee_bps: fee,
        };
        let gov_sk = rsc_crypto::PrivateKey::from_bytes(&[0x11; 32]).unwrap();
        let out_sk = rsc_crypto::PrivateKey::from_bytes(&[0x22; 32]).unwrap();

        let outsider_tx = rsc_amm::native::sign_amm_tx(out_id, 0, &create, &out_sk).unwrap();
        let bad = pool.insert(outsider_tx.canonical_bytes()).unwrap();
        let err = super::commit_ready(&state, &store, &pool).unwrap_err();
        assert!(
            err.contains("permissionless") || err.contains("disabled"),
            "unexpected reject: {err}"
        );
        assert!(runtime.load_store().unwrap().pools.list_pools().is_empty());
        // Quitar la tx fallida del mempool (sigue Ready; commit no la removió).
        pool.remove(&bad);

        let gov_tx = rsc_amm::native::sign_amm_tx(gov_id, 0, &create, &gov_sk).unwrap();
        pool.insert(gov_tx.canonical_bytes()).unwrap();
        super::commit_ready(&state, &store, &pool).unwrap();
        let pool_id =
            rsc_amm::PoolIdentity::new(rsc_amm::AssetId(asset_a), rsc_amm::AssetId(asset_b), fee)
                .unwrap()
                .pool_id();
        assert_eq!(
            runtime.load_store().unwrap().pools.list_pools(),
            vec![pool_id]
        );

        drop(runtime);
        drop(pool);
        drop(consensus);
        drop(state);
        drop(store);

        let mut config = config_at(&path, "devnet", 3);
        config.genesis = Some(
            dir.path()
                .join("genesis.json")
                .to_str()
                .unwrap()
                .to_string(),
        );
        let reopened = Arc::new(open_database(&config).unwrap());
        let (state2, store2) = share_database(Arc::clone(&reopened)).unwrap();
        super::ensure_tip(&state2, &store2, &config).unwrap();
        let runtime2 = AmmRuntime::new(
            Arc::clone(&reopened),
            State::open(Arc::clone(&reopened)).unwrap(),
        );
        let store_amm = runtime2.load_store().unwrap();
        assert_eq!(store_amm.governors, vec![gov_id]);
        assert!(!store_amm.pools.policy().permissionless_creation);
        assert_eq!(store_amm.pools.list_pools(), vec![pool_id]);
    }

    #[test]
    fn rpc_get_pool_matches_store_and_quote_does_not_mutate() {
        let (dir, state, store, gov_id, _out_id, asset_a, asset_b, path) =
            opened_amm_closed_policy();
        let (pool, _execution, mut consensus) = super::open_services(&store).unwrap();
        reach_new_view(&state, &store, &mut consensus);

        let fee = rsc_amm::FeeBps::new(30).unwrap();
        let gov_sk = rsc_crypto::PrivateKey::from_bytes(&[0x11; 32]).unwrap();
        let create = rsc_amm::native::AmmInstruction::CreatePool {
            asset_left: rsc_amm::AssetId(asset_a),
            asset_right: rsc_amm::AssetId(asset_b),
            fee_bps: fee,
        };
        let pool_id =
            rsc_amm::PoolIdentity::new(rsc_amm::AssetId(asset_a), rsc_amm::AssetId(asset_b), fee)
                .unwrap()
                .pool_id();
        pool.insert(
            rsc_amm::native::sign_amm_tx(gov_id, 0, &create, &gov_sk)
                .unwrap()
                .canonical_bytes(),
        )
        .unwrap();
        super::commit_ready(&state, &store, &pool).unwrap();

        let mut config = config_at(&path, "devnet", 3);
        config.genesis = Some(
            dir.path()
                .join("genesis.json")
                .to_str()
                .unwrap()
                .to_string(),
        );
        let rpc_state = State::open(store.database()).unwrap();
        let rpc_store = RocksDbChainStore::from_shared(store.database());
        let (_extra_pool, _extra_execution, rpc_consensus) =
            super::open_services(&rpc_store).unwrap();
        let ctx = super::rpc_context(
            &config,
            rpc_state,
            rpc_store,
            rpc_consensus,
            Arc::clone(&pool),
        );
        let runtime = AmmRuntime::new(store.database(), State::open(store.database()).unwrap());
        let pool_hex = rsc_rpc::encode_hex(&pool_id.0 .0);
        let token_in = rsc_rpc::encode_hex(&asset_a.0);
        let token_out = rsc_rpc::encode_hex(&asset_b.0);

        let empty_root = state.state_root().unwrap();
        let empty_bytes = runtime.load_store().unwrap().pools.to_canonical_bytes();
        let empty_swap = wait(rsc_rpc::methods::dispatch(
            &ctx,
            "rsc_quoteSwap",
            Some(&serde_json::json!({
                "pool_id": pool_hex,
                "token_in": token_in,
                "token_out": token_out,
                "amount_in": "100",
            })),
        ))
        .unwrap_err();
        assert_eq!(empty_swap, rsc_rpc::RpcError::InvalidParams);
        let empty_add = wait(rsc_rpc::methods::dispatch(
            &ctx,
            "rsc_quoteAddLiquidity",
            Some(&serde_json::json!({
                "pool_id": pool_hex,
                "max_amount_a": "1000",
                "max_amount_b": "1000",
            })),
        ))
        .unwrap_err();
        assert_eq!(empty_add, rsc_rpc::RpcError::InvalidParams);
        assert_eq!(state.state_root().unwrap(), empty_root);
        assert_eq!(
            runtime.load_store().unwrap().pools.to_canonical_bytes(),
            empty_bytes
        );
        let missing = wait(rsc_rpc::methods::dispatch(
            &ctx,
            "rsc_getPool",
            Some(&serde_json::json!({ "pool_id": "ab".repeat(32) })),
        ))
        .unwrap_err();
        assert_eq!(missing.code(), -32000);

        let add = rsc_amm::native::AmmInstruction::AddLiquidity {
            pool_id,
            max_amount_a: rsc_core::Amount(5_000),
            max_amount_b: rsc_core::Amount(10_000),
            min_lp_shares: rsc_amm::LpShares(0),
            deadline: u64::MAX,
        };
        pool.insert(
            rsc_amm::native::sign_amm_tx(gov_id, 1, &add, &gov_sk)
                .unwrap()
                .canonical_bytes(),
        )
        .unwrap();
        super::commit_ready(&state, &store, &pool).unwrap();

        let activate = rsc_amm::native::AmmInstruction::ActivatePool { pool_id };
        pool.insert(
            rsc_amm::native::sign_amm_tx(gov_id, 2, &activate, &gov_sk)
                .unwrap()
                .canonical_bytes(),
        )
        .unwrap();
        super::commit_ready(&state, &store, &pool).unwrap();

        let amm_store = runtime.load_store().unwrap();
        let on_chain = amm_store.pools.get_pool(&pool_id).unwrap();
        let seen = wait(rsc_rpc::methods::dispatch(
            &ctx,
            "rsc_getPool",
            Some(&serde_json::json!({ "pool_id": pool_hex })),
        ))
        .unwrap();
        assert_eq!(seen["pool_id"], serde_json::json!(pool_hex));
        assert_eq!(seen["status"], serde_json::json!("active"));
        assert_eq!(
            seen["reserve_a"],
            serde_json::json!(on_chain.reserves.reserve_a.0.to_string())
        );
        assert_eq!(
            seen["reserve_b"],
            serde_json::json!(on_chain.reserves.reserve_b.0.to_string())
        );
        assert_eq!(
            seen["total_supply"],
            serde_json::json!(on_chain.total_supply.0.to_string())
        );
        assert_eq!(seen["fee_bps"], serde_json::json!(30));
        assert_eq!(seen["decimals_a"], serde_json::json!(18));
        assert_eq!(seen["decimals_b"], serde_json::json!(6));

        let root_before = state.state_root().unwrap();
        let reserves_before = on_chain.reserves;
        let pools_bytes_before = amm_store.pools.to_canonical_bytes();
        let quote = wait(rsc_rpc::methods::dispatch(
            &ctx,
            "rsc_quoteSwap",
            Some(&serde_json::json!({
                "pool_id": pool_hex,
                "token_in": token_in,
                "token_out": token_out,
                "amount_in": "100",
            })),
        ))
        .unwrap();
        assert!(
            quote["amount_out"]
                .as_str()
                .unwrap()
                .parse::<u128>()
                .unwrap()
                > 0
        );
        assert_eq!(quote["decimals_in"], serde_json::json!(18));
        assert_eq!(quote["decimals_out"], serde_json::json!(6));
        assert_ne!(quote["spot_price"], quote["spot_price_normalized"]);

        let added = wait(rsc_rpc::methods::dispatch(
            &ctx,
            "rsc_quoteAddLiquidity",
            Some(&serde_json::json!({
                "pool_id": pool_hex,
                "max_amount_a": "1000",
                "max_amount_b": "4000",
            })),
        ))
        .unwrap();
        assert!(added["amount_a"].as_str().unwrap().parse::<u128>().unwrap() > 0);

        let amm_after = runtime.load_store().unwrap();
        assert_eq!(state.state_root().unwrap(), root_before);
        assert_eq!(
            amm_after.pools.get_pool(&pool_id).unwrap().reserves,
            reserves_before
        );
        assert_eq!(amm_after.pools.to_canonical_bytes(), pools_bytes_before);

        let listed = wait(rsc_rpc::methods::dispatch(
            &ctx,
            "rsc_getPools",
            Some(&serde_json::json!({})),
        ))
        .unwrap();
        assert_eq!(listed.as_array().unwrap().len(), 1);
        assert_eq!(listed[0]["pool_id"], serde_json::json!(pool_hex));

        let lp = wait(rsc_rpc::methods::dispatch(
            &ctx,
            "rsc_getLpPosition",
            Some(&serde_json::json!({
                "pool_id": pool_hex,
                "account": rsc_rpc::encode_hex(&gov_id.0),
            })),
        ))
        .unwrap();
        assert!(lp["lp_shares"].as_str().unwrap().parse::<u128>().unwrap() > 0);

        let pause = rsc_amm::native::AmmInstruction::PausePool { pool_id };
        pool.insert(
            rsc_amm::native::sign_amm_tx(gov_id, 3, &pause, &gov_sk)
                .unwrap()
                .canonical_bytes(),
        )
        .unwrap();
        super::commit_ready(&state, &store, &pool).unwrap();
        let paused_root = state.state_root().unwrap();
        let paused_bytes = runtime.load_store().unwrap().pools.to_canonical_bytes();
        let paused = wait(rsc_rpc::methods::dispatch(
            &ctx,
            "rsc_quoteSwap",
            Some(&serde_json::json!({
                "pool_id": pool_hex,
                "token_in": token_in,
                "token_out": token_out,
                "amount_out": "10",
            })),
        ))
        .unwrap_err();
        assert_eq!(paused, rsc_rpc::RpcError::InvalidParams);
        assert_eq!(state.state_root().unwrap(), paused_root);
        assert_eq!(
            runtime.load_store().unwrap().pools.to_canonical_bytes(),
            paused_bytes
        );
    }

    #[test]
    fn rejects_genesis_for_another_chain_without_writing_a_tip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        let genesis = write_genesis(dir.path(), 1, derived_account());
        let mut config = config_at(path, "devnet", 3);
        config.genesis = Some(genesis);
        let db = Arc::new(open_database(&config).unwrap());
        let (state, store) = share_database(db).unwrap();
        let err = super::ensure_tip(&state, &store, &config).unwrap_err();
        assert_eq!(err, "chain-id del genesis distinto");
        assert!(store.get_canonical_tip().unwrap().is_none());
    }

    #[test]
    fn rejects_an_account_id_that_is_not_derived() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        let genesis = write_genesis(dir.path(), 3, rsc_core::AccountId::ZERO);
        let mut config = config_at(path, "devnet", 3);
        config.genesis = Some(genesis);
        let db = Arc::new(open_database(&config).unwrap());
        let (state, store) = share_database(db).unwrap();
        let err = super::ensure_tip(&state, &store, &config).unwrap_err();
        assert_eq!(err, "genesis account id distinto");
        assert!(store.get_canonical_tip().unwrap().is_none());
    }

    #[test]
    fn missing_genesis_leaves_the_database_without_a_tip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        let config = config_at(path, "devnet", 3);
        let db = Arc::new(open_database(&config).unwrap());
        let (state, store) = share_database(db).unwrap();
        let err = super::ensure_tip(&state, &store, &config).unwrap_err();
        assert_eq!(err, "falta genesis");
        assert!(store.get_canonical_tip().unwrap().is_none());
    }

    fn opened() -> (tempfile::TempDir, State, RocksDbChainStore) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        let genesis = write_genesis(dir.path(), 3, derived_account());
        let mut config = config_at(path, "devnet", 3);
        config.genesis = Some(genesis);
        let db = Arc::new(open_database(&config).unwrap());
        let (state, store) = share_database(db).unwrap();
        super::ensure_tip(&state, &store, &config).unwrap();
        (dir, state, store)
    }

    fn write_pair_genesis(dir: &std::path::Path) -> (String, rsc_core::AccountId) {
        let sender_key = rsc_crypto::PrivateKey::from_bytes(&[0x11; 32])
            .unwrap()
            .public_key()
            .key_id();
        let peer_key = rsc_crypto::PrivateKey::from_bytes(&[0x22; 32])
            .unwrap()
            .public_key()
            .key_id();
        let peer_id =
            rsc_core::AccountId::from(rsc_state::GenesisAccount::new(peer_key, 100).account_id());
        let genesis = rsc_genesis::Genesis::new(
            rsc_genesis::GENESIS_VERSION_V1,
            rsc_core::ChainId(3),
            1_700_000_000,
            fixture_parameters(),
            vec![
                rsc_genesis::GenesisAccount {
                    account_id: derived_account(),
                    owner_key_id: sender_key,
                    native_balance: 500,
                    allocation: rsc_genesis::GenesisAllocationCategory::Community,
                },
                rsc_genesis::GenesisAccount {
                    account_id: peer_id,
                    owner_key_id: peer_key,
                    native_balance: 100,
                    allocation: rsc_genesis::GenesisAllocationCategory::Community,
                },
            ],
            Vec::new(),
        );
        let file = dir.join("genesis.json");
        std::fs::write(&file, serde_json::to_vec(&genesis).unwrap()).unwrap();
        (file.to_str().unwrap().to_string(), peer_id)
    }

    fn opened_pair() -> (
        tempfile::TempDir,
        State,
        RocksDbChainStore,
        rsc_core::AccountId,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_str().unwrap();
        let (genesis, peer_id) = write_pair_genesis(dir.path());
        let mut config = config_at(path, "devnet", 3);
        config.genesis = Some(genesis);
        let db = Arc::new(open_database(&config).unwrap());
        let (state, store) = share_database(db).unwrap();
        super::ensure_tip(&state, &store, &config).unwrap();
        (dir, state, store, peer_id)
    }

    fn reach_new_view(
        state: &State,
        store: &RocksDbChainStore,
        consensus: &mut rsc_consensus::ConsensusEngine,
    ) {
        super::propose_tip(store, consensus).unwrap();
        let prepare =
            super::cast_local_vote(store, consensus, rsc_consensus::VotePhase::Prepare).unwrap();
        super::accept_prepare_quorum(consensus, &prepare).unwrap();
        let precommit =
            super::cast_local_vote(store, consensus, rsc_consensus::VotePhase::PreCommit).unwrap();
        super::accept_precommit_quorum(consensus, &precommit).unwrap();
        let commit =
            super::cast_local_vote(store, consensus, rsc_consensus::VotePhase::Commit).unwrap();
        let proof = super::accept_commit(consensus, &commit).unwrap();
        super::apply_finalized(state, store, consensus, &proof).unwrap();
    }

    fn transfer(nonce: u64, amount: u128) -> Transaction {
        let key = rsc_crypto::PrivateKey::from_bytes(&[0x21; 32]).unwrap();
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

    fn signed_genesis_transfer(nonce: u64, amount: u128) -> Transaction {
        let key = rsc_crypto::PrivateKey::from_bytes(&[0x11; 32]).unwrap();
        let mut transaction = Transaction {
            version: TRANSACTION_VERSION_V1,
            sender: derived_account(),
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

    fn signed_to(recipient: AccountId, nonce: u64, amount: u128) -> Transaction {
        let key = rsc_crypto::PrivateKey::from_bytes(&[0x11; 32]).unwrap();
        let mut transaction = Transaction {
            version: TRANSACTION_VERSION_V1,
            sender: derived_account(),
            nonce,
            payload: TransactionPayload::Transfer { recipient, amount },
            signature: rsc_crypto::Signature::from_bytes(&[0u8; 64]).unwrap(),
        };
        transaction.sign(&key).unwrap();
        transaction
    }

    fn append_block(store: &RocksDbChainStore, parent: &Block, raw: Vec<Vec<u8>>) -> Block {
        let mut block = Block {
            header: BlockHeader {
                version: parent.header.version,
                chain_id: parent.header.chain_id,
                height: parent.header.height.checked_next().unwrap(),
                previous_block_hash: parent.block_hash().unwrap(),
                timestamp: parent.header.timestamp,
                transactions_root: Hash::ZERO,
                state_root: parent.header.state_root,
                proposer: AccountId::ZERO,
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

    fn tip_block(store: &RocksDbChainStore) -> Block {
        let tip = store.get_canonical_tip().unwrap().unwrap();
        store.get_block(&tip).unwrap().unwrap()
    }

    #[test]
    fn block_zero_rebuilds_an_empty_ledger() {
        let (_dir, state, store) = opened();
        let tip = store.get_canonical_tip().unwrap();
        let root = state.state_root().unwrap();
        let ledger = super::rebuild_ledger(&store).unwrap();
        assert!(ledger.is_empty());
        assert_eq!(ledger.root(), Ledger::new().root());
        assert_eq!(store.get_canonical_tip().unwrap(), tip);
        assert_eq!(state.state_root().unwrap(), root);
        assert_eq!(super::rebuild_ledger(&store).unwrap().root(), ledger.root());
    }

    #[test]
    fn rebuild_keeps_canonical_transaction_order() {
        let (_dir, state, store) = opened();
        let root = state.state_root().unwrap();
        let first = transfer(0, 10);
        let second = transfer(1, 20);
        let genesis = tip_block(&store);
        let child = append_block(
            &store,
            &genesis,
            vec![first.canonical_bytes(), second.canonical_bytes()],
        );
        let later = append_block(&store, &child, vec![transfer(2, 30).canonical_bytes()]);
        let tip = store.get_canonical_tip().unwrap();
        let ledger = super::rebuild_ledger(&store).unwrap();
        assert_eq!(ledger.len(), 3);
        assert_eq!(ledger.get(0).unwrap().transaction_id, first.id());
        assert_eq!(ledger.get(1).unwrap().transaction_id, second.id());
        assert_eq!(ledger.get(2).unwrap().transaction_id, transfer(2, 30).id());
        let mut expected = Ledger::new();
        expected.append(first.id()).unwrap();
        expected.append(second.id()).unwrap();
        expected.append(transfer(2, 30).id()).unwrap();
        assert_eq!(ledger.root(), expected.root());
        assert_eq!(store.get_canonical_tip().unwrap(), tip);
        assert_eq!(
            store
                .get_block(&later.block_hash().unwrap())
                .unwrap()
                .unwrap(),
            later
        );
        assert_eq!(state.state_root().unwrap(), root);
    }

    #[test]
    fn reversed_transaction_order_changes_the_root() {
        let (_dir, _state, store) = opened();
        let first = transfer(0, 10);
        let second = transfer(1, 20);
        let genesis = tip_block(&store);
        append_block(
            &store,
            &genesis,
            vec![second.canonical_bytes(), first.canonical_bytes()],
        );
        let ledger = super::rebuild_ledger(&store).unwrap();
        assert_eq!(ledger.get(0).unwrap().transaction_id, second.id());
        assert_eq!(ledger.get(1).unwrap().transaction_id, first.id());
        let mut forward = Ledger::new();
        forward.append(first.id()).unwrap();
        forward.append(second.id()).unwrap();
        assert_ne!(ledger.root(), forward.root());
    }

    #[test]
    fn duplicate_transaction_stops_the_rebuild() {
        let (_dir, _state, store) = opened();
        let tx = transfer(0, 10);
        let genesis = tip_block(&store);
        let child = append_block(&store, &genesis, vec![tx.canonical_bytes()]);
        append_block(&store, &child, vec![tx.canonical_bytes()]);
        let tip = store.get_canonical_tip().unwrap();
        let err = super::rebuild_ledger(&store).unwrap_err();
        assert_eq!(err, "duplicate transaction");
        assert_eq!(store.get_canonical_tip().unwrap(), tip);
    }

    #[test]
    fn missing_parent_stops_the_rebuild() {
        let (_dir, _state, store) = opened();
        let genesis = tip_block(&store);
        let mut orphan = genesis.clone();
        orphan.header.height = BlockHeight(1);
        orphan.header.previous_block_hash = rsc_core::BlockHash(Hash([9u8; 32]));
        orphan.header.transactions_root = orphan.compute_transaction_root();
        store.put_block(&orphan).unwrap();
        store
            .set_canonical_tip(&orphan.block_hash().unwrap())
            .unwrap();
        let tip = store.get_canonical_tip().unwrap();
        let err = super::rebuild_ledger(&store).unwrap_err();
        assert_eq!(err, "bloque ausente");
        assert_eq!(store.get_canonical_tip().unwrap(), tip);
        assert_eq!(tip_block(&store), orphan);
    }

    #[test]
    fn services_start_on_the_genesis_tip() {
        let (_dir, state, store) = opened();
        let tip = store.get_canonical_tip().unwrap();
        let root = state.state_root().unwrap();
        let ledger = super::rebuild_ledger(&store).unwrap();
        let (pool, execution, consensus) = super::open_services(&store).unwrap();
        assert!(pool.is_empty());
        assert_eq!(pool.status(), MempoolStatus::Empty);
        assert_eq!(
            execution.config().environment_id(),
            EXECUTION_ENVIRONMENT_ID
        );
        assert_eq!(consensus.state().height, 0);
        assert_eq!(consensus.state().view, 0);
        assert_eq!(consensus.state().phase, ConsensusPhase::NewView);
        assert!(consensus.state().proposal.is_none());
        assert_eq!(store.get_canonical_tip().unwrap(), tip);
        assert_eq!(state.state_root().unwrap(), root);
        assert_eq!(super::rebuild_ledger(&store).unwrap().root(), ledger.root());
    }

    #[test]
    fn genesis_tip_becomes_the_proposal() {
        let (_dir, state, store) = opened();
        let tip = store.get_canonical_tip().unwrap();
        let root = state.state_root().unwrap();
        let ledger = super::rebuild_ledger(&store).unwrap();
        let (pool, execution, mut consensus) = super::open_services(&store).unwrap();
        super::propose_tip(&store, &mut consensus).unwrap();
        assert_eq!(consensus.state().phase, ConsensusPhase::Propose);
        assert_eq!(consensus.state().proposal, tip);
        assert_eq!(consensus.state().height, 0);
        assert_eq!(consensus.state().view, 0);
        assert!(consensus.state().locked.is_none());
        assert!(pool.is_empty());
        assert_eq!(
            execution.config().environment_id(),
            EXECUTION_ENVIRONMENT_ID
        );
        assert_eq!(store.get_canonical_tip().unwrap(), tip);
        assert_eq!(state.state_root().unwrap(), root);
        assert_eq!(super::rebuild_ledger(&store).unwrap().root(), ledger.root());
    }

    #[test]
    fn local_prepare_vote_matches_the_tip() {
        let (_dir, state, store) = opened();
        let tip = store.get_canonical_tip().unwrap();
        let root = state.state_root().unwrap();
        let ledger = super::rebuild_ledger(&store).unwrap();
        let (pool, execution, mut consensus) = super::open_services(&store).unwrap();
        super::propose_tip(&store, &mut consensus).unwrap();
        let vote =
            super::cast_local_vote(&store, &mut consensus, rsc_consensus::VotePhase::Prepare)
                .unwrap();
        assert_eq!(vote.block_hash, tip.unwrap());
        assert_eq!(vote.validator, AccountId::ZERO);
        assert_eq!(vote.phase, rsc_consensus::VotePhase::Prepare);
        assert_eq!(vote.epoch, consensus.state().epoch);
        assert_eq!(vote.height, 0);
        assert_eq!(vote.view, 0);
        assert_eq!(consensus.state().phase, ConsensusPhase::Propose);
        assert!(consensus.state().locked.is_none());
        let mut altered = vote;
        altered.signature.0[0] ^= 0xff;
        let err = consensus.receive_vote(altered).unwrap_err();
        assert_eq!(err.to_string(), "invalid signature");
        assert_eq!(consensus.state().phase, ConsensusPhase::Propose);
        assert_eq!(consensus.state().proposal, tip);
        assert!(pool.is_empty());
        assert_eq!(
            execution.config().environment_id(),
            EXECUTION_ENVIRONMENT_ID
        );
        assert_eq!(store.get_canonical_tip().unwrap(), tip);
        assert_eq!(state.state_root().unwrap(), root);
        assert_eq!(super::rebuild_ledger(&store).unwrap().root(), ledger.root());
    }

    #[test]
    fn prepare_quorum_verifies_the_certificate() {
        let (_dir, state, store) = opened();
        let tip = store.get_canonical_tip().unwrap();
        let root = state.state_root().unwrap();
        let ledger = super::rebuild_ledger(&store).unwrap();
        let (pool, execution, mut consensus) = super::open_services(&store).unwrap();
        super::propose_tip(&store, &mut consensus).unwrap();
        let vote =
            super::cast_local_vote(&store, &mut consensus, rsc_consensus::VotePhase::Prepare)
                .unwrap();
        let certificate = super::accept_prepare_quorum(&mut consensus, &vote).unwrap();
        assert_eq!(certificate.block_hash, tip.unwrap());
        assert_eq!(certificate.phase, rsc_consensus::VotePhase::Prepare);
        assert_eq!(certificate.height, 0);
        assert_eq!(certificate.view, 0);
        assert_eq!(certificate.epoch, vote.epoch);
        assert_eq!(consensus.state().phase, ConsensusPhase::Prepare);
        assert!(consensus.state().locked.is_none());
        assert_eq!(consensus.state().height, 0);
        assert_eq!(consensus.state().proposal, tip);
        assert!(pool.is_empty());
        assert_eq!(
            execution.config().environment_id(),
            EXECUTION_ENVIRONMENT_ID
        );
        assert_eq!(store.get_canonical_tip().unwrap(), tip);
        assert_eq!(state.state_root().unwrap(), root);
        assert_eq!(super::rebuild_ledger(&store).unwrap().root(), ledger.root());
    }

    #[test]
    fn precommit_quorum_locks_the_same_tip() {
        let (_dir, state, store) = opened();
        let tip = store.get_canonical_tip().unwrap();
        let root = state.state_root().unwrap();
        let ledger = super::rebuild_ledger(&store).unwrap();
        let (pool, execution, mut consensus) = super::open_services(&store).unwrap();
        super::propose_tip(&store, &mut consensus).unwrap();
        let prepare =
            super::cast_local_vote(&store, &mut consensus, rsc_consensus::VotePhase::Prepare)
                .unwrap();
        super::accept_prepare_quorum(&mut consensus, &prepare).unwrap();
        let precommit =
            super::cast_local_vote(&store, &mut consensus, rsc_consensus::VotePhase::PreCommit)
                .unwrap();
        let certificate = super::accept_precommit_quorum(&mut consensus, &precommit).unwrap();
        assert_eq!(precommit.block_hash, tip.unwrap());
        assert_eq!(precommit.phase, rsc_consensus::VotePhase::PreCommit);
        assert_eq!(certificate.block_hash, tip.unwrap());
        assert_eq!(certificate.phase, rsc_consensus::VotePhase::PreCommit);
        assert_eq!(certificate.height, 0);
        assert_eq!(certificate.view, 0);
        assert_eq!(certificate.epoch, precommit.epoch);
        assert_eq!(consensus.state().phase, ConsensusPhase::PreCommit);
        assert_eq!(consensus.state().locked, tip);
        assert_eq!(consensus.state().height, 0);
        assert_eq!(consensus.state().view, 0);
        assert_eq!(consensus.state().proposal, tip);
        assert!(pool.is_empty());
        assert_eq!(
            execution.config().environment_id(),
            EXECUTION_ENVIRONMENT_ID
        );
        assert_eq!(store.get_canonical_tip().unwrap(), tip);
        assert_eq!(state.state_root().unwrap(), root);
        assert_eq!(super::rebuild_ledger(&store).unwrap().root(), ledger.root());
    }

    #[test]
    fn commit_proof_finalizes_the_same_tip() {
        let (_dir, state, store) = opened();
        let tip = store.get_canonical_tip().unwrap();
        let root = state.state_root().unwrap();
        let ledger = super::rebuild_ledger(&store).unwrap();
        let (pool, execution, mut consensus) = super::open_services(&store).unwrap();
        super::propose_tip(&store, &mut consensus).unwrap();
        let prepare =
            super::cast_local_vote(&store, &mut consensus, rsc_consensus::VotePhase::Prepare)
                .unwrap();
        super::accept_prepare_quorum(&mut consensus, &prepare).unwrap();
        let precommit =
            super::cast_local_vote(&store, &mut consensus, rsc_consensus::VotePhase::PreCommit)
                .unwrap();
        super::accept_precommit_quorum(&mut consensus, &precommit).unwrap();
        let commit =
            super::cast_local_vote(&store, &mut consensus, rsc_consensus::VotePhase::Commit)
                .unwrap();
        let proof = super::accept_commit(&mut consensus, &commit).unwrap();
        assert_eq!(commit.phase, rsc_consensus::VotePhase::Commit);
        assert_eq!(proof.block_hash, tip.unwrap());
        assert_eq!(proof.height, 0);
        assert_eq!(proof.view, 0);
        assert_eq!(proof.commit.phase, rsc_consensus::VotePhase::Commit);
        assert_eq!(consensus.state().phase, ConsensusPhase::Decide);
        assert_eq!(consensus.state().locked, tip);
        assert_eq!(consensus.state().height, 0);
        assert_eq!(consensus.state().proposal, tip);
        assert!(pool.is_empty());
        assert_eq!(
            execution.config().environment_id(),
            EXECUTION_ENVIRONMENT_ID
        );
        assert_eq!(store.get_canonical_tip().unwrap(), tip);
        assert_eq!(state.state_root().unwrap(), root);
        assert_eq!(super::rebuild_ledger(&store).unwrap().root(), ledger.root());
    }

    #[test]
    fn finalized_tip_advances_the_engine_without_a_new_block() {
        let (_dir, state, store) = opened();
        let tip = store.get_canonical_tip().unwrap();
        let root = state.state_root().unwrap();
        let version = state.version().unwrap();
        let ledger = super::rebuild_ledger(&store).unwrap();
        let (pool, execution, mut consensus) = super::open_services(&store).unwrap();
        super::propose_tip(&store, &mut consensus).unwrap();
        let prepare =
            super::cast_local_vote(&store, &mut consensus, rsc_consensus::VotePhase::Prepare)
                .unwrap();
        super::accept_prepare_quorum(&mut consensus, &prepare).unwrap();
        let precommit =
            super::cast_local_vote(&store, &mut consensus, rsc_consensus::VotePhase::PreCommit)
                .unwrap();
        super::accept_precommit_quorum(&mut consensus, &precommit).unwrap();
        let commit =
            super::cast_local_vote(&store, &mut consensus, rsc_consensus::VotePhase::Commit)
                .unwrap();
        let proof = super::accept_commit(&mut consensus, &commit).unwrap();
        super::apply_finalized(&state, &store, &mut consensus, &proof).unwrap();
        assert_eq!(consensus.state().height, 1);
        assert_eq!(consensus.state().epoch, 1);
        assert_eq!(consensus.state().view, 0);
        assert_eq!(consensus.state().phase, ConsensusPhase::NewView);
        assert!(consensus.state().proposal.is_none());
        assert!(consensus.state().locked.is_none());
        assert_eq!(
            consensus.network_mode(),
            rsc_consensus::NetworkMode::Distributed
        );
        assert!(pool.is_empty());
        assert_eq!(
            execution.config().environment_id(),
            EXECUTION_ENVIRONMENT_ID
        );
        assert_eq!(store.get_canonical_tip().unwrap(), tip);
        assert_eq!(state.state_root().unwrap(), root);
        assert_eq!(state.version().unwrap(), version);
        assert_eq!(super::rebuild_ledger(&store).unwrap().root(), ledger.root());
    }

    #[test]
    fn a_signed_transfer_is_ready_at_the_new_height() {
        let (_dir, state, store) = opened();
        let tip = store.get_canonical_tip().unwrap();
        let root = state.state_root().unwrap();
        let version = state.version().unwrap();
        let ledger = super::rebuild_ledger(&store).unwrap();
        let (pool, execution, mut consensus) = super::open_services(&store).unwrap();
        super::propose_tip(&store, &mut consensus).unwrap();
        let prepare =
            super::cast_local_vote(&store, &mut consensus, rsc_consensus::VotePhase::Prepare)
                .unwrap();
        super::accept_prepare_quorum(&mut consensus, &prepare).unwrap();
        let precommit =
            super::cast_local_vote(&store, &mut consensus, rsc_consensus::VotePhase::PreCommit)
                .unwrap();
        super::accept_precommit_quorum(&mut consensus, &precommit).unwrap();
        let commit =
            super::cast_local_vote(&store, &mut consensus, rsc_consensus::VotePhase::Commit)
                .unwrap();
        let proof = super::accept_commit(&mut consensus, &commit).unwrap();
        super::apply_finalized(&state, &store, &mut consensus, &proof).unwrap();
        assert_eq!(consensus.state().height, 1);
        assert_eq!(consensus.state().phase, ConsensusPhase::NewView);
        assert!(pool.is_empty());

        let key = rsc_crypto::PrivateKey::from_bytes(&[0x11; 32]).unwrap();
        let signed = signed_genesis_transfer(0, 7);
        signed.validate(&key.public_key()).unwrap();
        let hash = pool.insert(signed.canonical_bytes()).unwrap();
        assert_eq!(pool.queue_state(&hash), Some(TransactionQueueState::Ready));
        assert_eq!(pool.info().ready_count, 1);
        assert_eq!(pool.status(), MempoolStatus::NonEmpty);
        assert_eq!(
            pool.insert(vec![0x00]).unwrap_err().to_string(),
            "transaction is invalid"
        );
        assert_eq!(
            pool.insert(signed_genesis_transfer(0, 0).canonical_bytes())
                .unwrap_err()
                .to_string(),
            "transaction validation failed"
        );
        assert_eq!(
            pool.insert(transfer(0, 1).canonical_bytes())
                .unwrap_err()
                .to_string(),
            "transaction validation failed"
        );
        assert_eq!(pool.info().ready_count, 1);
        assert_eq!(pool.len(), 1);
        assert_eq!(
            execution.config().environment_id(),
            EXECUTION_ENVIRONMENT_ID
        );
        assert_eq!(consensus.state().height, 1);
        assert_eq!(consensus.state().phase, ConsensusPhase::NewView);
        assert!(consensus.state().proposal.is_none());
        assert_eq!(store.get_canonical_tip().unwrap(), tip);
        assert_eq!(state.state_root().unwrap(), root);
        assert_eq!(state.version().unwrap(), version);
        assert_eq!(super::rebuild_ledger(&store).unwrap().root(), ledger.root());
    }

    #[test]
    fn a_ready_transfer_without_a_recipient_does_not_write() {
        let (_dir, state, store) = opened();
        let tip = store.get_canonical_tip().unwrap();
        let root = state.state_root().unwrap();
        let version = state.version().unwrap();
        let ledger = super::rebuild_ledger(&store).unwrap();
        let (pool, execution, mut consensus) = super::open_services(&store).unwrap();
        reach_new_view(&state, &store, &mut consensus);
        let signed = signed_genesis_transfer(0, 7);
        let hash = pool.insert(signed.canonical_bytes()).unwrap();
        let err = super::prepare_ready(&state, &pool).unwrap_err();
        assert_eq!(err, "account not found");
        assert_eq!(pool.queue_state(&hash), Some(TransactionQueueState::Ready));
        assert_eq!(pool.info().ready_count, 1);
        assert_eq!(
            execution.config().environment_id(),
            EXECUTION_ENVIRONMENT_ID
        );
        assert_eq!(consensus.state().height, 1);
        assert_eq!(consensus.state().phase, ConsensusPhase::NewView);
        assert_eq!(store.get_canonical_tip().unwrap(), tip);
        assert_eq!(state.state_root().unwrap(), root);
        assert_eq!(state.version().unwrap(), version);
        assert_eq!(super::rebuild_ledger(&store).unwrap().root(), ledger.root());
    }

    #[test]
    fn a_ready_transfer_applies_the_state_transition() {
        let (_dir, state, store, peer) = opened_pair();
        let tip = store.get_canonical_tip().unwrap();
        let root = state.state_root().unwrap();
        let version = state.version().unwrap();
        let ledger = super::rebuild_ledger(&store).unwrap();
        let (pool, execution, mut consensus) = super::open_services(&store).unwrap();
        reach_new_view(&state, &store, &mut consensus);
        let too_much = signed_to(peer, 0, 501);
        let rejected = pool.insert(too_much.canonical_bytes()).unwrap();
        assert_eq!(
            super::prepare_ready(&state, &pool).unwrap_err(),
            "insufficient balance"
        );
        assert_eq!(
            pool.queue_state(&rejected),
            Some(TransactionQueueState::Ready)
        );
        assert_eq!(state.state_root().unwrap(), root);
        assert_eq!(state.version().unwrap(), version);
        pool.remove(&rejected);

        let key = rsc_crypto::PrivateKey::from_bytes(&[0x11; 32]).unwrap();
        let signed = signed_to(peer, 0, 7);
        signed.validate(&key.public_key()).unwrap();
        let hash = pool.insert(signed.canonical_bytes()).unwrap();
        assert_eq!(pool.queue_state(&hash), Some(TransactionQueueState::Ready));
        super::prepare_ready(&state, &pool).unwrap();
        let sender = state
            .get_account(&derived_account().into())
            .unwrap()
            .unwrap();
        let receiver = state.get_account(&peer.into()).unwrap().unwrap();
        assert_eq!(sender.native_balance(), 493);
        assert_eq!(sender.nonce(), 1);
        assert_eq!(receiver.native_balance(), 107);
        assert_eq!(receiver.nonce(), 0);
        assert_eq!(state.version().unwrap(), version + 1);
        assert_ne!(state.state_root().unwrap(), root);
        assert_eq!(
            pool.queue_state(&hash),
            Some(TransactionQueueState::Pending)
        );
        assert_eq!(pool.info().ready_count, 0);
        assert_eq!(pool.len(), 1);
        super::prepare_ready(&state, &pool).unwrap();
        assert_eq!(state.version().unwrap(), version + 1);
        let stored = store.get_block(&tip.unwrap()).unwrap().unwrap();
        assert!(stored.transactions.is_empty());
        assert_eq!(stored.header.state_root, Hash(*root.as_bytes()));
        assert_eq!(
            execution.config().environment_id(),
            EXECUTION_ENVIRONMENT_ID
        );
        assert_eq!(consensus.state().height, 1);
        assert_eq!(consensus.state().phase, ConsensusPhase::NewView);
        assert!(consensus.state().proposal.is_none());
        assert_eq!(store.get_canonical_tip().unwrap(), tip);
        assert_eq!(super::rebuild_ledger(&store).unwrap().root(), ledger.root());
    }

    #[test]
    fn a_rejected_transfer_does_not_commit_a_block() {
        let (_dir, state, store) = opened();
        let tip = store.get_canonical_tip().unwrap();
        let root = state.state_root().unwrap();
        let version = state.version().unwrap();
        let ledger = super::rebuild_ledger(&store).unwrap();
        let (pool, _execution, mut consensus) = super::open_services(&store).unwrap();
        reach_new_view(&state, &store, &mut consensus);
        let hash = pool
            .insert(signed_genesis_transfer(0, 7).canonical_bytes())
            .unwrap();
        assert_eq!(
            super::commit_ready(&state, &store, &pool).unwrap_err(),
            "account not found"
        );
        assert_eq!(pool.queue_state(&hash), Some(TransactionQueueState::Ready));
        assert_eq!(store.get_canonical_tip().unwrap(), tip);
        assert!(store.get_children(&tip.unwrap()).unwrap().is_empty());
        assert_eq!(state.state_root().unwrap(), root);
        assert_eq!(state.version().unwrap(), version);
        assert_eq!(super::rebuild_ledger(&store).unwrap().root(), ledger.root());
        assert_eq!(consensus.state().height, 1);
        assert_eq!(consensus.state().phase, ConsensusPhase::NewView);
    }

    #[test]
    fn a_ready_transfer_commits_the_block_and_reopens() {
        let (dir, state, store, peer) = opened_pair();
        let path = dir.path().to_path_buf();
        let (pool, execution, mut consensus) = super::open_services(&store).unwrap();
        reach_new_view(&state, &store, &mut consensus);
        let genesis_tip = store.get_canonical_tip().unwrap().unwrap();
        let previous_root = state.state_root().unwrap();
        let version = state.version().unwrap();
        let key = rsc_crypto::PrivateKey::from_bytes(&[0x11; 32]).unwrap();
        let signed = signed_to(peer, 0, 7);
        signed.validate(&key.public_key()).unwrap();
        pool.insert(signed.canonical_bytes()).unwrap();
        super::commit_ready(&state, &store, &pool).unwrap();
        let tip = store.get_canonical_tip().unwrap().unwrap();
        let block = store.get_block(&tip).unwrap().unwrap();
        let ledger_root = super::rebuild_ledger(&store).unwrap().root();
        assert_ne!(tip, genesis_tip);
        assert_eq!(block.header.height, BlockHeight(1));
        assert_eq!(block.header.previous_block_hash, genesis_tip);
        assert_eq!(block.transactions.len(), 1);
        assert_eq!(
            block.header.state_root,
            Hash(*state.state_root().unwrap().as_bytes())
        );
        assert_ne!(state.state_root().unwrap(), previous_root);
        assert_eq!(state.version().unwrap(), version + 1);
        assert_eq!(store.get_children(&genesis_tip).unwrap(), vec![tip]);
        assert!(pool.is_empty());
        super::commit_ready(&state, &store, &pool).unwrap();
        assert_eq!(store.get_canonical_tip().unwrap().unwrap(), tip);
        assert_eq!(state.version().unwrap(), version + 1);
        assert_eq!(consensus.state().height, 1);
        assert_eq!(consensus.state().phase, ConsensusPhase::NewView);
        assert_eq!(
            execution.config().environment_id(),
            EXECUTION_ENVIRONMENT_ID
        );
        drop(pool);
        drop(execution);
        drop(consensus);
        drop(state);
        drop(store);
        let config = config_at(path.to_str().unwrap(), "devnet", 3);
        let db = Arc::new(open_database(&config).unwrap());
        let (state, store) = share_database(db).unwrap();
        assert_eq!(store.get_canonical_tip().unwrap().unwrap(), tip);
        assert_eq!(
            store.get_block_by_height(BlockHeight(0)).unwrap(),
            Some(genesis_tip)
        );
        assert_eq!(
            store.get_block_by_height(BlockHeight(1)).unwrap(),
            Some(tip)
        );
        let block = store.get_block(&tip).unwrap().unwrap();
        assert_eq!(block.header.height, BlockHeight(1));
        assert_eq!(block.transactions.len(), 1);
        let transaction = Transaction::from_canonical_bytes(&block.transactions[0].0).unwrap();
        assert_eq!(transaction.sender, derived_account());
        assert_eq!(transaction.nonce, 0);
        assert_eq!(
            Hash(*state.state_root().unwrap().as_bytes()),
            block.header.state_root
        );
        assert_eq!(state.version().unwrap(), version + 1);
        let sender = state
            .get_account(&derived_account().into())
            .unwrap()
            .unwrap();
        let receiver = state.get_account(&peer.into()).unwrap().unwrap();
        assert_eq!(sender.native_balance(), 493);
        assert_eq!(sender.nonce(), 1);
        assert_eq!(receiver.native_balance(), 107);
        assert_eq!(receiver.nonce(), 0);
        assert_eq!(super::rebuild_ledger(&store).unwrap().root(), ledger_root);
        assert_eq!(store.get_children(&genesis_tip).unwrap(), vec![tip]);
    }

    #[test]
    fn the_engine_advances_past_the_committed_tip() {
        let (_dir, state, store, peer) = opened_pair();
        let (pool, _execution, mut consensus) = super::open_services(&store).unwrap();
        reach_new_view(&state, &store, &mut consensus);
        super::align_engine(&state, &store, &mut consensus).unwrap();
        assert_eq!(consensus.state().height, 1);
        assert_eq!(consensus.state().phase, ConsensusPhase::NewView);
        let root = state.state_root().unwrap();
        let version = state.version().unwrap();
        let ledger = super::rebuild_ledger(&store).unwrap();
        let signed = signed_to(peer, 0, 7);
        pool.insert(signed.canonical_bytes()).unwrap();
        super::commit_ready(&state, &store, &pool).unwrap();
        let tip = store.get_canonical_tip().unwrap().unwrap();
        let committed_root = state.state_root().unwrap();
        let committed_version = state.version().unwrap();
        let committed_ledger = super::rebuild_ledger(&store).unwrap().root();
        assert_eq!(
            store.get_block(&tip).unwrap().unwrap().header.height,
            BlockHeight(1)
        );
        assert_eq!(consensus.state().height, 1);
        assert_eq!(consensus.state().phase, ConsensusPhase::NewView);
        let err = consensus.new_height(None, Hash::ZERO).unwrap_err();
        assert_eq!(err.to_string(), "invalid phase");
        assert_eq!(consensus.state().height, 1);
        assert!(consensus.state().proposal.is_none());
        assert!(consensus.state().locked.is_none());
        super::align_engine(&state, &store, &mut consensus).unwrap();
        assert_eq!(consensus.state().height, 2);
        assert_eq!(consensus.state().epoch, 2);
        assert_eq!(consensus.state().view, 0);
        assert_eq!(consensus.state().phase, ConsensusPhase::NewView);
        assert!(consensus.state().proposal.is_none());
        assert!(consensus.state().locked.is_none());
        assert_eq!(
            consensus.network_mode(),
            rsc_consensus::NetworkMode::Distributed
        );
        assert_eq!(store.get_canonical_tip().unwrap().unwrap(), tip);
        assert!(store.get_block_by_height(BlockHeight(2)).unwrap().is_none());
        assert!(store.get_children(&tip).unwrap().is_empty());
        assert_eq!(state.state_root().unwrap(), committed_root);
        assert_ne!(committed_root, root);
        assert_eq!(state.version().unwrap(), committed_version);
        assert_eq!(committed_version, version + 1);
        assert_eq!(
            super::rebuild_ledger(&store).unwrap().root(),
            committed_ledger
        );
        assert_ne!(committed_ledger, ledger.root());
        super::align_engine(&state, &store, &mut consensus).unwrap();
        assert_eq!(consensus.state().height, 2);
        assert_eq!(store.get_canonical_tip().unwrap().unwrap(), tip);
    }

    #[test]
    fn a_restart_rebuilds_the_committed_chain_without_rewriting_it() {
        let (dir, state, store, peer) = opened_pair();
        let path = dir.path().to_path_buf();
        let (pool, execution, mut consensus) = super::open_services(&store).unwrap();
        reach_new_view(&state, &store, &mut consensus);
        let signed = signed_to(peer, 0, 7);
        let transaction_id = signed.id();
        pool.insert(signed.canonical_bytes()).unwrap();
        super::commit_ready(&state, &store, &pool).unwrap();
        super::align_engine(&state, &store, &mut consensus).unwrap();
        let tip = store.get_canonical_tip().unwrap().unwrap();
        let block_bytes =
            rsc_chain::encode_block(&store.get_block(&tip).unwrap().unwrap()).unwrap();
        let root = state.state_root().unwrap();
        let version = state.version().unwrap();
        let ledger_root = super::rebuild_ledger(&store).unwrap().root();
        let sender = state
            .get_account(&derived_account().into())
            .unwrap()
            .unwrap();
        let receiver = state.get_account(&peer.into()).unwrap().unwrap();
        let sender_balance = sender.native_balance();
        let sender_nonce = sender.nonce();
        let receiver_balance = receiver.native_balance();
        let receiver_nonce = receiver.nonce();
        assert_eq!(consensus.state().height, 2);
        assert_eq!(consensus.state().epoch, 2);
        assert_eq!(consensus.state().view, 0);
        assert_eq!(consensus.state().phase, ConsensusPhase::NewView);
        assert!(consensus.state().proposal.is_none());
        assert!(consensus.state().locked.is_none());
        assert_eq!(
            consensus.network_mode(),
            rsc_consensus::NetworkMode::Distributed
        );
        drop(pool);
        drop(execution);
        drop(consensus);
        drop(state);
        drop(store);

        let path_str = path.to_str().unwrap();
        assert!(matches!(
            open_database(&config_at(path_str, "other", 3)),
            Err(StorageError::NetworkMismatch)
        ));
        assert!(matches!(
            open_database(&config_at(path_str, "devnet", 4)),
            Err(StorageError::ChainMismatch)
        ));
        let config = config_at(path_str, "devnet", 3);
        let db = Arc::new(open_database(&config).unwrap());
        let (state, store) = share_database(db).unwrap();
        super::ensure_tip(&state, &store, &config).unwrap();
        assert_eq!(store.get_canonical_tip().unwrap().unwrap(), tip);
        assert_eq!(
            rsc_chain::encode_block(&store.get_block(&tip).unwrap().unwrap()).unwrap(),
            block_bytes
        );
        let stored = Transaction::from_canonical_bytes(
            &store.get_block(&tip).unwrap().unwrap().transactions[0].0,
        )
        .unwrap();
        assert_eq!(stored.id(), transaction_id);
        assert_eq!(state.state_root().unwrap(), root);
        assert_eq!(state.version().unwrap(), version);
        assert_eq!(super::rebuild_ledger(&store).unwrap().root(), ledger_root);
        let sender = state
            .get_account(&derived_account().into())
            .unwrap()
            .unwrap();
        let receiver = state.get_account(&peer.into()).unwrap().unwrap();
        assert_eq!(sender.native_balance(), sender_balance);
        assert_eq!(sender.nonce(), sender_nonce);
        assert_eq!(receiver.native_balance(), receiver_balance);
        assert_eq!(receiver.nonce(), receiver_nonce);
        let (pool, execution, mut consensus) = super::open_services(&store).unwrap();
        assert!(pool.is_empty());
        assert_eq!(consensus.state().height, 1);
        assert_eq!(consensus.state().phase, ConsensusPhase::NewView);
        assert_eq!(
            consensus.network_mode(),
            rsc_consensus::NetworkMode::Bootstrap
        );
        super::resume(&state, &store, &pool, &mut consensus).unwrap();
        assert_eq!(store.get_canonical_tip().unwrap().unwrap(), tip);
        assert_eq!(
            rsc_chain::encode_block(&store.get_block(&tip).unwrap().unwrap()).unwrap(),
            block_bytes
        );
        assert_eq!(state.state_root().unwrap(), root);
        assert_eq!(state.version().unwrap(), version);
        assert_eq!(super::rebuild_ledger(&store).unwrap().root(), ledger_root);
        let sender = state
            .get_account(&derived_account().into())
            .unwrap()
            .unwrap();
        let receiver = state.get_account(&peer.into()).unwrap().unwrap();
        assert_eq!(sender.native_balance(), sender_balance);
        assert_eq!(sender.nonce(), sender_nonce);
        assert_eq!(receiver.native_balance(), receiver_balance);
        assert_eq!(receiver.nonce(), receiver_nonce);
        assert_eq!(consensus.state().height, 2);
        assert_eq!(consensus.state().epoch, 2);
        assert_eq!(consensus.state().view, 0);
        assert_eq!(consensus.state().phase, ConsensusPhase::NewView);
        assert!(consensus.state().proposal.is_none());
        assert!(consensus.state().locked.is_none());
        assert_eq!(
            consensus.network_mode(),
            rsc_consensus::NetworkMode::Distributed
        );
        assert!(pool.is_empty());
        assert!(store.get_block_by_height(BlockHeight(2)).unwrap().is_none());
        assert_eq!(
            execution.config().environment_id(),
            EXECUTION_ENVIRONMENT_ID
        );
    }

    #[test]
    fn the_operator_process_commits_and_recovers() {
        let dir = tempfile::tempdir().unwrap();
        let (genesis, peer) = write_pair_genesis(dir.path());
        let path = dir.path().to_str().unwrap().to_string();
        let config = parse_args(args(&[
            "--data-dir",
            &path,
            "--network-id",
            "devnet",
            "--chain-id",
            "3",
            "--genesis",
            &genesis,
        ]))
        .unwrap();
        let db = Arc::new(open_database(&config).unwrap());
        let (state, store) = share_database(db).unwrap();
        super::ensure_tip(&state, &store, &config).unwrap();
        assert_eq!(
            super::rebuild_ledger(&store).unwrap().root(),
            Ledger::new().root()
        );
        let (pool, execution, mut consensus) = super::open_services(&store).unwrap();
        super::resume(&state, &store, &pool, &mut consensus).unwrap();
        assert_eq!(
            store
                .get_block(&store.get_canonical_tip().unwrap().unwrap())
                .unwrap()
                .unwrap()
                .header
                .height,
            BlockHeight::GENESIS
        );
        assert_eq!(consensus.state().height, 1);
        assert!(pool.is_empty());

        let key = rsc_crypto::PrivateKey::from_bytes(&[0x11; 32]).unwrap();
        let signed = signed_to(peer, 0, 7);
        signed.validate(&key.public_key()).unwrap();
        let tx_hex = rsc_rpc::encode_hex(&signed.canonical_bytes());
        let tx_id = rsc_rpc::encode_hex(signed.id().0.as_bytes());
        let rpc_state = State::open(store.database()).unwrap();
        let rpc_store = RocksDbChainStore::from_shared(store.database());
        let (extra_pool, extra_execution, rpc_consensus) =
            super::open_services(&rpc_store).unwrap();
        let ctx = super::rpc_context(
            &config,
            rpc_state,
            rpc_store,
            rpc_consensus,
            Arc::clone(&pool),
        );
        let rejected = wait(rsc_rpc::methods::dispatch(
            &ctx,
            "rsc_sendTransaction",
            Some(&serde_json::json!({ "signed_transaction": "00" })),
        ))
        .unwrap_err();
        assert_eq!(rejected, rsc_rpc::RpcError::TransactionRejected);
        let accepted = wait(rsc_rpc::methods::dispatch(
            &ctx,
            "rsc_sendTransaction",
            Some(&serde_json::json!({ "signed_transaction": tx_hex })),
        ))
        .unwrap();
        assert_eq!(accepted["transaction_hash"], serde_json::json!(tx_id));
        assert_eq!(pool.info().ready_count, 1);
        super::commit_ready(&state, &store, &pool).unwrap();
        super::align_engine(&state, &store, &mut consensus).unwrap();
        let tip = store.get_canonical_tip().unwrap().unwrap();
        let block = store.get_block(&tip).unwrap().unwrap();
        let block_bytes = rsc_chain::encode_block(&block).unwrap();
        let root = state.state_root().unwrap();
        let version = state.version().unwrap();
        let ledger_root = super::rebuild_ledger(&store).unwrap().root();
        assert_eq!(block.header.height, BlockHeight(1));
        assert_eq!(block.header.state_root, Hash(*root.as_bytes()));
        assert_eq!(block.transactions.len(), 1);
        assert!(pool.is_empty());
        assert_eq!(consensus.state().height, 2);
        assert_eq!(consensus.state().epoch, 2);
        assert_eq!(consensus.state().view, 0);
        assert_eq!(consensus.state().phase, ConsensusPhase::NewView);
        assert!(consensus.state().proposal.is_none());
        assert!(consensus.state().locked.is_none());
        assert_eq!(
            consensus.network_mode(),
            rsc_consensus::NetworkMode::Distributed
        );
        let sender = state
            .get_account(&derived_account().into())
            .unwrap()
            .unwrap();
        let receiver = state.get_account(&peer.into()).unwrap().unwrap();
        assert_eq!(sender.native_balance(), 493);
        assert_eq!(sender.nonce(), 1);
        assert_eq!(receiver.native_balance(), 107);
        assert_eq!(receiver.nonce(), 0);
        let seen = wait(rsc_rpc::methods::dispatch(
            &ctx,
            "rsc_getBlockByHeight",
            Some(&serde_json::json!({ "height": 1 })),
        ))
        .unwrap();
        assert_eq!(seen["transactions"], serde_json::json!([tx_id]));
        let account = wait(rsc_rpc::methods::dispatch(
            &ctx,
            "rsc_getAccount",
            Some(&serde_json::json!({
                "account": rsc_rpc::encode_hex(&derived_account().0)
            })),
        ))
        .unwrap();
        assert_eq!(account["balance"], serde_json::json!("493"));
        assert_eq!(account["nonce"], serde_json::json!(1));
        let finality = wait(rsc_rpc::methods::dispatch(
            &ctx,
            "rsc_getFinality",
            Some(&serde_json::json!({})),
        ))
        .unwrap();
        assert_eq!(finality["finalized_height"], serde_json::json!(1));
        assert_eq!(
            finality["finalized_hash"],
            serde_json::json!(rsc_rpc::encode_hex(&tip.0 .0))
        );
        assert_eq!(finality["view"], serde_json::json!(0));
        let status = wait(rsc_rpc::methods::dispatch(
            &ctx,
            "rsc_getStatus",
            Some(&serde_json::json!({})),
        ))
        .unwrap();
        assert_eq!(status["latest_height"], serde_json::json!(1));
        assert_eq!(status["finalized_height"], serde_json::json!(1));
        let p2p = super::p2p_config(&config).unwrap();
        let server = rsc_rpc::RpcServer::new(ctx.clone(), rsc_rpc::RpcConfig::default());
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async move {
                let task = tokio::spawn(async move {
                    let _ = server.run().await;
                });
                let mut connected = false;
                for _ in 0..50 {
                    if std::net::TcpStream::connect("127.0.0.1:8545").is_ok() {
                        connected = true;
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
                assert!(connected);
                let mut stream = std::net::TcpStream::connect("127.0.0.1:8545").unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                    .unwrap();
                std::io::Write::write_all(
                    &mut stream,
                    b"GET /health HTTP/1.1\r\nHost: 127.0.0.1:8545\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
                std::io::Write::flush(&mut stream).unwrap();
                let mut body = Vec::new();
                std::io::Read::read_to_end(&mut stream, &mut body).unwrap();
                assert!(body.windows(15).any(|chunk| chunk == br#"{"status":"ok"}"#));
                let mut network = super::start_p2p(p2p).await.unwrap();
                super::stop_p2p(&mut network).await.unwrap();
                task.abort();
            });
        assert_eq!(
            execution.config().environment_id(),
            EXECUTION_ENVIRONMENT_ID
        );
        drop(ctx);
        drop(extra_pool);
        drop(extra_execution);
        drop(pool);
        drop(execution);
        drop(consensus);
        drop(state);
        drop(store);

        let recovered = parse_args(args(&[
            "--data-dir",
            &path,
            "--network-id",
            "devnet",
            "--chain-id",
            "3",
        ]))
        .unwrap();
        let db = Arc::new(open_database(&recovered).unwrap());
        let (state, store) = share_database(db).unwrap();
        super::ensure_tip(&state, &store, &recovered).unwrap();
        let (pool, _execution, mut consensus) = super::open_services(&store).unwrap();
        assert!(pool.is_empty());
        assert_eq!(consensus.state().height, 1);
        super::resume(&state, &store, &pool, &mut consensus).unwrap();
        assert_eq!(store.get_canonical_tip().unwrap().unwrap(), tip);
        assert_eq!(
            rsc_chain::encode_block(&store.get_block(&tip).unwrap().unwrap()).unwrap(),
            block_bytes
        );
        assert_eq!(state.state_root().unwrap(), root);
        assert_eq!(state.version().unwrap(), version);
        assert_eq!(super::rebuild_ledger(&store).unwrap().root(), ledger_root);
        let sender = state
            .get_account(&derived_account().into())
            .unwrap()
            .unwrap();
        let receiver = state.get_account(&peer.into()).unwrap().unwrap();
        assert_eq!(sender.native_balance(), 493);
        assert_eq!(sender.nonce(), 1);
        assert_eq!(receiver.native_balance(), 107);
        assert_eq!(receiver.nonce(), 0);
        assert_eq!(consensus.state().height, 2);
        assert_eq!(consensus.state().epoch, 2);
        assert_eq!(consensus.state().view, 0);
        assert_eq!(consensus.state().phase, ConsensusPhase::NewView);
        assert!(consensus.state().proposal.is_none());
        assert!(consensus.state().locked.is_none());
        assert_eq!(
            consensus.network_mode(),
            rsc_consensus::NetworkMode::Distributed
        );
        assert!(pool.is_empty());
        assert!(store.get_block_by_height(BlockHeight(2)).unwrap().is_none());
    }

    #[test]
    fn a_foreign_proposer_does_not_move_the_engine() {
        let (_dir, _state, store) = opened();
        let mut foreign = tip_block(&store);
        foreign.header.proposer = AccountId([1; 32]);
        let hash = foreign.block_hash().unwrap();
        store.put_block(&foreign).unwrap();
        store.set_canonical_tip(&hash).unwrap();
        let (pool, _execution, mut consensus) = super::open_services(&store).unwrap();
        let err = super::propose_tip(&store, &mut consensus).unwrap_err();
        assert_eq!(err, "invalid proposer");
        assert_eq!(consensus.state().phase, ConsensusPhase::NewView);
        assert!(consensus.state().proposal.is_none());
        assert!(pool.is_empty());
        assert_eq!(store.get_canonical_tip().unwrap(), Some(hash));
    }

    #[test]
    fn consensus_height_follows_the_tip() {
        let (_dir, state, store) = opened();
        let genesis = tip_block(&store);
        append_block(&store, &genesis, Vec::new());
        let tip = store.get_canonical_tip().unwrap();
        let root = state.state_root().unwrap();
        let (_pool, _execution, consensus) = super::open_services(&store).unwrap();
        assert_eq!(consensus.state().height, 1);
        assert_eq!(consensus.state().view, 0);
        assert_eq!(store.get_canonical_tip().unwrap(), tip);
        assert_eq!(state.state_root().unwrap(), root);
    }

    fn wait<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
    }

    #[test]
    fn rpc_reads_genesis_and_answers_health() {
        let (_dir, state, store) = opened();
        let tip = store.get_canonical_tip().unwrap().unwrap();
        let tip_hex = rsc_rpc::encode_hex(&tip.0 .0);
        let account = rsc_rpc::encode_hex(&derived_account().0);
        let (pool, _execution, consensus) = super::open_services(&store).unwrap();
        let ctx = super::rpc_context(
            &config_at("unused", "devnet", 3),
            state,
            store,
            consensus,
            pool,
        );
        let chain_id = wait(rsc_rpc::methods::dispatch(
            &ctx,
            "rsc_chainId",
            Some(&serde_json::json!({})),
        ))
        .unwrap();
        assert_eq!(chain_id, serde_json::json!("3"));
        let status = wait(rsc_rpc::methods::dispatch(
            &ctx,
            "rsc_getStatus",
            Some(&serde_json::json!({})),
        ))
        .unwrap();
        assert_eq!(status["syncing"], serde_json::json!(false));
        assert_eq!(status["latest_height"], serde_json::json!(0));
        assert_eq!(status["latest_block_hash"], serde_json::json!(tip_hex));
        assert_eq!(status["finalized_height"], serde_json::json!(0));
        let block = wait(rsc_rpc::methods::dispatch(
            &ctx,
            "rsc_getBlockByHeight",
            Some(&serde_json::json!({ "height": 0 })),
        ))
        .unwrap();
        assert_eq!(block["hash"], serde_json::json!(tip_hex));
        assert_eq!(block["transactions"], serde_json::json!([]));
        let account_view = wait(rsc_rpc::methods::dispatch(
            &ctx,
            "rsc_getAccount",
            Some(&serde_json::json!({ "account": account })),
        ))
        .unwrap();
        assert_eq!(account_view["balance"], serde_json::json!("500"));
        assert_eq!(account_view["nonce"], serde_json::json!(0));
        let missing = wait(rsc_rpc::methods::dispatch(
            &ctx,
            "rsc_getTransaction",
            Some(&serde_json::json!({ "transaction_hash": rsc_rpc::encode_hex(&[1u8; 32]) })),
        ))
        .unwrap_err();
        assert_eq!(missing, rsc_rpc::RpcError::NotFound);
        let finality = wait(rsc_rpc::methods::dispatch(
            &ctx,
            "rsc_getFinality",
            Some(&serde_json::json!({})),
        ))
        .unwrap_err();
        assert_eq!(finality, rsc_rpc::RpcError::ServiceUnavailable);
        let rejected = wait(rsc_rpc::methods::dispatch(
            &ctx,
            "rsc_sendTransaction",
            Some(&serde_json::json!({ "signed_transaction": "00" })),
        ))
        .unwrap_err();
        assert_eq!(rejected, rsc_rpc::RpcError::TransactionRejected);
        let response = wait(tower::ServiceExt::oneshot(
            rsc_rpc::RpcServer::new(ctx, rsc_rpc::RpcConfig::default()).router(),
            axum::http::Request::builder()
                .uri("/health")
                .body(axum::body::Body::empty())
                .unwrap(),
        ))
        .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let bytes = wait(axum::body::to_bytes(response.into_body(), 1024)).unwrap();
        assert_eq!(bytes.as_ref(), br#"{"status":"ok"}"#);
    }

    #[test]
    fn the_node_seals_empty_blocks_and_keeps_going() {
        let (_dir, state, store) = opened();
        let version = state.version().unwrap();
        let root = state.state_root().unwrap();
        let balance = state
            .get_account(&derived_account().into())
            .unwrap()
            .unwrap()
            .native_balance();
        let (pool, _execution, mut consensus) = super::open_services(&store).unwrap();
        super::resume(&state, &store, &pool, &mut consensus).unwrap();
        assert_eq!(
            store
                .get_block(&store.get_canonical_tip().unwrap().unwrap())
                .unwrap()
                .unwrap()
                .header
                .height,
            BlockHeight::GENESIS
        );

        let first =
            super::produce_block(&state, &store, &pool, &mut consensus, &no_wallets()).unwrap();
        assert_eq!(first, 1);
        let block = store
            .get_block(&store.get_canonical_tip().unwrap().unwrap())
            .unwrap()
            .unwrap();
        assert!(block.transactions.is_empty());
        assert_eq!(block.header.height, BlockHeight(1));
        assert_eq!(block.header.state_root, Hash(*root.as_bytes()));
        assert!(block.header.timestamp.0 >= 1_700_000_000);
        assert_eq!(state.state_root().unwrap(), root);
        assert_eq!(state.version().unwrap(), version);
        assert_eq!(
            state
                .get_account(&derived_account().into())
                .unwrap()
                .unwrap()
                .native_balance(),
            balance
        );
        assert_eq!(consensus.state().height, 2);
        assert_eq!(consensus.state().phase, ConsensusPhase::NewView);

        let second =
            super::produce_block(&state, &store, &pool, &mut consensus, &no_wallets()).unwrap();
        assert_eq!(second, 2);
        let later = store
            .get_block(&store.get_canonical_tip().unwrap().unwrap())
            .unwrap()
            .unwrap();
        assert!(later.transactions.is_empty());
        assert_eq!(
            later.header.previous_block_hash,
            block.block_hash().unwrap()
        );
        assert_eq!(state.state_root().unwrap(), root);
        assert_eq!(consensus.state().height, 3);
        assert!(pool.is_empty());
        let tip = store.get_canonical_tip().unwrap().unwrap();
        let ctx = super::rpc_context(
            &config_at("unused", "devnet", 3),
            state,
            store,
            consensus,
            pool,
        );
        let finality = wait(rsc_rpc::methods::dispatch(
            &ctx,
            "rsc_getFinality",
            Some(&serde_json::json!({})),
        ))
        .unwrap();
        assert_eq!(finality["finalized_height"], serde_json::json!(2));
        assert_eq!(
            finality["finalized_hash"],
            serde_json::json!(rsc_rpc::encode_hex(&tip.0 .0))
        );
        let status = wait(rsc_rpc::methods::dispatch(
            &ctx,
            "rsc_getStatus",
            Some(&serde_json::json!({})),
        ))
        .unwrap();
        assert_eq!(status["latest_height"], serde_json::json!(2));
        assert_eq!(status["finalized_height"], serde_json::json!(2));
    }

    #[test]
    fn produce_block_includes_a_ready_transfer_then_seals_empty() {
        let dir = tempfile::tempdir().unwrap();
        let (genesis, peer) = write_pair_genesis(dir.path());
        let path = dir.path().to_str().unwrap().to_string();
        let config = parse_args(args(&[
            "--data-dir",
            &path,
            "--network-id",
            "devnet",
            "--chain-id",
            "3",
            "--genesis",
            &genesis,
        ]))
        .unwrap();
        let db = Arc::new(open_database(&config).unwrap());
        let (state, store) = share_database(db).unwrap();
        super::ensure_tip(&state, &store, &config).unwrap();
        let (pool, _execution, mut consensus) = super::open_services(&store).unwrap();
        super::resume(&state, &store, &pool, &mut consensus).unwrap();
        let signed = signed_to(peer, 0, 7);
        pool.insert(signed.canonical_bytes()).unwrap();
        let height =
            super::produce_block(&state, &store, &pool, &mut consensus, &no_wallets()).unwrap();
        assert_eq!(height, 1);
        let block = store
            .get_block(&store.get_canonical_tip().unwrap().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(block.transactions.len(), 1);
        assert!(pool.is_empty());
        let sender = state
            .get_account(&derived_account().into())
            .unwrap()
            .unwrap();
        assert_eq!(sender.native_balance(), 493);
        assert_eq!(sender.nonce(), 1);
        let empty =
            super::produce_block(&state, &store, &pool, &mut consensus, &no_wallets()).unwrap();
        assert_eq!(empty, 2);
        let later = store
            .get_block(&store.get_canonical_tip().unwrap().unwrap())
            .unwrap()
            .unwrap();
        assert!(later.transactions.is_empty());
        assert_eq!(
            state
                .get_account(&derived_account().into())
                .unwrap()
                .unwrap()
                .native_balance(),
            493
        );
        assert_eq!(consensus.state().height, 3);
    }

    #[test]
    fn the_chain_generates_a_wallet_and_opens_the_account() {
        let (_dir, state, store) = opened();
        let (pool, _execution, mut consensus) = super::open_services(&store).unwrap();
        super::resume(&state, &store, &pool, &mut consensus).unwrap();
        let (ctx, chain, wallets) = super::shared_chain(
            &config_at("unused", "devnet", 3),
            state,
            store,
            consensus,
            Arc::clone(&pool),
        );
        let created = wait(rsc_rpc::methods::dispatch(
            &ctx,
            "rsc_createWallet",
            Some(&serde_json::json!({})),
        ))
        .unwrap();
        let address = created["address"].as_str().unwrap();
        let account_id = created["account_id"].as_str().unwrap();
        assert!(address.starts_with("rsc"));
        assert_ne!(address, "rsc000000000");
        assert_eq!(address, format!("rsc{account_id}"));
        assert_eq!(created["public_key"].as_str().unwrap().len(), 64);
        assert_eq!(created["signing_key"].as_str().unwrap().len(), 64);
        assert_eq!(
            created["seed_phrase"]
                .as_str()
                .unwrap()
                .split_whitespace()
                .count(),
            12
        );
        assert_eq!(wallets.lock().unwrap().len(), 1);
        {
            let mut body = chain.lock().unwrap();
            let super::ChainBody {
                state,
                store,
                consensus,
            } = &mut *body;
            let height = super::produce_block(state, store, &pool, consensus, &wallets).unwrap();
            assert_eq!(height, 1);
        }
        assert!(wallets.lock().unwrap().is_empty());
        let account = wait(rsc_rpc::methods::dispatch(
            &ctx,
            "rsc_getAccount",
            Some(&serde_json::json!({ "account": account_id })),
        ))
        .unwrap();
        assert_eq!(account["balance"], serde_json::json!("0"));
        assert_eq!(account["nonce"], serde_json::json!(0));
        let block = {
            let body = chain.lock().unwrap();
            body.store
                .get_block(&body.store.get_canonical_tip().unwrap().unwrap())
                .unwrap()
                .unwrap()
        };
        assert_eq!(block.transactions.len(), 1);
    }

    #[test]
    fn p2p_listens_and_shuts_down() {
        let config = super::p2p_config(&config_at("unused", "devnet", 3)).unwrap();
        assert_eq!(config.network_id, "devnet");
        assert_eq!(config.chain_id, rsc_core::ChainId(3));
        assert!(super::p2p_config(&config_at("unused", "DEVNET", 3)).is_err());
        wait(async move {
            let mut network = super::start_p2p(config).await.unwrap();
            let peer = network.local_peer_id();
            assert_eq!(network.handle().local_peer_id(), peer);
            super::stop_p2p(&mut network).await.unwrap();
        });
    }
}
