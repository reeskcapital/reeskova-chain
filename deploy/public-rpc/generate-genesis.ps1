$ErrorActionPreference = "Stop"
$OutDir = Join-Path $PSScriptRoot "var"
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$Path = Join-Path $OutDir "genesis.json"

# Generador liviano (ed25519-dalek + sha2). No tira de rsc-genesis/RocksDB.
$toolDir = Join-Path $OutDir "gen-lite"
New-Item -ItemType Directory -Force -Path (Join-Path $toolDir "src") | Out-Null

Set-Content -Path (Join-Path $toolDir "Cargo.toml") -Encoding utf8 -Value @'
[workspace]
[package]
name = "rsc-gen-lite"
version = "0.1.0"
edition = "2021"

[dependencies]
ed25519-dalek = { version = "2", features = ["zeroize"] }
sha2 = "0.10"
serde_json = "1"
'@

Set-Content -Path (Join-Path $toolDir "src\main.rs") -Encoding utf8 -Value @'
use ed25519_dalek::SigningKey;
use sha2::{Digest, Sha256};

fn hex0x(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(2 + bytes.len() * 2);
    s.push_str("0x");
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn sha256(data: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    out.copy_from_slice(&Sha256::digest(data));
    out
}

fn main() {
    let seed = [0x11u8; 32];
    let signing = SigningKey::from_bytes(&seed);
    let pk = signing.verifying_key().to_bytes();

    let mut key_payload = Vec::new();
    key_payload.extend_from_slice(b"KEY-ID");
    key_payload.extend_from_slice(&pk);
    let mut envelope = Vec::new();
    envelope.extend_from_slice(b"RSC");
    envelope.extend_from_slice(&1u16.to_le_bytes());
    envelope.push(0);
    envelope.extend_from_slice(&key_payload);
    let key_id = sha256(&envelope);

    let mut account_input = Vec::new();
    account_input.extend_from_slice(b"RSC/STATE/ACCOUNT-ID/V1");
    account_input.extend_from_slice(&key_id);
    let account_id = sha256(&account_input);

    let seed_hash = [0xA5u8; 32];
    let genesis = serde_json::json!({
        "version": 1,
        "chain_id": 3,
        "timestamp": 1_700_000_000u64,
        "parameters": {
            "amount_scale": 1_000_000u64,
            "max_block_size": 1_048_576u64,
            "max_transactions_per_block": 1_000u64,
            "max_transaction_bytes": 131_072u64,
            "minimum_fee": 0u64,
            "committee_size": 8u64,
            "epoch_length": 32u64,
            "min_validator_weight": 1u64,
            "max_validator_weight": 1_000u64,
            "min_validators_for_distributed": 4u64,
            "consensus_randomness_seed": hex0x(&seed_hash),
        },
        "initial_accounts": [{
            "account_id": hex0x(&account_id),
            "owner_key_id": hex0x(&key_id),
            "native_balance": 1_000_000u64,
            "allocation": "community",
        }],
        "validators": [],
        "amm_assets": [],
        "amm_balances": [],
        "amm_governors": [],
        "amm_permissionless_creation": true,
    });
    print!("{}", serde_json::to_string_pretty(&genesis).unwrap());
}
'@

Push-Location $toolDir
try {
  cargo run --quiet | Set-Content -Path $Path -Encoding utf8
} finally {
  Pop-Location
}
if (-not (Test-Path $Path) -or (Get-Item $Path).Length -lt 50) {
  throw "genesis generation failed: $Path"
}
Write-Host "genesis -> $Path ($((Get-Item $Path).Length) bytes)"
