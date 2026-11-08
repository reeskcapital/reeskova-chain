//! Stage 7 — validación nativa multi-validador (AmmRuntime + storage real).

use crate::native::{
    sign_amm_tx, AmmInstruction, AmmRuntime, BlockContext, NativeAmmError,
};
use crate::pool::PoolIdentity;
use crate::types::{AssetId, FeeBps, LpShares};
use rsc_core::{AccountId, Amount, BlockHeight, Hash};
use rsc_crypto::PrivateKey;
use rsc_state::{derive_account_id, GenesisAccount, State};
use rsc_storage::{ColumnFamily, DatabaseConfig, RscDatabase, SnapshotManager, SnapshotMetadata};
use std::sync::Arc;
use tempfile::tempdir;

fn asset_rsk() -> AssetId {
    AssetId(Hash([0x52; 32]))
}
fn asset_usdc() -> AssetId {
    AssetId(Hash([0x55; 32]))
}

fn open_validator(label: &str) -> (tempfile::TempDir, AmmRuntime, PrivateKey, AccountId) {
    let dir = tempdir().unwrap();
    let db = Arc::new(
        RscDatabase::open(
            dir.path().join(label),
            DatabaseConfig {
                max_value_size: 8 << 20,
                network_id: "rsc-amm-testnet".into(),
                chain_id: "rsc-amm-testnet".into(),
            },
        )
        .unwrap(),
    );
    let state = State::open(Arc::clone(&db)).unwrap();
    let sk = PrivateKey::from_bytes(&[0x71u8; 32]).unwrap();
    let owner = sk.key_id();
    let account = AccountId(*derive_account_id(&owner).as_bytes());
    state
        .initialize_genesis(&[GenesisAccount::new(owner, 1_000_000)], 1)
        .unwrap();
    let runtime = AmmRuntime::new(db, state);
    runtime.register_asset(asset_rsk(), 18, "tRSK").unwrap();
    runtime.register_asset(asset_usdc(), 6, "tUSDC").unwrap();
    runtime
        .credit_asset(account, asset_rsk(), Amount(5_000_000))
        .unwrap();
    runtime
        .credit_asset(account, asset_usdc(), Amount(5_000_000))
        .unwrap();
    runtime.set_governors(vec![account]).unwrap();
    (dir, runtime, sk, account)
}

fn exec(
    rt: &AmmRuntime,
    sk: &PrivateKey,
    sender: AccountId,
    nonce: u64,
    instr: AmmInstruction,
    height: u64,
    version: u64,
) -> Result<crate::native::AmmExecutionResult, NativeAmmError> {
    let tx = sign_amm_tx(sender, nonce, &instr, sk).unwrap();
    rt.execute_transaction(
        &tx,
        &sk.public_key(),
        BlockContext {
            height: BlockHeight(height),
            new_state_version: version,
        },
    )
}

fn bootstrap_pool(
    rt: &AmmRuntime,
    sk: &PrivateKey,
    account: AccountId,
) -> (crate::types::PoolId, u64) {
    let fee = FeeBps::new(30).unwrap();
    let mut version = 2u64;
    exec(
        rt,
        sk,
        account,
        0,
        AmmInstruction::CreatePool {
            asset_left: asset_rsk(),
            asset_right: asset_usdc(),
            fee_bps: fee,
        },
        1,
        version,
    )
    .unwrap();
    version += 1;
    let pool_id = PoolIdentity::new(asset_rsk(), asset_usdc(), fee)
        .unwrap()
        .pool_id();
    exec(
        rt,
        sk,
        account,
        1,
        AmmInstruction::AddLiquidity {
            pool_id,
            max_amount_a: Amount(200_000),
            max_amount_b: Amount(200_000),
            min_lp_shares: LpShares(0),
            deadline: u64::MAX,
        },
        2,
        version,
    )
    .unwrap();
    version += 1;
    exec(
        rt,
        sk,
        account,
        2,
        AmmInstruction::ActivatePool { pool_id },
        3,
        version,
    )
    .unwrap();
    (pool_id, version + 1)
}

#[test]
fn four_native_validators_same_state_after_identical_txs() {
    let mut fingerprints = Vec::new();
    for i in 0..4 {
        let (_dir, rt, sk, account) = open_validator(&format!("v{i}"));
        let (pool_id, mut version) = bootstrap_pool(&rt, &sk, account);
        let mut nonce = 3u64;
        for s in 0u8..10 {
            let r = exec(
                &rt,
                &sk,
                account,
                nonce,
                AmmInstruction::SwapExactInput {
                    pool_id,
                    token_in: asset_rsk(),
                    token_out: asset_usdc(),
                    amount_in: Amount(50),
                    min_amount_out: Amount(1),
                    deadline: u64::MAX,
                },
                10 + u64::from(s),
                version,
            )
            .unwrap();
            version = r.state_commitment.new_version() + 1;
            nonce += 1;
        }
        let store = rt.load_store().unwrap();
        let assets = rt.load_assets().unwrap();
        let mut fp = store.pools.to_canonical_bytes();
        fp.extend_from_slice(&store.accounting.to_canonical_bytes());
        fp.extend_from_slice(&assets.to_canonical_bytes());
        fp.extend_from_slice(rt.state().state_root().unwrap().as_bytes());
        fingerprints.push(fp);
    }
    for i in 1..4 {
        assert_eq!(fingerprints[0], fingerprints[i], "validator {i} diverged");
    }
}

#[test]
fn native_invalid_signature_and_replay_nonce() {
    let (_dir, rt, sk, account) = open_validator("sec");
    let (pool_id, version) = bootstrap_pool(&rt, &sk, account);
    let other = PrivateKey::from_bytes(&[0x99u8; 32]).unwrap();
    let tx = sign_amm_tx(
        account,
        3,
        &AmmInstruction::SwapExactInput {
            pool_id,
            token_in: asset_rsk(),
            token_out: asset_usdc(),
            amount_in: Amount(10),
            min_amount_out: Amount(1),
            deadline: u64::MAX,
        },
        &sk,
    )
    .unwrap();
    assert!(matches!(
        rt.execute_transaction(
            &tx,
            &other.public_key(),
            BlockContext {
                height: BlockHeight(20),
                new_state_version: version,
            },
        ),
        Err(NativeAmmError::Transaction(_))
    ));
    exec(
        &rt,
        &sk,
        account,
        3,
        AmmInstruction::SwapExactInput {
            pool_id,
            token_in: asset_rsk(),
            token_out: asset_usdc(),
            amount_in: Amount(10),
            min_amount_out: Amount(1),
            deadline: u64::MAX,
        },
        21,
        version,
    )
    .unwrap();
    // Same nonce, different payload → distinct tx_id/op_id, so anti-replay
    // is enforced by ConsumeNonce (not AMM duplicate-op).
    let err = exec(
        &rt,
        &sk,
        account,
        3,
        AmmInstruction::SwapExactInput {
            pool_id,
            token_in: asset_rsk(),
            token_out: asset_usdc(),
            amount_in: Amount(11),
            min_amount_out: Amount(1),
            deadline: u64::MAX,
        },
        22,
        version + 1,
    )
    .unwrap_err();
    assert!(
        matches!(err, NativeAmmError::State(_)),
        "expected nonce mismatch via state, got {err:?}"
    );
}

#[test]
fn snapshot_includes_default_cf_amm_prefix() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("live");
    let snap_root = dir.path().join("snaps");
    let db = RscDatabase::open(
        &db_path,
        DatabaseConfig {
            max_value_size: 8 << 20,
            network_id: "rsc-amm-testnet".into(),
            chain_id: "rsc-amm-testnet".into(),
        },
    )
    .unwrap();
    db.put(ColumnFamily::Default, b"RSC/AMM/V1/POOLS", b"stage7")
        .unwrap();
    let mgr = SnapshotManager::new(db, &snap_root).unwrap();
    let snap = mgr
        .create_snapshot_with_id(
            "amm-stage7",
            SnapshotMetadata {
                height: 1,
                block_hash: vec![1],
                state_root: vec![2],
            },
        )
        .unwrap();
    assert!(snap.path.is_dir());
}
