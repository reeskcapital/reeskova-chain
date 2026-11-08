# RSC AMM Testnet (Stage 7)

Private, reproducible lab for validating **real** RSC AMM execution against
`rsc-assets` + `rsc-state` + `rsc-storage` (not a simulated exchange).

## Consensus sizing

Quorum is strict `3 * voting_power > 2 * total` (`rsc-consensus::has_quorum`).
Distributed mode requires `min_validators_for_distributed = 4`.

**Testnet default: 4 validators** (Byzantine `n = 3f + 1` with `f = 1`).

## Layout

| Path | Role |
|------|------|
| `config/testnet.toml` | Shared protocol ids / ports / asset tags |
| `scripts/start-testnet.ps1` | Start 4 validator data dirs + document RPC |
| `scripts/stop-testnet.ps1` | Stop tracked PIDs |
| `scripts/reset-testnet.ps1` | Wipe `var/` |
| `scripts/inspect-testnet.ps1` | Print dirs / tips |
| `scripts/run-amm-validation.ps1` | Run Stage 7 cargo suites |
| `var/` | Runtime data (gitignored locally) |

## Assets (no monetary value)

| Tag | Decimals | Purpose |
|-----|----------|---------|
| `tRSK` | 18 | Test RSK-like asset |
| `tUSDC` | 6 | Test USDC-like asset |

Funded only via `AmmRuntime::credit_asset` / genesis helpers in tests.

## Reproduce

```powershell
# From repo root (Windows)
.\testnet\scripts\reset-testnet.ps1
.\testnet\scripts\start-testnet.ps1
.\testnet\scripts\run-amm-validation.ps1
.\testnet\scripts\inspect-testnet.ps1
.\testnet\scripts\stop-testnet.ps1
```

### Cargo suites

```powershell
# Motor AMM + Stage 7 math/adversarial/consistency (no RocksDB)
cargo test -p rsc-amm --lib --no-default-features

# Native multi-validator + snapshot (requires RocksDB build)
$env:LIBCLANG_PATH = "C:\Program Files\LLVM\bin"
cargo test -p rsc-amm --lib --features native native::
cargo test -p rsc-assets --lib

# Quality gates
cargo fmt --check
cargo clippy -p rsc-amm --no-default-features -- -D warnings
```

### Benchmarks

```powershell
cargo bench -p rsc-amm --bench amm_swaps --no-default-features
```

## RPC discovery (sin API fija)

Ver `docs/RPC_DISCOVERY.md`. Semillas de ejemplo: `testnet/config/rpc-seeds.json`.

Cada nodo anuncia endpoints con `--rpc-public-url` / `--rpc-seed` o
`<data-dir>/rpc-seeds.json`. Clientes usan `rsc_getRpcEndpoints` o `GET /network`
y hacen failover. No hace falta un único `rpc.reeskova-chain.xyz` para que la
red exista.

## Architecture note

Full gossip of `TransactionPayload::Amm` across `rsc-p2p` is still Transfer-centric
in places. Stage 7 treats **four independent `AmmRuntime` replicas** executing the
same signed instruction stream as the primary multi-validator consistency proof
for AMM state (identical pools/assets/state roots). Process-level validator
scripts prepare data dirs for later P2P wiring.
