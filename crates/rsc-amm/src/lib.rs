//! Motor AMM de producto constante para RSC Chain.
//!
//! - Etapa 1: tipos, errores, constantes y aritmética pura.
//! - Etapa 2: gestión determinista de pools ([`pool`]).
//! - Etapa 3: contabilidad y liquidez ([`accounting`], [`liquidity`]).
//! - Etapa 4: cotizaciones, fees y swaps ([`pricing`], [`fees`], [`swap`]).
//! - Etapa 5: fees/accounting/events endurecidos (sin doble conteo, eventos LP/fee).
//! - Etapa 6: integración nativa ([`native`]) con `rsc-assets`, ledger, state y storage.
//!
//! Los saldos reutilizan [`rsc_core::Amount`] (`u128`). El registro de pools
//! y el ledger AMM son subestado lógico con transiciones atómicas. En la vía
//! nativa, los saldos de usuario/vault son autoritativos en `rsc-assets`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod accounting;
pub mod constants;
pub mod error;
pub mod events;
pub mod fees;
pub mod liquidity;
pub mod math;
#[cfg(feature = "native")]
pub mod native;
pub mod pool;
pub mod pricing;
pub mod swap;
pub mod types;

pub use accounting::{AmmAccounting, LiquidityPosition, PoolLiquidity};
pub use constants::{
    AMM_VERSION, BPS_DENOMINATOR, DEFAULT_MAX_SLIPPAGE_BPS, DEFAULT_MIN_LIQUIDITY,
    DEFAULT_SWAP_FEE_BPS, DOMAIN_POOL_ID, LP_SINK_ACCOUNT, MAX_SWAP_FEE_BPS, PRICE_SCALE,
};
pub use error::AmmError;
pub use events::{AmmEvent, TxId};
pub use fees::{
    amount_after_fee, default_swap_fee, fee_amount, get_pool_fee, max_fee_bps, split_fee,
    validate_fee, FeeBreakdown, FeeTracker, PROTOCOL_FEE_SHARE_BPS,
};
pub use liquidity::{
    calculate_initial_shares, calculate_liquidity_shares, calculate_withdrawal, get_pool_liquidity,
    get_user_liquidity, get_user_lp_balance, quote_balanced_deposit, validate_deposit,
    validate_withdrawal, AddLiquidityParams, AddLiquidityResult, DepositPlan, LiquidityEngine,
    RemoveLiquidityParams, RemoveLiquidityResult, WithdrawPlan,
};
pub use math::{amount, invariant, shares};
pub use pool::{
    AmmAuthority, AssetCatalog, GovernanceAuthority, InMemoryAssetCatalog, LiquidityPool,
    PoolIdentity, PoolPolicy, PoolRegistry, PoolStatus,
};
pub use pricing::{
    calculate_amount_in, calculate_amount_out, calculate_execution_price, calculate_price_impact,
    calculate_slippage, calculate_spot_price, calculate_spot_price_normalized, get_quote,
    get_quote_exact_out, resolve_path, validate_quote, AssetDecimals, SwapPath, SwapQuote,
    UniformDecimals, DEFAULT_ASSET_DECIMALS,
};
pub use swap::{
    calculate_swap_result, transfer_swap_assets, update_pool_reserves, validate_max_amount_in,
    validate_min_amount_out, validate_swap, validate_swap_deadline, validate_swap_exact_input,
    validate_swap_exact_output, ExactInputSwap, ExactOutputSwap, SwapEngine, SwapResult,
};
pub use types::{
    AssetId, FeeBps, LpShares, PoolId, PoolKey, ProtocolParams, Reserves, SwapAmounts,
};

#[cfg(feature = "native")]
pub use native::{
    sign_amm_tx, AmmExecutionResult, AmmInstruction, AmmRuntime, AmmStore, BlockContext,
    NativeAmmError,
};

#[cfg(test)]
mod tests {
    mod accounting_tests;
    mod fee_tests;
    mod liquidity_tests;
    mod pool_tests;
    mod stage7_adversarial;
    mod stage7_consistency;
    mod stage7_harness;
    mod stage7_math_security;
    mod swap_tests;
}
