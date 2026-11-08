# Stage 7 — AMM Testnet & Security Validation Report

**Date:** 2026-11-01 (local)  
**Scope:** Real RSC AMM motor + ledger payload; native RocksDB path prepared but not fully executed on this host.

Legend: ✅ PASS (executed) · ❌ FAIL · ⏸ NOT EXECUTED · ⚠ BLOCKED

---

## 1. Testnet architecture

| Item | Value |
|------|--------|
| Network id | `rsc-amm-testnet` |
| Validators | **4** (`n = 3f+1`, `f=1`) |
| Quorum | `3 * power > 2 * total` (`rsc_consensus::has_quorum`) |
| Distributed threshold | `min_validators_for_distributed = 4` |
| AMM execution proof | Four independent motor/native replicas, identical signed instruction streams |
| Process scripts | `testnet/scripts/*.ps1` prepare 4 data dirs under `testnet/var/` |
| Docs | `testnet/README.md` |

P2P gossip of `TransactionPayload::Amm` is still Transfer-centric in `rsc-p2p`; Stage 7 consistency for AMM state is proven at the **executor replica** layer (same code path as native commit).

## 2. Validator configuration

Prepared by `start-testnet.ps1`:

- `testnet/var/validator-{1..4}/READY`
- RPC ports reserved `18547..18550`
- Shared `testnet/config/testnet.toml`

## 3. Test accounts and assets

| Asset | Decimals | Tag | Value |
|-------|----------|-----|-------|
| Test RSK-like | 18 | `tRSK` / harness `0x52…` | **No monetary value** |
| Test USDC-like | 6 | `tUSDC` / harness `0x55…` | **No monetary value** |

Harness accounts: provider `0x10…`, trader `0x20…`, authority `0xAA…`.

**Unsolicited vault credits:** documented policy in `AmmAccounting::policy_unsolicited_vault_credits` — ignored until an explicit protocol op; not treated as reserves.

## 4. Tests implemented

| Module | Coverage |
|--------|----------|
| `tests/stage7_math_security.rs` | Overflow/underflow/div0, dust, large swaps, isqrt, fees, cumulative rounding, proptest |
| `tests/stage7_adversarial.rs` | Replay, slippage, deadline, pause, duplicate pool, rollback, sink lock, unbalanced mint, competing swaps |
| `tests/stage7_consistency.rs` | 4-replica fingerprints, divergent seeds, 1000-swap stress |
| `tests/stage7_harness.rs` | Deterministic RSK/USDC world + accounting asserts |
| `native/stage7_native.rs` | 4× `AmmRuntime` + signature/nonce + snapshot CF (needs RocksDB) |
| `benches/amm_swaps.rs` | Criterion exact-input + ×100 |
| Prior Stages 1–5 suites | Math, pool, liquidity, swap, fees, accounting |

## 5. Tests executed (this host)

| Command | Result |
|---------|--------|
| `cargo test -p rsc-amm --lib --no-default-features -- stage7_` | ✅ **25/25 PASS** |
| `cargo test -p rsc-amm --lib --no-default-features` | ✅ **110/110 PASS** |
| `cargo test -p rsc-ledger --lib` | ✅ **37/37 PASS** (incl. Amm payload) |
| `cargo test -p rsc-amm --features native -- native::stage7_native` | ⏸ **NOT EXECUTED** (RocksDB/`librocksdb-sys` toolchain) |
| `cargo test -p rsc-assets --lib` | ⏸ **NOT EXECUTED** (same) |
| `cargo bench -p rsc-amm --bench amm_swaps` | ⏸ **NOT EXECUTED** (interrupted / long compile) |
| `cargo fmt --check` / `cargo clippy` | ⏸ **NOT EXECUTED** |
| `cargo-fuzz` | ⏸ **NOT SET UP** (optional; not required to invent) |
| Full process multi-node P2P AMM | ⏸ **NOT EXECUTED** |

## 6. Passed / failed

- **Passed:** all Stage 7 motor tests listed above (25) + full AMM lib (110) + ledger (37).
- **Failed:** none among executed suites.
- **Not passed (not run):** native multi-validator, assets crate, Criterion numbers, clippy/fmt, fuzz, live P2P testnet load.

## 7. Security findings

| Finding | Severity | Status |
|---------|----------|--------|
| Dust `amount_in=1` with 30 bps → `InsufficientLiquidity` (no silent free output) | Info | Expected ✅ |
| `op_id` replay rejected | — | Enforced ✅ |
| Failed ops restore canonical fingerprint | — | Enforced ✅ |
| Locked sink LP not withdrawable | — | Enforced ✅ |
| Amm payload bounds in ledger | — | Enforced ✅ |
| Native signature / nonce path | — | Code present; ⏸ not run here |
| P2P may reject Amm txs on older match arms | Medium | Known limitation |

## 8. Mathematical results

Executed: overflow/underflow/div0 explicit; floor ≤ ceil; exact-in out &lt; reserve (proptest); cumulative 1000× swaps keep invariant; LP redeem ≤ reserves; fee split conserves.

No floats in financial path. No wrapping arithmetic APIs.

## 9. State consistency

`four_validator_replicas_identical_fingerprints` ✅ — four independent `Stage7World` runs with identical seeds produce identical canonical bytes (pools + accounting + fees).

Native 4× `AmmRuntime` + state root: ⏸ not executed.

## 10. Performance benchmarks

**No invented figures.** Criterion bench crate is present (`benches/amm_swaps.rs`) but was **not completed** on this host. Stress test `stress_1000_swaps_accounting_holds` ✅ (functional, not a latency measurement).

## 11. Known limitations

1. RocksDB native validation blocked on this Windows toolchain.  
2. Indexer/RPC AMM event APIs not present (`Receipts` keys prepared in Stage 6).  
3. Process-level 4-validator network does not yet gossip Amm instructions end-to-end.  
4. `cargo-fuzz` / continuous 10k-swap wall-clock metrics not run.

## 12. Critical issues requiring fixes

None open in the **executed** motor/security suite.  
**Blocker for full Stage 7 acceptance:** run native + assets tests once RocksDB builds; wire Amm apply path in p2p/validator gossip if multi-process testnet is mandatory.

## 13. Readiness recommendation

| Gate | Status |
|------|--------|
| Critical AMM motor tests | ✅ |
| Swap / liquidity / rounding / rollback | ✅ |
| Accounting consistency (motor) | ✅ |
| Multi-replica identical state (motor) | ✅ |
| Native storage / restart / snapshot | ⏸ |
| Live multi-process testnet + perf numbers | ⏸ |

**Recommendation:** Stage 7 **motor validation is ready** to proceed engineering-wise; **do not claim full Testnet acceptance or production readiness** until native RocksDB suites and documented Criterion runs complete. Next stage should prioritize unblocking `librocksdb-sys` and executing `native::stage7_native` + `run-amm-validation.ps1` native section.
