//! Motor matemático puro del AMM.
//!
//! Todas las funciones son libres de efectos: no mutan reservas ni supplies.
//! La capa de ejecución aplica los resultados al estado.

pub mod amount;
pub mod invariant;
pub mod shares;

pub use amount::{
    amount_from_u128, amount_to_u128, checked_add, checked_div, checked_mul, checked_sub, full_mul,
    isqrt, isqrt_256, mul_div_ceil, mul_div_floor, try_into_u128, Rounding,
};
pub use invariant::{
    amount_in_for_exact_out, amount_out_for_exact_in, product, quote_exact_in, quote_exact_out,
    spot_price, validate_invariant, validate_reserves,
};
pub use shares::{
    initial_shares, mint_shares, ownership_bps, redeem_amounts, share_value, MintResult,
    RedeemResult,
};
