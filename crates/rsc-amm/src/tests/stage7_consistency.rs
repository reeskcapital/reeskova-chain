//! Stage 7 — consistencia multi-réplica del motor (mismo input → mismo estado).

use super::stage7_harness::{asset_rsk, asset_usdc, Stage7World};
use crate::swap::{ExactInputSwap, SwapEngine};
use rsc_core::{Amount, BlockHeight, Hash};

fn run_scenario(seed: u8) -> Vec<u8> {
    let mut w = Stage7World::bootstrap(100_000, 100_000);
    for i in 0u8..50 {
        let _ = SwapEngine::execute_swap_exact_input(
            &mut w.pools,
            &mut w.accounting,
            &mut w.fees,
            ExactInputSwap {
                pool_id: w.pool_id,
                trader: w.trader,
                token_in: if (i ^ seed) % 2 == 0 {
                    asset_rsk()
                } else {
                    asset_usdc()
                },
                token_out: if (i ^ seed) % 2 == 0 {
                    asset_usdc()
                } else {
                    asset_rsk()
                },
                amount_in: Amount(10 + u128::from(i)),
                min_amount_out: Amount(1),
                deadline: None,
                current_block: BlockHeight(u64::from(i) + 2),
                op_id: Some({
                    let mut h = [seed; 32];
                    h[30] = i;
                    h[31] = seed;
                    Hash(h)
                }),
            },
        );
        w.assert_accounting_integrity();
    }
    w.canonical_fingerprint()
}

#[test]
fn four_validator_replicas_identical_fingerprints() {
    // Modelo de faltas: quorum >2/3 ⇒ n=4 (f=1) es el mínimo distribuido del protocolo.
    let a = run_scenario(1);
    let b = run_scenario(1);
    let c = run_scenario(1);
    let d = run_scenario(1);
    assert_eq!(a, b);
    assert_eq!(b, c);
    assert_eq!(c, d);
}

#[test]
fn divergent_seed_produces_divergent_state() {
    let a = run_scenario(1);
    let b = run_scenario(2);
    assert_ne!(a, b);
}

#[test]
fn stress_1000_swaps_accounting_holds() {
    let mut w = Stage7World::bootstrap(500_000, 500_000);
    let mut ok = 0u32;
    for i in 0u32..1_000 {
        let mut op = [0u8; 32];
        op[0] = (i & 0xff) as u8;
        op[1] = ((i >> 8) & 0xff) as u8;
        op[2] = ((i >> 16) & 0xff) as u8;
        match SwapEngine::execute_swap_exact_input(
            &mut w.pools,
            &mut w.accounting,
            &mut w.fees,
            ExactInputSwap {
                pool_id: w.pool_id,
                trader: w.trader,
                token_in: if i % 2 == 0 {
                    asset_rsk()
                } else {
                    asset_usdc()
                },
                token_out: if i % 2 == 0 {
                    asset_usdc()
                } else {
                    asset_rsk()
                },
                amount_in: Amount(5),
                min_amount_out: Amount(0),
                deadline: None,
                current_block: BlockHeight(u64::from(i) + 2),
                op_id: Some(Hash(op)),
            },
        ) {
            Ok(_) => ok += 1,
            Err(_) => {}
        }
    }
    assert!(ok > 100, "expected many successful swaps, got {ok}");
    w.assert_accounting_integrity();
}
