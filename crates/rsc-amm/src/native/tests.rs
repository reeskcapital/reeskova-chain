//! Integración nativa: assets + state + storage + AMM executor.

use crate::native::{
    sign_amm_tx, AmmInstruction, AmmRuntime, BlockContext, NativeAmmError,
};
use crate::pool::PoolStatus;
use crate::types::{AssetId, FeeBps, LpShares, PoolId};
use rsc_assets::{native_asset_id, AssetLedger, AssetMetadata};
use rsc_core::{AccountId, Amount, BlockHeight, Hash};
use rsc_crypto::PrivateKey;
use rsc_ledger::{Transaction, TransactionPayload, TRANSACTION_VERSION_V1};
use rsc_state::{derive_account_id, GenesisAccount, State};
use rsc_storage::{DatabaseConfig, RscDatabase};
use std::sync::Arc;
use tempfile::tempdir;

fn open_runtime() -> (tempfile::TempDir, AmmRuntime, PrivateKey, AccountId) {
    let dir = tempdir().unwrap();
    let db = Arc::new(
        RscDatabase::open(
            dir.path().join("db"),
            DatabaseConfig {
                max_value_size: 8 << 20,
                network_id: "amm-test".into(),
                chain_id: "amm-test".into(),
            },
        )
        .unwrap(),
    );
    let state = State::open(Arc::clone(&db)).unwrap();
    let sk = PrivateKey::from_bytes(&[7u8; 32]).unwrap();
    let owner = sk.key_id();
    let account = AccountId(*derive_account_id(&owner).as_bytes());
    state
        .initialize_genesis(&[GenesisAccount::new(owner, 1_000_000)], 1)
        .unwrap();

    let runtime = AmmRuntime::new(db, state);
    let a = AssetId(Hash([0xA1; 32]));
    let b = AssetId(Hash([0xB2; 32]));
    runtime.register_asset(a, 18, "AAA").unwrap();
    runtime.register_asset(b, 18, "BBB").unwrap();
    runtime.credit_asset(account, a, Amount(1_000_000)).unwrap();
    runtime.credit_asset(account, b, Amount(1_000_000)).unwrap();
    runtime.set_governors(vec![account]).unwrap();
    (dir, runtime, sk, account)
}

fn asset_pair() -> (AssetId, AssetId) {
    (AssetId(Hash([0xA1; 32])), AssetId(Hash([0xB2; 32])))
}

fn exec(
    runtime: &AmmRuntime,
    sk: &PrivateKey,
    sender: AccountId,
    nonce: u64,
    instr: AmmInstruction,
    height: u64,
    new_version: u64,
) -> Result<crate::native::AmmExecutionResult, NativeAmmError> {
    let tx = sign_amm_tx(sender, nonce, &instr, sk).unwrap();
    runtime.execute_transaction(
        &tx,
        &sk.public_key(),
        BlockContext {
            height: BlockHeight(height),
            new_state_version: new_version,
        },
    )
}

#[test]
fn create_pool_add_liquidity_swap_persist_restart() {
    let (dir, runtime, sk, account) = open_runtime();
    let (a, b) = asset_pair();
    let fee = FeeBps::new(30).unwrap();

    let r = exec(
        &runtime,
        &sk,
        account,
        0,
        AmmInstruction::CreatePool {
            asset_left: a,
            asset_right: b,
            fee_bps: fee,
        },
        1,
        2,
    )
    .unwrap();
    assert!(r.events.iter().any(|e| matches!(e, crate::AmmEvent::PoolCreated { .. })));

    let pool_id = crate::pool::PoolIdentity::new(a, b, fee).unwrap().pool_id();

    exec(
        &runtime,
        &sk,
        account,
        1,
        AmmInstruction::AddLiquidity {
            pool_id,
            max_amount_a: Amount(100_000),
            max_amount_b: Amount(100_000),
            min_lp_shares: LpShares(0),
            deadline: u64::MAX,
        },
        2,
        3,
    )
    .unwrap();

    exec(
        &runtime,
        &sk,
        account,
        2,
        AmmInstruction::ActivatePool { pool_id },
        3,
        4,
    )
    .unwrap();

    let swap = exec(
        &runtime,
        &sk,
        account,
        3,
        AmmInstruction::SwapExactInput {
            pool_id,
            token_in: a,
            token_out: b,
            amount_in: Amount(1_000),
            min_amount_out: Amount(1),
            deadline: u64::MAX,
        },
        4,
        5,
    )
    .unwrap();
    assert!(swap
        .events
        .iter()
        .any(|e| matches!(e, crate::AmmEvent::SwapExecuted { .. })));

    // Persistencia / restart: reabrir DB.
    let path = dir.path().join("db");
    drop(runtime);
    let db = Arc::new(
        RscDatabase::open(
            &path,
            DatabaseConfig {
                max_value_size: 8 << 20,
                network_id: "amm-test".into(),
                chain_id: "amm-test".into(),
            },
        )
        .unwrap(),
    );
    let state = State::open(Arc::clone(&db)).unwrap();
    let runtime = AmmRuntime::new(db, state);
    let store = runtime.load_store().unwrap();
    assert_eq!(
        store.pools.get_pool_status(&pool_id).unwrap(),
        PoolStatus::Active
    );
    let assets = runtime.load_assets().unwrap();
    assert!(assets.vault_balance(&pool_id.0, &rsc_assets::AssetId(a.0)).0 > 0);

    // Replay de la misma tx (mismo nonce) debe fallar.
    let err = exec(
        &runtime,
        &sk,
        account,
        3,
        AmmInstruction::SwapExactInput {
            pool_id,
            token_in: a,
            token_out: b,
            amount_in: Amount(1_000),
            min_amount_out: Amount(1),
            deadline: u64::MAX,
        },
        5,
        6,
    )
    .unwrap_err();
    assert!(matches!(err, NativeAmmError::State(_)));
}

#[test]
fn invalid_signature_and_insufficient_balance() {
    let (_dir, runtime, sk, account) = open_runtime();
    let (a, b) = asset_pair();
    let fee = FeeBps::new(30).unwrap();
    exec(
        &runtime,
        &sk,
        account,
        0,
        AmmInstruction::CreatePool {
            asset_left: a,
            asset_right: b,
            fee_bps: fee,
        },
        1,
        2,
    )
    .unwrap();
    let pool_id = crate::pool::PoolIdentity::new(a, b, fee).unwrap().pool_id();

    let other = PrivateKey::from_bytes(&[9u8; 32]).unwrap();
    let mut tx = sign_amm_tx(
        account,
        1,
        &AmmInstruction::AddLiquidity {
            pool_id,
            max_amount_a: Amount(10),
            max_amount_b: Amount(10),
            min_lp_shares: LpShares(0),
            deadline: u64::MAX,
        },
        &sk,
    )
    .unwrap();
    // Firma con otra clave.
    assert!(matches!(
        runtime.execute_transaction(
            &tx,
            &other.public_key(),
            BlockContext {
                height: BlockHeight(2),
                new_state_version: 3,
            },
        ),
        Err(NativeAmmError::Transaction(_))
    ));

    // Re-firmar correctamente pero con cantidades mayores al saldo.
    tx = sign_amm_tx(
        account,
        1,
        &AmmInstruction::AddLiquidity {
            pool_id,
            max_amount_a: Amount(9_999_999),
            max_amount_b: Amount(9_999_999),
            min_lp_shares: LpShares(0),
            deadline: u64::MAX,
        },
        &sk,
    )
    .unwrap();
    let err = runtime
        .execute_transaction(
            &tx,
            &sk.public_key(),
            BlockContext {
                height: BlockHeight(2),
                new_state_version: 3,
            },
        )
        .unwrap_err();
    assert!(matches!(err, NativeAmmError::Amm(crate::AmmError::InsufficientBalance)));
}

#[test]
fn exact_output_slippage_and_pause() {
    let (_dir, runtime, sk, account) = open_runtime();
    let (a, b) = asset_pair();
    let fee = FeeBps::new(30).unwrap();
    exec(
        &runtime,
        &sk,
        account,
        0,
        AmmInstruction::CreatePool {
            asset_left: a,
            asset_right: b,
            fee_bps: fee,
        },
        1,
        2,
    )
    .unwrap();
    let pool_id = crate::pool::PoolIdentity::new(a, b, fee).unwrap().pool_id();
    exec(
        &runtime,
        &sk,
        account,
        1,
        AmmInstruction::AddLiquidity {
            pool_id,
            max_amount_a: Amount(50_000),
            max_amount_b: Amount(50_000),
            min_lp_shares: LpShares(0),
            deadline: u64::MAX,
        },
        2,
        3,
    )
    .unwrap();
    exec(
        &runtime,
        &sk,
        account,
        2,
        AmmInstruction::ActivatePool { pool_id },
        3,
        4,
    )
    .unwrap();

    // Slippage: max_amount_in demasiado bajo.
    let err = exec(
        &runtime,
        &sk,
        account,
        3,
        AmmInstruction::SwapExactOutput {
            pool_id,
            token_in: a,
            token_out: b,
            amount_out: Amount(1_000),
            max_amount_in: Amount(1),
            deadline: u64::MAX,
        },
        4,
        5,
    )
    .unwrap_err();
    assert!(matches!(err, NativeAmmError::Amm(crate::AmmError::SlippageExceeded)));

    // Swap exact-output válido.
    exec(
        &runtime,
        &sk,
        account,
        3,
        AmmInstruction::SwapExactOutput {
            pool_id,
            token_in: a,
            token_out: b,
            amount_out: Amount(100),
            max_amount_in: Amount(10_000),
            deadline: u64::MAX,
        },
        4,
        5,
    )
    .unwrap();

    exec(
        &runtime,
        &sk,
        account,
        4,
        AmmInstruction::PausePool { pool_id },
        5,
        6,
    )
    .unwrap();
    let err = exec(
        &runtime,
        &sk,
        account,
        5,
        AmmInstruction::SwapExactInput {
            pool_id,
            token_in: a,
            token_out: b,
            amount_in: Amount(10),
            min_amount_out: Amount(1),
            deadline: u64::MAX,
        },
        6,
        7,
    )
    .unwrap_err();
    assert!(matches!(err, NativeAmmError::Amm(crate::AmmError::SwapNotPermitted)));

    // Remove liquidity permitido en pausa.
    let lp = runtime
        .load_store()
        .unwrap()
        .accounting
        .lp_balance_of(&pool_id, &account);
    exec(
        &runtime,
        &sk,
        account,
        5,
        AmmInstruction::RemoveLiquidity {
            pool_id,
            lp_shares: LpShares(lp.0 / 2),
            min_amount_a: Amount(0),
            min_amount_b: Amount(0),
            deadline: u64::MAX,
        },
        6,
        7,
    )
    .unwrap();
}

#[test]
fn multi_tx_same_block_sequence_and_deterministic_roots() {
    let (dir, runtime, sk, account) = open_runtime();
    let (a, b) = asset_pair();
    let fee = FeeBps::new(30).unwrap();
    let pool_id = crate::pool::PoolIdentity::new(a, b, fee).unwrap().pool_id();

    let mut version = 2u64;
    for (nonce, instr) in [
        (
            0u64,
            AmmInstruction::CreatePool {
                asset_left: a,
                asset_right: b,
                fee_bps: fee,
            },
        ),
        (
            1,
            AmmInstruction::AddLiquidity {
                pool_id,
                max_amount_a: Amount(20_000),
                max_amount_b: Amount(20_000),
                min_lp_shares: LpShares(0),
                deadline: u64::MAX,
            },
        ),
        (2, AmmInstruction::ActivatePool { pool_id }),
        (
            3,
            AmmInstruction::SwapExactInput {
                pool_id,
                token_in: a,
                token_out: b,
                amount_in: Amount(500),
                min_amount_out: Amount(1),
                deadline: u64::MAX,
            },
        ),
        (
            4,
            AmmInstruction::SwapExactInput {
                pool_id,
                token_in: b,
                token_out: a,
                amount_in: Amount(200),
                min_amount_out: Amount(1),
                deadline: u64::MAX,
            },
        ),
    ] {
        let r = exec(&runtime, &sk, account, nonce, instr, 10, version).unwrap();
        version = r.state_commitment.new_version() + 1;
    }

    let root_a = runtime.state().state_root().unwrap();
    let pools_a = runtime.load_store().unwrap().pools.to_canonical_bytes();
    let assets_a = runtime.load_assets().unwrap().to_canonical_bytes();

    // Segundo validador: misma secuencia sobre DB fresca → mismos bytes.
    let dir2 = tempdir().unwrap();
    let db2 = Arc::new(
        RscDatabase::open(
            dir2.path(),
            DatabaseConfig {
                max_value_size: 8 << 20,
                network_id: "amm-test".into(),
                chain_id: "amm-test".into(),
            },
        )
        .unwrap(),
    );
    let state2 = State::open(Arc::clone(&db2)).unwrap();
    let owner = sk.key_id();
    state2
        .initialize_genesis(&[GenesisAccount::new(owner, 1_000_000)], 1)
        .unwrap();
    let runtime2 = AmmRuntime::new(db2, state2);
    runtime2.register_asset(a, 18, "AAA").unwrap();
    runtime2.register_asset(b, 18, "BBB").unwrap();
    runtime2
        .credit_asset(account, a, Amount(1_000_000))
        .unwrap();
    runtime2
        .credit_asset(account, b, Amount(1_000_000))
        .unwrap();
    runtime2.set_governors(vec![account]).unwrap();

    let mut version = 2u64;
    for (nonce, instr) in [
        (
            0u64,
            AmmInstruction::CreatePool {
                asset_left: a,
                asset_right: b,
                fee_bps: fee,
            },
        ),
        (
            1,
            AmmInstruction::AddLiquidity {
                pool_id,
                max_amount_a: Amount(20_000),
                max_amount_b: Amount(20_000),
                min_lp_shares: LpShares(0),
                deadline: u64::MAX,
            },
        ),
        (2, AmmInstruction::ActivatePool { pool_id }),
        (
            3,
            AmmInstruction::SwapExactInput {
                pool_id,
                token_in: a,
                token_out: b,
                amount_in: Amount(500),
                min_amount_out: Amount(1),
                deadline: u64::MAX,
            },
        ),
        (
            4,
            AmmInstruction::SwapExactInput {
                pool_id,
                token_in: b,
                token_out: a,
                amount_in: Amount(200),
                min_amount_out: Amount(1),
                deadline: u64::MAX,
            },
        ),
    ] {
        let r = exec(&runtime2, &sk, account, nonce, instr, 10, version).unwrap();
        version = r.state_commitment.new_version() + 1;
    }

    assert_eq!(runtime2.state().state_root().unwrap(), root_a);
    assert_eq!(
        runtime2.load_store().unwrap().pools.to_canonical_bytes(),
        pools_a
    );
    assert_eq!(
        runtime2.load_assets().unwrap().to_canonical_bytes(),
        assets_a
    );
    let _ = dir;
}

#[test]
fn unregistered_asset_rejected() {
    let (_dir, runtime, sk, account) = open_runtime();
    let bad = AssetId(Hash([0xFF; 32]));
    let (a, _) = asset_pair();
    let err = exec(
        &runtime,
        &sk,
        account,
        0,
        AmmInstruction::CreatePool {
            asset_left: a,
            asset_right: bad,
            fee_bps: FeeBps::new(30).unwrap(),
        },
        1,
        2,
    )
    .unwrap_err();
    assert!(matches!(err, NativeAmmError::Amm(crate::AmmError::AssetNotRegistered)));
}

#[test]
fn closed_policy_governor_creates_outsider_rejected_persists() {
    let (dir, runtime, gov_sk, gov) = open_runtime();
    let (a, b) = asset_pair();
    let fee = FeeBps::new(30).unwrap();

    runtime
        .set_policy(crate::pool::PoolPolicy {
            permissionless_creation: false,
            min_liquidity: crate::DEFAULT_MIN_LIQUIDITY,
        })
        .unwrap();

    let out_sk = PrivateKey::from_bytes(&[9u8; 32]).unwrap();
    let out = AccountId(*derive_account_id(&out_sk.key_id()).as_bytes());
    {
        use rsc_state::{StateOperation, StateTransition};
        let root = runtime.state().state_root().unwrap();
        let version = runtime.state().version().unwrap();
        let mut t = StateTransition::new(root, version);
        t.push(StateOperation::CreateAccount {
            owner_key_id: out_sk.key_id(),
        });
        runtime
            .state()
            .apply_transition(&t, version + 1)
            .unwrap();
    }
    runtime.credit_asset(out, a, Amount(50_000)).unwrap();
    runtime.credit_asset(out, b, Amount(50_000)).unwrap();

    let create = AmmInstruction::CreatePool {
        asset_left: a,
        asset_right: b,
        fee_bps: fee,
    };
    let next = runtime.state().version().unwrap() + 1;
    let err = exec(&runtime, &out_sk, out, 0, create.clone(), 1, next).unwrap_err();
    assert!(matches!(
        err,
        NativeAmmError::Amm(crate::AmmError::CreationDisabled)
    ));
    assert!(runtime.load_store().unwrap().pools.list_pools().is_empty());

    let next = runtime.state().version().unwrap() + 1;
    exec(&runtime, &gov_sk, gov, 0, create, 2, next).unwrap();
    let pool_id = crate::pool::PoolIdentity::new(a, b, fee).unwrap().pool_id();
    assert_eq!(runtime.load_store().unwrap().pools.list_pools(), vec![pool_id]);

    let db_path = dir.path().join("db");
    drop(runtime);
    let db2 = Arc::new(
        RscDatabase::open(
            &db_path,
            DatabaseConfig {
                max_value_size: 8 << 20,
                network_id: "amm-test".into(),
                chain_id: "amm-test".into(),
            },
        )
        .unwrap(),
    );
    let runtime2 = AmmRuntime::new(Arc::clone(&db2), State::open(db2).unwrap());
    let store = runtime2.load_store().unwrap();
    assert_eq!(store.governors, vec![gov]);
    assert!(!store.pools.policy().permissionless_creation);
    assert_eq!(store.pools.list_pools(), vec![pool_id]);
}

#[test]
fn native_asset_id_stable() {
    assert_eq!(native_asset_id(), native_asset_id());
    let mut ledger = AssetLedger::with_native();
    assert!(ledger.is_registered(&native_asset_id()));
    let _ = AssetMetadata::new(18, "RSK");
}

#[test]
fn ledger_amm_payload_roundtrip() {
    let sk = PrivateKey::from_bytes(&[3u8; 32]).unwrap();
    let instr = AmmInstruction::PausePool {
        pool_id: PoolId(Hash([1; 32])),
    };
    let tx = sign_amm_tx(AccountId([2; 32]), 0, &instr, &sk).unwrap();
    assert_eq!(tx.version, TRANSACTION_VERSION_V1);
    let restored = Transaction::from_canonical_bytes(&tx.canonical_bytes()).unwrap();
    assert_eq!(restored, tx);
    match restored.payload {
        TransactionPayload::Amm { instruction } => {
            assert_eq!(AmmInstruction::from_bytes(&instruction).unwrap(), instr);
        }
        _ => panic!("expected amm"),
    }
}
